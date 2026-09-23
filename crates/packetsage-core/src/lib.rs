//! PacketSage core engine (L1).
//!
//! Owns the deterministic half of the pipeline:
//! `reader -> decoder -> reassembly -> conversation -> query -> pipeline`.

#![forbid(unsafe_code)]
// Test code asserts with `expect()` on purpose (the workspace lints keep it a
// warning in production code, where `?` is mandatory).
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod config;
pub mod conversation;
pub mod decoder;
pub mod error;
pub mod ids;
pub mod model;
pub mod pipeline;
pub mod query;
pub mod reader;
pub mod reassembly;
pub mod store;

pub use config::{EngineConfig, PacketEventMode};
pub use decoder::{DecodeOptions, Decoder};
pub use error::{PacketSageError, Result};
pub use model::{ClientSide, Decoded, Endpoint, PacketRecord, SessionKey, TransportProto};
pub use pipeline::{
    analyze_file, AnalysisResult, AnalyzePipeline, CollectingSink, CountingSink, EventSink,
    JsonlSink, ProgressCounters, ProgressPhase, ProgressSnapshot, RuleHook, TaskSummary,
};
pub use query::{
    CaptureSummary, ConversationDto, ConversationQuery, PacketFilter, PacketIndex, PacketMeta,
    ProtocolStats, QueryEngine, StreamPreview, StreamQuery,
};
pub use reader::{
    detect_format, CaptureFormat, CaptureReader, InterfaceState, NonFatalInfo, NonFatalKind,
    RawPacketRecord, ReaderSummary, SourceItem, TsResolution,
};
pub use store::{capture_fingerprint, AnalysisStore, ReportMeta};
