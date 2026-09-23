//! `packetsage serve`: the JSONL RPC worker (开发文档 §21, M0~M2 §5.3).
//!
//! stdout carries JSONL responses, stderr carries logs; that separation is a
//! hard requirement of the agent integration (M0~M2 §7.3).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufRead, Write};
use std::path::PathBuf;

use packetsage_core::pipeline::{now_rfc3339, AnalyzePipeline, CountingSink, EventSink, RuleHook};
use packetsage_core::{ids, ConversationQuery, PacketFilter, QueryEngine, StreamQuery};
use packetsage_protocol::{
    method, validate_finding_v1_v4, FindingDraft, Layer, LedgerEntry, RpcErrorCode, RpcRequest,
    RpcResponse, ToolEnvelope, TrustedSource, SCHEMA_VERSION,
};
use packetsage_rules::{RuleEngine, WindowBudget};

use crate::exit::ExitCode;
use crate::redact::redact_preview;

/// Maximum number of tasks kept in the process (M0~M2 §5.3: LRU of 8).
const MAX_TASKS: usize = 8;
/// Maximum tool result characters before summary mode kicks in.
const MAX_RESULT_CHARS: usize = 8_000;
/// Maximum bytes of a reconstructed stream preview.
const MAX_STREAM_BYTES: usize = 262_144;

type RpcResult<T> = std::result::Result<T, (RpcErrorCode, String)>;

/// The RPC worker state.
struct Worker<W: Write> {
    tasks: BTreeMap<String, packetsage_core::AnalysisStore>,
    order: VecDeque<String>,
    out: W,
    database: Option<String>,
    /// Rules directory resolved through the discovery chain (§7.1).
    rules_dir: PathBuf,
    /// Arguments and start time of the in-flight call, so the ledger entry can
    /// record what was asked (`args_json`) and how long it took (`duration_ms`).
    call_args: serde_json::Value,
    call_started: std::time::Instant,
}

/// Runs the worker until stdin closes.
pub fn run(loaded: &crate::config::Loaded) -> ExitCode {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut worker = Worker::new(stdout.lock(), loaded);
    let mut lines = stdin.lock().lines();
    while let Some(Ok(line)) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: RpcRequest = match serde_json::from_str(trimmed) {
            Ok(request) => request,
            Err(error) => {
                let response = RpcResponse::err(
                    "",
                    RpcErrorCode::InvalidArgument,
                    format!("malformed request: {error}"),
                );
                if worker.write(&response).is_err() {
                    break;
                }
                continue;
            }
        };
        let response = worker.dispatch(&request);
        if worker.write(&response).is_err() {
            break;
        }
    }
    ExitCode::Success
}

impl<W: Write> Worker<W> {
    fn new(out: W, loaded: &crate::config::Loaded) -> Self {
        // §7.1: the environment layer wins over the configuration file, and the
        // `PACKETSAGE_*` names announced by the CLI spec are accepted next to
        // the historical ones (#35: existing names keep working).
        let database = crate::config::env_first(&["PACKETSAGE_STORAGE_URL", "PACKETSAGE_DB"])
            .or_else(|| loaded.config.storage.url.clone());
        let rules_dir = crate::config::env_first(&["PACKETSAGE_RULES_PATH", "PACKETSAGE_RULES"])
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| loaded.config.rules.path.clone());
        Self {
            tasks: BTreeMap::new(),
            order: VecDeque::new(),
            out,
            database,
            rules_dir,
            call_args: serde_json::Value::Null,
            call_started: std::time::Instant::now(),
        }
    }

    fn write(&mut self, response: &RpcResponse) -> std::io::Result<()> {
        serde_json::to_writer(&mut self.out, response)?;
        self.out.write_all(b"\n")?;
        self.out.flush()
    }

    fn dispatch(&mut self, request: &RpcRequest) -> RpcResponse {
        self.call_args = request.params.clone();
        self.call_started = std::time::Instant::now();
        // ADR-019: a task that left memory (serve restart, LRU eviction) is
        // rebuilt from its persisted source before the call is answered, so the
        // RPC surface of an already analysed task does not depend on this
        // process having analysed it.
        self.prime_cold_task(&request.params);
        match self.handle(request) {
            Ok(result) => RpcResponse::ok(&request.id, result),
            Err((code, message)) => RpcResponse::err(&request.id, code, message),
        }
    }

    /// Best effort cold recovery: replay the persisted capture under the same
    /// task id. The source file is the recovery input (its absence is reported
    /// downstream as `CAPTURE_UNAVAILABLE`); everything else comes from the
    /// database row the analysis stage wrote (ADR-018 single writer).
    fn prime_cold_task(&mut self, params: &serde_json::Value) {
        let Some(task_id) = params.get("task_id").and_then(|v| v.as_str()) else {
            return;
        };
        if self.tasks.contains_key(task_id) {
            return;
        }
        let Some(url) = self.database.clone() else {
            return;
        };
        let Ok(wanted) = packetsage_protocol::TaskId::parse(task_id) else {
            return;
        };
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::warn!(%error, "cold recovery runtime failed");
                return;
            }
        };
        let source_path = runtime.block_on(async {
            use packetsage_storage::{Repository, SqliteRepo};
            // Read-only: a cold lookup must not create a database file.
            let repo = SqliteRepo::connect_readonly_default(&url).await.ok()?;
            let task = repo.get_task(task_id).await.ok()??;
            Some(task.source_path)
        });
        let Some(source_path) = source_path else {
            return;
        };
        let path = PathBuf::from(&source_path);
        if !path.is_file() {
            tracing::warn!(task = task_id, path = %source_path, "cold recovery: source file is gone");
            return;
        }
        let config = packetsage_core::EngineConfig::default();
        let mut pipeline = AnalyzePipeline::new(config, Box::new(CountingSink::default()));
        pipeline.rules = self.build_rules();
        let started_at = now_rfc3339();
        match pipeline.run(path.as_path(), &wanted, &started_at) {
            Ok(result) => {
                tracing::info!(task = task_id, path = %source_path, "cold recovery replay");
                let mut store = result.store;
                // A replay rebuilds summary/sessions/alerts, but findings are
                // *submitted*, not derivable: ADR-019 recovery has to attach the
                // persisted ones or `report` would render an empty summary.
                attach_persisted_findings(&mut store, &url);
                // Same for the ledger: the numbers and entity ids a stored
                // finding cites were produced by an earlier process, and the
                // anti-hallucination lint needs them to be citable again.
                attach_persisted_ledger(&mut store, &url);
                self.insert_task(task_id.to_owned(), store);
            }
            Err(error) => {
                tracing::warn!(task = task_id, %error, "cold recovery replay failed");
            }
        }
    }

    fn handle(&mut self, request: &RpcRequest) -> RpcResult<serde_json::Value> {
        let params = &request.params;
        match request.method.as_str() {
            method::PING => Ok(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "schema_version": SCHEMA_VERSION,
            })),
            method::ANALYZE_FILE => self.analyze_file(params),
            method::GET_CAPTURE_SUMMARY => self.get_capture_summary(params),
            method::GET_PROTOCOL_STATS => self.get_protocol_stats(params),
            method::GET_CONVERSATIONS => self.get_conversations(params),
            method::FILTER_PACKETS => self.filter_packets(params),
            method::INSPECT_PACKETS => self.inspect_packets(params),
            method::RECONSTRUCT_STREAM => self.reconstruct_stream(params),
            method::CHECK_ALERTS => self.check_alerts(params),
            method::QUERY_HISTORY => self.query_history(params),
            method::GET_TASK_ARTIFACTS => self.get_task_artifacts(params),
            method::VALIDATE_FINDING => self.validate_finding(params),
            method::LIST_RULES | method::CHECK_RULES => self.rules(params),
            method::SUBMIT_FINDING => self.submit_finding(params),
            method::SUBMIT_REPORT_META => self.submit_report_meta(params),
            other => Err((
                RpcErrorCode::NotImplemented,
                format!("unknown method {other:?}"),
            )),
        }
    }

    fn analyze_file(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let path = params.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
            (
                RpcErrorCode::InvalidArgument,
                "analyze_file requires `path`".to_owned(),
            )
        })?;
        let task_id = ids::new_task_id().map_err(|e| (RpcErrorCode::Internal, e))?;
        let mut config = packetsage_core::EngineConfig::default();
        if let Some(emit) = params.get("emit").and_then(|v| v.as_str()) {
            config.emit.packet_events = match emit {
                "none" => packetsage_core::PacketEventMode::None,
                "errors_only" => packetsage_core::PacketEventMode::ErrorsOnly,
                _ => packetsage_core::PacketEventMode::All,
            };
        }
        let rules = self.build_rules();
        let sink: Box<dyn EventSink> = Box::new(CountingSink::default());
        let mut pipeline = AnalyzePipeline::new(config, sink);
        pipeline.rules = rules;
        let started_at = now_rfc3339();
        let result = pipeline
            .run(PathBuf::from(path).as_path(), &task_id, &started_at)
            .map_err(|error| {
                let code = match error {
                    packetsage_core::PacketSageError::UnsupportedCapture { .. } => {
                        RpcErrorCode::UnsupportedCapture
                    }
                    packetsage_core::PacketSageError::CaptureCorrupted { .. }
                    | packetsage_core::PacketSageError::Io(_) => RpcErrorCode::Io,
                    _ => RpcErrorCode::Internal,
                };
                (code, error.to_string())
            })?;
        let summary = result.summary.clone();
        // Optional persistence: `PACKETSAGE_DB=sqlite://packetsage.db`.
        if let Some(url) = self.database.clone() {
            let persisted = {
                let store = &result.store;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| (RpcErrorCode::Internal, e.to_string()))?;
                runtime.block_on(async {
                    use packetsage_storage::{Repository, SqliteRepo};
                    let repo = SqliteRepo::connect(&url).await?;
                    repo.migrate().await?;
                    crate::persist::persist(&repo, store).await
                })
            };
            if let Err(error) = persisted {
                tracing::warn!(%error, "task persistence failed");
            }
        }
        self.insert_task(task_id.as_str().to_owned(), result.store);
        Ok(serde_json::json!({
            "task_id": task_id.as_str(),
            "summary": summary,
        }))
    }

    fn build_rules(&self) -> Option<Box<dyn RuleHook>> {
        let dir = self.rules_dir.display().to_string();
        let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
        let path = PathBuf::from(&dir);
        let path = if path.join("builtin").is_dir() {
            path.join("builtin")
        } else {
            path
        };
        if path.is_dir() {
            if let Ok(report) = engine.load_dir_lenient(&path) {
                for dead in &report.dead_letters {
                    tracing::warn!(path = %dead.path.display(), "rule dead-lettered: {}", dead.reason);
                }
            }
        }
        Some(Box::new(engine))
    }

    fn insert_task(&mut self, task_id: String, store: packetsage_core::AnalysisStore) {
        while self.order.len() >= MAX_TASKS {
            if let Some(oldest) = self.order.pop_front() {
                self.tasks.remove(&oldest);
            }
        }
        self.order.push_back(task_id.clone());
        self.tasks.insert(task_id, store);
    }

    fn task(&self, params: &serde_json::Value) -> RpcResult<&packetsage_core::AnalysisStore> {
        let task_id = params
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                (
                    RpcErrorCode::InvalidArgument,
                    "this method requires `task_id`".to_owned(),
                )
            })?;
        self.tasks.get(task_id).ok_or_else(|| {
            self.cold_recovery(task_id).unwrap_or_else(|| {
                (
                    RpcErrorCode::NotFound,
                    format!("task {task_id} is not in memory; re-run analyze_file"),
                )
            })
        })
    }

    /// ADR-019 cold recovery: the task is gone from memory, but the ledger may
    /// still know it — and then the question is whether its source file survived
    /// (§9 `CAPTURE_UNAVAILABLE`).
    fn cold_recovery(&self, task_id: &str) -> Option<(RpcErrorCode, String)> {
        let url = self.database.clone()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        let wanted = task_id.to_owned();
        let source_path = runtime.block_on(async move {
            use packetsage_storage::{Repository, SqliteRepo};
            // Read-only: a cold lookup must not create a database file.
            let repo = SqliteRepo::connect_readonly_default(&url).await.ok()?;
            let task = repo.get_task(&wanted).await.ok()??;
            Some(task.source_path)
        })?;
        let path = std::path::PathBuf::from(&source_path);
        if path.exists() {
            Some((
                RpcErrorCode::NotFound,
                format!(
                    "task {task_id} is not in memory; re-run analyze_file \
                     (its capture is still at {source_path})"
                ),
            ))
        } else {
            Some((
                RpcErrorCode::CaptureUnavailable,
                crate::errors::capture_unavailable(task_id, &path)
                    .trim_start_matches("packetsage: ")
                    .to_owned(),
            ))
        }
    }

    fn query_engine<'a>(store: &'a packetsage_core::AnalysisStore) -> QueryEngine<'a> {
        QueryEngine {
            summary: &store.summary,
            sessions: &store.sessions,
            index: &store.index,
            stats: &store.stats,
            payloads: Some(store),
            streams: Some(store),
        }
    }

    /// Wraps a result body in the frozen tool envelope: allocates the tc id,
    /// records the ledger entry and applies the result budget.
    fn tool_result(
        &mut self,
        task_id: &str,
        method_name: &str,
        mut content: serde_json::Value,
        refs: BTreeSet<String>,
        numbers: BTreeMap<String, f64>,
        redactions: Vec<String>,
    ) -> RpcResult<serde_json::Value> {
        // Clone what the content block needs before the sets move into the ledger.
        let ref_ids_for_content: Vec<String> = refs.iter().take(200).cloned().collect();
        let numbers_for_content: BTreeMap<String, f64> = numbers.clone();
        // What a later process may still quote from this result (the body itself
        // is not persisted): see `LedgerEntry::tokens`.
        let tokens = collect_citable_tokens(&content);
        let (id, ts_unix_ns) = {
            let store = self.tasks.get_mut(task_id).ok_or_else(|| {
                (
                    RpcErrorCode::NotFound,
                    format!("task {task_id} is not in memory"),
                )
            })?;
            let id = store.next_tool_call_id();
            let ts = store
                .summary
                .last_ts_ns
                .clone()
                .unwrap_or_else(|| "0".to_owned());
            store.ledger.issue(
                &id,
                LedgerEntry {
                    task_id: task_id.to_owned(),
                    method: method_name.to_owned(),
                    ts_unix_ns: ts.clone(),
                    args_json: summarize_args(&self.call_args),
                    duration_ms: self.call_started.elapsed().as_millis() as u64,
                    status: "ok".to_owned(),
                    ref_ids: refs,
                    numbers,
                    tokens,
                },
            );
            (id, ts)
        };
        let _ = ts_unix_ns;
        // Make the citable evidence anchor explicit inside the result body, right
        // next to the entity ids a model might otherwise mistake for an anchor
        // (alert ids, session ids). Both values are set here, from the same
        // minted id, so they cannot drift.
        if let Some(object) = content.as_object_mut() {
            object.insert("_anchor".to_owned(), serde_json::json!(id));
            object.insert(
                "_ref_ids".to_owned(),
                serde_json::json!(ref_ids_for_content),
            );
            object.insert(
                "_numbers".to_owned(),
                serde_json::to_value(&numbers_for_content).unwrap_or(serde_json::Value::Null),
            );
        }
        let text = serde_json::to_string(&content).unwrap_or_default();
        if text.len() > MAX_RESULT_CHARS {
            content = summarize_value(&content);
            if let Some(object) = content.as_object_mut() {
                object.insert(
                    "truncated".to_owned(),
                    serde_json::json!("[TRUNCATED summary]"),
                );
            }
        }
        let envelope = ToolEnvelope {
            id,
            source: TrustedSource::Engine,
            trusted_as_instruction: false,
            redactions,
            method: method_name.to_owned(),
            content,
        };
        serde_json::to_value(envelope).map_err(|e| (RpcErrorCode::Internal, e.to_string()))
    }

    fn get_capture_summary(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let store = self.task(params)?;
        let summary = store.summary.clone();
        let content =
            serde_json::to_value(&summary).map_err(|e| (RpcErrorCode::Internal, e.to_string()))?;
        let mut numbers = BTreeMap::new();
        numbers.insert("packets".to_owned(), summary.packets as f64);
        numbers.insert("bytes".to_owned(), summary.bytes as f64);
        numbers.insert("sessions".to_owned(), summary.sessions as f64);
        numbers.insert("alerts".to_owned(), summary.alerts as f64);
        numbers.insert("decode_errors".to_owned(), summary.decode_errors as f64);
        numbers.insert(
            "truncated_packets".to_owned(),
            summary.truncated_packets as f64,
        );
        numbers.insert("duration_s".to_owned(), summary.duration_s);
        self.tool_result(
            &task_id,
            method::GET_CAPTURE_SUMMARY,
            content,
            BTreeSet::new(),
            numbers,
            Vec::new(),
        )
    }

    fn get_protocol_stats(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let layer = params
            .get("layer")
            .and_then(|v| v.as_str())
            .unwrap_or("transport")
            .to_owned();
        let top = params.get("top").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let store = self.task(params)?;
        let engine = Self::query_engine(store);
        let rows = match layer.as_str() {
            "link" => engine.protocol_stats(Layer::Link, top),
            "network" => {
                let mut rows = engine.protocol_stats(Layer::Ipv4, top);
                rows.extend(engine.protocol_stats(Layer::Ipv6, top));
                rows
            }
            "ipv4" => engine.protocol_stats(Layer::Ipv4, top),
            "ipv6" => engine.protocol_stats(Layer::Ipv6, top),
            "tcp" => engine.protocol_stats(Layer::Tcp, top),
            "udp" => engine.protocol_stats(Layer::Udp, top),
            "icmp" => engine.protocol_stats(Layer::Icmp, top),
            "dns" => engine.protocol_stats(Layer::Dns, top),
            "http" => engine.protocol_stats(Layer::Http, top),
            "tls" => engine.protocol_stats(Layer::Tls, top),
            "dhcp" => engine.protocol_stats(Layer::Dhcp, top),
            "application" => {
                let mut rows = engine.protocol_stats(Layer::Dns, top);
                rows.extend(engine.protocol_stats(Layer::Http, top));
                rows.extend(engine.protocol_stats(Layer::Tls, top));
                rows.extend(engine.protocol_stats(Layer::Dhcp, top));
                rows
            }
            "transport" => {
                let mut rows = engine.protocol_stats(Layer::Tcp, top);
                rows.extend(engine.protocol_stats(Layer::Udp, top));
                rows.extend(engine.protocol_stats(Layer::Icmp, top));
                rows
            }
            other => {
                return Err((
                    RpcErrorCode::InvalidArgument,
                    format!("unknown layer {other:?}"),
                ))
            }
        };
        let mut numbers = BTreeMap::new();
        for row in &rows {
            numbers.insert(format!("{}.packets", row.protocol), row.packets as f64);
            numbers.insert(format!("{}.bytes", row.protocol), row.bytes as f64);
        }
        let content = serde_json::json!({ "layer": layer, "rows": rows });
        self.tool_result(
            &task_id,
            method::GET_PROTOCOL_STATS,
            content,
            BTreeSet::new(),
            numbers,
            Vec::new(),
        )
    }

    fn get_conversations(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let mut query = ConversationQuery::default();
        if let Some(sort_by) = params.get("sort_by").and_then(|v| v.as_str()) {
            query.sort_by = match sort_by {
                "packets" => packetsage_core::query::ConversationSort::Packets,
                "duration" => packetsage_core::query::ConversationSort::Duration,
                _ => packetsage_core::query::ConversationSort::Bytes,
            };
        }
        if let Some(limit) = params.get("limit").and_then(|v| v.as_u64()) {
            query.limit = limit as usize;
        }
        if let Some(filter) = params.get("filter") {
            query.filter.protocol = filter
                .get("protocol")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            query.filter.ip = filter.get("ip").and_then(|v| v.as_str()).map(str::to_owned);
            query.filter.min_bytes = filter.get("min_bytes").and_then(|v| v.as_u64());
            query.filter.min_packets = filter.get("min_packets").and_then(|v| v.as_u64());
        }
        let rows = {
            let store = self.task(params)?;
            Self::query_engine(store).conversations(&query)
        };
        let mut refs = BTreeSet::new();
        let mut numbers = BTreeMap::new();
        for row in &rows {
            refs.insert(row.session_id.clone());
            numbers.insert(format!("{}.packets", row.session_id), row.packets as f64);
            numbers.insert(format!("{}.bytes", row.session_id), row.bytes as f64);
            numbers.insert(
                format!("{}.syn_count", row.session_id),
                f64::from(row.syn_count),
            );
        }
        numbers.insert("count".to_owned(), rows.len() as f64);
        let content = serde_json::json!({ "conversations": rows });
        self.tool_result(
            &task_id,
            method::GET_CONVERSATIONS,
            content,
            refs,
            numbers,
            Vec::new(),
        )
    }

    fn filter_packets(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let mut filter = PacketFilter {
            src_ip: params
                .get("src_ip")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            dst_ip: params
                .get("dst_ip")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            src_port: params
                .get("src_port")
                .and_then(|v| v.as_u64())
                .and_then(|v| u16::try_from(v).ok()),
            dst_port: params
                .get("dst_port")
                .and_then(|v| v.as_u64())
                .and_then(|v| u16::try_from(v).ok()),
            protocol: params
                .get("protocol")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            time_start: params.get("time_start").and_then(|v| v.as_f64()),
            time_end: params.get("time_end").and_then(|v| v.as_f64()),
            limit: params
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            decode_errors_only: params.get("decode_errors_only").and_then(|v| v.as_bool()),
            ..PacketFilter::default()
        };
        if let Some(flags) = params.get("tcp_flags").and_then(|v| v.as_array()) {
            for flag in flags {
                if let Some(parsed) = flag.as_str().and_then(parse_flag) {
                    filter.tcp_flags.push(parsed);
                }
            }
        }
        let limit = filter.limit.unwrap_or(100).min(10_000);
        let indices = {
            let store = self.task(params)?;
            Self::query_engine(store).filter_packets(&filter, limit)
        };
        let mut refs = BTreeSet::new();
        for index in &indices {
            refs.insert(index.to_string());
        }
        let mut numbers = BTreeMap::new();
        numbers.insert("count".to_owned(), indices.len() as f64);
        let content = serde_json::json!({
            "count": indices.len(),
            "packet_indices": indices,
        });
        self.tool_result(
            &task_id,
            method::FILTER_PACKETS,
            content,
            refs,
            numbers,
            Vec::new(),
        )
    }

    fn inspect_packets(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let indices: Vec<u64> = params
            .get("packet_indices")
            .and_then(|v| v.as_array())
            .map(|list| list.iter().filter_map(|v| v.as_u64()).collect())
            .ok_or_else(|| {
                (
                    RpcErrorCode::InvalidArgument,
                    "inspect_packets requires `packet_indices`".to_owned(),
                )
            })?;
        if indices.is_empty() {
            return Err((
                RpcErrorCode::InvalidArgument,
                "packet_indices must not be empty".to_owned(),
            ));
        }
        if indices.len() > 100 {
            return Err((
                RpcErrorCode::InvalidArgument,
                "at most 100 packets can be inspected per call".to_owned(),
            ));
        }
        let preview_bytes = params
            .get("preview_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(256)
            .min(4096) as usize;
        let rows = {
            let store = self.task(params)?;
            Self::query_engine(store)
                .inspect_packets(&indices, preview_bytes)
                .map_err(|e| (RpcErrorCode::Io, e.to_string()))?
        };

        let mut redactions = Vec::new();
        let mut safe_rows = Vec::new();
        for row in rows {
            let mut value =
                serde_json::to_value(&row).map_err(|e| (RpcErrorCode::Internal, e.to_string()))?;
            if let Some(preview) = row.payload_preview.as_deref() {
                let (redacted, markers) = redact_preview(preview);
                if !markers.is_empty() {
                    redactions.extend(markers);
                    if let Some(object) = value.as_object_mut() {
                        object.insert("payload_preview".to_owned(), serde_json::json!(redacted));
                    }
                }
            }
            safe_rows.push(value);
        }
        let mut refs = BTreeSet::new();
        for index in &indices {
            refs.insert(index.to_string());
        }
        let mut numbers = BTreeMap::new();
        numbers.insert("count".to_owned(), safe_rows.len() as f64);
        let content = serde_json::json!({ "packets": safe_rows });
        self.tool_result(
            &task_id,
            method::INSPECT_PACKETS,
            content,
            refs,
            numbers,
            redactions,
        )
    }

    fn reconstruct_stream(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let session_id = params
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                (
                    RpcErrorCode::InvalidArgument,
                    "reconstruct_stream requires `session_id`".to_owned(),
                )
            })?
            .to_owned();
        let direction = params
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("client_to_server")
            .to_owned();
        let max_bytes = params
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(65_536)
            .min(MAX_STREAM_BYTES as u64);
        let preview = {
            let store = self.task(params)?;
            Self::query_engine(store)
                .reconstruct_stream(&StreamQuery {
                    session_id: session_id.clone(),
                    direction: Some(direction),
                    max_bytes: Some(max_bytes),
                })
                .map_err(|e| (RpcErrorCode::NotFound, e.to_string()))?
        };
        let (redacted, redactions) = redact_preview(&preview.preview);
        let mut refs = BTreeSet::new();
        refs.insert(session_id);
        let mut numbers = BTreeMap::new();
        numbers.insert("bytes".to_owned(), preview.bytes as f64);
        let content = serde_json::json!({
            "session_id": preview.session_id,
            "direction": preview.direction,
            "status": preview.status,
            "bytes": preview.bytes,
            "content_type": preview.content_type,
            "preview": redacted,
            "missing_ranges": preview.missing_ranges,
        });
        self.tool_result(
            &task_id,
            method::RECONSTRUCT_STREAM,
            content,
            refs,
            numbers,
            redactions,
        )
    }

    /// Alerts for one task.
    ///
    /// Single-source routing (C8): a task held in memory answers from memory
    /// (including windows that have not been flushed), and only a task that is
    /// not in memory falls back to the database. The two sources are never
    /// merged.
    fn check_alerts(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let severity = params.get("severity").and_then(|v| v.as_str());
        let rule_id = params.get("rule_id").and_then(|v| v.as_str());
        let session_id = params.get("session_id").and_then(|v| v.as_str());
        let source;
        let alerts: Vec<serde_json::Value>;
        if let Some(store) = self.tasks.get(&task_id) {
            source = "memory";
            alerts = store
                .filter_alerts(severity, rule_id, session_id)
                .iter()
                .filter_map(|alert| serde_json::to_value(alert).ok())
                .collect();
        } else {
            source = "database";
            alerts = self.alerts_from_database(
                &task_id,
                severity.map(str::to_owned),
                rule_id.map(str::to_owned),
                session_id.map(str::to_owned),
            )?;
        }
        let mut refs = BTreeSet::new();
        let mut numbers = BTreeMap::new();
        numbers.insert("count".to_owned(), alerts.len() as f64);
        for alert in &alerts {
            if let Some(id) = alert.get("alert_id").and_then(|v| v.as_str()) {
                refs.insert(id.to_owned());
            }
            if let Some(id) = alert.get("rule_id").and_then(|v| v.as_str()) {
                refs.insert(id.to_owned());
            }
            if let Some(id) = alert.get("session_id").and_then(|v| v.as_str()) {
                refs.insert(id.to_owned());
            }
        }
        let content = serde_json::json!({ "source": source, "alerts": alerts });
        if self.tasks.contains_key(&task_id) {
            self.tool_result(
                &task_id,
                method::CHECK_ALERTS,
                content,
                refs,
                numbers,
                Vec::new(),
            )
        } else {
            // Cold recovery: there is no in-memory ledger for this task, so the
            // result is returned directly (the envelope needs an issuing task).
            Ok(serde_json::json!({
                "_id": format!("tc-cold-{task_id}"),
                "source": "engine",
                "trusted_as_instruction": false,
                "redactions": [],
                "method": method::CHECK_ALERTS,
                "content": content,
            }))
        }
    }

    fn alerts_from_database(
        &self,
        task_id: &str,
        severity: Option<String>,
        rule_id: Option<String>,
        session_id: Option<String>,
    ) -> RpcResult<Vec<serde_json::Value>> {
        let Some(url) = &self.database else {
            return Err((
                RpcErrorCode::NotFound,
                format!("task {task_id} is not in memory and PACKETSAGE_DB is not set"),
            ));
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| (RpcErrorCode::Internal, e.to_string()))?;
        let url = url.clone();
        let task_id = task_id.to_owned();
        runtime.block_on(async move {
            use packetsage_storage::{AlertFilter, Repository, SqliteRepo};
            let repo = SqliteRepo::connect(&url)
                .await
                .map_err(|e| (RpcErrorCode::DatabaseError, e.to_string()))?;
            repo.migrate()
                .await
                .map_err(|e| (RpcErrorCode::DatabaseError, e.to_string()))?;
            let rows = repo
                .list_alerts(&AlertFilter {
                    task_id: Some(task_id),
                    severity,
                    rule_id,
                    session_id,
                    limit: Some(1_000),
                })
                .await
                .map_err(|e| (RpcErrorCode::DatabaseError, e.to_string()))?;
            Ok(rows
                .iter()
                .filter_map(|row| serde_json::to_value(row).ok())
                .collect())
        })
    }

    fn query_history(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let kind = params
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("alerts");
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        match kind {
            "alerts" => self.check_alerts(params),
            "findings" => {
                let store = self.task(params)?;
                let rows: Vec<serde_json::Value> = store
                    .findings
                    .iter()
                    .take(limit.max(1))
                    .filter_map(|f| serde_json::to_value(f).ok())
                    .collect();
                let mut refs = BTreeSet::new();
                for row in &rows {
                    if let Some(id) = row.get("finding_id").and_then(|v| v.as_str()) {
                        refs.insert(id.to_owned());
                    }
                }
                let mut numbers = BTreeMap::new();
                numbers.insert("count".to_owned(), rows.len() as f64);
                let content = serde_json::json!({ "kind": "findings", "findings": rows });
                self.tool_result(
                    &task_id,
                    method::QUERY_HISTORY,
                    content,
                    refs,
                    numbers,
                    Vec::new(),
                )
            }
            "sessions" => {
                let rows = {
                    let store = self.task(params)?;
                    let query = ConversationQuery {
                        limit,
                        ..ConversationQuery::default()
                    };
                    Self::query_engine(store).conversations(&query)
                };
                let mut refs = BTreeSet::new();
                for row in &rows {
                    refs.insert(row.session_id.clone());
                }
                let mut numbers = BTreeMap::new();
                numbers.insert("count".to_owned(), rows.len() as f64);
                let content = serde_json::json!({ "kind": "sessions", "sessions": rows });
                self.tool_result(
                    &task_id,
                    method::QUERY_HISTORY,
                    content,
                    refs,
                    numbers,
                    Vec::new(),
                )
            }
            // The tc-id ledger is the authoritative analysis trace: it lists the
            // tool calls the agent actually made (M3~M6 §5.2 section 9).
            "trace" => {
                let store = self.task(params)?;
                let rows: Vec<serde_json::Value> = store
                    .ledger
                    .entries()
                    .into_iter()
                    .map(|(id, entry)| {
                        serde_json::json!({
                            "_id": id,
                            "method": entry.method,
                            "ts_unix_ns": entry.ts_unix_ns,
                            "args": entry.args_json,
                            "duration_ms": entry.duration_ms,
                            "status": entry.status,
                            // The lint facts: every number and entity id the
                            // engine produced for this call (ADR-023).
                            "numbers": entry.numbers,
                            "ref_ids": entry.ref_ids,
                            "tokens": entry.tokens,
                        })
                    })
                    .collect();
                let mut refs = BTreeSet::new();
                for row in &rows {
                    if let Some(id) = row.get("_id").and_then(|v| v.as_str()) {
                        refs.insert(id.to_owned());
                    }
                }
                let mut numbers = BTreeMap::new();
                numbers.insert("count".to_owned(), rows.len() as f64);
                let content = serde_json::json!({ "kind": "trace", "trace": rows });
                self.tool_result(
                    &task_id,
                    method::QUERY_HISTORY,
                    content,
                    refs,
                    numbers,
                    Vec::new(),
                )
            }
            other => Err((
                RpcErrorCode::InvalidArgument,
                format!("unknown history kind {other:?}"),
            )),
        }
    }

    fn get_task_artifacts(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let store = self.task(params)?;
        let mut numbers = BTreeMap::new();
        numbers.insert("alert_count".to_owned(), store.alerts.len() as f64);
        numbers.insert("session_count".to_owned(), store.sessions.len() as f64);
        numbers.insert("finding_count".to_owned(), store.findings.len() as f64);
        let content = serde_json::json!({
            "task_id": store.task_id,
            "report_path": store.report_path,
            "report_meta": store.report_meta,
            "alert_count": store.alerts.len(),
            "session_count": store.sessions.len(),
            "finding_count": store.findings.len(),
            "source_path": store.source_path.to_string_lossy(),
            "source_sha256": store.source_sha256,
            "events_schema_version": store.schema_version(),
        });
        self.tool_result(
            &task_id,
            method::GET_TASK_ARTIFACTS,
            content,
            BTreeSet::new(),
            numbers,
            Vec::new(),
        )
    }

    fn validate_finding(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let draft: FindingDraft =
            serde_json::from_value(params.get("draft").cloned().ok_or_else(|| {
                (
                    RpcErrorCode::InvalidArgument,
                    "validate_finding requires `draft`".to_owned(),
                )
            })?)
            .map_err(|e| {
                (
                    RpcErrorCode::InvalidArgument,
                    format!("draft does not match the FindingDraft schema: {e}"),
                )
            })?;
        let outcome = {
            let store = self.task(params)?;
            validate_finding_v1_v4(&draft, &store.ledger, &store.task_id)
        };
        let content = serde_json::json!({
            "status": outcome.status,
            "basis": outcome.basis,
            "issues": outcome.issues,
            "checks": ["V1 evidence present", "V2 tc id issued by this task",
                       "V3 ref_id reachable", "V4 structured numbers match the ledger"],
        });
        self.tool_result(
            &task_id,
            method::VALIDATE_FINDING,
            content,
            BTreeSet::new(),
            BTreeMap::new(),
            Vec::new(),
        )
    }

    fn rules(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);
        let mut engine = RuleEngine::new(WindowBudget::default());
        let mut loaded = 0usize;
        let mut dead: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        for (_, text) in packetsage_rules::BUILTIN_RULES {
            if engine.add_rule_text(text).is_ok() {
                loaded += 1;
            }
        }
        if let Some(path) = &path {
            let report = engine
                .load_dir_lenient(path)
                .map_err(|e| (RpcErrorCode::RuleError, e.to_string()))?;
            loaded = report.loaded;
            dead = report
                .dead_letters
                .iter()
                .map(|d| format!("{}: {}", d.path.display(), d.reason))
                .collect();
            warnings = report.warnings;
        }
        let rules: Vec<serde_json::Value> = engine
            .rules()
            .iter()
            .filter(|rule| path.is_none() || rule.source.to_string_lossy() != "<embedded>")
            .map(|rule| {
                serde_json::json!({
                    "id": rule.file.id,
                    "version": rule.file.version,
                    "severity": rule.file.severity,
                    "scope": rule.file.scope,
                    "metric": rule.file.threshold.metric,
                    "window": rule.file.threshold.window,
                    "cooldown": rule.file.dedupe.per_group_cooldown,
                    "content_hash": rule.short_hash(),
                    "source": rule.source.to_string_lossy(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "loaded": loaded,
            "rules": rules,
            "dead_letters": dead,
            "warnings": warnings,
        }))
    }

    fn submit_finding(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        let drafts: Vec<FindingDraft> =
            serde_json::from_value(params.get("drafts").cloned().ok_or_else(|| {
                (
                    RpcErrorCode::InvalidArgument,
                    "submit_finding requires `drafts`".to_owned(),
                )
            })?)
            .map_err(|e| {
                (
                    RpcErrorCode::InvalidArgument,
                    format!("drafts do not match the FindingDraft schema: {e}"),
                )
            })?;
        let mut stored = Vec::new();
        let mut rejected = Vec::new();
        let mut unknown_tc = 0usize;
        for (index, draft) in drafts.iter().enumerate() {
            let (outcome, unknown) = {
                let store = self.task(params)?;
                // tc-id issuance check (M3~M6 §4.3): an envelope the engine
                // never produced cannot be cited.
                let unknown = draft
                    .evidence
                    .iter()
                    .filter(|reference| !store.ledger.contains(&reference.id))
                    .count();
                let outcome = validate_finding_v1_v4(draft, &store.ledger, &store.task_id);
                (outcome, unknown)
            };
            unknown_tc += unknown;
            let store = self.tasks.get_mut(&task_id).ok_or_else(|| {
                (
                    RpcErrorCode::NotFound,
                    format!("task {task_id} is not in memory"),
                )
            })?;
            if unknown > 0 {
                store.counters.submit_rejects = store.counters.submit_rejects.saturating_add(1);
                rejected.push(serde_json::json!({
                    "draft_index": index,
                    "reason": "unknown_tc_id",
                }));
                continue;
            }
            match store.store_finding(draft.clone(), &outcome) {
                Ok(finding) => stored.push(finding),
                Err(error) => rejected.push(serde_json::json!({
                    "draft_index": index,
                    "reason": error.to_string(),
                })),
            }
        }
        let mut refs = BTreeSet::new();
        for finding in &stored {
            refs.insert(finding.finding_id.clone());
            for evidence in &finding.evidence {
                refs.insert(evidence.id.clone());
            }
        }
        // Write-through: the findings have to outlive this process, otherwise a
        // later `packetsage-agent report --task-id` (a *new* engine, recovered
        // from the database) would see a task with no findings at all.
        if let Some(url) = self.database.clone() {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    let outcome = {
                        let Some(store) = self.tasks.get(&task_id) else {
                            return Err((
                                RpcErrorCode::NotFound,
                                format!("task {task_id} is not in memory"),
                            ));
                        };
                        runtime.block_on(async {
                            use packetsage_storage::SqliteRepo;
                            let repo = SqliteRepo::connect(&url).await?;
                            crate::persist::persist(&repo, store).await
                        })
                    };
                    if let Err(error) = outcome {
                        tracing::warn!(%error, "findings persistence failed");
                    }
                }
                Err(error) => tracing::warn!(%error, "findings persistence runtime failed"),
            }
        }
        let mut numbers = BTreeMap::new();
        numbers.insert("stored".to_owned(), stored.len() as f64);
        numbers.insert("rejected".to_owned(), rejected.len() as f64);
        numbers.insert("unknown_tc_id".to_owned(), unknown_tc as f64);
        let content = serde_json::json!({
            "stored": stored,
            "rejected": rejected,
            "unknown_tc_id": unknown_tc,
        });
        self.tool_result(
            &task_id,
            method::SUBMIT_FINDING,
            content,
            refs,
            numbers,
            Vec::new(),
        )
    }

    fn submit_report_meta(&mut self, params: &serde_json::Value) -> RpcResult<serde_json::Value> {
        let task_id = task_id_of(params)?;
        // Three-fixed metadata (M2v0.2 §9.3): a report is only reproducible if
        // model, temperature and prompt version travel with it.
        let meta = packetsage_core::store::ReportMeta {
            agent_run_id: params
                .get("agent_run_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            model: params
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            provider: params
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            temperature: params
                .get("temperature")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            prompt_version: params
                .get("prompt_version")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            template_version: params
                .get("template_version")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            report_path: params
                .get("report_path")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned(),
            status: params
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("ok")
                .to_owned(),
            unverified_count: params
                .get("unverified_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            tokens_in: params
                .get("tokens_in")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            tokens_out: params
                .get("tokens_out")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cost_cents: params
                .get("cost_cents")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
        };
        let store = self.tasks.get_mut(&task_id).ok_or_else(|| {
            (
                RpcErrorCode::NotFound,
                format!("task {task_id} is not in memory"),
            )
        })?;
        store.report_path = Some(meta.report_path.clone());
        store.status = meta.status.clone();
        store.report_meta = Some(meta.clone());
        let ledger: Vec<(String, LedgerEntry)> = store.ledger.entries();
        let content =
            serde_json::to_value(&meta).map_err(|e| (RpcErrorCode::Internal, e.to_string()))?;
        // The engine is the single writer (ADR-018): record the agent run when a
        // database is configured.
        if let Some(url) = self.database.clone() {
            let recorded = {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| (RpcErrorCode::Internal, e.to_string()))?;
                runtime.block_on(async {
                    use packetsage_storage::{AgentRunRow, Repository, SqliteRepo, ToolCallRow};
                    let repo = SqliteRepo::connect(&url).await?;
                    repo.migrate().await?;
                    repo.upsert_agent_run(&AgentRunRow {
                        id: meta.agent_run_id.clone(),
                        task_id: task_id.clone(),
                        model: meta.model.clone(),
                        status: meta.status.clone(),
                        started_at: packetsage_core::pipeline::now_rfc3339(),
                        finished_at: Some(packetsage_core::pipeline::now_rfc3339()),
                        report_path: Some(meta.report_path.clone()),
                        prompt_version: meta.prompt_version.clone(),
                        temperature: meta.temperature,
                        tokens_in: i64::try_from(meta.tokens_in).unwrap_or(i64::MAX),
                        tokens_out: i64::try_from(meta.tokens_out).unwrap_or(i64::MAX),
                        cost_cents: i64::try_from(meta.cost_cents).unwrap_or(i64::MAX),
                    })
                    .await?;
                    // The tc-id ledger is the analysis trace: persist it with the
                    // run so the report can be reproduced after a restart.
                    for (step, (id, entry)) in ledger.into_iter().enumerate() {
                        repo.insert_tool_call(&ToolCallRow {
                            id,
                            agent_run_id: meta.agent_run_id.clone(),
                            step: i64::try_from(step).unwrap_or(i64::MAX),
                            tool_name: entry.method,
                            args_json: entry.args_json,
                            result_summary: None,
                            status: entry.status,
                            duration_ms: i64::try_from(entry.duration_ms).unwrap_or(i64::MAX),
                            created_at: packetsage_core::pipeline::now_rfc3339(),
                            envelope_hash: None,
                            redactions_json: None,
                            // The V3/V4 facts travel with the row: a later
                            // process has no other way to prove them.
                            numbers_json: serde_json::to_string(&entry.numbers)
                                .unwrap_or_else(|_| "{}".to_owned()),
                            ref_ids_json: serde_json::to_string(&entry.ref_ids)
                                .unwrap_or_else(|_| "[]".to_owned()),
                            tokens_json: serde_json::to_string(&entry.tokens)
                                .unwrap_or_else(|_| "[]".to_owned()),
                        })
                        .await?;
                    }
                    Ok::<(), packetsage_storage::StorageError>(())
                })
            };
            if let Err(error) = recorded {
                tracing::warn!(%error, "agent_runs persistence failed");
            }
        }
        self.tool_result(
            &task_id,
            method::SUBMIT_REPORT_META,
            content,
            BTreeSet::new(),
            BTreeMap::new(),
            Vec::new(),
        )
    }
}

fn task_id_of(params: &serde_json::Value) -> RpcResult<String> {
    params
        .get("task_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            (
                RpcErrorCode::InvalidArgument,
                "this method requires `task_id`".to_owned(),
            )
        })
}

/// Loads the persisted findings of a task into a recovered store (ADR-019).
///
/// Findings are model output that the validator accepted once; they cannot be
/// re-derived by replaying the capture, so the database is the only source.
fn attach_persisted_findings(store: &mut packetsage_core::AnalysisStore, url: &str) {
    let task_id = store.task_id.clone();
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let rows = runtime.block_on(async move {
        // `list_findings` is a `Repository` method: the trait must be in scope.
        use packetsage_storage::{Repository, SqliteRepo};
        let repo = SqliteRepo::connect_readonly_default(url).await.ok()?;
        repo.list_findings(&task_id, 200).await.ok()
    });
    let Some(rows) = rows else { return };
    for row in &rows {
        if let Some(finding) = finding_from_row(row) {
            store.findings.push(finding);
        }
    }
    if !rows.is_empty() {
        tracing::info!(task = %store.task_id, findings = rows.len(), "cold recovery findings");
    }
}

/// `FindingRow` (SQLite) → `Finding` (protocol): the JSON shape of one finding.
fn finding_from_row(row: &packetsage_storage::FindingRow) -> Option<packetsage_protocol::Finding> {
    let evidence: serde_json::Value =
        serde_json::from_str(&row.evidence_json).unwrap_or_else(|_| serde_json::json!([]));
    let value = serde_json::json!({
        "finding_id": row.id,
        "task_id": row.task_id,
        "title": row.title,
        "severity": row.severity,
        "basis": normalize_basis(&row.basis),
        "summary": row.summary,
        "evidence": evidence,
        "validator_status": row.validator_status,
    });
    match serde_json::from_value(value) {
        Ok(finding) => Some(finding),
        Err(error) => {
            // Never drop a stored finding silently: it would surface as an empty
            // Executive Summary with no explanation.
            tracing::warn!(%error, finding = %row.id, "persisted finding could not be restored");
            None
        }
    }
}

/// Re-issues the persisted tool-call ledger of a recovered task.
///
/// `query_history kind=trace` then answers with the *original* run's calls,
/// including their `numbers` / `ref_ids`, so the report's lint can accept every
/// number the task ever produced — and V2/V3 can re-validate stored findings.
fn attach_persisted_ledger(store: &mut packetsage_core::AnalysisStore, url: &str) {
    let task_id = store.task_id.clone();
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let rows = runtime.block_on(async move {
        use packetsage_storage::{Repository, SqliteRepo};
        let repo = SqliteRepo::connect_readonly_default(url).await.ok()?;
        repo.list_tool_calls_for_task(&task_id, 500).await.ok()
    });
    let Some(rows) = rows else { return };
    let issued = rows.len();
    for row in rows {
        store.ledger.issue(
            &row.id,
            packetsage_protocol::LedgerEntry {
                task_id: store.task_id.clone(),
                method: row.tool_name,
                ts_unix_ns: row.created_at,
                args_json: row.args_json,
                duration_ms: u64::try_from(row.duration_ms).unwrap_or_default(),
                status: row.status,
                ref_ids: serde_json::from_str(&row.ref_ids_json).unwrap_or_default(),
                numbers: serde_json::from_str(&row.numbers_json).unwrap_or_default(),
                tokens: serde_json::from_str(&row.tokens_json).unwrap_or_default(),
            },
        );
    }
    if issued > 0 {
        tracing::info!(task = %store.task_id, calls = issued, "cold recovery ledger");
    }
}

/// `persist.rs` writes `format!("{basis:?}").to_lowercase()`, which loses the
/// underscore of the multi-word variants (`correlated_observation` →
/// `correlatedobservation`). Accept both spellings when reading back.
fn normalize_basis(raw: &str) -> String {
    match raw {
        "rulematch" => "rule_match".to_owned(),
        "directobservation" => "direct_observation".to_owned(),
        "correlatedobservation" => "correlated_observation".to_owned(),
        other => other.to_owned(),
    }
}

fn parse_flag(name: &str) -> Option<packetsage_protocol::TcpFlag> {
    use packetsage_protocol::TcpFlag;
    match name.to_ascii_uppercase().as_str() {
        "FIN" => Some(TcpFlag::Fin),
        "SYN" => Some(TcpFlag::Syn),
        "RST" => Some(TcpFlag::Rst),
        "PSH" => Some(TcpFlag::Psh),
        "ACK" => Some(TcpFlag::Ack),
        "URG" => Some(TcpFlag::Urg),
        "ECE" => Some(TcpFlag::Ece),
        "CWR" => Some(TcpFlag::Cwr),
        _ => None,
    }
}

/// Token-shaped strings a tool result surfaced: IPs, `ip:port`, session / rule /
/// alert / task ids, packet indices and bare numbers.
///
/// The report lint must accept every number the engine ever produced for a task,
/// but the tool result body is not persisted — only this whitelist is. Free text
/// (payload previews, finding summaries) never qualifies: it is long, contains
/// whitespace, and may hold secrets.
fn collect_citable_tokens(value: &serde_json::Value) -> BTreeSet<String> {
    /// Longest token we keep; payload text and base64 blobs are longer.
    const MAX_TOKEN: usize = 64;
    /// Upper bound on one call's tokens, so a huge result cannot bloat the row.
    const MAX_TOKENS: usize = 400;

    fn shaped(piece: &str) -> bool {
        if piece.is_empty() || piece.len() > MAX_TOKEN {
            return false;
        }
        if piece
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')))
        {
            return false;
        }
        let numeric = piece.chars().any(|c| c.is_ascii_digit())
            && piece.chars().all(|c| c.is_ascii_digit() || c == '.');
        let ipv4 = {
            let parts: Vec<&str> = piece.split('.').collect();
            parts.len() == 4
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        };
        numeric
            || ipv4
            || piece.starts_with("S-")
            || piece.starts_with("F-")
            || piece.contains("alert_")
            || piece.contains("tc_")
            || piece.contains("task_")
            || (piece.contains('-') && piece.chars().any(|c| c.is_ascii_uppercase()))
            || (piece.contains('_') && piece.len() >= 3)
    }

    fn walk(node: &serde_json::Value, out: &mut BTreeSet<String>) {
        if out.len() >= MAX_TOKENS {
            return;
        }
        match node {
            serde_json::Value::String(text) => {
                for piece in text.split(|c: char| {
                    !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
                }) {
                    // `10.0.2.20:8080` is two citable facts, not one.
                    for part in piece.split(':') {
                        if shaped(part) {
                            out.insert(part.to_owned());
                        }
                    }
                }
            }
            serde_json::Value::Number(number) => {
                out.insert(number.to_string());
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, out);
                }
            }
            serde_json::Value::Object(map) => {
                for item in map.values() {
                    walk(item, out);
                }
            }
            _ => {}
        }
    }

    let mut out = BTreeSet::new();
    walk(value, &mut out);
    out
}

/// Compact, redacted argument summary for the ledger (`args_json`).
///
/// Long free text (payload previews, finding summaries) is replaced by a length
/// marker so the trace never becomes a channel for capture content.
fn summarize_args(args: &serde_json::Value) -> String {
    let compact = match args {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                let rendered = match value {
                    serde_json::Value::String(text) if text.len() > 64 => {
                        serde_json::json!(format!("<{} chars>", text.len()))
                    }
                    serde_json::Value::Array(items) if items.len() > 8 => {
                        serde_json::json!(format!("<{} items>", items.len()))
                    }
                    other => other.clone(),
                };
                out.insert(key.clone(), rendered);
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    };
    serde_json::to_string(&compact).unwrap_or_else(|_| "{}".to_owned())
}

/// Keeps numeric fields, booleans and short strings, drops long text.
fn summarize_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                match item {
                    serde_json::Value::String(text) if text.len() > 128 => {
                        out.insert(key.clone(), serde_json::json!("[omitted]"));
                    }
                    serde_json::Value::Array(items) if items.len() > 32 => {
                        let head: Vec<serde_json::Value> =
                            items.iter().take(32).map(summarize_value).collect();
                        out.insert(key.clone(), serde_json::Value::Array(head));
                    }
                    _ => {
                        out.insert(key.clone(), summarize_value(item));
                    }
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(summarize_value).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_value_drops_long_strings() {
        let long = "x".repeat(500);
        let value = serde_json::json!({"preview": long, "count": 3});
        let summarized = summarize_value(&value);
        assert_eq!(summarized["preview"], serde_json::json!("[omitted]"));
        assert_eq!(summarized["count"], serde_json::json!(3));
    }

    #[test]
    fn flag_parsing_is_case_insensitive() {
        assert_eq!(parse_flag("syn"), Some(packetsage_protocol::TcpFlag::Syn));
        assert_eq!(parse_flag("nope"), None);
    }

    #[test]
    fn persisted_findings_are_restored_with_their_basis() {
        // The stored spelling drops underscores (`{:?}` + lowercase), so the
        // reader has to normalise it — otherwise cold recovery silently loses
        // every multi-word finding and the report comes out empty.
        let row = packetsage_storage::FindingRow {
            id: "F-001".to_owned(),
            task_id: "task_X".to_owned(),
            title: "t".to_owned(),
            severity: "high".to_owned(),
            basis: "correlatedobservation".to_owned(),
            summary: "s".to_owned(),
            evidence_json: "[{\"_id\":\"tc_1\",\"method\":\"filter_packets\"}]".to_owned(),
            validator_status: "accepted".to_owned(),
        };
        let finding = finding_from_row(&row).expect("row must round-trip");
        assert_eq!(finding.finding_id, "F-001");
        assert_eq!(format!("{:?}", finding.basis), "CorrelatedObservation");
        assert_eq!(finding.evidence.len(), 1);
        assert_eq!(normalize_basis("rulematch"), "rule_match");
    }

    #[test]
    fn citable_tokens_keep_facts_and_drop_prose() {
        let content = serde_json::json!({
            "src_ip": "10.0.2.20",
            "endpoint": "10.9.9.9:40000",
            "captured_len": 54,
            "session_id": "S-000012",
            "rule_id": "NET-TCP-SYN-BURST-001",
            "preview": "GET / HTTP/1.1\r\nHost: example.com\r\nAuthorization: Bearer sk-secret",
            "sample_packets": [10, 35, 110],
        });
        let tokens = collect_citable_tokens(&content);
        for expected in [
            "10.0.2.20",
            "10.9.9.9",
            "40000",
            "54",
            "S-000012",
            "NET-TCP-SYN-BURST-001",
            "110",
        ] {
            assert!(tokens.contains(expected), "missing {expected}: {tokens:?}");
        }
        // Prose stays out: only token-shaped pieces are kept.
        assert!(!tokens.iter().any(|t| t.contains(' ')), "{tokens:?}");
        assert!(!tokens.contains("Authorization"));
        assert!(!tokens.contains("Bearer"));
    }
}
