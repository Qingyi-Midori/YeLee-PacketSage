#!/usr/bin/env python3
"""GUI contract cases S40-S45 (GUI 前收口文档 v0.1 §4.4).

The GUI only ever reads machine output; these cases pin the field sets and the
three-stream discipline before the GUI is started. Every case asserts the exit
code, a key line of stdout and (where relevant) stderr.

    python tests/cli/gui_contract.py --binary target/debug/packetsage.exe

The script is hermetic: a temporary database, a scrubbed environment and the
deterministic `mock` provider only. It never calls an external LLM.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable, Optional


class Failure(AssertionError):
    """A failed contract expectation."""


class Case:
    """One executable row of §4.4."""

    def __init__(self, name: str, spec: str, fn: Callable[["Harness"], None]) -> None:
        self.name = name
        self.spec = spec
        self.fn = fn


CASES: list[Case] = []


def case(name: str, spec: str):
    def decorate(fn):
        CASES.append(Case(name, spec, fn))
        return fn

    return decorate


def expect_code(result: subprocess.CompletedProcess, expected: int) -> None:
    if result.returncode != expected:
        raise Failure(
            f"expected exit {expected}, got {result.returncode} | "
            f"stdout tail={result.stdout[-400:]!r} stderr tail={result.stderr[-400:]!r}"
        )


def expect_keys(row: dict, expected: set, where: str) -> None:
    if set(row) != expected:
        missing = sorted(expected - set(row))
        extra = sorted(set(row) - expected)
        raise Failure(f"{where}: key set changed (missing={missing}, extra={extra})")


def jsonl_rows(stdout: str) -> list[dict]:
    rows = []
    for line in stdout.splitlines():
        if line.strip():
            rows.append(json.loads(line))
    return rows


class Harness:
    """Runs the Rust binary and the Python agent against one temporary store."""

    def __init__(self, binary: Path, root: Path) -> None:
        self.binary = binary
        self.root = root
        self.sample = root / "samples" / "synth-mixed.pcap"
        self.work = Path(tempfile.mkdtemp(prefix="packetsage-gui-"))
        self.db_path = self.work / "gui.db"
        self.db_url = f"sqlite://{self.db_path}"
        self.task_id = "task_01J0000000000000000000000G"
        self._task_ready = False

    # ------------------------------------------------------------- helpers
    def base_env(self, extra: Optional[dict] = None) -> dict:
        env = os.environ.copy()
        for name in (
            "PACKETSAGE_CONFIG",
            "PACKETSAGE_ENGINE",
            "PACKETSAGE_STORAGE_URL",
            "PACKETSAGE_DB",
            "PACKETSAGE_LLM_PROVIDER",
            "PACKETSAGE_LLM_MODEL",
            "PACKETSAGE_LLM_API_KEY",
            "PACKETSAGE_PROVIDER",
            "OPENAI_API_KEY",
        ):
            env.pop(name, None)
        env["PACKETSAGE_ENV_FILE"] = str(self.work / "empty.env")
        if extra:
            env.update({key: str(value) for key, value in extra.items()})
        return env

    def run(self, *args: str, env: Optional[dict] = None) -> subprocess.CompletedProcess:
        return subprocess.run(
            [str(self.binary), *args],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=self.base_env(env),
            timeout=300,
        )

    def run_agent(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, "-m", "packetsage_agent", *args],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=self.base_env(
                {
                    "PYTHONPATH": str(self.root / "agent"),
                    "PACKETSAGE_LLM_PROVIDER": "mock",
                }
            ),
            cwd=str(self.work),
            timeout=300,
        )

    def engine_command(self) -> str:
        return shlex.join([str(self.binary), "serve"])

    def ensure_task(self) -> None:
        """One analysed task in the temporary store (analysis is a prerequisite)."""
        if self._task_ready:
            return
        result = self.run(
            "analyze",
            str(self.sample),
            "--db",
            self.db_url,
            "--task-id",
            self.task_id,
            "-q",
        )
        expect_code(result, 0)
        self._task_ready = True

    def ensure_findings(self) -> None:
        """One stored agent run (mock provider) over the temporary task."""
        self.ensure_task()
        result = self.run_agent(
            "run",
            "--task-id",
            self.task_id,
            "--engine",
            self.engine_command(),
            "--db",
            self.db_url,
            "--json",
        )
        expect_code(result, 0)
        payload = json.loads(result.stdout.strip().splitlines()[-1])
        if not payload["findings"]:
            raise Failure(f"the mock run stored no findings: {result.stdout!r}")


# --------------------------------------------------------------------------
# S40 / S41: query machine rows
# --------------------------------------------------------------------------
SESSION_KEYS = {
    "id",
    "task_id",
    "protocol",
    "src_ip",
    "src_port",
    "dst_ip",
    "dst_port",
    "first_ts",
    "last_ts",
    "packets",
    "bytes",
    "state",
    "state_stored",
    "app_protocol",
    "interface_id",
    "vlan_tag",
    "direction_basis",
}
STATS_KEYS = {"task_id", "layer", "protocol", "sessions", "bytes"}
ALERT_KEYS = {
    "id",
    "task_id",
    "rule_id",
    "rule_version",
    "rule_content_hash",
    "severity",
    "first_packet",
    "last_packet",
    "first_ts",
    "last_ts",
    "src_ip",
    "dst_ip",
    "session_id",
    "group_json",
    "evidence_json",
    "group",
    "evidence",
}
FINDING_KEYS = {
    "id",
    "task_id",
    "title",
    "severity",
    "basis",
    "basis_stored",
    "summary",
    "evidence_json",
    "evidence",
    "validator_status",
}


@case("S40 findings --jsonl is machine readable", "§4.4 S40, G1-1")
def _s40(h: Harness) -> None:
    h.ensure_findings()
    result = h.run("query", "findings", "--db", h.db_url, "--jsonl")
    expect_code(result, 0)
    rows = jsonl_rows(result.stdout)
    assert rows, "the mock run must leave stored findings"
    for row in rows:
        expect_keys(row, FINDING_KEYS, "query findings")
        assert row["task_id"] == h.task_id
        assert row["basis"] in {
            "rule_match",
            "direct_observation",
            "correlated_observation",
            "hypothesis",
        }, row["basis"]
        assert isinstance(row["evidence"], list) and row["evidence"]
    assert "task " + h.task_id not in result.stdout, "the human table must not leak in"


@case("S41 sessions / stats / alerts --jsonl are machine readable", "§4.4 S41, G1-1")
def _s41(h: Harness) -> None:
    h.ensure_findings()
    sessions = h.run("query", "sessions", "--db", h.db_url, "--jsonl", "--limit", "3")
    expect_code(sessions, 0)
    rows = jsonl_rows(sessions.stdout)
    assert len(rows) == 3, f"limit=3 must bound the stream: {sessions.stdout!r}"
    for row in rows:
        expect_keys(row, SESSION_KEYS, "query sessions")
        assert row["state"] in {
            "new",
            "active",
            "half_closed",
            "closed",
            "incomplete",
            "buffer_overflow",
        }, row["state"]

    stats = h.run("query", "stats", "--db", h.db_url, "--jsonl")
    expect_code(stats, 0)
    stats_rows = jsonl_rows(stats.stdout)
    assert stats_rows, "synth-mixed has sessions to aggregate"
    for row in stats_rows:
        expect_keys(row, STATS_KEYS, "query stats")
        assert row["sessions"] > 0

    alerts = h.run("query", "alerts", "--db", h.db_url, "--jsonl")
    expect_code(alerts, 0)
    alert_rows = jsonl_rows(alerts.stdout)
    assert alert_rows, "synth-mixed triggers two rules"
    for row in alert_rows:
        expect_keys(row, ALERT_KEYS, "query alerts")
        assert isinstance(row["evidence"], dict) and row["evidence"]
        assert isinstance(row["group"], list)


@case("query --json writes the same rows to a file", "§4.4 S41, G1-1")
def _query_json_file(h: Harness) -> None:
    h.ensure_task()
    target = h.work / "sessions.jsonl"
    result = h.run(
        "query", "sessions", "--db", h.db_url, "--json", str(target), "--limit", "3"
    )
    expect_code(result, 0)
    assert target.is_file(), "query --json must create the file"
    assert jsonl_rows(target.read_text(encoding="utf-8"))
    assert "task " + h.task_id not in result.stdout, "--json replaces the human table"
    assert "wrote 3 rows" in result.stderr


# --------------------------------------------------------------------------
# S42: packetsage-agent run --json
# --------------------------------------------------------------------------
RUN_KEYS = {
    "summary_version",
    "run_id",
    "task_id",
    "status",
    "stop_reason",
    "accepted",
    "submit_rejects",
    "malformed_output",
    "steps",
    "calls",
    "tool_calls",
    "tokens",
    # v0.4：输入侧缓存命中情况（DeepSeek 上下文硬盘缓存），与 §4.5 的手写表同步。
    "cache",
    "cost_cents",
    "prompt_version",
    "model",
    "provider",
    # v0.4：模型自己写的那段总结（对话里"它说了什么"），与 §4.5 的手写表同步。
    "summary",
    "findings",
}
FINDING_SUMMARY_KEYS = {"id", "severity", "basis", "title", "evidence_ids"}


@case("S42 agent run --json emits exactly one structured line", "§4.4 S42, G1-3")
def _s42(h: Harness) -> None:
    h.ensure_task()
    result = h.run_agent(
        "run",
        "--task-id",
        h.task_id,
        "--engine",
        h.engine_command(),
        "--db",
        h.db_url,
        "--json",
    )
    expect_code(result, 0)
    lines = [line for line in result.stdout.splitlines() if line.strip()]
    assert len(lines) == 1, f"stdout must be exactly one JSON line: {result.stdout!r}"
    payload = json.loads(lines[0])
    expect_keys(payload, RUN_KEYS, "agent run --json")
    assert payload["task_id"] == h.task_id
    assert payload["status"] == "completed"
    assert set(payload["tokens"]) == {"in", "out", "total"}
    assert payload["tokens"]["total"] == payload["tokens"]["in"] + payload["tokens"]["out"]
    assert payload["findings"], "the mock provider submits findings"
    for finding in payload["findings"]:
        expect_keys(finding, FINDING_SUMMARY_KEYS, "run findings[]")
        assert finding["evidence_ids"]


# --------------------------------------------------------------------------
# S43 / S44: version and schema key snapshots
# --------------------------------------------------------------------------
VERSION_KEYS = ["version", "schema_version", "git_hash", "build_time", "profile", "rustc", "target"]


@case("S43 version --json key set and order are frozen", "§4.4 S43, G1-5")
def _s43(h: Harness) -> None:
    first = h.run("version", "--json")
    second = h.run("version", "--json")
    expect_code(first, 0)
    assert first.stdout == second.stdout, "version --json must be deterministic"
    raw = first.stdout.strip()
    payload = json.loads(raw)
    assert list(payload) == VERSION_KEYS, f"key order changed: {list(payload)}"
    positions = [raw.index(f'"{key}"') for key in VERSION_KEYS]
    assert positions == sorted(positions), "serialised key order changed"


@case("S44 schema exposes every frozen limit", "§4.4 S44, G1-4")
def _s44(h: Harness) -> None:
    first = h.run("schema")
    second = h.run("schema")
    expect_code(first, 0)
    assert first.stdout == second.stdout, "schema must be deterministic"
    payload = json.loads(first.stdout)
    assert payload["schema_version"] == 2
    assert payload["limits"] == {
        "tool_result_chars": 8000,
        "reconstruct_stream_bytes": 262144,
        "agent_max_steps": 12,
        "agent_max_llm_calls": 24,
        "agent_max_tool_calls": 20,
    }
    assert len(payload["events"]) == 9
    assert len(payload["rpc_methods"]) == 16


# --------------------------------------------------------------------------
# S45: the file mode and the stdout mode carry the same stream
# --------------------------------------------------------------------------
@case("S45 analyze --json file equals analyze --jsonl stdout", "§4.4 S45, G1-2")
def _s45(h: Harness) -> None:
    streamed = h.run(
        "analyze", str(h.sample), "--jsonl", "--task-id", h.task_id, "-q"
    )
    expect_code(streamed, 0)
    target = h.work / "events.jsonl"
    written = h.run(
        "analyze", str(h.sample), "--json", str(target), "--task-id", h.task_id, "-q"
    )
    expect_code(written, 0)
    assert target.is_file()

    dynamic = re.compile(r'("started_at":"[^"]*"|"alert_id":"[^"]*")')
    from_file = dynamic.sub('"dynamic"', target.read_text(encoding="utf-8"))
    from_stdout = dynamic.sub('"dynamic"', streamed.stdout)
    assert from_file == from_stdout, "file mode and stdout mode drifted"

    events = jsonl_rows(streamed.stdout)
    assert events, "the event stream must not be empty"
    kinds = {event["event"] for event in events}
    assert {"task_started", "capture_info", "stats", "alert", "task_finished"} <= kinds
    stats = next(event for event in events if event["event"] == "stats")
    # G1-2: the GUI must find the window/evidence numbers without a second call.
    assert {
        "packets",
        "bytes",
        "sessions",
        "protocol_stats",
        "decode_errors",
        "truncated_packets",
        "incomplete_sessions",
        "dropped_sessions",
        "rule_window_evictions",
        "submit_rejects",
        "top_conversations",
    } <= set(stats)
    alert = next(event for event in events if event["event"] == "alert")
    assert {"severity", "first_packet", "last_packet", "evidence"} <= set(alert)
    assert {"metric", "value", "window_ns", "operator", "threshold", "sample_packets"} <= set(
        alert["evidence"]
    )
    assert "event" not in streamed.stderr, "stderr must never carry events"


def main(argv: Optional[list[str]] = None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace")
        except (AttributeError, ValueError, OSError):
            pass
    parser = argparse.ArgumentParser(description="GUI contract cases S40-S45")
    parser.add_argument("--binary", default=None)
    parser.add_argument("--filter", default=None)
    args = parser.parse_args(argv)

    root = Path(__file__).resolve().parents[2]
    default = root / "target" / "debug" / ("packetsage.exe" if os.name == "nt" else "packetsage")
    # Resolve relative paths now: the agent spawns its engine from a temporary
    # working directory, so a relative `target/debug/...` would not be found.
    binary = (Path(args.binary) if args.binary else default).resolve()
    if not binary.is_file():
        print(f"gui_contract: binary not found: {binary}", file=sys.stderr)
        return 2
    harness = Harness(binary, root)
    failures: list[tuple[str, str]] = []
    selected = [c for c in CASES if not args.filter or args.filter in c.name]
    for entry in selected:
        try:
            entry.fn(harness)
        except Exception as exc:  # noqa: BLE001 - a failing case is the report
            failures.append((entry.name, str(exc)))
            print(f"FAIL {entry.name}  [{entry.spec}]\n     {exc}")
        else:
            print(f"ok   {entry.name}  [{entry.spec}]")
    print(f"\n{len(selected) - len(failures)}/{len(selected)} GUI contract cases passed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
