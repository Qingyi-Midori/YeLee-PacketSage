//! Analysis query engine (M0~M2 §4.7, 开发文档 §14.2).
//!
//! The engine answers questions **only** from deterministic aggregates, never
//! from a language model. Every number returned here is traceable to a packet
//! index, a session id or a rule evaluation.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::path::PathBuf;

use packetsage_protocol::{
    AppProto, DecodeStatus, DirectionBasis, InterfaceInfo, Layer, NetProto, PayloadRef,
    ProtocolCount, StreamState, TcpFlag,
};
use serde::{Deserialize, Serialize};

use crate::conversation::{ConversationAggregator, SessionEntry};
use crate::error::{PacketSageError, Result};
use crate::model::Endpoint;
use crate::reader::CaptureReader;
use crate::reassembly::{ByteRange, Direction};

/// Capture level summary (data behind `get_capture_summary`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CaptureSummary {
    /// Task identifier.
    pub task_id: String,
    /// Analysed file.
    pub source_path: String,
    /// SHA-256 of the file.
    pub source_sha256: String,
    /// `pcap` or `pcapng`.
    pub format: String,
    /// Interfaces seen in the file.
    pub interfaces: Vec<InterfaceInfo>,
    /// Packets processed.
    pub packets: u64,
    /// Bytes captured.
    pub bytes: u64,
    /// First packet timestamp (ns), as a string.
    pub first_ts_ns: Option<String>,
    /// Last packet timestamp (ns), as a string.
    pub last_ts_ns: Option<String>,
    /// Duration in seconds.
    pub duration_s: f64,
    /// Sessions (TCP + UDP).
    pub sessions: u64,
    /// Alerts emitted by the rule engine.
    pub alerts: u64,
    /// Decode errors.
    pub decode_errors: u64,
    /// Packets whose captured length was shorter than the original length.
    pub truncated_packets: u64,
    /// Non-fatal reader events.
    pub reader_nonfatal: u64,
    /// Sessions with holes or overflowed buffers.
    pub incomplete_sessions: u64,
    /// Sessions refused because of the session cap.
    pub dropped_sessions: u64,
    /// Per-protocol packet counts (top level convenience view).
    pub protocols: BTreeMap<String, u64>,
}

/// Per-layer protocol counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtocolStats {
    rows: BTreeMap<(Layer, String), (u64, u64)>,
}

impl ProtocolStats {
    /// Empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one packet to a bucket.
    pub fn add(&mut self, layer: Layer, protocol: &str, bytes: u64) {
        let entry = self
            .rows
            .entry((layer, protocol.to_owned()))
            .or_insert((0, 0));
        entry.0 = entry.0.saturating_add(1);
        entry.1 = entry.1.saturating_add(bytes);
    }

    /// Rows of one layer, most frequent first, capped at `top`.
    #[must_use]
    pub fn top(&self, layer: Layer, top: usize) -> Vec<ProtocolCount> {
        let mut rows: Vec<ProtocolCount> = self
            .rows
            .iter()
            .filter(|((l, _), _)| *l == layer)
            .map(|((l, name), (packets, bytes))| ProtocolCount {
                layer: *l,
                protocol: name.clone(),
                packets: *packets,
                bytes: *bytes,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.packets
                .cmp(&a.packets)
                .then_with(|| a.protocol.cmp(&b.protocol))
        });
        rows.truncate(top);
        rows
    }

    /// Every row, ordered by layer then packet count.
    #[must_use]
    pub fn all(&self) -> Vec<ProtocolCount> {
        let mut rows: Vec<ProtocolCount> = self
            .rows
            .iter()
            .map(|((l, name), (packets, bytes))| ProtocolCount {
                layer: *l,
                protocol: name.clone(),
                packets: *packets,
                bytes: *bytes,
            })
            .collect();
        rows.sort_by(|a, b| {
            (a.layer, std::cmp::Reverse(a.packets), &a.protocol).cmp(&(
                b.layer,
                std::cmp::Reverse(b.packets),
                &b.protocol,
            ))
        });
        rows
    }

    /// Packet count of one protocol inside one layer.
    #[must_use]
    pub fn count(&self, layer: Layer, protocol: &str) -> u64 {
        self.rows
            .get(&(layer, protocol.to_owned()))
            .map_or(0, |(packets, _)| *packets)
    }
}

/// Lightweight per-packet header record (no payload bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketMeta {
    /// Zero-based index.
    pub packet_index: u64,
    /// Canonical timestamp (ns).
    pub ts_ns: i128,
    /// Captured length.
    pub captured_len: u32,
    /// Original length.
    pub original_len: u32,
    /// Interface id.
    pub interface_id: u32,
    /// Network protocol.
    pub protocol: NetProto,
    /// Source endpoint when the packet has one.
    pub src: Option<Endpoint>,
    /// Destination endpoint when the packet has one.
    pub dst: Option<Endpoint>,
    /// TCP flag bitmask.
    pub flags: u8,
    /// Application protocol.
    pub app: Option<AppProto>,
    /// Decode status.
    pub decode_status: DecodeStatus,
    /// Payload location.
    pub payload: Option<PayloadRef>,
    /// Truncated by snaplen.
    pub truncated: bool,
}

/// Bit of a TCP flag inside [`PacketMeta::flags`].
#[must_use]
pub fn flag_bit(flag: TcpFlag) -> u8 {
    match flag {
        TcpFlag::Fin => 0x01,
        TcpFlag::Syn => 0x02,
        TcpFlag::Rst => 0x04,
        TcpFlag::Psh => 0x08,
        TcpFlag::Ack => 0x10,
        TcpFlag::Urg => 0x20,
        TcpFlag::Ece => 0x40,
        TcpFlag::Cwr => 0x80,
    }
}

/// Decodes a flag bitmask back into names.
#[must_use]
pub fn flags_to_names(mask: u8) -> Vec<&'static str> {
    [
        (0x01, "FIN"),
        (0x02, "SYN"),
        (0x04, "RST"),
        (0x08, "PSH"),
        (0x10, "ACK"),
        (0x20, "URG"),
        (0x40, "ECE"),
        (0x80, "CWR"),
    ]
    .into_iter()
    .filter(|(bit, _)| mask & bit != 0)
    .map(|(_, name)| name)
    .collect()
}

/// Packet index: metadata only, no payload bytes (M0~M2 §4.7).
#[derive(Debug, Clone, Default)]
pub struct PacketIndex {
    packets: Vec<PacketMeta>,
}

impl PacketIndex {
    /// Empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a record.
    pub fn push(&mut self, meta: PacketMeta) {
        self.packets.push(meta);
    }

    /// Number of indexed packets.
    #[must_use]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    /// True when the index holds no packets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    /// Metadata of one packet.
    #[must_use]
    pub fn get(&self, packet_index: u64) -> Option<&PacketMeta> {
        self.packets
            .get(usize::try_from(packet_index).unwrap_or(usize::MAX))
    }

    /// All metadata rows.
    #[must_use]
    pub fn packets(&self) -> &[PacketMeta] {
        &self.packets
    }
}

/// Filter accepted by `filter_packets`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PacketFilter {
    /// Source IP.
    pub src_ip: Option<String>,
    /// Destination IP.
    pub dst_ip: Option<String>,
    /// Source port.
    pub src_port: Option<u16>,
    /// Destination port.
    pub dst_port: Option<u16>,
    /// TCP flags that must all be present.
    pub tcp_flags: Vec<TcpFlag>,
    /// Protocol name (`tcp`, `udp`, `icmp`, `icmpv6`).
    pub protocol: Option<String>,
    /// Inclusive start of the time window (seconds since epoch).
    pub time_start: Option<f64>,
    /// Inclusive end of the time window (seconds since epoch).
    pub time_end: Option<f64>,
    /// Maximum number of matches.
    pub limit: Option<usize>,
    /// Only packets carrying a decode error.
    pub decode_errors_only: Option<bool>,
}

/// Sort key for `get_conversations`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSort {
    /// Largest byte count first.
    #[default]
    Bytes,
    /// Largest packet count first.
    Packets,
    /// Longest duration first.
    Duration,
}

/// Filter accepted by `get_conversations`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationFilter {
    /// `tcp` or `udp`.
    pub protocol: Option<String>,
    /// Sessions involving this address on either side.
    pub ip: Option<String>,
    /// Minimum byte count.
    pub min_bytes: Option<u64>,
    /// Minimum packet count.
    pub min_packets: Option<u64>,
}

/// Conversation query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationQuery {
    /// Sort key.
    pub sort_by: ConversationSort,
    /// Maximum number of rows.
    pub limit: usize,
    /// Filter.
    pub filter: ConversationFilter,
}

impl Default for ConversationQuery {
    fn default() -> Self {
        Self {
            sort_by: ConversationSort::Bytes,
            limit: 20,
            filter: ConversationFilter::default(),
        }
    }
}

/// Conversation row returned to callers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationDto {
    /// `S-{n:06}`.
    pub session_id: String,
    /// `tcp` / `udp`.
    pub protocol: String,
    /// Endpoint A address.
    pub src_ip: String,
    /// Endpoint A port.
    pub src_port: u16,
    /// Endpoint B address.
    pub dst_ip: String,
    /// Endpoint B port.
    pub dst_port: u16,
    /// First packet timestamp (ns string).
    pub first_ts_ns: String,
    /// Last packet timestamp (ns string).
    pub last_ts_ns: String,
    /// Duration in seconds.
    pub duration_s: f64,
    /// Packets in both directions.
    pub packets: u64,
    /// Bytes in both directions.
    pub bytes: u64,
    /// Packets from endpoint A.
    pub src_packets: u64,
    /// Packets from endpoint B.
    pub dst_packets: u64,
    /// Bytes from endpoint A.
    pub src_bytes: u64,
    /// Bytes from endpoint B.
    pub dst_bytes: u64,
    /// Bare SYN count.
    pub syn_count: u32,
    /// SYN+ACK count.
    pub syn_ack_count: u32,
    /// RST count.
    pub rst_count: u32,
    /// FIN count.
    pub fin_count: u32,
    /// Retransmissions.
    pub retransmission_count: u32,
    /// Out-of-order segments.
    pub out_of_order_count: u32,
    /// Reassembly state.
    pub state: StreamState,
    /// Application protocol.
    pub app_protocol: Option<AppProto>,
    /// How the client side was chosen.
    pub direction_basis: DirectionBasis,
    /// Client address (the endpoint that initiated the session).
    pub client_ip: String,
    /// Client port.
    pub client_port: u16,
    /// Server address.
    pub server_ip: String,
    /// Server port.
    pub server_port: u16,
}

/// Packet inspection row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketInspectDto {
    /// Packet index.
    pub packet_index: u64,
    /// Canonical timestamp (ns string).
    pub ts_unix_ns: String,
    /// Source address.
    pub src: Option<String>,
    /// Destination address.
    pub dst: Option<String>,
    /// Source port.
    pub src_port: Option<u16>,
    /// Destination port.
    pub dst_port: Option<u16>,
    /// Protocol name.
    pub protocol: String,
    /// TCP flags.
    pub flags: Vec<String>,
    /// Captured length.
    pub captured_len: u32,
    /// Original length.
    pub original_len: u32,
    /// Decode status.
    pub decode_status: DecodeStatus,
    /// Printable payload preview (bounded).
    pub payload_preview: Option<String>,
    /// True when the preview was cut short.
    pub preview_truncated: bool,
}

/// Stream reconstruction request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamQuery {
    /// Session id.
    pub session_id: String,
    /// Direction, defaults to client to server.
    pub direction: Option<String>,
    /// Maximum number of bytes to return.
    pub max_bytes: Option<u64>,
}

/// Reconstructed stream preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamPreview {
    /// Session id.
    pub session_id: String,
    /// Direction.
    pub direction: String,
    /// `complete`, `incomplete` or `buffer_overflow`.
    pub status: String,
    /// Bytes delivered.
    pub bytes: u64,
    /// Content type guess.
    pub content_type: String,
    /// Printable preview.
    pub preview: String,
    /// Missing byte ranges (relative to the stream start).
    pub missing_ranges: Vec<(u32, u32)>,
}

/// Request for payload bytes of one packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadRequest {
    /// Packet index.
    pub packet_index: u64,
    /// Offset inside the original record.
    pub offset: u32,
    /// Number of bytes wanted.
    pub length: u32,
}

/// Supplies payload bytes for already indexed packets.
pub trait PayloadProvider {
    /// Reads the requested payload slices in one pass when possible.
    ///
    /// # Errors
    /// Returns [`PacketSageError`] when the capture cannot be read.
    fn read_many(&self, requests: &[PayloadRequest]) -> Result<HashMap<u64, Vec<u8>>>;
}

/// Reconstructs TCP payload for a session (implemented by the analysis store).
pub trait StreamReconstructor {
    /// Rebuilds one direction of a session with a bounded output.
    ///
    /// # Errors
    /// Returns [`PacketSageError`] when the session or the capture is missing.
    fn reconstruct(
        &self,
        session_id: &str,
        direction: Direction,
        max_bytes: usize,
    ) -> Result<StreamPreview>;
}

/// Reopens a capture file and reads payload slices by packet index (ADR-019).
#[derive(Debug, Clone)]
pub struct PayloadReader {
    path: PathBuf,
}

impl PayloadReader {
    /// Creates a reader for one capture file.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl PayloadProvider for PayloadReader {
    fn read_many(&self, requests: &[PayloadRequest]) -> Result<HashMap<u64, Vec<u8>>> {
        let mut wanted: BTreeMap<u64, (u32, u32)> = BTreeMap::new();
        for request in requests {
            wanted.insert(request.packet_index, (request.offset, request.length));
        }
        let mut result = HashMap::new();
        if wanted.is_empty() {
            return Ok(result);
        }
        let mut reader = CaptureReader::open(&self.path, 1 << 20)?;
        reader.drive(|item, _interfaces| {
            if let crate::reader::SourceItem::Packet(record) = item {
                if let Some((offset, length)) = wanted.get(&record.packet_index) {
                    let start = usize::try_from(*offset).unwrap_or(usize::MAX);
                    let end = start.saturating_add(usize::try_from(*length).unwrap_or(usize::MAX));
                    if let Some(slice) = record.data.get(start..end.min(record.data.len())) {
                        result.insert(record.packet_index, slice.to_vec());
                    }
                }
            }
            Ok(())
        })?;
        Ok(result)
    }
}

/// Printable preview of a payload slice.
#[must_use]
pub fn printable_preview(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    let truncated = bytes.len() > max_bytes;
    let slice = &bytes[..bytes.len().min(max_bytes)];
    let text: String = slice
        .iter()
        .map(|b| {
            if *b == b'\n' || *b == b'\r' || *b == b'\t' {
                ' '
            } else if b.is_ascii_graphic() || *b == b' ' {
                char::from(*b)
            } else {
                '.'
            }
        })
        .collect();
    (text.trim().to_owned(), truncated)
}

/// Guesses a coarse content type for a payload preview.
#[must_use]
pub fn guess_content_type(bytes: &[u8]) -> String {
    if bytes.starts_with(b"\x16\x03") || bytes.starts_with(b"\x17\x03") {
        return "tls".to_owned();
    }
    if bytes.starts_with(b"HTTP/1.") {
        return "http_response".to_owned();
    }
    // `METHOD SP ...` with an all-uppercase method of at least 3 characters.
    let method_like = bytes
        .iter()
        .take(8)
        .position(|b| *b == b' ')
        .is_some_and(|end| end >= 3 && bytes[..end].iter().all(u8::is_ascii_uppercase));
    if method_like {
        return "http_request".to_owned();
    }
    if bytes
        .iter()
        .all(|b| b.is_ascii_graphic() || b" \r\n\t".contains(b))
    {
        "text".to_owned()
    } else {
        "binary".to_owned()
    }
}

/// Query engine over one analysed task.
pub struct QueryEngine<'a> {
    /// Capture summary.
    pub summary: &'a CaptureSummary,
    /// Session aggregator.
    pub sessions: &'a ConversationAggregator,
    /// Packet index.
    pub index: &'a PacketIndex,
    /// Protocol statistics.
    pub stats: &'a ProtocolStats,
    /// Payload access (absent in counter-only mode).
    pub payloads: Option<&'a dyn PayloadProvider>,
    /// Session reconstruction (absent in counter-only mode).
    pub streams: Option<&'a dyn StreamReconstructor>,
}

impl QueryEngine<'_> {
    /// Capture summary.
    #[must_use]
    pub fn capture_summary(&self) -> CaptureSummary {
        self.summary.clone()
    }

    /// Protocol statistics of one layer.
    #[must_use]
    pub fn protocol_stats(&self, layer: Layer, top: usize) -> Vec<ProtocolCount> {
        self.stats.top(layer, top.max(1))
    }

    /// Filters sessions.
    #[must_use]
    pub fn conversations(&self, query: &ConversationQuery) -> Vec<ConversationDto> {
        let mut rows: Vec<ConversationDto> = self
            .sessions
            .sessions()
            .into_iter()
            .filter(|entry| matches_filter(entry, &query.filter))
            .map(to_dto)
            .collect();
        match query.sort_by {
            ConversationSort::Bytes => rows.sort_by(|a, b| {
                b.bytes
                    .cmp(&a.bytes)
                    .then_with(|| a.session_id.cmp(&b.session_id))
            }),
            ConversationSort::Packets => rows.sort_by(|a, b| {
                b.packets
                    .cmp(&a.packets)
                    .then_with(|| a.session_id.cmp(&b.session_id))
            }),
            ConversationSort::Duration => rows.sort_by(|a, b| {
                b.duration_s
                    .partial_cmp(&a.duration_s)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.session_id.cmp(&b.session_id))
            }),
        }
        let limit = if query.limit == 0 { 20 } else { query.limit };
        rows.truncate(limit);
        rows
    }

    /// Returns the packet indices that match `filter`.
    #[must_use]
    pub fn filter_packets(&self, filter: &PacketFilter, limit: usize) -> Vec<u64> {
        let mut matches = Vec::new();
        for meta in self.index.packets() {
            if packet_matches(meta, filter) {
                matches.push(meta.packet_index);
                if matches.len() >= limit.max(1) {
                    break;
                }
            }
        }
        matches
    }

    /// Returns metadata plus a bounded payload preview for the given packets.
    ///
    /// # Errors
    /// Returns [`PacketSageError`] when payload bytes are requested but no
    /// provider is configured, or when reading fails.
    pub fn inspect_packets(
        &self,
        indices: &[u64],
        preview_bytes: usize,
    ) -> Result<Vec<PacketInspectDto>> {
        let mut requests = Vec::new();
        for index in indices {
            if let Some(meta) = self.index.get(*index) {
                if let Some(payload) = &meta.payload {
                    let length = u32::try_from(preview_bytes)
                        .unwrap_or(u32::MAX)
                        .min(payload.length);
                    requests.push(PayloadRequest {
                        packet_index: *index,
                        offset: payload.offset,
                        length: length.max(1),
                    });
                }
            }
        }
        let payloads = match self.payloads {
            Some(provider) if !requests.is_empty() => Some(provider.read_many(&requests)?),
            _ => None,
        };

        let mut out = Vec::with_capacity(indices.len());
        for index in indices {
            let Some(meta) = self.index.get(*index) else {
                continue;
            };
            let (preview, truncated) =
                payloads
                    .as_ref()
                    .and_then(|map| map.get(index))
                    .map_or((None, false), |bytes| {
                        let (text, cut) = printable_preview(bytes, preview_bytes);
                        (Some(text), cut)
                    });
            out.push(PacketInspectDto {
                packet_index: meta.packet_index,
                ts_unix_ns: meta.ts_ns.to_string(),
                src: meta.src.map(|e| e.ip.to_string()),
                dst: meta.dst.map(|e| e.ip.to_string()),
                src_port: meta.src.map(|e| e.port),
                dst_port: meta.dst.map(|e| e.port),
                protocol: protocol_name(meta.protocol).to_owned(),
                flags: flags_to_names(meta.flags)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                captured_len: meta.captured_len,
                original_len: meta.original_len,
                decode_status: meta.decode_status,
                payload_preview: preview,
                preview_truncated: truncated,
            });
        }
        Ok(out)
    }

    /// Rebuilds a TCP stream preview.
    ///
    /// # Errors
    /// Returns [`PacketSageError::InvalidArgument`] for an unknown direction or
    /// when reconstruction is unavailable, and propagates storage errors.
    pub fn reconstruct_stream(&self, query: &StreamQuery) -> Result<StreamPreview> {
        let direction = match query.direction.as_deref() {
            None | Some("client_to_server") => Direction::ClientToServer,
            Some("server_to_client") => Direction::ServerToClient,
            Some(other) => {
                return Err(PacketSageError::InvalidArgument(format!(
                    "unknown direction {other:?}"
                )))
            }
        };
        let max_bytes = query.max_bytes.unwrap_or(262_144).clamp(1, 262_144) as usize;
        let Some(reconstructor) = self.streams else {
            return Err(PacketSageError::InvalidArgument(
                "stream reconstruction is not available for this task".to_owned(),
            ));
        };
        reconstructor.reconstruct(&query.session_id, direction, max_bytes)
    }
}

fn matches_filter(entry: &SessionEntry, filter: &ConversationFilter) -> bool {
    if let Some(protocol) = &filter.protocol {
        if !entry.key.proto.as_str().eq_ignore_ascii_case(protocol) {
            return false;
        }
    }
    if let Some(ip) = &filter.ip {
        let matches = ip
            .parse::<IpAddr>()
            .is_ok_and(|address| entry.key.a.ip == address || entry.key.b.ip == address);
        if !matches {
            return false;
        }
    }
    if let Some(min) = filter.min_bytes {
        if entry.stats.bytes < min {
            return false;
        }
    }
    if let Some(min) = filter.min_packets {
        if entry.stats.packets < min {
            return false;
        }
    }
    true
}

fn to_dto(entry: &SessionEntry) -> ConversationDto {
    ConversationDto {
        session_id: entry.session_id.clone(),
        protocol: entry.key.proto.as_str().to_owned(),
        src_ip: entry.key.a.ip.to_string(),
        src_port: entry.key.a.port,
        dst_ip: entry.key.b.ip.to_string(),
        dst_port: entry.key.b.port,
        first_ts_ns: entry.stats.first_ts_ns.to_string(),
        last_ts_ns: entry.stats.last_ts_ns.to_string(),
        duration_s: duration_seconds(entry.stats.first_ts_ns, entry.stats.last_ts_ns),
        packets: entry.stats.packets,
        bytes: entry.stats.bytes,
        src_packets: entry.stats.src_packets,
        dst_packets: entry.stats.dst_packets,
        src_bytes: entry.stats.src_bytes,
        dst_bytes: entry.stats.dst_bytes,
        syn_count: entry.stats.syn_count,
        syn_ack_count: entry.stats.syn_ack_count,
        rst_count: entry.stats.rst_count,
        fin_count: entry.stats.fin_count,
        retransmission_count: entry.stats.retransmission_count,
        out_of_order_count: entry.stats.out_of_order_count,
        state: entry.stats.state,
        app_protocol: entry.stats.app_protocol,
        direction_basis: entry.direction_basis,
        client_ip: entry.client_endpoint().ip.to_string(),
        client_port: entry.client_endpoint().port,
        server_ip: entry
            .key
            .other_endpoint(entry.client_endpoint())
            .ip
            .to_string(),
        server_port: entry.key.other_endpoint(entry.client_endpoint()).port,
    }
}

/// Difference of two nanosecond timestamps in seconds.
#[must_use]
pub fn duration_seconds(first_ns: i128, last_ns: i128) -> f64 {
    let delta = last_ns.saturating_sub(first_ns);
    (delta as f64) / 1_000_000_000.0
}

fn packet_matches(meta: &PacketMeta, filter: &PacketFilter) -> bool {
    if let Some(src) = &filter.src_ip {
        if meta.src.map(|e| e.ip.to_string()).as_deref() != Some(src.as_str()) {
            return false;
        }
    }
    if let Some(dst) = &filter.dst_ip {
        if meta.dst.map(|e| e.ip.to_string()).as_deref() != Some(dst.as_str()) {
            return false;
        }
    }
    if let Some(port) = filter.src_port {
        if meta.src.map(|e| e.port) != Some(port) {
            return false;
        }
    }
    if let Some(port) = filter.dst_port {
        if meta.dst.map(|e| e.port) != Some(port) {
            return false;
        }
    }
    if let Some(protocol) = &filter.protocol {
        if !protocol_name(meta.protocol).eq_ignore_ascii_case(protocol) {
            return false;
        }
    }
    for flag in &filter.tcp_flags {
        if meta.flags & flag_bit(*flag) == 0 {
            return false;
        }
    }
    if let Some(start) = filter.time_start {
        let start_ns = (start * 1e9) as i128;
        if meta.ts_ns < start_ns {
            return false;
        }
    }
    if let Some(end) = filter.time_end {
        let end_ns = (end * 1e9) as i128;
        if meta.ts_ns > end_ns {
            return false;
        }
    }
    if filter.decode_errors_only == Some(true) && meta.decode_status == DecodeStatus::Ok {
        return false;
    }
    true
}

/// Lowercase protocol name of a network protocol.
#[must_use]
pub fn protocol_name(proto: NetProto) -> &'static str {
    match proto {
        NetProto::Tcp => "tcp",
        NetProto::Udp => "udp",
        NetProto::Icmp => "icmp",
        NetProto::Icmpv6 => "icmpv6",
        NetProto::Arp => "arp",
        NetProto::Ipv4 => "ipv4",
        NetProto::Ipv6 => "ipv6",
        NetProto::Other => "other",
    }
}

/// Missing ranges rendered as pairs.
#[must_use]
pub fn ranges_to_pairs(ranges: &[ByteRange]) -> Vec<(u32, u32)> {
    ranges.iter().map(|r| (r.start, r.end)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::ConversationAggregator;
    use crate::decoder::Decoder;
    use crate::model::PacketRecord;
    use crate::reader::LINKTYPE_ETHERNET;
    use etherparse::PacketBuilder;
    use packetsage_protocol::{TaskId, TsPrecision, GOLDEN_TASK_ID};

    fn build_index() -> (PacketIndex, ProtocolStats) {
        let task = TaskId::parse(GOLDEN_TASK_ID).expect("task");
        let decoder = Decoder::all();
        let mut index = PacketIndex::new();
        let mut stats = ProtocolStats::new();
        for packet_index in 0..5u64 {
            let builder = PacketBuilder::ethernet2([1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12])
                .ipv4([10, 0, 0, 1], [10, 0, 0, 2], 64)
                .tcp(40000, 80, 1, 8192);
            let payload = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
            let mut raw = Vec::with_capacity(builder.size(payload.len()));
            builder.write(&mut raw, payload).expect("build");
            if let Some(flags) = raw.get_mut(14 + 20 + 13) {
                *flags |= 0x02 | 0x10;
            }
            let decoded = decoder.decode(LINKTYPE_ETHERNET, &raw);
            let record = PacketRecord {
                task_id: task.clone(),
                packet_index,
                ts_ns: 1_700_000_000_000_000_000 + i128::from(packet_index) * 1_000_000,
                ts_precision: TsPrecision::Us,
                interface_id: 0,
                captured_len: raw.len() as u32,
                original_len: raw.len() as u32,
                linktype: LINKTYPE_ETHERNET,
                raw: &raw,
                decoded,
                decode_status: DecodeStatus::Ok,
            };
            let payload_ref = record.payload_ref();
            let network = record.decoded.network.clone().expect("network");
            let transport = record.decoded.transport.clone().expect("transport");
            let mut flags = 0u8;
            for flag in &transport.flags {
                flags |= flag_bit(*flag);
            }
            index.push(PacketMeta {
                packet_index,
                ts_ns: record.ts_ns,
                captured_len: record.captured_len,
                original_len: record.original_len,
                interface_id: 0,
                protocol: network.protocol,
                src: Some(Endpoint {
                    ip: network
                        .src
                        .parse()
                        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
                    port: transport.src_port.unwrap_or(0),
                }),
                dst: Some(Endpoint {
                    ip: network
                        .dst
                        .parse()
                        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
                    port: transport.dst_port.unwrap_or(0),
                }),
                flags,
                app: record.decoded.application.as_ref().map(|a| a.protocol),
                decode_status: DecodeStatus::Ok,
                payload: payload_ref,
                truncated: false,
            });
            stats.add(Layer::Link, "ethernet", u64::from(record.captured_len));
            stats.add(Layer::Ipv4, "ipv4", u64::from(record.captured_len));
            stats.add(Layer::Tcp, "tcp", u64::from(record.captured_len));
            if record
                .decoded
                .application
                .as_ref()
                .is_some_and(|a| a.protocol == AppProto::Http)
            {
                stats.add(Layer::Http, "http", u64::from(record.captured_len));
            }
        }
        (index, stats)
    }

    fn engine<'a>(
        summary: &'a CaptureSummary,
        sessions: &'a ConversationAggregator,
        index: &'a PacketIndex,
        stats: &'a ProtocolStats,
    ) -> QueryEngine<'a> {
        QueryEngine {
            summary,
            sessions,
            index,
            stats,
            payloads: None,
            streams: None,
        }
    }

    #[test]
    fn filter_packets_by_port_and_flag() {
        let (index, stats) = build_index();
        let sessions = ConversationAggregator::new();
        let summary = CaptureSummary::default();
        let engine = engine(&summary, &sessions, &index, &stats);
        let filter = PacketFilter {
            dst_port: Some(80),
            tcp_flags: vec![TcpFlag::Syn, TcpFlag::Ack],
            ..PacketFilter::default()
        };
        assert_eq!(engine.filter_packets(&filter, 10).len(), 5);
        let filter = PacketFilter {
            tcp_flags: vec![TcpFlag::Rst],
            ..PacketFilter::default()
        };
        assert!(engine.filter_packets(&filter, 10).is_empty());
        assert_eq!(engine.filter_packets(&PacketFilter::default(), 2).len(), 2);
    }

    #[test]
    fn protocol_stats_are_layer_scoped() {
        let (_, stats) = build_index();
        assert_eq!(stats.count(Layer::Tcp, "tcp"), 5);
        assert_eq!(stats.count(Layer::Http, "http"), 5);
        assert_eq!(stats.count(Layer::Udp, "udp"), 0);
        assert_eq!(stats.top(Layer::Tcp, 10)[0].protocol, "tcp");
    }

    #[test]
    fn inspect_packets_without_payload_provider_returns_metadata() {
        let (index, stats) = build_index();
        let sessions = ConversationAggregator::new();
        let summary = CaptureSummary::default();
        let engine = engine(&summary, &sessions, &index, &stats);
        let rows = engine.inspect_packets(&[0, 1], 32).expect("inspect");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].protocol, "tcp");
        assert!(rows[0].flags.contains(&"SYN".to_owned()));
        assert!(rows[0].payload_preview.is_none());
    }

    #[test]
    fn preview_sanitises_binary_bytes() {
        let (text, truncated) = printable_preview(b"line1\nline2\x00\x01", 64);
        assert_eq!(text, "line1 line2..");
        assert!(!truncated);
        let (_, truncated) = printable_preview(&[b'a'; 100], 10);
        assert!(truncated);
    }

    #[test]
    fn content_type_guesses() {
        assert_eq!(guess_content_type(b"GET /x HTTP/1.1\r\n"), "http_request");
        assert_eq!(guess_content_type(b"HTTP/1.1 200 OK\r\n"), "http_response");
        assert_eq!(guess_content_type(&[0x16, 0x03, 0x01, 0x00, 0x10]), "tls");
        assert_eq!(guess_content_type(b"hello world"), "text");
        assert_eq!(guess_content_type(&[0x00, 0x01, 0x02]), "binary");
    }

    #[test]
    fn empty_conversation_query_is_stable() {
        let (index, stats) = build_index();
        let sessions = ConversationAggregator::new();
        let summary = CaptureSummary::default();
        let engine = engine(&summary, &sessions, &index, &stats);
        assert!(engine
            .conversations(&ConversationQuery::default())
            .is_empty());
        assert!(engine
            .reconstruct_stream(&StreamQuery {
                session_id: "S-000001".to_owned(),
                direction: None,
                max_bytes: None,
            })
            .is_err());
    }
}
