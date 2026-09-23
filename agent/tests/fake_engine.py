"""Minimal fake `packetsage serve` used by the engine-client and CLI tests.

Usage: ``python fake_engine.py [mode]`` where mode is one of

* ``normal``          — well behaved worker (default);
* ``timeout``         — never answers;
* ``crash``           — exits 3 before reading stdin (``crash:<code>`` picks
                        another exit code, e.g. ``crash:2``);
* ``out-of-order``    — answers a stale id first, then the real one;
* ``bad-envelope``    — claims ``trusted_as_instruction = true``;
* ``no-task``         — answers every task scoped method with NotFound;
* ``slow:<seconds>``  — sleeps before every answer (a run long enough to cancel
                        or to try a concurrent command; used by the sidecar cases).

The payloads are deliberately close to the real engine's shapes so the CLI
contract (run summary, REPL banner, report rendering) can be driven end to end
without the Rust binary.
"""

from __future__ import annotations

import json
import sys
import time

TC_SUMMARY = "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH"
TC_ALERTS = "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGJ"
TC_SESSIONS = "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGK"
ALERT_ID = "alert_01J9Z4M8YQ2V7C1W3N5B6D8FGH"


def envelope(method: str, content: object, tc_id: str, trusted: bool = False) -> dict:
    """One frozen tool envelope."""
    return {
        "_id": tc_id,
        "source": "engine",
        "trusted_as_instruction": trusted,
        "redactions": [],
        "method": method,
        "content": content,
    }


def content_for(method: str, params: dict) -> tuple[object, str]:
    """The `(content, tc id)` of one method."""
    if method == "get_capture_summary":
        return (
            {
                "task_id": params.get("task_id"),
                "source_path": "samples/synth-mixed.pcap",
                "source_sha256": "0" * 64,
                "format": "pcap",
                "packets": 612,
                "bytes": 29045,
                "sessions": 570,
                "alerts": 1,
                "decode_errors": 0,
                "truncated_packets": 0,
                "incomplete_sessions": 0,
                "reader_nonfatal": 0,
                "dropped_sessions": 0,
                "duration_s": 0.0611,
                "first_ts_ns": "1700000000000100000",
                "last_ts_ns": "1700000000061200000",
            },
            TC_SUMMARY,
        )
    if method == "list_rules":
        return (
            {
                "loaded": 4,
                "rules": [
                    {"id": "NET-TCP-SYN-BURST-001"},
                    {"id": "NET-TCP-PORT-SWEEP-001"},
                    {"id": "NET-DNS-SUSPICIOUS-001"},
                    {"id": "NET-MALFORMED-BURST-001"},
                ],
            },
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGM",
        )
    if method == "get_protocol_stats":
        return (
            {
                "layer": params.get("layer", "transport"),
                "rows": [{"protocol": "tcp", "packets": 600, "bytes": 28000}],
            },
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGN",
        )
    if method == "get_conversations":
        return (
            {
                "conversations": [
                    {
                        "session_id": "S-000001",
                        "protocol": "tcp",
                        "src_ip": "127.0.0.1",
                        "src_port": 8080,
                        "dst_ip": "127.0.0.1",
                        "dst_port": 33412,
                        "packets": 612,
                        "bytes": 29045,
                        "state": "active",
                        "app_protocol": "http",
                        "direction_basis": "syn_first",
                    }
                ]
            },
            TC_SESSIONS,
        )
    if method == "check_alerts":
        return (
            {
                "alerts": [
                    {
                        "alert_id": ALERT_ID,
                        "rule_id": "NET-TCP-SYN-BURST-001",
                        "rule_version": 1,
                        "severity": "high",
                        "first_packet": 8,
                        "last_packet": 108,
                        "group_key": [["dst_port", 80], ["src_ip", "127.0.0.1"]],
                        "evidence": {"value": 101.0, "threshold": 100.0},
                    }
                ]
            },
            TC_ALERTS,
        )
    if method == "filter_packets":
        return ({"packet_indices": [0, 1, 2]}, "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGP")
    if method == "inspect_packets":
        return ({"packets": [{"index": 0, "protocol": "tcp"}]}, "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGQ")
    if method == "reconstruct_stream":
        return (
            {"session_id": params.get("session_id"), "bytes": 74, "preview": "GET / HTTP/1.1"},
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGR",
        )
    if method == "query_history":
        kind = params.get("kind", "alerts")
        if kind == "findings":
            body: object = {"kind": "findings", "findings": []}
        elif kind == "trace":
            body = {
                "kind": "trace",
                "trace": [
                    {
                        "_id": TC_SUMMARY,
                        "method": "get_capture_summary",
                        "ts_unix_ns": "1700000000000100000",
                        "args": "{}",
                        "duration_ms": 1,
                        "status": "ok",
                    }
                ],
            }
        else:
            body = {"kind": "sessions", "sessions": []}
        return (body, "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGS")
    if method == "get_task_artifacts":
        return (
            {
                "task_id": params.get("task_id"),
                "report_path": None,
                "report_meta": None,
                "alert_count": 1,
                "session_count": 1,
                "finding_count": 0,
                "source_path": "samples/synth-mixed.pcap",
                "source_sha256": "0" * 64,
                "events_schema_version": 2,
            },
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGT",
        )
    if method == "validate_finding":
        return (
            {"status": "accepted", "basis": "rule_match", "issues": [], "checks": []},
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGV",
        )
    if method == "submit_finding":
        drafts = params.get("drafts") or []
        stored = [
            {
                "finding_id": f"F-{index + 1:03d}",
                "title": draft.get("title"),
                "severity": draft.get("severity"),
                "basis": draft.get("basis"),
                "summary": draft.get("summary"),
                "validator_status": "accepted",
                "evidence": draft.get("evidence", []),
            }
            for index, draft in enumerate(drafts)
        ]
        return (
            {"stored": stored, "rejected": [], "unknown_tc_id": 0},
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGW",
        )
    if method == "submit_report_meta":
        return ({"status": params.get("status", "ok")}, "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGX")
    if method == "analyze_file":
        return (
            {"task_id": "task_01J9Z4M8YQ2V7C1W3N5B6D8FGH"},
            "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGY",
        )
    return ({}, "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGZ")


def main() -> int:
    """JSONL loop mirroring the real worker's framing."""
    mode = sys.argv[1] if len(sys.argv) > 1 else "normal"
    delay = 0.0
    if mode.startswith("slow"):
        _, _, raw = mode.partition(":")
        delay = float(raw or "0.3")
        mode = "normal"
    if mode.startswith("crash"):
        _, _, code = mode.partition(":")
        print("fake engine: starting", file=sys.stderr)
        return int(code) if code else 3
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        request = json.loads(line)
        method = request.get("method")
        if delay:
            time.sleep(delay)
        if mode == "timeout":
            continue
        if mode == "out-of-order":
            stale = {"id": "stale-id", "ok": True, "result": {"stale": True}}
            sys.stdout.write(json.dumps(stale) + "\n")
            sys.stdout.flush()
        if method == "ping":
            result = {"version": "0.0.0-fake", "schema_version": 2}
        elif mode == "no-task" and method not in ("ping", "list_rules", "analyze_file"):
            sys.stdout.write(
                json.dumps(
                    {
                        "id": request["id"],
                        "ok": False,
                        "error": {
                            "code": "NOT_FOUND",
                            "message": "task is not in memory; re-run analyze_file",
                        },
                    }
                )
                + "\n"
            )
            sys.stdout.flush()
            continue
        else:
            content, tc_id = content_for(method, request.get("params") or {})
            result = envelope(
                method,
                content,
                tc_id,
                trusted=(mode == "bad-envelope" and method == "get_capture_summary"),
            )
        sys.stdout.write(json.dumps({"id": request["id"], "ok": True, "result": result}) + "\n")
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
