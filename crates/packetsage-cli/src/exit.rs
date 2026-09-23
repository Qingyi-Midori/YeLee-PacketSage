//! Exit codes (M0~M2 §5.4, 开发文档 §23).

/// Process exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitCode {
    /// Success.
    Success = 0,
    /// Invalid CLI argument / user input.
    Usage = 1,
    /// Capture format / parsing error.
    CaptureError = 2,
    /// Configuration / database / LLM error.
    ConfigError = 3,
    /// Internal error, or the M5 anti-hallucination hard failure.
    Internal = 4,
    /// Feature not implemented yet.
    Unsupported = 5,
}

impl ExitCode {
    /// Numeric value.
    #[must_use]
    pub fn code(self) -> u8 {
        self as u8
    }
}

impl From<packetsage_core::PacketSageError> for ExitCode {
    fn from(error: packetsage_core::PacketSageError) -> Self {
        match error {
            packetsage_core::PacketSageError::InvalidArgument(_) => ExitCode::Usage,
            packetsage_core::PacketSageError::UnsupportedCapture { .. }
            | packetsage_core::PacketSageError::CaptureCorrupted { .. }
            | packetsage_core::PacketSageError::Io(_) => ExitCode::CaptureError,
            packetsage_core::PacketSageError::RuleError(_)
            | packetsage_core::PacketSageError::DatabaseError(_)
            | packetsage_core::PacketSageError::LlmError(_) => ExitCode::ConfigError,
            packetsage_core::PacketSageError::DecodeError { .. } => ExitCode::Success,
            packetsage_core::PacketSageError::ToolError(_)
            | packetsage_core::PacketSageError::Internal(_) => ExitCode::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use packetsage_core::PacketSageError;

    #[test]
    fn error_to_exit_code_mapping() {
        assert_eq!(
            ExitCode::from(PacketSageError::InvalidArgument("x".to_owned())).code(),
            1
        );
        assert_eq!(
            ExitCode::from(PacketSageError::CaptureCorrupted {
                offset: 1,
                reason: "x".to_owned()
            })
            .code(),
            2
        );
        assert_eq!(
            ExitCode::from(PacketSageError::DatabaseError("x".to_owned())).code(),
            3
        );
        assert_eq!(
            ExitCode::from(PacketSageError::Internal("x".to_owned())).code(),
            4
        );
    }
}
