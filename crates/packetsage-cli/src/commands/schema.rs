//! `packetsage schema`: machine readable digest of the event and RPC contract.

use packetsage_protocol::{method, SCHEMA_VERSION};

/// Builds the schema digest.
///
/// The `limits` block is the single frozen list of the agent budget constants.
/// The values mirror the Python defaults (`agent/packetsage_agent/policy.py`):
/// `max_steps=12`, `max_llm_calls=24`, `max_tool_calls=20`.
#[must_use]
pub fn digest() -> serde_json::Value {
    let events = [
        "task_started",
        "capture_info",
        "packet",
        "decode_error",
        "session_summary",
        "stream_state",
        "stats",
        "alert",
        "task_finished",
    ];
    serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "engine_version": env!("CARGO_PKG_VERSION"),
        "events": events,
        "rpc_methods": method::ALL,
        "exit_codes": {
            "0": "success",
            "1": "invalid argument",
            "2": "capture error",
            "3": "config / database / llm error",
            "4": "internal error | anti-hallucination hard failure",
            "5": "unsupported feature",
        },
        "limits": {
            "tool_result_chars": 8000,
            "reconstruct_stream_bytes": 262_144,
            "agent_max_steps": 12,
            "agent_max_llm_calls": 24,
            "agent_max_tool_calls": 20,
        }
    })
}

/// Prints the schema digest as JSON.
pub fn print_schema() {
    println!(
        "{}",
        serde_json::to_string_pretty(&digest()).unwrap_or_else(|_| "{}".to_owned())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_expose_the_four_agent_budget_constants() {
        let limits = &digest()["limits"];
        assert_eq!(limits["agent_max_steps"], 12);
        assert_eq!(limits["agent_max_llm_calls"], 24);
        assert_eq!(limits["agent_max_tool_calls"], 20);
        assert_eq!(limits["tool_result_chars"], 8000);
        assert_eq!(limits["reconstruct_stream_bytes"], 262_144);
    }

    #[test]
    fn event_names_match_the_frozen_nine() {
        let events = digest()["events"].clone();
        assert_eq!(
            events,
            serde_json::json!([
                "task_started",
                "capture_info",
                "packet",
                "decode_error",
                "session_summary",
                "stream_state",
                "stats",
                "alert",
                "task_finished",
            ])
        );
    }
}
