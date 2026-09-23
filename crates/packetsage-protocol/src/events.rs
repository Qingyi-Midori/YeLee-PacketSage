//! `EngineEvent`: the JSONL event stream contract (开发文档 §8, M0~M2 §3.2).

use serde::{Deserialize, Serialize};

/// Timestamp precision of the original capture record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TsPrecision {
    /// Nanoseconds.
    Ns,
    /// Microseconds.
    Us,
    /// Milliseconds.
    Ms,
    /// Seconds.
    S,
}

/// Decode outcome of a single packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeStatus {
    /// Every layer in the protocol matrix decoded.
    Ok,
    /// Some layers decoded, some produced errors.
    Partial,
    /// No layer decoded successfully.
    Error,
}

/// Layer identifiers used by decode errors and protocol statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// Ethernet / SLL / VLAN.
    Link,
    /// 802.1Q VLAN tag (used when reporting VLAN-specific errors).
    Vlan,
    /// ARP.
    Arp,
    /// IPv4.
    Ipv4,
    /// IPv6.
    Ipv6,
    /// TCP.
    Tcp,
    /// UDP.
    Udp,
    /// ICMP.
    Icmp,
    /// ICMPv6.
    Icmpv6,
    /// DNS.
    Dns,
    /// HTTP/1.1.
    Http,
    /// TLS record metadata.
    Tls,
    /// DHCP.
    Dhcp,
}

/// First-batch decode error codes (M0~M2 §3.2). New codes must be added here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeErrorCode {
    /// Header shorter than the protocol requires.
    TruncatedHeader,
    /// Header length field inconsistent with the captured bytes.
    BadLength,
    /// A field value is invalid for this protocol.
    InvalidField,
    /// Link type outside the supported matrix.
    UnsupportedLinktype,
    /// Protocol outside the supported matrix.
    UnsupportedProtocol,
}

/// One decode error attached to a layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeErrorInfo {
    /// Failing layer.
    pub layer: Layer,
    /// Machine-readable code.
    pub code: DecodeErrorCode,
    /// Human readable detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// TCP control flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TcpFlag {
    /// FIN.
    Fin,
    /// SYN.
    Syn,
    /// RST.
    Rst,
    /// PSH.
    Psh,
    /// ACK.
    Ack,
    /// URG.
    Urg,
    /// ECE.
    Ece,
    /// CWR.
    Cwr,
}

/// Network-layer protocol discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetProto {
    /// TCP.
    Tcp,
    /// UDP.
    Udp,
    /// ICMP (IPv4).
    Icmp,
    /// ICMPv6.
    Icmpv6,
    /// ARP.
    Arp,
    /// IPv4 header only (no transport decoded).
    Ipv4,
    /// IPv6 header only (no transport decoded).
    Ipv6,
    /// Anything else.
    Other,
}

/// Application-layer protocol discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppProto {
    /// DNS.
    Dns,
    /// HTTP/1.1.
    Http,
    /// TLS record layer.
    Tls,
    /// DHCP.
    Dhcp,
    /// Detected but not classified.
    Unknown,
}

/// Link-layer summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkInfo {
    /// Source MAC address (Ethernet only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_mac: Option<String>,
    /// Destination MAC address (Ethernet only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_mac: Option<String>,
    /// EtherType (Ethernet only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ethertype: Option<u16>,
    /// 802.1Q VLAN identifiers, outermost first (QinQ keeps both).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vlan_ids: Vec<u16>,
    /// Linux cooked capture metadata when linktype 113 was used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sll_protocol: Option<u16>,
}

/// Network-layer summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkInfo {
    /// Source address.
    pub src: String,
    /// Destination address.
    pub dst: String,
    /// Protocol discriminator.
    pub protocol: NetProto,
    /// IP version (4 or 6).
    pub ip_version: u8,
    /// IPv4 TTL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u8>,
    /// IPv6 hop limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hop_limit: Option<u8>,
    /// True when this packet carries a fragment (IPv4 MF/offset or IPv6 frag header).
    pub is_fragment: bool,
}

/// ICMP / ICMPv6 summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IcmpInfo {
    /// ICMP type.
    pub icmp_type: u8,
    /// ICMP code.
    pub code: u8,
}

/// Transport-layer summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportInfo {
    /// Source port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_port: Option<u16>,
    /// Destination port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_port: Option<u16>,
    /// TCP flags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<TcpFlag>,
    /// TCP sequence number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u32>,
    /// TCP acknowledgement number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ack: Option<u32>,
    /// TCP window size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<u16>,
    /// UDP payload length field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub udp_len: Option<u16>,
    /// ICMP metadata when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icmp: Option<IcmpInfo>,
}

/// DNS metadata (single-packet parseable fields only, M2v0.2 §3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DnsDetail {
    /// Transaction id.
    pub transaction_id: u16,
    /// True for responses.
    pub is_response: bool,
    /// Query name (first question), lowercase, no trailing dot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qname: Option<String>,
    /// Query type (first question).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qtype: Option<u16>,
    /// Length of the query name in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qname_len: Option<u32>,
    /// Longest label length in the query name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_label_len: Option<u32>,
    /// Shannon entropy of the query name (bits/char), rounded to 3 decimals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entropy: Option<f32>,
    /// Total length of TXT record data in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txt_len: Option<u32>,
    /// Answer count from the header.
    pub answer_count: u16,
}

/// HTTP/1.1 metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpDetail {
    /// True when the payload starts with a request line.
    pub is_request: bool,
    /// Request method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Response status code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    /// Host header (lowercase).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// User-Agent header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Content-Length header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_length: Option<u64>,
    /// Connection header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
}

/// TLS record-layer metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsDetail {
    /// Record content type of the first record.
    pub record_type: u8,
    /// Legacy record version.
    pub version: u16,
    /// True when the record is a ClientHello.
    pub is_client_hello: bool,
    /// SNI when present and parseable from this single packet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
}

/// DHCP metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DhcpDetail {
    /// BOOTP message type (1 = request, 2 = reply).
    pub message_type: u8,
    /// DHCP option 53 message type when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dhcp_type: Option<u8>,
    /// Client identifier (option 61) rendered as text when printable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Requested IP (option 50).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_ip: Option<String>,
}

/// Application-layer detail payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppDetail {
    /// DNS detail.
    Dns(DnsDetail),
    /// HTTP detail.
    Http(HttpDetail),
    /// TLS detail.
    Tls(TlsDetail),
    /// DHCP detail.
    Dhcp(DhcpDetail),
}

/// Application-layer summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplicationInfo {
    /// Protocol discriminator.
    pub protocol: AppProto,
    /// Protocol-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<AppDetail>,
}

/// Location of the application payload inside the original capture record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadRef {
    /// Packet index the offset refers to.
    pub packet_index: u64,
    /// Offset relative to the link-layer start of the record.
    pub offset: u32,
    /// Payload length in bytes.
    pub length: u32,
}

/// Task started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStartedEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// Task identifier.
    pub task_id: String,
    /// Wall-clock start timestamp (RFC3339, diagnostic only).
    pub started_at: String,
    /// Absolute path of the analysed capture.
    pub source_path: String,
    /// Engine version.
    pub engine_version: String,
}

/// One interface discovered in the capture file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfaceInfo {
    /// Interface id (0-based, file order).
    pub interface_id: u32,
    /// Link type of the interface.
    pub linktype: u32,
    /// Snap length when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snaplen: Option<u32>,
    /// Timestamp resolution expressed in ticks per second.
    pub ticks_per_second: u64,
}

/// Capture-level metadata, emitted once after the format is known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureInfoEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// Task identifier.
    pub task_id: String,
    /// `pcap` or `pcapng`.
    pub format: String,
    /// Interfaces declared by the file.
    pub interfaces: Vec<InterfaceInfo>,
    /// First packet timestamp (canonical ns) when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_ts_unix_ns: Option<String>,
    /// Last packet timestamp (canonical ns) when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ts_unix_ns: Option<String>,
    /// SHA-256 of the capture file.
    pub source_sha256: String,
}

/// One decoded packet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PacketEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// Task identifier.
    pub task_id: String,
    /// Zero-based index inside the capture file.
    pub packet_index: u64,
    /// Canonical nanosecond timestamp as a string (JS number safety).
    pub ts_unix_ns: String,
    /// Original timestamp precision.
    pub ts_precision: TsPrecision,
    /// Interface the packet was captured on.
    pub interface_id: u32,
    /// Captured length.
    pub captured_len: u32,
    /// Original length on the wire.
    pub original_len: u32,
    /// Link type of the capturing interface.
    pub linktype: u32,
    /// True when `captured_len < original_len`.
    pub truncated: bool,
    /// Link-layer summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkInfo>,
    /// Network-layer summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkInfo>,
    /// Transport-layer summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportInfo>,
    /// Application-layer summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<ApplicationInfo>,
    /// Application payload location (never the bytes themselves).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_ref: Option<PayloadRef>,
    /// Decode outcome.
    pub decode_status: DecodeStatus,
}

/// Decode failure of a single packet (one event may carry several entries).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeErrorEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// Task identifier.
    pub task_id: String,
    /// Zero-based index inside the capture file.
    pub packet_index: u64,
    /// Failing layer.
    pub layer: Layer,
    /// Machine-readable code.
    pub code: DecodeErrorCode,
    /// Human readable detail.
    pub message: String,
}

/// Reassembly state of a TCP session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamState {
    /// Session seen but no data yet.
    New,
    /// Data flowing.
    Active,
    /// One direction closed.
    HalfClosed,
    /// Both directions closed cleanly.
    Closed,
    /// Missing data or abnormal close: payload must not be presented as complete.
    Incomplete,
    /// Buffered data exceeded the per-stream budget.
    BufferOverflow,
}

/// Why a session's direction was chosen (M2v0.2 §4.3, wire values in
/// `snake_case`; `PortHeuristic` and `ConfigOverride` are reserved for future
/// heuristics and are never produced by the current aggregator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectionBasis {
    /// Client is the sender of the first bare SYN.
    SynFirst,
    /// Client was inferred from a low source port (reserved).
    PortHeuristic,
    /// No SYN was observed; the sender of the first seen packet is the client.
    FirstSeen,
    /// The direction came from configuration (reserved).
    ConfigOverride,
}

/// Aggregated session summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummaryEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// Task identifier.
    pub task_id: String,
    /// `S-{n:06}`.
    pub session_id: String,
    /// `tcp` or `udp`.
    pub protocol: String,
    /// Lexically smaller endpoint address.
    pub src_ip: String,
    /// Lexically smaller endpoint port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_port: Option<u16>,
    /// Lexically larger endpoint address.
    pub dst_ip: String,
    /// Lexically larger endpoint port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_port: Option<u16>,
    /// First packet timestamp (canonical ns).
    pub first_ts_ns: String,
    /// Last packet timestamp (canonical ns).
    pub last_ts_ns: String,
    /// Total packets in both directions.
    pub packets: u64,
    /// Total bytes in both directions.
    pub bytes: u64,
    /// Packets from the client side.
    pub src_packets: u64,
    /// Packets from the server side.
    pub dst_packets: u64,
    /// Bytes from the client side.
    pub src_bytes: u64,
    /// Bytes from the server side.
    pub dst_bytes: u64,
    /// Bare SYN count.
    pub syn_count: u32,
    /// SYN+ACK count.
    pub syn_ack_count: u32,
    /// RST count.
    pub rst_count: u32,
    /// FIN count.
    pub fin_count: u32,
    /// Retransmissions observed by the reassembler.
    pub retransmission_count: u32,
    /// Out-of-order segments observed by the reassembler.
    pub out_of_order_count: u32,
    /// Reassembly state.
    pub state: StreamState,
    /// First recognised application protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_protocol: Option<AppProto>,
    /// How client/server roles were assigned.
    pub direction_basis: DirectionBasis,
    /// Interface the session was first seen on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_id: Option<u32>,
    /// Outermost VLAN id when the session was first seen on a tagged frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlan_tag: Option<u16>,
}

/// Stream-level anomaly (does not abort the task).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamStateEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// Task identifier.
    pub task_id: String,
    /// Session the anomaly belongs to (absent when the session cap was hit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Session key rendered as `proto|ip:port|ip:port`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// State the session moved to.
    pub state: StreamState,
    /// Machine readable reason.
    pub reason: String,
    /// Human readable detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One row of protocol statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolCount {
    /// Layer the counter belongs to.
    pub layer: Layer,
    /// Protocol name inside that layer.
    pub protocol: String,
    /// Matching packet count.
    pub packets: u64,
    /// Matching byte count.
    pub bytes: u64,
}

/// One row of decode-error statistics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeErrorCount {
    /// Failing layer.
    pub layer: Layer,
    /// Machine readable code.
    pub code: DecodeErrorCode,
    /// Occurrences.
    pub count: u64,
}

/// Aggregate statistics for a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// Task identifier.
    pub task_id: String,
    /// Packets processed in total.
    pub packets: u64,
    /// Bytes captured in total.
    pub bytes: u64,
    /// Sessions (TCP + UDP) observed.
    pub sessions: u64,
    /// Per-layer protocol counters.
    pub protocol_stats: Vec<ProtocolCount>,
    /// Decode errors grouped by layer/code.
    pub decode_errors: Vec<DecodeErrorCount>,
    /// Packets whose capture length was shorter than the original length.
    pub truncated_packets: u64,
    /// Non-fatal reader events (unknown blocks etc.).
    pub reader_nonfatal: u64,
    /// Sessions whose payload could not be reconstructed completely.
    pub incomplete_sessions: u64,
    /// Sessions dropped because the session cap was reached.
    pub dropped_sessions: u64,
    /// Rule windows evicted by `WindowBudget`.
    pub rule_window_evictions: u64,
    /// Findings rejected by the submission validator.
    pub submit_rejects: u64,
    /// Top conversations by bytes.
    pub top_conversations: Vec<serde_json::Value>,
}

/// Alert evidence block (M3~M6 §3.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertEvidence {
    /// Metric primitive that fired.
    pub metric: String,
    /// Observed metric value.
    pub value: f64,
    /// Window length in nanoseconds (string for JS safety).
    pub window_ns: String,
    /// Comparison operator.
    pub operator: String,
    /// Configured threshold.
    pub threshold: f64,
    /// Sample packet indices inside the window, ascending, bounded.
    #[serde(default)]
    pub sample_packets: Vec<u64>,
    /// True when the window budget forced evidence degradation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub degraded: bool,
    /// Ratio numerator (window-local), present only for `metric: ratio`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numerator: Option<f64>,
    /// Ratio denominator (window-local), present only for `metric: ratio`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denominator: Option<f64>,
    /// Session state copied from the SessionSummary event (scope: session only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_state: Option<StreamState>,
}

/// A rule match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// `alert_{ulid}`.
    pub alert_id: String,
    /// Task identifier.
    pub task_id: String,
    /// Rule id.
    pub rule_id: String,
    /// Rule version.
    pub rule_version: u32,
    /// First 8 hex chars of the rule content hash.
    pub rule_content_hash: String,
    /// Severity.
    pub severity: String,
    /// First packet index in the window.
    pub first_packet: u64,
    /// Last packet index in the window.
    pub last_packet: u64,
    /// Window start timestamp (canonical ns).
    pub first_ts_ns: String,
    /// Window end timestamp (canonical ns).
    pub last_ts_ns: String,
    /// Group key as ordered pairs.
    pub group_key: Vec<(String, String)>,
    /// Rule name from the YAML file.
    pub rule_name: String,
    /// Evidence.
    pub evidence: AlertEvidence,
    /// Session id for scope: session rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Final event of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskFinishedEvent {
    /// Event schema version.
    pub schema_version: u32,
    /// Task identifier.
    pub task_id: String,
    /// `ok` or `aborted`.
    pub status: String,
    /// Packets processed (including packets with decode errors).
    pub packets: u64,
    /// Bytes captured.
    pub bytes: u64,
    /// Sessions observed.
    pub sessions: u64,
    /// Alerts emitted.
    pub alerts: u64,
    /// Decode errors emitted.
    pub decode_errors: u64,
    /// Failure code when `status == "aborted"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// First packet timestamp in nanoseconds, as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_ts_ns: Option<String>,
    /// Last packet timestamp in nanoseconds, as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ts_ns: Option<String>,
}

/// Top-level event enum. The serde tag is fixed to `event`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Task startup marker.
    TaskStarted(TaskStartedEvent),
    /// Capture metadata.
    CaptureInfo(CaptureInfoEvent),
    /// One decoded packet.
    Packet(PacketEvent),
    /// One packet-level decode failure.
    DecodeError(DecodeErrorEvent),
    /// Session aggregation result.
    SessionSummary(SessionSummaryEvent),
    /// Stream-level anomaly.
    StreamState(StreamStateEvent),
    /// Aggregate statistics.
    Stats(StatsEvent),
    /// Rule alert (M3).
    Alert(AlertEvent),
    /// Task teardown marker.
    TaskFinished(TaskFinishedEvent),
}

impl EngineEvent {
    /// Borrows the task id carried by every event variant.
    #[must_use]
    pub fn task_id(&self) -> &str {
        match self {
            EngineEvent::TaskStarted(e) => &e.task_id,
            EngineEvent::CaptureInfo(e) => &e.task_id,
            EngineEvent::Packet(e) => &e.task_id,
            EngineEvent::DecodeError(e) => &e.task_id,
            EngineEvent::SessionSummary(e) => &e.task_id,
            EngineEvent::StreamState(e) => &e.task_id,
            EngineEvent::Stats(e) => &e.task_id,
            EngineEvent::Alert(e) => &e.task_id,
            EngineEvent::TaskFinished(e) => &e.task_id,
        }
    }

    /// Borrows the canonical packet timestamp of packet-scoped events.
    #[must_use]
    pub fn packet_ts_ns(&self) -> Option<i128> {
        match self {
            EngineEvent::Packet(e) => e.ts_unix_ns.parse::<i128>().ok(),
            EngineEvent::DecodeError(_) => None,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_event_serialises_with_event_tag() {
        let ev = EngineEvent::Packet(PacketEvent {
            schema_version: crate::SCHEMA_VERSION,
            task_id: "task_TEST00000000000000000000".to_owned(),
            packet_index: 0,
            ts_unix_ns: "1717689600123456789".to_owned(),
            ts_precision: TsPrecision::Us,
            interface_id: 0,
            captured_len: 74,
            original_len: 74,
            linktype: 1,
            truncated: false,
            link: None,
            network: Some(NetworkInfo {
                src: "10.0.0.1".to_owned(),
                dst: "10.0.0.2".to_owned(),
                protocol: NetProto::Tcp,
                ip_version: 4,
                ttl: Some(64),
                hop_limit: None,
                is_fragment: false,
            }),
            transport: None,
            application: None,
            payload_ref: None,
            decode_status: DecodeStatus::Ok,
        });
        let json = serde_json::to_string(&ev).expect("serialisable");
        assert!(json.contains("\"event\":\"packet\""));
        assert!(json.contains("\"ts_unix_ns\":\"1717689600123456789\""));
        assert!(json.contains("\"linktype\":1"));
    }

    #[test]
    fn ts_precision_and_flags_are_snake_upper() {
        assert_eq!(
            serde_json::to_string(&TsPrecision::Us).expect("ok"),
            "\"us\""
        );
        assert_eq!(serde_json::to_string(&TcpFlag::Syn).expect("ok"), "\"SYN\"");
        assert_eq!(
            serde_json::to_string(&Layer::Icmpv6).expect("ok"),
            "\"icmpv6\""
        );
    }

    #[test]
    fn direction_basis_wire_values_match_the_frozen_enum() {
        for (value, wire) in [
            (DirectionBasis::SynFirst, "\"syn_first\""),
            (DirectionBasis::PortHeuristic, "\"port_heuristic\""),
            (DirectionBasis::FirstSeen, "\"first_seen\""),
            (DirectionBasis::ConfigOverride, "\"config_override\""),
        ] {
            assert_eq!(serde_json::to_string(&value).expect("serde"), wire);
        }
    }
}
