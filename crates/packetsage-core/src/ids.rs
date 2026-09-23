//! Identifier generation.
//!
//! ULID generation needs a clock and randomness, which is why it lives in L1
//! instead of `packetsage-protocol` (L0 purity rule).

use packetsage_protocol::{alert_id_from_ulid, TaskId};

/// Generates a fresh `task_{ulid}` identifier.
///
/// # Errors
/// Returns an error string when the ULID cannot be formatted (never in
/// practice; ULIDs are always Crockford base32).
pub fn new_task_id() -> Result<TaskId, String> {
    let raw = ulid::Ulid::new().to_string();
    TaskId::parse(&format!("task_{raw}")).map_err(|e| e.to_string())
}

/// Generates a fresh `alert_{ulid}` identifier.
#[must_use]
pub fn new_alert_id() -> String {
    alert_id_from_ulid(&ulid::Ulid::new().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use packetsage_protocol::validate_task_id;

    #[test]
    fn generated_task_ids_are_valid_and_unique() {
        let a = new_task_id().expect("task id");
        let b = new_task_id().expect("task id");
        assert!(validate_task_id(a.as_str()));
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn generated_alert_ids_have_the_prefix() {
        assert!(new_alert_id().starts_with("alert_"));
    }
}
