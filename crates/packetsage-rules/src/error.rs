//! Rule engine errors.

/// Rule loading / evaluation failures.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    /// The YAML does not match the DSL.
    #[error("rule schema error: {0}")]
    Schema(String),
    /// The YAML is syntactically invalid.
    #[error("rule io error: {0}")]
    Io(String),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, RuleError>;
