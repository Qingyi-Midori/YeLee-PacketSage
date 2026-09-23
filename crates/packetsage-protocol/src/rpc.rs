//! JSONL RPC contract between the Rust engine and the Python agent
//! (开发文档 §21, M0~M2 §3.3, M3~M6 §4.2/§4.3).

use serde::{Deserialize, Serialize};

/// Canonical RPC method names. This module is the single source of truth.
pub mod method {
    /// Liveness probe.
    pub const PING: &str = "ping";
    /// Analyse a capture file and return a task summary.
    pub const ANALYZE_FILE: &str = "analyze_file";
    /// Capture level summary.
    pub const GET_CAPTURE_SUMMARY: &str = "get_capture_summary";
    /// Per-layer protocol statistics.
    pub const GET_PROTOCOL_STATS: &str = "get_protocol_stats";
    /// Session aggregations.
    pub const GET_CONVERSATIONS: &str = "get_conversations";
    /// Packet index filtering.
    pub const FILTER_PACKETS: &str = "filter_packets";
    /// Bounded packet metadata inspection.
    pub const INSPECT_PACKETS: &str = "inspect_packets";
    /// Bounded TCP payload reconstruction.
    pub const RECONSTRUCT_STREAM: &str = "reconstruct_stream";
    /// Rule alerts (single-source routing: memory, else database).
    pub const CHECK_ALERTS: &str = "check_alerts";
    /// History queries for alerts / findings / sessions.
    pub const QUERY_HISTORY: &str = "query_history";
    /// Task artefacts (report path, counters).
    pub const GET_TASK_ARTIFACTS: &str = "get_task_artifacts";
    /// Evidence validation V1-V4 (Rust side of ADR-023).
    pub const VALIDATE_FINDING: &str = "validate_finding";
    /// List the loaded rules.
    pub const LIST_RULES: &str = "list_rules";
    /// Validate a rule file or directory.
    pub const CHECK_RULES: &str = "check_rules";
    /// Submit findings for storage (agent -> engine, ADR-018).
    pub const SUBMIT_FINDING: &str = "submit_finding";
    /// Store report metadata after rendering.
    pub const SUBMIT_REPORT_META: &str = "submit_report_meta";
    /// Every method name, for schema export and tests.
    pub const ALL: &[&str] = &[
        PING,
        ANALYZE_FILE,
        GET_CAPTURE_SUMMARY,
        GET_PROTOCOL_STATS,
        GET_CONVERSATIONS,
        FILTER_PACKETS,
        INSPECT_PACKETS,
        RECONSTRUCT_STREAM,
        CHECK_ALERTS,
        QUERY_HISTORY,
        GET_TASK_ARTIFACTS,
        VALIDATE_FINDING,
        LIST_RULES,
        CHECK_RULES,
        SUBMIT_FINDING,
        SUBMIT_REPORT_META,
    ];
}

/// Incoming RPC request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    /// Caller supplied correlation id (never persisted, never evidence).
    pub id: String,
    /// Method name, one of [`method`].
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Error codes returned to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RpcErrorCode {
    /// Bad or missing parameters.
    InvalidArgument,
    /// The referenced entity does not exist.
    NotFound,
    /// Method exists but is not implemented yet.
    NotImplemented,
    /// Unexpected engine failure.
    Internal,
    /// Filesystem / stream failure.
    Io,
    /// The capture format is outside the supported matrix.
    UnsupportedCapture,
    /// Cold recovery was requested but the source file is gone (ADR-019).
    CaptureUnavailable,
    /// Rule loading or evaluation failure.
    RuleError,
    /// Persistence failure.
    DatabaseError,
    /// The submitted finding failed evidence validation.
    ValidationFailed,
}

/// Error payload of a failed RPC call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    /// Machine readable code.
    pub code: RpcErrorCode,
    /// Human readable message.
    pub message: String,
}

/// RPC response envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    /// Echo of [`RpcRequest::id`].
    pub id: String,
    /// True on success.
    pub ok: bool,
    /// Result payload when `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error payload when `!ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    /// Builds a success response.
    #[must_use]
    pub fn ok(id: &str, result: serde_json::Value) -> Self {
        Self {
            id: id.to_owned(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// Builds an error response.
    #[must_use]
    pub fn err(id: &str, code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            ok: false,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// Where a tool result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustedSource {
    /// Produced by the deterministic Rust engine.
    Engine,
    /// Derived from capture content (untrusted by definition).
    Pcap,
}

/// Tool result envelope (M2v0.2 §9.1 frozen contract).
///
/// `_id` is the tool-call anchor allocated **by Rust** when the result is
/// produced; it is the only legal evidence anchor (M3~M6 §4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolEnvelope {
    /// `tc-{n:06}` allocated by the engine.
    #[serde(rename = "_id")]
    pub id: String,
    /// Result origin.
    pub source: TrustedSource,
    /// Always false: tool results are data, never instructions.
    pub trusted_as_instruction: bool,
    /// Names of the fields that were redacted before leaving the engine.
    #[serde(default)]
    pub redactions: Vec<String>,
    /// Method that produced this result.
    pub method: String,
    /// Result body.
    pub content: serde_json::Value,
}

impl ToolEnvelope {
    /// Creates a tool envelope with `trusted_as_instruction = false`.
    #[must_use]
    pub fn new(
        id: String,
        method: &str,
        source: TrustedSource,
        content: serde_json::Value,
    ) -> Self {
        Self {
            id,
            source,
            trusted_as_instruction: false,
            redactions: Vec::new(),
            method: method.to_owned(),
            content,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_names_are_unique_and_snake_case() {
        let mut seen = std::collections::BTreeSet::new();
        for m in method::ALL {
            assert!(seen.insert(*m), "duplicate method {m}");
            assert!(m.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{m}");
        }
        assert_eq!(seen.len(), method::ALL.len());
    }

    #[test]
    fn rpc_response_error_code_is_screaming_snake() {
        let response = RpcResponse::err("req-1", RpcErrorCode::CaptureUnavailable, "gone");
        let json = serde_json::to_value(&response).expect("serialisable");
        assert_eq!(json["ok"], serde_json::json!(false));
        assert_eq!(
            json["error"]["code"],
            serde_json::json!("CAPTURE_UNAVAILABLE")
        );
    }

    #[test]
    fn envelope_uses_underscore_id() {
        let env = ToolEnvelope::new(
            "tc-000001".to_owned(),
            method::PING,
            TrustedSource::Engine,
            serde_json::json!({"ok": true}),
        );
        let json = serde_json::to_value(&env).expect("serialisable");
        assert_eq!(json["_id"], serde_json::json!("tc-000001"));
        assert_eq!(json["trusted_as_instruction"], serde_json::json!(false));
    }
}
