"""engine_client tests: framing, timeouts, crashes, heartbeats (§4.9)."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent.engine_client import (  # noqa: E402
    EngineClient,
    EngineCrashed,
    EngineSpawnError,
    RpcTimeouts,
    RpcToolError,
    ToolTimeout,
)

FAKE = str(Path(__file__).resolve().parent / "fake_engine.py")


def client(mode: str = "normal", **kwargs) -> EngineClient:
    return EngineClient([sys.executable, FAKE, mode], **kwargs)


def test_ping_round_trip() -> None:
    with client() as engine:
        result = engine.call("ping", {})
        assert result["schema_version"] == 2


def test_tool_envelope_is_returned_from_call_envelope() -> None:
    with client() as engine:
        call = engine.call_envelope("get_capture_summary", {"task_id": "t"})
        assert call.ok
        assert call.tc_id == "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH"
        assert call.content["packets"] == 612


def test_timeout_is_reported_as_tool_timeout() -> None:
    with client("timeout") as engine:
        with pytest.raises(ToolTimeout):
            engine.call("ping", {}, timeout_s=0.3)


def test_crashed_engine_raises_engine_crashed() -> None:
    engine = client("crash")
    try:
        with pytest.raises(EngineCrashed):
            engine.call("ping", {}, timeout_s=1.0)
    finally:
        engine.close()


def test_out_of_order_responses_are_paired_by_id() -> None:
    with client("out-of-order") as engine:
        first = engine.call("get_capture_summary", {"task_id": "t"})
        second = engine.call("get_conversations", {"task_id": "t"})
        assert first["_id"] == "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH"
        assert second["_id"] == "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGK"


def test_missing_engine_binary_is_a_spawn_error() -> None:
    with pytest.raises(EngineSpawnError):
        EngineClient(["packetsage-does-not-exist-xyz", "serve"])


def test_crashed_engine_carries_its_exit_code_and_stderr_tail(tmp_path) -> None:
    engine = EngineClient([sys.executable, FAKE, "crash:2"], log_path=tmp_path / "engine.log")
    try:
        assert engine.wait(timeout=10) == 2
        with pytest.raises(EngineCrashed) as excinfo:
            engine.call("ping", {}, timeout_s=2.0)
    finally:
        engine.close()
    assert excinfo.value.returncode == 2
    assert excinfo.value.stderr_tail(3) == ["fake engine: starting"]
    assert "fake engine: starting" in (tmp_path / "engine.log").read_text(encoding="utf-8")


def test_envelope_without_the_trust_flag_is_rejected() -> None:
    with client("bad-envelope") as engine:
        call = engine.call_envelope("get_capture_summary", {"task_id": "t"})
        assert not call.ok
        assert "trusted_as_instruction" in (call.error or "")


def test_error_codes_are_surfaced() -> None:
    timeouts = RpcTimeouts(overrides={"ping": 0.2})
    with client("timeout", timeouts=timeouts) as engine:
        with pytest.raises(ToolTimeout):
            engine.call("ping", {})
        assert issubclass(RpcToolError, RuntimeError)


def test_heartbeat_marks_unhealthy_after_two_failures() -> None:
    with client("timeout") as engine:
        engine.timeouts = RpcTimeouts(overrides={"ping": 0.2})
        assert not engine.heartbeat()
        assert not engine.heartbeat()
        assert engine.unhealthy
        with pytest.raises(EngineCrashed):
            engine.call("ping", {})
