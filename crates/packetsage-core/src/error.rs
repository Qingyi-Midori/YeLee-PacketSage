//! Unified error model (M0~M2 §4.9, 开发文档 §25).

use std::path::PathBuf;

use packetsage_protocol::{DecodeErrorCode, Layer};

/// Every failure that may cross a crate boundary.
#[derive(Debug, thiserror::Error)]
pub enum PacketSageError {
    /// Bad user input or bad parameters.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// File is not a capture we support.
    #[error("unsupported capture: {path}: {reason}")]
    UnsupportedCapture {
        /// Path that failed.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// Structural damage that prevents locating the next record.
    #[error("capture corrupted at byte {offset}: {reason}")]
    CaptureCorrupted {
        /// Byte offset where the reader gave up.
        offset: u64,
        /// Why.
        reason: String,
    },
    /// A packet-level decode failure surfaced as an error value.
    #[error("decode error packet #{packet_index} layer {layer:?}: {code:?}")]
    DecodeError {
        /// Packet index.
        packet_index: u64,
        /// Failing layer.
        layer: Layer,
        /// Machine readable code.
        code: DecodeErrorCode,
    },
    /// Rule loading or evaluation failure (M3).
    #[error("rule error: {0}")]
    RuleError(String),
    /// Persistence failure (M3).
    #[error("database error: {0}")]
    DatabaseError(String),
    /// LLM failure (M4).
    #[error("llm error: {0}")]
    LlmError(String),
    /// Tool failure (M4).
    #[error("tool error: {0}")]
    ToolError(String),
    /// IO failure.
    #[error("io error: {0}")]
    Io(String),
    /// Anything the engine did not anticipate.
    #[error("internal error: {0}")]
    Internal(String),
}

impl PacketSageError {
    /// Exit code this error maps to (M0~M2 §5.4).
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            PacketSageError::InvalidArgument(_) => 1,
            PacketSageError::UnsupportedCapture { .. }
            | PacketSageError::CaptureCorrupted { .. }
            | PacketSageError::Io(_) => 2,
            PacketSageError::RuleError(_)
            | PacketSageError::DatabaseError(_)
            | PacketSageError::LlmError(_) => 3,
            PacketSageError::ToolError(_) | PacketSageError::Internal(_) => 4,
            PacketSageError::DecodeError { .. } => 0,
        }
    }

    /// Stable machine-readable code string used in `TaskFinished.error_code`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            PacketSageError::InvalidArgument(_) => "INVALID_ARGUMENT",
            PacketSageError::UnsupportedCapture { .. } => "UNSUPPORTED_CAPTURE",
            PacketSageError::CaptureCorrupted { .. } => "CAPTURE_CORRUPTED",
            PacketSageError::DecodeError { .. } => "DECODE_ERROR",
            PacketSageError::RuleError(_) => "RULE_ERROR",
            PacketSageError::DatabaseError(_) => "DATABASE_ERROR",
            PacketSageError::LlmError(_) => "LLM_ERROR",
            PacketSageError::ToolError(_) => "TOOL_ERROR",
            PacketSageError::Io(_) => "IO",
            PacketSageError::Internal(_) => "INTERNAL",
        }
    }
}

impl From<std::io::Error> for PacketSageError {
    fn from(value: std::io::Error) -> Self {
        PacketSageError::Io(value.to_string())
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, PacketSageError>;
