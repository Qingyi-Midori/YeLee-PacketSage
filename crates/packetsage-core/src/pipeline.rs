//! `reader -> decoder -> reassembly -> conversation -> rules -> sink`
//! (M0~M2 §4.8).

use std::io::Write;
use std::path::Path;

use packetsage_protocol::{
    AlertEvent, AppProto, CaptureInfoEvent, DecodeErrorEvent, EngineEvent, InterfaceInfo, Layer,
    PacketEvent, StatsEvent, StreamState, StreamStateEvent, TaskFinishedEvent, TaskId,
    TaskStartedEvent, TsPrecision, SCHEMA_VERSION,
};
use serde::Serialize;

use crate::config::{EngineConfig, PacketEventMode};
use crate::decoder::{DecodeOptions, Decoder};
use crate::error::{PacketSageError, Result};
use crate::ids;
use crate::model::PacketRecord;
use crate::reader::{
    CaptureFormat, CaptureReader, SourceItem, LINKTYPE_ETHERNET, LINKTYPE_LINUX_SLL,
};
use crate::reassembly::{CloseHow, Direction, OverlapPolicy, Reassembler, Segment};
use crate::store::{capture_fingerprint, AnalysisStore};

/// Max number of `StreamState` anomaly events emitted per task.
const MAX_STREAM_STATE_EVENTS: usize = 1_000;

/// Where events go.
pub trait EventSink {
    /// Emits one event.
    ///
    /// # Errors
    /// Propagates the writer error.
    fn emit(&mut self, event: &EngineEvent) -> std::io::Result<()>;
}

/// Writes one JSON object per line (stdout or a file).
pub struct JsonlSink<W: Write> {
    writer: W,
    emitted: u64,
    limit: u64,
}

impl<W: Write> JsonlSink<W> {
    /// Creates a JSONL sink. `limit == 0` means unlimited.
    pub fn new(writer: W, limit: u64) -> Self {
        Self {
            writer,
            emitted: 0,
            limit,
        }
    }

    /// Number of emitted events.
    #[must_use]
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Flushes the underlying writer.
    ///
    /// # Errors
    /// Propagates the writer error.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }

    /// Returns the underlying writer.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

impl<W: Write> EventSink for JsonlSink<W> {
    fn emit(&mut self, event: &EngineEvent) -> std::io::Result<()> {
        if self.limit != 0 && self.emitted >= self.limit {
            return Ok(());
        }
        serde_json::to_writer(&mut self.writer, event)?;
        self.writer.write_all(b"\n")?;
        self.emitted += 1;
        Ok(())
    }
}

/// Counts events without keeping them (human readable mode).
#[derive(Debug, Default)]
pub struct CountingSink {
    /// Events seen.
    pub emitted: u64,
}

impl EventSink for CountingSink {
    fn emit(&mut self, _event: &EngineEvent) -> std::io::Result<()> {
        self.emitted += 1;
        Ok(())
    }
}

/// Keeps every event in memory (tests, golden fixtures).
#[derive(Debug, Default)]
pub struct CollectingSink {
    /// Events in emission order.
    pub events: Vec<EngineEvent>,
}

impl EventSink for CollectingSink {
    fn emit(&mut self, event: &EngineEvent) -> std::io::Result<()> {
        self.events.push(event.clone());
        Ok(())
    }
}

/// Rule engine hook.
///
/// `packetsage-rules` implements this trait, which keeps the dependency arrow
/// `rules -> core` (M0~M2 §2.3) intact: core never depends on the rule engine.
pub trait RuleHook {
    /// Evaluates one event and returns the alerts it produced.
    fn evaluate(&mut self, event: &EngineEvent, now_ns: i128) -> Vec<AlertEvent>;
    /// Final evaluation before the task is closed.
    fn flush(&mut self, now_ns: i128) -> Vec<AlertEvent>;
    /// Rule windows evicted by the window budget.
    fn window_evictions(&self) -> u64 {
        0
    }
}

/// Machine readable task result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskSummary {
    /// Task identifier.
    pub task_id: String,
    /// `ok` or `aborted`.
    pub status: String,
    /// Packets processed.
    pub packets: u64,
    /// Bytes captured.
    pub bytes: u64,
    /// Sessions observed.
    pub sessions: u64,
    /// Alerts emitted.
    pub alerts: u64,
    /// Decode errors emitted.
    pub decode_errors: u64,
    /// First packet timestamp.
    pub first_ts_ns: Option<String>,
    /// Last packet timestamp.
    pub last_ts_ns: Option<String>,
    /// Failure code when aborted.
    pub error_code: Option<String>,
}

/// Phase of a running analysis, as reported by the CLI progress line (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressPhase {
    /// Decoding packets from the capture.
    #[default]
    Parse,
    /// Session summaries, stream states and the rule flush.
    Rules,
    /// Statistics and task closure.
    Finalise,
}

impl ProgressPhase {
    /// Stable lowercase name used in the progress line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ProgressPhase::Parse => "parse",
            ProgressPhase::Rules => "rules",
            ProgressPhase::Finalise => "finalise",
        }
    }

    fn from_code(code: u8) -> Self {
        match code {
            1 => ProgressPhase::Rules,
            2 => ProgressPhase::Finalise,
            _ => ProgressPhase::Parse,
        }
    }
}

/// Lock-free counters sampled from another thread.
///
/// This is the minimal progress hook allowed by CLI 收口工程规格书 §3.3 (#30):
/// plain atomics, no locks, no channels, no backpressure — the pipeline never
/// waits for a reader, so the event stream stays byte-for-byte deterministic
/// whether or not progress is being printed.
#[derive(Debug, Default)]
pub struct ProgressCounters {
    packets: std::sync::atomic::AtomicU64,
    bytes: std::sync::atomic::AtomicU64,
    phase: std::sync::atomic::AtomicU8,
}

/// One consistent-enough reading of [`ProgressCounters`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProgressSnapshot {
    /// Packets decoded so far.
    pub packets: u64,
    /// Captured bytes seen so far.
    pub bytes: u64,
    /// Current phase.
    pub phase: ProgressPhase,
}

impl ProgressCounters {
    /// Fresh counters at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts one packet (relaxed: presentation only, never a business input).
    pub fn note_packet(&self, captured_len: u64) {
        use std::sync::atomic::Ordering::Relaxed;
        self.packets.fetch_add(1, Relaxed);
        self.bytes.fetch_add(captured_len, Relaxed);
    }

    /// Moves to the next phase.
    pub fn set_phase(&self, phase: ProgressPhase) {
        use std::sync::atomic::Ordering::Relaxed;
        self.phase.store(phase as u8, Relaxed);
    }

    /// Reads the counters (order between fields is not significant).
    #[must_use]
    pub fn snapshot(&self) -> ProgressSnapshot {
        use std::sync::atomic::Ordering::Relaxed;
        ProgressSnapshot {
            packets: self.packets.load(Relaxed),
            bytes: self.bytes.load(Relaxed),
            phase: ProgressPhase::from_code(self.phase.load(Relaxed)),
        }
    }
}

/// Result of one analysis run.
pub struct AnalysisResult {
    /// Populated store, ready for queries.
    pub store: AnalysisStore,
    /// Task summary.
    pub summary: TaskSummary,
}

/// The analysis pipeline.
pub struct AnalyzePipeline {
    /// Engine configuration.
    pub config: EngineConfig,
    /// Event sink.
    pub sink: Box<dyn EventSink>,
    /// Optional rule engine.
    pub rules: Option<Box<dyn RuleHook>>,
    /// Decoder options.
    pub decode_options: DecodeOptions,
    /// Optional progress counters sampled by the CLI (§3.3).
    pub progress: Option<std::sync::Arc<ProgressCounters>>,
}

impl std::fmt::Debug for AnalyzePipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalyzePipeline")
            .field("config", &self.config)
            .field("has_rules", &self.rules.is_some())
            .field("has_progress", &self.progress.is_some())
            .finish()
    }
}

/// Aggregated statistics of an `EventSink` that only counts.
#[must_use]
pub fn counted_events(sink: &CountingSink) -> u64 {
    sink.emitted
}

/// Layer of an application protocol (used by rules and reports).
#[must_use]
pub fn app_layer(proto: AppProto) -> Layer {
    match proto {
        AppProto::Dns => Layer::Dns,
        AppProto::Http => Layer::Http,
        AppProto::Tls => Layer::Tls,
        AppProto::Dhcp => Layer::Dhcp,
        AppProto::Unknown => Layer::Link,
    }
}

/// Link types the decoder supports.
#[must_use]
pub fn supported_linktype(linktype: u32) -> bool {
    matches!(linktype, LINKTYPE_ETHERNET | LINKTYPE_LINUX_SLL)
}

/// Nominal timestamp precision of a capture format (diagnostics only).
#[must_use]
pub fn format_precision(format: CaptureFormat) -> TsPrecision {
    match format {
        CaptureFormat::Pcap => TsPrecision::Us,
        CaptureFormat::PcapNg => TsPrecision::Ns,
    }
}

/// UTC timestamp in RFC3339 with second precision.
#[must_use]
pub fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_rfc3339(now.as_secs())
}

/// Formats seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
#[must_use]
#[allow(clippy::integer_division)] // calendar arithmetic: every division is exact
pub fn format_rfc3339(seconds: u64) -> String {
    let days = seconds / 86_400;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60
    )
}

/// Howard Hinnant's civil-from-days algorithm.
#[allow(clippy::integer_division)] // calendar arithmetic: exact by construction
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

impl AnalyzePipeline {
    /// Creates a pipeline.
    pub fn new(config: EngineConfig, sink: Box<dyn EventSink>) -> Self {
        Self {
            config,
            sink,
            rules: None,
            decode_options: DecodeOptions::default(),
            progress: None,
        }
    }

    /// Attaches progress counters (presentation only, §3.3).
    #[must_use]
    pub fn with_progress(mut self, progress: std::sync::Arc<ProgressCounters>) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Attaches a rule engine.
    #[must_use]
    pub fn with_rules(mut self, rules: Box<dyn RuleHook>) -> Self {
        self.rules = Some(rules);
        self
    }

    /// Evaluates the rules (if any) and writes the event plus its alerts.
    ///
    /// `emit` is false when the configured emission policy hides this event
    /// from the sink — the rule engine still sees it, because rules are part of
    /// the deterministic pipeline, not of the output formatting.
    fn dispatch(
        &mut self,
        store: &mut AnalysisStore,
        event: EngineEvent,
        clock: i128,
        emit: bool,
    ) -> Result<()> {
        let now = if clock == i128::MIN { 0 } else { clock };
        let alerts = match self.rules.as_deref_mut() {
            Some(hook) => hook.evaluate(&event, now),
            None => Vec::new(),
        };
        if emit {
            self.sink.emit(&event)?;
        }
        for alert in alerts {
            store.alerts.push(alert.clone());
            self.sink.emit(&EngineEvent::Alert(alert))?;
        }
        Ok(())
    }

    /// Runs the pipeline over one capture file.
    ///
    /// # Errors
    /// Returns [`PacketSageError::UnsupportedCapture`] when the file cannot be
    /// opened, [`PacketSageError::CaptureCorrupted`] when the structure is
    /// damaged beyond recovery, and sink failures as [`PacketSageError::Io`].
    pub fn run(
        &mut self,
        path: &Path,
        task_id: &TaskId,
        started_at: &str,
    ) -> Result<AnalysisResult> {
        let progress = self.progress.clone();
        if let Some(counters) = &progress {
            counters.set_phase(ProgressPhase::Parse);
        }
        let (sha256, _size) = capture_fingerprint(path)?;
        let mut store =
            AnalysisStore::new(task_id.as_str(), path, sha256.clone(), self.config.clone());
        store.started_at = started_at.to_owned();

        self.sink.emit(&EngineEvent::TaskStarted(TaskStartedEvent {
            schema_version: SCHEMA_VERSION,
            task_id: task_id.as_str().to_owned(),
            started_at: started_at.to_owned(),
            source_path: path.to_string_lossy().to_string(),
            engine_version: env!("CARGO_PKG_VERSION").to_owned(),
        }))?;

        let mut reader = CaptureReader::open(path, self.config.engine.read_buffer as usize)?;
        store.format = reader.format();

        let packet_events = self.config.emit.packet_events;
        let decoder = Decoder::new(self.decode_options.clone());
        let mut reassembler =
            Reassembler::new(self.config.reassembly.clone(), OverlapPolicy::FirstWins);
        let mut clock: i128 = i128::MIN;
        let mut capture_info_emitted = false;
        let mut decode_errors_emitted: u64 = 0;

        let drive_result = reader.drive(|item, interfaces| {
            match item {
                SourceItem::NonFatal(info) => {
                    tracing::warn!(message = %info.message, "non-fatal capture event");
                    store.counters.reader_nonfatal =
                        store.counters.reader_nonfatal.saturating_add(1);
                }
                SourceItem::Packet(raw) => {
                    clock = clock.max(raw.ts_ns);
                    if !capture_info_emitted {
                        capture_info_emitted = true;
                        store.interfaces = interface_infos(interfaces);
                        let format = store.format;
                        let event = EngineEvent::CaptureInfo(CaptureInfoEvent {
                            schema_version: SCHEMA_VERSION,
                            task_id: task_id.as_str().to_owned(),
                            format: format.as_str().to_owned(),
                            interfaces: store.interfaces.clone(),
                            first_ts_unix_ns: None,
                            last_ts_unix_ns: None,
                            source_sha256: sha256.clone(),
                        });
                        self.dispatch(&mut store, event, clock, true)?;
                    }
                    let decoded = decoder.decode(raw.linktype, raw.data);
                    for error in &decoded.errors {
                        store.note_decode_error(error.layer, error.code);
                        let event = EngineEvent::DecodeError(DecodeErrorEvent {
                            schema_version: SCHEMA_VERSION,
                            task_id: task_id.as_str().to_owned(),
                            packet_index: raw.packet_index,
                            layer: error.layer,
                            code: error.code,
                            message: error
                                .message
                                .clone()
                                .unwrap_or_else(|| format!("{:?}", error.code)),
                        });
                        decode_errors_emitted = decode_errors_emitted.saturating_add(1);
                        self.dispatch(&mut store, event, clock, true)?;
                    }
                    let status = Decoder::status(&decoded);
                    let record = PacketRecord {
                        task_id: task_id.clone(),
                        packet_index: raw.packet_index,
                        ts_ns: raw.ts_ns,
                        ts_precision: raw.ts_precision,
                        interface_id: raw.interface_id,
                        captured_len: raw.caplen,
                        original_len: raw.origlen,
                        linktype: raw.linktype,
                        raw: raw.data,
                        decoded,
                        decode_status: status,
                    };
                    store.observe_packet(&record);
                    if let Some(counters) = &progress {
                        counters.note_packet(u64::from(record.captured_len));
                    }
                    observe_reassembly(&mut reassembler, &mut store, &record);
                    let emit = should_emit_packet(packet_events, &record);
                    let event = EngineEvent::Packet(packet_event(&record));
                    self.dispatch(&mut store, event, clock, emit)?;
                }
            }
            Ok(())
        });
        let reader_summary = drive_result?;

        if let Some(counters) = &progress {
            counters.set_phase(ProgressPhase::Rules);
        }
        let entries: Vec<_> = store.sessions.sessions().into_iter().cloned().collect();
        for entry in &entries {
            let event =
                EngineEvent::SessionSummary(store.sessions.summary_event(entry, task_id.as_str()));
            self.dispatch(&mut store, event, clock, true)?;
        }

        let mut anomalies = 0usize;
        for entry in &entries {
            if anomalies >= MAX_STREAM_STATE_EVENTS {
                break;
            }
            if matches!(
                entry.stats.state,
                StreamState::Incomplete | StreamState::BufferOverflow
            ) {
                anomalies += 1;
                let event = EngineEvent::StreamState(StreamStateEvent {
                    schema_version: SCHEMA_VERSION,
                    task_id: task_id.as_str().to_owned(),
                    session_id: Some(entry.session_id.clone()),
                    session_key: Some(entry.key.render()),
                    state: entry.stats.state,
                    reason: if entry.stats.state == StreamState::BufferOverflow {
                        "buffer_overflow".to_owned()
                    } else {
                        "missing_segments".to_owned()
                    },
                    message: None,
                });
                self.dispatch(&mut store, event, clock, true)?;
            }
        }
        for pruned in reassembler.prune_expired(clock.max(0)) {
            if anomalies >= MAX_STREAM_STATE_EVENTS {
                break;
            }
            anomalies += 1;
            let event = EngineEvent::StreamState(StreamStateEvent {
                schema_version: SCHEMA_VERSION,
                task_id: task_id.as_str().to_owned(),
                session_id: None,
                session_key: Some(pruned.key.render()),
                state: pruned.state,
                reason: match pruned.reason {
                    CloseHow::Timeout => "timeout".to_owned(),
                    CloseHow::Fin => "fin".to_owned(),
                    CloseHow::Rst => "rst".to_owned(),
                },
                message: None,
            });
            self.dispatch(&mut store, event, clock, true)?;
        }

        if let Some(hook) = self.rules.as_deref_mut() {
            let alerts = hook.flush(clock.max(0));
            store.counters.rule_window_evictions = hook.window_evictions();
            for alert in alerts {
                store.alerts.push(alert.clone());
                self.sink.emit(&EngineEvent::Alert(alert))?;
            }
        }

        store.counters.dropped_sessions = reassembler.dropped_sessions();
        if let Some(counters) = &progress {
            counters.set_phase(ProgressPhase::Finalise);
        }
        store.finalise_summary(reader_summary.packets, reader_summary.bytes);

        // Sort the session entries first and only then build DTOs: a 1 GB
        // capture can hold hundreds of thousands of sessions, and converting
        // all of them just to drop 99% would dominate the run time.
        let mut top_entries: Vec<&crate::conversation::SessionEntry> = store.sessions.sessions();
        top_entries.sort_by_key(|entry| std::cmp::Reverse(entry.stats.bytes));
        top_entries.truncate(20);
        let top_conversations: Vec<crate::query::ConversationDto> = top_entries
            .iter()
            .map(|entry| conversation_dto(entry))
            .collect();

        let stats_event = EngineEvent::Stats(StatsEvent {
            schema_version: SCHEMA_VERSION,
            task_id: task_id.as_str().to_owned(),
            packets: store.summary.packets,
            bytes: store.summary.bytes,
            sessions: store.summary.sessions,
            protocol_stats: store.stats.all(),
            decode_errors: store.decode_error_counts(8),
            truncated_packets: store.counters.truncated_packets,
            reader_nonfatal: store.counters.reader_nonfatal,
            incomplete_sessions: store.summary.incomplete_sessions,
            dropped_sessions: store.counters.dropped_sessions,
            rule_window_evictions: store.counters.rule_window_evictions,
            submit_rejects: store.counters.submit_rejects,
            top_conversations: top_conversations
                .iter()
                .filter_map(|c| serde_json::to_value(c).ok())
                .collect(),
        });
        self.sink.emit(&stats_event)?;

        let finished = EngineEvent::TaskFinished(TaskFinishedEvent {
            schema_version: SCHEMA_VERSION,
            task_id: task_id.as_str().to_owned(),
            status: store.status.clone(),
            packets: store.summary.packets,
            bytes: store.summary.bytes,
            sessions: store.summary.sessions,
            alerts: store.alert_count(),
            decode_errors: decode_errors_emitted,
            error_code: store.error_code.clone(),
            first_ts_ns: store.summary.first_ts_ns.clone(),
            last_ts_ns: store.summary.last_ts_ns.clone(),
        });
        self.sink.emit(&finished)?;

        let summary = TaskSummary {
            task_id: task_id.as_str().to_owned(),
            status: store.status.clone(),
            packets: store.summary.packets,
            bytes: store.summary.bytes,
            sessions: store.summary.sessions,
            alerts: store.alert_count(),
            decode_errors: decode_errors_emitted,
            first_ts_ns: store.summary.first_ts_ns.clone(),
            last_ts_ns: store.summary.last_ts_ns.clone(),
            error_code: store.error_code.clone(),
        };
        Ok(AnalysisResult { store, summary })
    }
}

fn interface_infos(interfaces: &[crate::reader::InterfaceState]) -> Vec<InterfaceInfo> {
    interfaces
        .iter()
        .map(|i| InterfaceInfo {
            interface_id: i.interface_id,
            linktype: i.linktype,
            snaplen: i.snaplen,
            ticks_per_second: i.ts_resol.ticks_per_second(),
        })
        .collect()
}

fn should_emit_packet(mode: PacketEventMode, record: &PacketRecord<'_>) -> bool {
    match mode {
        PacketEventMode::All => true,
        PacketEventMode::ErrorsOnly => !record.decoded.errors.is_empty(),
        PacketEventMode::None => false,
    }
}

fn conversation_dto(entry: &crate::conversation::SessionEntry) -> crate::query::ConversationDto {
    crate::query::ConversationDto {
        session_id: entry.session_id.clone(),
        protocol: entry.key.proto.as_str().to_owned(),
        src_ip: entry.key.a.ip.to_string(),
        src_port: entry.key.a.port,
        dst_ip: entry.key.b.ip.to_string(),
        dst_port: entry.key.b.port,
        first_ts_ns: entry.stats.first_ts_ns.to_string(),
        last_ts_ns: entry.stats.last_ts_ns.to_string(),
        duration_s: crate::query::duration_seconds(entry.stats.first_ts_ns, entry.stats.last_ts_ns),
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

fn packet_event(record: &PacketRecord<'_>) -> PacketEvent {
    PacketEvent {
        schema_version: SCHEMA_VERSION,
        task_id: record.task_id.as_str().to_owned(),
        packet_index: record.packet_index,
        ts_unix_ns: record.ts_ns.to_string(),
        ts_precision: record.ts_precision,
        interface_id: record.interface_id,
        captured_len: record.captured_len,
        original_len: record.original_len,
        linktype: record.linktype,
        truncated: record.captured_len < record.original_len,
        link: record.decoded.link.clone(),
        network: record.decoded.network.clone(),
        transport: record.decoded.transport.clone(),
        application: record.decoded.application.clone(),
        payload_ref: record.payload_ref(),
        decode_status: record.decode_status,
    }
}

fn observe_reassembly(
    reassembler: &mut Reassembler,
    store: &mut AnalysisStore,
    record: &PacketRecord<'_>,
) {
    let Some(key) = record.session_key() else {
        return;
    };
    let Some(network) = record.decoded.network.as_ref() else {
        return;
    };
    if network.is_fragment {
        return;
    }
    if network.protocol != packetsage_protocol::NetProto::Tcp {
        return;
    }
    let Some(transport) = record.decoded.transport.as_ref() else {
        return;
    };
    // Zero length segments still carry state: FIN/RST close the stream and a
    // bare SYN moves the stream position by one sequence number.
    let payload = record.decoded.payload.unwrap_or(&[]);
    let Ok(src_ip) = network.src.parse::<std::net::IpAddr>() else {
        return;
    };
    let src = crate::model::Endpoint {
        ip: src_ip,
        port: transport.src_port.unwrap_or(0),
    };
    let direction = store
        .sessions
        .get_by_key(&key)
        .map_or(Direction::ClientToServer, |entry| {
            if entry.client_endpoint() == src {
                Direction::ClientToServer
            } else {
                Direction::ServerToClient
            }
        });
    let verdict = reassembler.feed(
        &key,
        direction,
        Segment {
            seq: transport.seq.unwrap_or(0),
            data: payload,
            ts_ns: record.ts_ns,
            fin: record.has_fin(),
            rst: record.has_rst(),
            syn: record.is_bare_syn() || record.is_syn_ack(),
        },
    );
    store.note_verdict(&key, verdict);
    // Payload is not retained by the aggregate path: draining the reassembly
    // buffer keeps memory bounded by the configured stream budget.
    let _drained = reassembler.take_contiguous(&key, direction);
    let state = reassembler.state(&key);
    store.note_session_state(&key, state);
    if matches!(state, StreamState::Incomplete | StreamState::BufferOverflow) {
        store.note_stream_state(state);
    }
}

/// Convenience helper: analyse a file with a sink and optional rules.
///
/// # Errors
/// Propagates pipeline errors.
pub fn analyze_file(
    path: &Path,
    config: EngineConfig,
    sink: Box<dyn EventSink>,
    rules: Option<Box<dyn RuleHook>>,
) -> Result<AnalysisResult> {
    let task_id = ids::new_task_id().map_err(PacketSageError::Internal)?;
    let started_at = now_rfc3339();
    let mut pipeline = AnalyzePipeline::new(config, sink);
    pipeline.rules = rules;
    pipeline.run(path, &task_id, &started_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids;
    use crate::query::QueryEngine;
    use etherparse::{PacketBuilder, TcpHeader};
    use packetsage_protocol::{DecodeErrorCode, NetProto};
    use std::path::PathBuf;
    use std::rc::Rc;

    /// Writer shared with the test so the JSONL bytes can be inspected after
    /// the pipeline has taken ownership of the sink.
    #[derive(Clone)]
    struct SharedBuffer(Rc<std::cell::RefCell<Vec<u8>>>);

    impl SharedBuffer {
        fn new() -> Self {
            Self(Rc::new(std::cell::RefCell::new(Vec::new())))
        }

        fn writer(&self) -> SharedWriter {
            SharedWriter(self.0.clone())
        }

        fn contents(&self) -> Vec<u8> {
            self.0.borrow().clone()
        }
    }

    struct SharedWriter(Rc<std::cell::RefCell<Vec<u8>>>);

    impl Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Minimal legacy-pcap writer used by the tests (little endian, µs).
    fn write_pcap(path: &Path, packets: &[(u32, u32, Vec<u8>)]) {
        let mut out = Vec::new();
        out.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&65_535u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        for (sec, usec, data) in packets {
            out.extend_from_slice(&sec.to_le_bytes());
            out.extend_from_slice(&usec.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        std::fs::write(path, out).expect("write pcap");
    }

    #[allow(clippy::too_many_arguments)] // packet builder in a test fixture
    fn tcp_segment(
        src: [u8; 4],
        dst: [u8; 4],
        sport: u16,
        dport: u16,
        seq: u32,
        ack: u32,
        syn: bool,
        ack_flag: bool,
        fin: bool,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut header = TcpHeader::new(sport, dport, seq, 8192);
        header.acknowledgment_number = ack;
        header.syn = syn;
        header.ack = ack_flag;
        header.fin = fin;
        let builder = PacketBuilder::ethernet2([1, 2, 3, 4, 5, 6], [7, 8, 9, 10, 11, 12])
            .ipv4(src, dst, 64)
            .tcp_header(header);
        let mut packet = Vec::with_capacity(builder.size(payload.len()));
        builder.write(&mut packet, payload).expect("build tcp");
        packet
    }

    fn truncated_ethernet() -> Vec<u8> {
        let mut bytes = vec![0u8; 14];
        bytes[12] = 0x08;
        bytes[13] = 0x00;
        bytes.extend_from_slice(&[0x45, 0, 0, 20, 0, 0]);
        bytes
    }

    fn sample_capture(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("packetsage-{name}.pcap"));
        // Sequence arithmetic is exact: the SYN consumed ISN 1000, so the first
        // payload byte is 1001. A fixture that reuses the ISN hides offset and
        // direction bugs (see tests/flow_directions.rs).
        let get = tcp_segment(
            [10, 0, 0, 1],
            [10, 0, 0, 2],
            40000,
            80,
            1_001,
            5_001,
            false,
            true,
            false,
            b"GET /index.html HTTP/1.1\r\nHost: example.com\r\nUser-Agent: curl/8.0\r\n\r\n",
        );
        let response = tcp_segment(
            [10, 0, 0, 2],
            [10, 0, 0, 1],
            80,
            40000,
            5_000,
            1_053,
            false,
            true,
            false,
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
        );
        let syn = tcp_segment(
            [10, 0, 0, 1],
            [10, 0, 0, 2],
            40000,
            80,
            1_000,
            0,
            true,
            false,
            false,
            &[],
        );
        let syn_ack = tcp_segment(
            [10, 0, 0, 2],
            [10, 0, 0, 1],
            80,
            40000,
            5_000,
            1_001,
            true,
            true,
            false,
            &[],
        );
        let packets = vec![
            (1_700_000_000u32, 0u32, syn),
            (1_700_000_000, 1_000, syn_ack),
            (1_700_000_000, 2_000, get.clone()),
            (1_700_000_000, 3_000, response),
            // retransmission of the request payload
            (1_700_000_000, 4_000, get.clone()),
            // malformed frame: IPv4 header cut in half
            (1_700_000_000, 5_000, truncated_ethernet()),
            // FIN
            (
                1_700_000_001,
                0,
                tcp_segment(
                    [10, 0, 0, 1],
                    [10, 0, 0, 2],
                    40000,
                    80,
                    1_070,
                    5_044,
                    false,
                    true,
                    true,
                    &[],
                ),
            ),
        ];
        write_pcap(&path, &packets);
        path
    }

    #[test]
    fn pipeline_emits_the_full_event_stream() {
        let path = sample_capture("full");
        let task = ids::new_task_id().expect("task id");
        let mut pipeline =
            AnalyzePipeline::new(EngineConfig::default(), Box::new(CollectingSink::default()));
        let result = pipeline
            .run(&path, &task, "2026-01-01T00:00:00Z")
            .expect("run");

        assert_eq!(result.summary.packets, 7);
        assert_eq!(result.summary.sessions, 1);
        assert_eq!(result.summary.decode_errors, 1);
        assert_eq!(result.store.counters.truncated_packets, 0);
        let entry = result.store.sessions.sessions()[0];
        assert_eq!(entry.stats.packets, 6);
        assert_eq!(entry.stats.syn_count, 1);
        assert_eq!(entry.stats.syn_ack_count, 1);
        assert_eq!(entry.stats.fin_count, 1);
        assert_eq!(entry.stats.retransmission_count, 1);
        assert_eq!(entry.stats.app_protocol, Some(AppProto::Http));

        let summary = result.store.summary.clone();
        let engine = QueryEngine {
            summary: &summary,
            sessions: &result.store.sessions,
            index: &result.store.index,
            stats: &result.store.stats,
            payloads: None,
            streams: Some(&result.store),
        };
        let conversations = engine.conversations(&crate::query::ConversationQuery::default());
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].session_id, "S-000001");
        assert_eq!(conversations[0].packets, 6);
        assert_eq!(conversations[0].app_protocol, Some(AppProto::Http));
        assert_eq!(
            engine
                .filter_packets(
                    &crate::query::PacketFilter {
                        dst_port: Some(80),
                        ..crate::query::PacketFilter::default()
                    },
                    50
                )
                .len(),
            4
        );
        let preview = engine
            .reconstruct_stream(&crate::query::StreamQuery {
                session_id: "S-000001".to_owned(),
                direction: Some("client_to_server".to_owned()),
                max_bytes: Some(4096),
            })
            .expect("reconstruct");
        assert_eq!(preview.status, "complete");
        assert_eq!(preview.content_type, "http_request");
        assert!(preview.preview.starts_with("GET /index.html"));
        // "GET /index.html HTTP/1.1\r\nHost: example.com\r\nUser-Agent: curl/8.0\r\n\r\n"
        assert_eq!(preview.bytes, 69);
    }

    #[test]
    fn jsonl_sink_writes_one_object_per_line() {
        let path = sample_capture("jsonl");
        let task = ids::new_task_id().expect("task id");
        let shared = SharedBuffer::new();
        {
            let sink = JsonlSink::new(shared.writer(), 0);
            let mut pipeline = AnalyzePipeline::new(EngineConfig::default(), Box::new(sink));
            pipeline
                .run(&path, &task, "2026-01-01T00:00:00Z")
                .expect("run");
        }
        let buffer = shared.contents();
        let text = String::from_utf8(buffer).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.len() >= 12,
            "expected many events, got {}",
            lines.len()
        );
        for line in &lines {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid json line");
            assert_eq!(value["schema_version"], serde_json::json!(SCHEMA_VERSION));
            assert_eq!(value["task_id"], serde_json::json!(task.as_str()));
            assert!(value.get("event").is_some());
        }
        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(first["event"], serde_json::json!("task_started"));
        let last: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).expect("json");
        assert_eq!(last["event"], serde_json::json!("task_finished"));
        assert!(lines
            .iter()
            .any(|l| l.contains("\"event\":\"capture_info\"")));
        assert!(lines
            .iter()
            .any(|l| l.contains("\"event\":\"decode_error\"")));
        assert!(lines
            .iter()
            .any(|l| l.contains("\"event\":\"session_summary\"")));
        assert!(lines.iter().any(|l| l.contains("\"event\":\"stats\"")));
    }

    #[test]
    fn capture_info_precedes_the_first_packet() {
        let path = sample_capture("order");
        let task = ids::new_task_id().expect("task id");
        let sink = CollectingSink::default();
        let mut pipeline = AnalyzePipeline::new(EngineConfig::default(), Box::new(sink));
        let _ = pipeline
            .run(&path, &task, "2026-01-01T00:00:00Z")
            .expect("run");
        // The sink is owned by the pipeline; re-run through analyze_file to get
        // the events back.
        let sink = CollectingSink::default();
        let result =
            analyze_file(&path, EngineConfig::default(), Box::new(sink), None).expect("analyze");
        assert_eq!(result.store.summary.packets, 7);
        // TCP packets: SYN, SYN-ACK, request, response, retransmission, FIN.
        assert_eq!(result.store.summary.protocols.get("tcp"), Some(&6));
        let packet = result.store.index.get(0).expect("first packet");
        assert_eq!(packet.protocol, NetProto::Tcp);
        let decode_error = result.store.decode_error_counts(1);
        assert_eq!(decode_error[0].code, DecodeErrorCode::TruncatedHeader);
    }

    #[test]
    fn errors_only_mode_skips_clean_packets() {
        let path = sample_capture("errors-only");
        let task = ids::new_task_id().expect("task id");
        let mut config = EngineConfig::default();
        config.emit.packet_events = PacketEventMode::ErrorsOnly;
        let sink = CollectingSink::default();
        let mut pipeline = AnalyzePipeline::new(config, Box::new(sink));
        let result = pipeline
            .run(&path, &task, "2026-01-01T00:00:00Z")
            .expect("run");
        assert_eq!(result.summary.packets, 7);
    }
}
