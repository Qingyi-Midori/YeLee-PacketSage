//! Identifier formatting and validation.
//!
//! Generation that requires a clock or randomness (ULID) lives in
//! `packetsage-core::ids`; this module owns the *format* contract only.

use std::fmt;

/// Errors produced by identifier parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    /// The value does not match the expected pattern.
    InvalidFormat {
        /// The identifier kind (for diagnostics).
        kind: &'static str,
        /// The offending value.
        value: String,
    },
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::InvalidFormat { kind, value } => {
                write!(f, "invalid {kind}: {value:?}")
            }
        }
    }
}

impl std::error::Error for IdError {}

/// A validated `task_{ulid}` identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaskId(String);

impl TaskId {
    /// Wraps an already generated `task_{ulid}` string after validation.
    ///
    /// # Errors
    /// Returns [`IdError`] when the value does not match `task_<26 Crockford chars>`.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        if validate_task_id(value) {
            Ok(TaskId(value.to_owned()))
        } else {
            Err(IdError::InvalidFormat {
                kind: "task_id",
                value: value.to_owned(),
            })
        }
    }

    /// Borrows the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TaskId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// `task_` + 26 uppercase Crockford base32 characters.
#[must_use]
pub fn validate_task_id(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("task_") else {
        return false;
    };
    let is_crockford = |b: u8| {
        b.is_ascii_digit() || (b.is_ascii_uppercase() && !matches!(b, b'I' | b'L' | b'O' | b'U'))
    };
    rest.len() == 26 && rest.bytes().all(is_crockford)
}

/// `S-{n:06}`, one-based.
#[must_use]
pub fn session_id(n: u64) -> String {
    format!("S-{n:06}")
}

/// `F-{n:03}`, one-based.
#[must_use]
pub fn finding_id(n: u64) -> String {
    format!("F-{n:03}")
}

/// `alert_{ulid}`.
#[must_use]
pub fn alert_id_from_ulid(ulid: &str) -> String {
    format!("alert_{ulid}")
}

/// Tool-call anchor `tc_{ulid}` (ADR-024).
///
/// The ULID is minted by the engine (`packetsage-core`); this crate only owns
/// the *format* helpers because L0 is not allowed to read a clock.
///
/// ```text
/// tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH
/// ```
#[must_use]
pub fn format_tool_call_id(ulid: &str) -> String {
    format!("tc_{ulid}")
}

/// `tc_` + 26 Crockford base32 characters.
#[must_use]
pub fn validate_tool_call_id(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("tc_") else {
        return false;
    };
    let is_crockford = |b: u8| {
        b.is_ascii_digit() || (b.is_ascii_uppercase() && !matches!(b, b'I' | b'L' | b'O' | b'U'))
    };
    rest.len() == 26 && rest.bytes().all(is_crockford)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_round_trip() {
        let ok = "task_01J9Z4M8YQ2V7C1W3N5B6D8FGH";
        assert!(validate_task_id(ok));
        assert_eq!(
            TaskId::parse(ok).map(|t| t.to_string()).ok(),
            Some(ok.to_owned())
        );
    }

    #[test]
    fn task_id_rejected() {
        for bad in [
            "task_01J9Z4M8YQ2V7C1W3N5B6D8FG",   // 25 chars
            "task_01J9Z4M8YQ2V7C1W3N5B6D8FGHI", // I is not Crockford
            "01J9Z4M8YQ2V7C1W3N5B6D8FGH",
            "task_01J9Z4M8YQ2V7C1W3N5B6D8FGh", // lowercase
        ] {
            assert!(!validate_task_id(bad), "{bad} should be rejected");
        }
    }

    #[test]
    fn sequence_formats() {
        assert_eq!(session_id(1), "S-000001");
        assert_eq!(session_id(421), "S-000421");
        assert_eq!(finding_id(7), "F-007");
    }

    #[test]
    fn tool_call_id_format_is_ulid_based() {
        let id = format_tool_call_id("01J9Z4M8YQ2V7C1W3N5B6D8FGH");
        assert_eq!(id, "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH");
        assert!(validate_tool_call_id(&id));
        for bad in [
            "tc-000123",
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FG",
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGh",
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGI",
        ] {
            assert!(!validate_tool_call_id(bad), "{bad} should be rejected");
        }
    }
}
