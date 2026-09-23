//! `packetsage-protocol` (L0): pure data-transfer objects for the PacketSage
//! JSONL event stream and the JSONL RPC channel.
//!
//! Layer rules (see M0~M2 §2.3):
//!
//! - this crate depends only on `serde` / `serde_json`;
//! - no IO, no clock, no randomness is allowed here;
//! - identifiers produced here are *format* helpers; the ULID generation that
//!   needs a clock lives in `packetsage-core::ids`.

#![forbid(unsafe_code)]
// Tests assert with `expect()`; production code uses `?` (workspace lints).
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod events;
pub mod evidence;
pub mod ids;
pub mod rpc;

pub use events::{
    AlertEvent, AlertEvidence, AppDetail, AppProto, ApplicationInfo, CaptureInfoEvent,
    DecodeErrorCode, DecodeErrorCount, DecodeErrorEvent, DecodeErrorInfo, DecodeStatus, DhcpDetail,
    DirectionBasis, DnsDetail, EngineEvent, HttpDetail, IcmpInfo, InterfaceInfo, Layer, LinkInfo,
    NetProto, NetworkInfo, PacketEvent, PayloadRef, ProtocolCount, SessionSummaryEvent, StatsEvent,
    StreamState, StreamStateEvent, TaskFinishedEvent, TaskStartedEvent, TcpFlag, TlsDetail,
    TransportInfo, TsPrecision,
};
pub use evidence::{
    validate_finding_v1_v4, Basis, EvidenceLedger, EvidenceRef, Finding, FindingDraft, LedgerEntry,
    NumericClaim, Severity, ValidationIssue, ValidationOutcome, ValidatorStatus,
};
pub use ids::{
    alert_id_from_ulid, finding_id, format_tool_call_id, session_id, validate_task_id,
    validate_tool_call_id, IdError, TaskId,
};
pub use rpc::{
    method, RpcError, RpcErrorCode, RpcRequest, RpcResponse, ToolEnvelope, TrustedSource,
};

/// Version of the JSONL event schema produced by the engine.
///
/// ADR-013: M3 bumped 1 -> 2 because the event stream gained the `alert`
/// variant. Consumers must branch on this value.
pub const SCHEMA_VERSION: u32 = 2;

/// Deterministic task id used by golden fixtures, tests and the mock engine.
///
/// The M0~M2 §8.2 sample writes `task_TEST00000000000000000000`, which is 24
/// characters after the prefix and therefore *not* a valid ULID. The verified
/// value below keeps the readable `TEST` marker and the mandatory 26
/// characters (see deviation D-6 in `docs/architecture.md`).
pub const GOLDEN_TASK_ID: &str = "task_TEST0000000000000000000000";
