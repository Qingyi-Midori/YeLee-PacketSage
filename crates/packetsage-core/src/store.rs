//! In-memory analysis store: everything one analysed task can be asked about
//! (M0~M2 §4.7, M3~M6 §3.8 cold-recovery shape).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use packetsage_protocol::{
    AlertEvent, DecodeErrorCode, DecodeErrorCount, EvidenceLedger, Finding, InterfaceInfo, Layer,
    StreamState, ValidationOutcome, ValidatorStatus, SCHEMA_VERSION,
};

use crate::config::{EngineConfig, ReassemblyLimits};
use crate::conversation::ConversationAggregator;
use crate::decoder::Decoder;
use crate::error::{PacketSageError, Result};
use crate::model::{Endpoint, PacketRecord, SessionKey, TransportProto};
use crate::query::{
    guess_content_type, printable_preview, ranges_to_pairs, CaptureSummary, PacketIndex,
    PayloadProvider, PayloadReader, PayloadRequest, ProtocolStats, StreamPreview,
    StreamReconstructor,
};
use crate::reader::{CaptureFormat, CaptureReader, SourceItem};
use crate::reassembly::{Direction, OverlapPolicy, Reassembler, Segment, SegmentVerdict};

/// Per-task counters that are not part of the session aggregation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskCounters {
    /// Decode errors grouped by layer and code.
    pub decode_errors: BTreeMap<(Layer, DecodeErrorCode), u64>,
    /// Packets whose capture length was shorter than the original length.
    pub truncated_packets: u64,
    /// Non-fatal reader events.
    pub reader_nonfatal: u64,
    /// Sessions dropped because the global session cap was reached.
    pub dropped_sessions: u64,
    /// Rule windows evicted by the window budget.
    pub rule_window_evictions: u64,
    /// Findings rejected by the submission validator.
    pub submit_rejects: u64,
    /// Stream states observed while aggregating.
    pub stream_states: BTreeMap<StreamState, u64>,
    /// Sessions with a hole or an overflowed buffer.
    pub incomplete_sessions: u64,
}

/// Metadata of the agent run that produced a report (three fixed values plus
/// usage), recorded through `submit_report_meta` (M2v0.2 §9.3 / M3~M6 §5.1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportMeta {
    /// Agent run id.
    pub agent_run_id: String,
    /// Model name.
    pub model: String,
    /// Provider kind (`mock` / `openai` / `local`).
    pub provider: String,
    /// Sampling temperature (0 for the supported providers).
    pub temperature: f64,
    /// Prompt version.
    pub prompt_version: String,
    /// Template version.
    pub template_version: String,
    /// Report path.
    pub report_path: String,
    /// `ok` / `degraded`.
    pub status: String,
    /// Anti-hallucination replacements applied to the body.
    pub unverified_count: u64,
    /// Input tokens.
    pub tokens_in: u64,
    /// Output tokens.
    pub tokens_out: u64,
    /// Estimated cost in cents.
    pub cost_cents: u64,
}

/// Everything the engine knows about one analysed capture.
#[derive(Debug)]
pub struct AnalysisStore {
    /// Task identifier.
    pub task_id: String,
    /// Source file.
    pub source_path: PathBuf,
    /// SHA-256 of the source file.
    pub source_sha256: String,
    /// Container format.
    pub format: CaptureFormat,
    /// Interfaces declared by the file.
    pub interfaces: Vec<InterfaceInfo>,
    /// Wall-clock start time (RFC3339) supplied by the caller.
    pub started_at: String,
    /// Engine configuration snapshot.
    pub config: EngineConfig,
    /// Capture summary (completed when the task finishes).
    pub summary: CaptureSummary,
    /// Session aggregation.
    pub sessions: ConversationAggregator,
    /// Packet index (metadata only).
    pub index: PacketIndex,
    /// Protocol statistics.
    pub stats: ProtocolStats,
    /// Counters.
    pub counters: TaskCounters,
    /// Alerts in emission order.
    pub alerts: Vec<AlertEvent>,
    /// Findings stored through `submit_finding`.
    pub findings: Vec<Finding>,
    /// Evidence ledger used by the V1-V4 validator.
    pub ledger: EvidenceLedger,
    /// How many tool-call anchors have been minted for this task (diagnostics
    /// only; the anchors themselves are ULIDs, ADR-024).
    pub issued_tool_calls: u64,
    /// Report path recorded through `submit_report_meta`.
    pub report_path: Option<String>,
    /// Three-fixed metadata of the agent run that produced the report
    /// (model / provider / temperature / prompt version / usage).
    pub report_meta: Option<ReportMeta>,
    /// Task status: `ok` or `aborted`.
    pub status: String,
    /// Failure code when the task aborted.
    pub error_code: Option<String>,
}

impl AnalysisStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new(
        task_id: &str,
        source_path: impl Into<PathBuf>,
        source_sha256: String,
        config: EngineConfig,
    ) -> Self {
        let source_path = source_path.into();
        let summary = CaptureSummary {
            task_id: task_id.to_owned(),
            source_path: source_path.to_string_lossy().to_string(),
            source_sha256: source_sha256.clone(),
            ..CaptureSummary::default()
        };
        Self {
            task_id: task_id.to_owned(),
            source_path,
            source_sha256,
            format: CaptureFormat::Pcap,
            interfaces: Vec::new(),
            started_at: String::new(),
            config,
            summary,
            sessions: ConversationAggregator::new(),
            index: PacketIndex::new(),
            stats: ProtocolStats::new(),
            counters: TaskCounters::default(),
            alerts: Vec::new(),
            findings: Vec::new(),
            ledger: EvidenceLedger::new(),
            issued_tool_calls: 0,
            report_path: None,
            report_meta: None,
            status: "ok".to_owned(),
            error_code: None,
        }
    }

    /// Decode error rows, most frequent first.
    #[must_use]
    pub fn decode_error_counts(&self, top: usize) -> Vec<DecodeErrorCount> {
        let mut rows: Vec<DecodeErrorCount> = self
            .counters
            .decode_errors
            .iter()
            .map(|((layer, code), count)| DecodeErrorCount {
                layer: *layer,
                code: *code,
                count: *count,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| (a.layer, a.code).cmp(&(b.layer, b.code)))
        });
        rows.truncate(top);
        rows
    }

    /// Records one decode error.
    pub fn note_decode_error(&mut self, layer: Layer, code: DecodeErrorCode) {
        let entry = self
            .counters
            .decode_errors
            .entry((layer, code))
            .or_insert(0);
        *entry = entry.saturating_add(1);
        self.summary.decode_errors = self.summary.decode_errors.saturating_add(1);
    }

    /// Records a `StreamState` observation.
    pub fn note_stream_state(&mut self, state: StreamState) {
        let entry = self.counters.stream_states.entry(state).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    /// Total number of alerts.
    #[must_use]
    pub fn alert_count(&self) -> u64 {
        self.alerts.len() as u64
    }

    /// Alerts filtered by severity, rule and session.
    #[must_use]
    pub fn filter_alerts(
        &self,
        severity: Option<&str>,
        rule_id: Option<&str>,
        session_id: Option<&str>,
    ) -> Vec<AlertEvent> {
        self.alerts
            .iter()
            .filter(|alert| {
                severity.map_or(true, |s| alert.severity.eq_ignore_ascii_case(s))
                    && rule_id.map_or(true, |r| alert.rule_id == r)
                    && session_id.map_or(true, |s| alert.session_id.as_deref() == Some(s))
            })
            .cloned()
            .collect()
    }

    /// Applies a validation outcome to a draft and stores the finding.
    ///
    /// # Errors
    /// Returns [`PacketSageError`] when the draft must be rejected.
    pub fn store_finding(
        &mut self,
        draft: packetsage_protocol::FindingDraft,
        outcome: &ValidationOutcome,
    ) -> Result<Finding> {
        if outcome.status == ValidatorStatus::Rejected {
            self.counters.submit_rejects = self.counters.submit_rejects.saturating_add(1);
            return Err(PacketSageError::InvalidArgument(format!(
                "finding rejected: {}",
                outcome
                    .issues
                    .iter()
                    .map(|i| i.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        let finding = Finding {
            finding_id: packetsage_protocol::finding_id(self.findings.len() as u64 + 1),
            task_id: self.task_id.clone(),
            title: draft.title,
            severity: draft.severity,
            basis: outcome.basis,
            summary: draft.summary,
            evidence: draft.evidence,
            validator_status: outcome.status,
        };
        self.findings.push(finding.clone());
        Ok(finding)
    }

    /// Mints the next tool-call anchor `tc_{ulid}` (ADR-024).
    ///
    /// ULIDs are unique by construction (48 bit timestamp + 80 bit randomness),
    /// so anchors stay unique across tasks and across `serve` restarts without
    /// persisting any cursor.
    pub fn next_tool_call_id(&mut self) -> String {
        self.issued_tool_calls = self.issued_tool_calls.saturating_add(1);
        packetsage_protocol::format_tool_call_id(&ulid::Ulid::new().to_string())
    }

    /// Finalises the capture summary from the accumulated counters.
    pub fn finalise_summary(&mut self, packets: u64, bytes: u64) {
        self.summary.packets = packets;
        self.summary.bytes = bytes;
        self.summary.sessions = self.sessions.len() as u64;
        self.summary.alerts = self.alerts.len() as u64;
        self.summary.truncated_packets = self.counters.truncated_packets;
        self.summary.reader_nonfatal = self.counters.reader_nonfatal;
        self.summary.dropped_sessions = self.counters.dropped_sessions;
        self.summary.incomplete_sessions = self.sessions.incomplete_count();
        self.counters.incomplete_sessions = self.summary.incomplete_sessions;
        self.summary.format = self.format.as_str().to_owned();
        self.summary.interfaces = self.interfaces.clone();
        self.summary.protocols = BTreeMap::new();
        for row in self.stats.all() {
            if matches!(
                row.layer,
                Layer::Tcp | Layer::Udp | Layer::Icmp | Layer::Icmpv6
            ) {
                self.summary
                    .protocols
                    .insert(row.protocol.clone(), row.packets);
            }
        }
        self.summary.first_ts_ns = self.index.packets().first().map(|p| p.ts_ns.to_string());
        self.summary.last_ts_ns = self.index.packets().last().map(|p| p.ts_ns.to_string());
        self.summary.duration_s = match (
            self.summary.first_ts_ns.as_ref(),
            self.summary.last_ts_ns.as_ref(),
        ) {
            (Some(first), Some(last)) => {
                let first = first.parse::<i128>().unwrap_or(0);
                let last = last.parse::<i128>().unwrap_or(0);
                crate::query::duration_seconds(first, last)
            }
            _ => 0.0,
        };
    }

    /// Reassembly limits used for single-session replay.
    #[must_use]
    pub fn replay_limits(&self) -> ReassemblyLimits {
        self.config.reassembly.clone()
    }

    /// Records a reassembly verdict against a session.
    pub fn note_verdict(&mut self, key: &SessionKey, verdict: SegmentVerdict) {
        self.sessions.note_verdict(key, verdict);
    }

    /// Records the reassembly state of a session.
    pub fn note_session_state(&mut self, key: &SessionKey, state: StreamState) {
        self.sessions.note_state(key, state);
    }

    /// Schema version of the event stream produced by this store.
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        SCHEMA_VERSION
    }

    /// Packets processed.
    #[must_use]
    pub fn packets(&self) -> u64 {
        self.summary.packets
    }

    /// Alerts grouped by rule id.
    #[must_use]
    pub fn alerts_by_rule(&self) -> HashMap<String, Vec<&AlertEvent>> {
        let mut map: HashMap<String, Vec<&AlertEvent>> = HashMap::new();
        for alert in &self.alerts {
            map.entry(alert.rule_id.clone()).or_default().push(alert);
        }
        map
    }

    /// Folds one decoded packet into the index, the statistics and the sessions.
    pub fn observe_packet(&mut self, record: &PacketRecord<'_>) {
        use crate::query::{flag_bit, PacketMeta};
        use packetsage_protocol::{AppProto, NetProto};

        if record.captured_len < record.original_len {
            self.counters.truncated_packets = self.counters.truncated_packets.saturating_add(1);
        }
        let bytes = u64::from(record.captured_len);
        if let Some(link) = record.decoded.link.as_ref() {
            if link.src_mac.is_some() {
                self.stats.add(Layer::Link, "ethernet", bytes);
            } else {
                self.stats.add(Layer::Link, "linux_sll", bytes);
            }
        }
        if let Some(network) = record.decoded.network.as_ref() {
            let layer = if network.ip_version == 6 {
                Layer::Ipv6
            } else {
                Layer::Ipv4
            };
            let name = match network.protocol {
                NetProto::Arp => "arp",
                _ if network.ip_version == 6 => "ipv6",
                _ => "ipv4",
            };
            self.stats.add(layer, name, bytes);
            match network.protocol {
                NetProto::Tcp => self.stats.add(Layer::Tcp, "tcp", bytes),
                NetProto::Udp => self.stats.add(Layer::Udp, "udp", bytes),
                NetProto::Icmp => self.stats.add(Layer::Icmp, "icmp", bytes),
                NetProto::Icmpv6 => self.stats.add(Layer::Icmpv6, "icmpv6", bytes),
                NetProto::Arp => self.stats.add(Layer::Arp, "arp", bytes),
                _ => {}
            }
        }
        if let Some(app) = record.decoded.application.as_ref() {
            let (layer, name) = match app.protocol {
                AppProto::Dns => (Layer::Dns, "dns"),
                AppProto::Http => (Layer::Http, "http"),
                AppProto::Tls => (Layer::Tls, "tls"),
                AppProto::Dhcp => (Layer::Dhcp, "dhcp"),
                AppProto::Unknown => (Layer::Link, ""),
            };
            if app.protocol != AppProto::Unknown {
                self.stats.add(layer, name, bytes);
            }
        }

        let network = record.decoded.network.as_ref();
        let transport = record.decoded.transport.as_ref();
        let mut flags = 0u8;
        if let Some(transport) = transport {
            for flag in &transport.flags {
                flags |= flag_bit(*flag);
            }
        }
        let src = network.zip(transport).and_then(|(n, t)| {
            n.src.parse::<std::net::IpAddr>().ok().map(|ip| Endpoint {
                ip,
                port: t.src_port.unwrap_or(0),
            })
        });
        let dst = network.zip(transport).and_then(|(n, t)| {
            n.dst.parse::<std::net::IpAddr>().ok().map(|ip| Endpoint {
                ip,
                port: t.dst_port.unwrap_or(0),
            })
        });
        self.index.push(PacketMeta {
            packet_index: record.packet_index,
            ts_ns: record.ts_ns,
            captured_len: record.captured_len,
            original_len: record.original_len,
            interface_id: record.interface_id,
            protocol: network.map_or(NetProto::Other, |n| n.protocol),
            src,
            dst,
            flags,
            app: record.decoded.application.as_ref().map(|a| a.protocol),
            decode_status: record.decode_status,
            payload: record.payload_ref(),
            truncated: record.captured_len < record.original_len,
        });
        let _ = self.sessions.observe(record);
    }
}

impl StreamReconstructor for AnalysisStore {
    fn reconstruct(
        &self,
        session_id: &str,
        direction: Direction,
        max_bytes: usize,
    ) -> Result<StreamPreview> {
        let entry = self.sessions.get(session_id).ok_or_else(|| {
            PacketSageError::InvalidArgument(format!("unknown session {session_id}"))
        })?;
        let key: SessionKey = entry.key;
        if key.proto != TransportProto::Tcp {
            return Err(PacketSageError::InvalidArgument(
                "stream reconstruction is only defined for TCP sessions".to_owned(),
            ));
        }
        let client = entry.client_endpoint();
        let mut reader = CaptureReader::open(&self.source_path, 1 << 20)?;
        let decoder = Decoder::all();
        let mut reassembler = Reassembler::new(self.replay_limits(), OverlapPolicy::FirstWins);
        let mut delivered: Vec<u8> = Vec::new();

        reader.drive(|item, _interfaces| {
            let SourceItem::Packet(raw) = item else {
                return Ok(());
            };
            let decoded = decoder.decode(raw.linktype, raw.data);
            let (Some(network), Some(transport)) =
                (decoded.network.as_ref(), decoded.transport.as_ref())
            else {
                return Ok(());
            };
            let (Ok(src_ip), Ok(dst_ip)) = (network.src.parse(), network.dst.parse()) else {
                return Ok(());
            };
            let src = Endpoint {
                ip: src_ip,
                port: transport.src_port.unwrap_or(0),
            };
            let dst = Endpoint {
                ip: dst_ip,
                port: transport.dst_port.unwrap_or(0),
            };
            if SessionKey::canonicalize(key.proto, src, dst) != key {
                return Ok(());
            }
            let segment_dir = if src == client {
                Direction::ClientToServer
            } else {
                Direction::ServerToClient
            };
            if segment_dir != direction {
                return Ok(());
            }
            if network.protocol != packetsage_protocol::NetProto::Tcp {
                return Ok(());
            }
            // Replay must observe FIN/RST/SYN as well, so zero length segments
            // are fed with an empty payload instead of being skipped.
            let payload = decoded.payload.unwrap_or(&[]);
            let _ = reassembler.feed(
                &key,
                direction,
                Segment {
                    seq: transport.seq.unwrap_or(0),
                    data: payload,
                    ts_ns: raw.ts_ns,
                    fin: transport.flags.contains(&packetsage_protocol::TcpFlag::Fin),
                    rst: transport.flags.contains(&packetsage_protocol::TcpFlag::Rst),
                    syn: transport.flags.contains(&packetsage_protocol::TcpFlag::Syn),
                },
            );
            for chunk in reassembler.take_contiguous(&key, direction) {
                if delivered.len() < max_bytes {
                    let room = max_bytes - delivered.len();
                    delivered.extend_from_slice(&chunk[..chunk.len().min(room)]);
                }
            }
            Ok(())
        })?;

        let missing = reassembler.missing_ranges(&key, direction);
        let state = reassembler.state(&key);
        let status = match state {
            StreamState::BufferOverflow => "buffer_overflow",
            StreamState::Incomplete => "incomplete",
            _ => "complete",
        };
        let (preview, _) = printable_preview(&delivered, max_bytes);
        Ok(StreamPreview {
            session_id: session_id.to_owned(),
            direction: direction.as_str().to_owned(),
            status: status.to_owned(),
            bytes: delivered.len() as u64,
            content_type: guess_content_type(&delivered),
            preview,
            missing_ranges: ranges_to_pairs(&missing),
        })
    }
}

impl PayloadProvider for AnalysisStore {
    fn read_many(&self, requests: &[PayloadRequest]) -> Result<HashMap<u64, Vec<u8>>> {
        PayloadReader::new(self.source_path.clone()).read_many(requests)
    }
}

/// Reads the SHA-256 and size of a capture file.
///
/// # Errors
/// Returns [`PacketSageError::Io`] when the file cannot be read.
pub fn capture_fingerprint(path: &Path) -> Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    Ok((hex_lower(&digest), bytes.len() as u64))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_error_counts_are_sorted() {
        let mut store = AnalysisStore::new(
            "task_TEST0000000000000000000000",
            "x.pcap",
            "deadbeef".to_owned(),
            EngineConfig::default(),
        );
        store.note_decode_error(Layer::Ipv4, DecodeErrorCode::TruncatedHeader);
        store.note_decode_error(Layer::Ipv4, DecodeErrorCode::TruncatedHeader);
        store.note_decode_error(Layer::Tcp, DecodeErrorCode::BadLength);
        let rows = store.decode_error_counts(5);
        assert_eq!(rows[0].count, 2);
        assert_eq!(rows[0].layer, Layer::Ipv4);
        assert_eq!(store.summary.decode_errors, 3);
    }

    #[test]
    fn fingerprint_is_sha256_hex() {
        let path = std::env::temp_dir().join("packetsage-fingerprint-test.bin");
        std::fs::write(&path, b"abc").expect("write");
        let (hash, size) = capture_fingerprint(&path).expect("fingerprint");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(size, 3);
        let _ = std::fs::remove_file(&path);
    }
}
