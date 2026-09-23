//! Row models mirroring `migrations/*.sql` (开发文档 §19 + M3~M6 §3.7).

use serde::{Deserialize, Serialize};

/// `analysis_tasks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRow {
    /// Task id.
    pub id: String,
    /// `ok` / `aborted` / `running`.
    pub status: String,
    /// Source capture path.
    pub source_path: String,
    /// SHA-256 of the capture.
    pub source_sha256: String,
    /// RFC3339 start time.
    pub started_at: String,
    /// RFC3339 finish time.
    pub finished_at: Option<String>,
    /// Packets processed.
    pub packet_count: i64,
    /// Bytes captured.
    pub byte_count: i64,
    /// Failure code when aborted.
    pub error_code: Option<String>,
    /// `FULL` or `CountersOnly`.
    pub index_mode: String,
    /// Hash of the rule set used for the run.
    pub rules_hash: String,
}

/// `captures`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRow {
    /// Task id.
    pub task_id: String,
    /// `pcap` / `pcapng`.
    pub format: String,
    /// First packet timestamp (ns, as text).
    pub first_ts: Option<String>,
    /// Last packet timestamp (ns, as text).
    pub last_ts: Option<String>,
    /// Interface count.
    pub interfaces: i64,
    /// Interface descriptions as JSON.
    pub linktypes_json: String,
}

/// `sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRow {
    /// Session id.
    pub id: String,
    /// Owning task.
    pub task_id: String,
    /// `tcp` / `udp`.
    pub protocol: String,
    /// Endpoint A address.
    pub src_ip: String,
    /// Endpoint A port.
    pub src_port: i64,
    /// Endpoint B address.
    pub dst_ip: String,
    /// Endpoint B port.
    pub dst_port: i64,
    /// First packet timestamp.
    pub first_ts: String,
    /// Last packet timestamp.
    pub last_ts: String,
    /// Packets.
    pub packets: i64,
    /// Bytes.
    pub bytes: i64,
    /// Reassembly state.
    pub state: String,
    /// Application protocol.
    pub app_protocol: Option<String>,
    /// Interface the session was first seen on.
    pub interface_id: Option<i64>,
    /// Outermost VLAN tag.
    pub vlan_tag: Option<i64>,
    /// How the client side was determined.
    pub direction_basis: String,
}

/// `alerts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertRow {
    /// Alert id.
    pub id: String,
    /// Owning task.
    pub task_id: String,
    /// Rule id.
    pub rule_id: String,
    /// Rule version.
    pub rule_version: i64,
    /// Rule content hash prefix.
    pub rule_content_hash: String,
    /// Severity.
    pub severity: String,
    /// First packet index.
    pub first_packet: i64,
    /// Last packet index.
    pub last_packet: i64,
    /// First packet timestamp.
    pub first_ts: String,
    /// Last packet timestamp.
    pub last_ts: String,
    /// Source address of the group (when present).
    pub src_ip: Option<String>,
    /// Destination address of the group (when present).
    pub dst_ip: Option<String>,
    /// Session id (scope: session rules).
    pub session_id: Option<String>,
    /// Group key JSON.
    pub group_json: String,
    /// Evidence JSON.
    pub evidence_json: String,
}

/// `agent_runs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRunRow {
    /// Run id.
    pub id: String,
    /// Owning task.
    pub task_id: String,
    /// Model name.
    pub model: String,
    /// `ok` / `degraded` / `failed`.
    pub status: String,
    /// RFC3339 start.
    pub started_at: String,
    /// RFC3339 finish.
    pub finished_at: Option<String>,
    /// Report path.
    pub report_path: Option<String>,
    /// Prompt version.
    pub prompt_version: String,
    /// Temperature.
    pub temperature: f64,
    /// Input tokens.
    pub tokens_in: i64,
    /// Output tokens.
    pub tokens_out: i64,
    /// Estimated cost in cents.
    pub cost_cents: i64,
}

/// `tool_calls` (also the tc-id ledger; `_id` is the primary key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRow {
    /// `tc-{n:06}` allocated by the engine.
    pub id: String,
    /// Owning agent run.
    pub agent_run_id: String,
    /// Step number.
    pub step: i64,
    /// Tool (RPC method) name.
    pub tool_name: String,
    /// Arguments JSON.
    pub args_json: String,
    /// Result summary.
    pub result_summary: Option<String>,
    /// `ok` / `error` / `timeout`.
    pub status: String,
    /// Duration in milliseconds.
    pub duration_ms: i64,
    /// RFC3339 timestamp.
    pub created_at: String,
    /// Envelope hash (reproducibility).
    pub envelope_hash: Option<String>,
    /// Redaction markers JSON.
    pub redactions_json: Option<String>,
    /// Engine-verified numeric fields of the result, JSON object (V4 facts).
    pub numbers_json: String,
    /// Entities reachable through the result, JSON array (V3 facts).
    pub ref_ids_json: String,
    /// Token-shaped strings the result surfaced, JSON array (lint facts).
    pub tokens_json: String,
}

/// `findings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingRow {
    /// `F-{n:03}` allocated by the engine.
    pub id: String,
    /// Owning task.
    pub task_id: String,
    /// Title.
    pub title: String,
    /// Severity.
    pub severity: String,
    /// Epistemic basis.
    pub basis: String,
    /// Summary.
    pub summary: String,
    /// Evidence JSON array.
    pub evidence_json: String,
    /// `accepted` / `downgraded` / `rejected`.
    pub validator_status: String,
}

/// `artifacts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRow {
    /// Artifact id.
    pub id: String,
    /// Owning task.
    pub task_id: String,
    /// `report` / `events`.
    pub kind: String,
    /// Path on disk.
    pub path: String,
    /// SHA-256 of the file.
    pub sha256: String,
    /// RFC3339 creation time.
    pub created_at: String,
}

/// Alert query filter (structured only: callers never pass SQL).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertFilter {
    /// Restrict to one task.
    pub task_id: Option<String>,
    /// Severity.
    pub severity: Option<String>,
    /// Rule id.
    pub rule_id: Option<String>,
    /// Session id.
    pub session_id: Option<String>,
    /// Maximum rows.
    pub limit: Option<usize>,
}
