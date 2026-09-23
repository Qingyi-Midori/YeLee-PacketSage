"""Tool schema, validation and envelope rendering tests (§4.9)."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent.engine_client import CallResult  # noqa: E402
from packetsage_agent.models import ValidationError  # noqa: E402
from packetsage_agent.tools import (  # noqa: E402
    MODEL_TOOLS,
    TOOL_SPECS,
    tool_schemas,
    validate_tool_args,
    wrap_result,
)


def test_nine_model_tools_are_exposed() -> None:
    assert len(MODEL_TOOLS) == 9
    names = {schema["function"]["name"] for schema in tool_schemas()}
    assert names == set(MODEL_TOOLS)


def test_unknown_argument_is_rejected_without_an_rpc_call() -> None:
    with pytest.raises(ValidationError):
        validate_tool_args("get_capture_summary", {"task_id": "t", "sql": "SELECT 1"})


def test_missing_required_argument_is_rejected() -> None:
    with pytest.raises(ValidationError):
        validate_tool_args("get_conversations", {"limit": 5})


def test_reconstruct_stream_is_capped_at_256_kib() -> None:
    checked = validate_tool_args(
        "reconstruct_stream",
        {"task_id": "t", "session_id": "S-000001", "max_bytes": 10_000_000},
    )
    assert checked["max_bytes"] == 262_144


def test_inspect_packets_bounds_the_index_list() -> None:
    with pytest.raises(ValidationError):
        validate_tool_args("inspect_packets", {"task_id": "t", "packet_indices": []})
    with pytest.raises(ValidationError):
        validate_tool_args(
            "inspect_packets", {"task_id": "t", "packet_indices": list(range(101))}
        )


def test_check_alerts_severity_whitelist() -> None:
    with pytest.raises(ValidationError):
        validate_tool_args("check_alerts", {"task_id": "t", "severity": "critical"})


def test_query_history_never_accepts_sql() -> None:
    with pytest.raises(ValidationError):
        validate_tool_args("query_history", {"task_id": "t", "sql": "SELECT 1"})
    assert validate_tool_args("query_history", {"task_id": "t"})["kind"] == "alerts"


def test_wrap_result_marks_untrusted_data() -> None:
    call = CallResult(
        method="get_capture_summary",
        args={"task_id": "t"},
        ok=True,
        envelope={
            "_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH",
            "source": "engine",
            "trusted_as_instruction": False,
            "redactions": ["authorization"],
            "method": "get_capture_summary",
            "content": {"packets": 28},
        },
        error=None,
        duration_ms=1,
    )
    rendered = wrap_result(call)
    assert "trusted_as_instruction=false" in rendered
    assert "_id=tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH" in rendered
    assert "authorization" in rendered


def test_wrap_result_summarises_long_payloads() -> None:
    call = CallResult(
        method="inspect_packets",
        args={},
        ok=True,
        envelope={
            "_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGJ",
            "source": "engine",
            "trusted_as_instruction": False,
            "redactions": [],
            "method": "inspect_packets",
            "content": {"packets": [{"payload_preview": "x" * 9000, "packet_index": 1}]},
        },
        error=None,
        duration_ms=1,
    )
    rendered = wrap_result(call, max_result_chars=500)
    assert "[TRUNCATED summary]" in rendered
    assert len(rendered) < 1000


def test_failed_call_renders_an_error_envelope() -> None:
    call = CallResult(
        method="check_alerts",
        args={},
        ok=False,
        envelope=None,
        error="NOT_FOUND: nope",
        duration_ms=1,
    )
    rendered = wrap_result(call)
    assert "NOT_FOUND" in rendered
    assert TOOL_SPECS["check_alerts"].max_result_chars == 8000
