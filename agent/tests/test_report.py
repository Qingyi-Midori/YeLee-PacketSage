"""Report rendering and the two level anti-hallucination lint (§5.6)."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent.report import (  # noqa: E402
    AntiHallucinationError,
    LegalFacts,
    ReportGenerator,
    lint,
)

TASK = "task_TEST0000000000000000000000"


class FakeClient:
    """Feeds prepared RPC payloads into the generator."""

    def __init__(self, payloads: dict) -> None:
        self.payloads = payloads
        self.submitted: list[dict] = []

    def call(self, method: str, params: dict):
        """Returns an envelope shaped result for the requested method."""
        if method == "submit_report_meta":
            self.submitted.append(params)
            return {"task_id": params.get("task_id")}
        if method == "query_history":
            kind = params.get("kind", "alerts")
            key = {"findings": "findings", "trace": "trace"}.get(kind, "sessions")
            content = self.payloads.get(key, {kind: []})
            return {
                "_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH",
                "source": "engine",
                "trusted_as_instruction": False,
                "redactions": [],
                "method": method,
                "content": content,
            }
        key = method if method != "get_protocol_stats" else f"stats:{params.get('layer')}"
        content = self.payloads.get(key, {})
        return {
            "_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH",
            "source": "engine",
            "trusted_as_instruction": False,
            "redactions": [],
            "method": method,
            "content": content,
        }


def payloads(findings=None, alerts=None) -> dict:
    """Baseline payload set for one benign HTTP capture."""
    return {
        "get_capture_summary": {
            "task_id": TASK,
            "source_path": "x.pcap",
            "source_sha256": "abc",
            "format": "pcap",
            "packets": 28,
            "bytes": 29045,
            "sessions": 1,
            "duration_s": 1.07,
            "truncated_packets": 0,
            "incomplete_sessions": 0,
            "decode_errors": 0,
            "reader_nonfatal": 0,
            "dropped_sessions": 0,
            "first_ts_ns": "1",
            "last_ts_ns": "2",
        },
        "stats:link": {
            "layer": "link",
            "rows": [{"protocol": "ethernet", "packets": 28, "bytes": 29045}],
        },
        "stats:network": {
            "layer": "network",
            "rows": [{"protocol": "ipv4", "packets": 28, "bytes": 29045}],
        },
        "stats:transport": {
            "layer": "transport",
            "rows": [{"protocol": "tcp", "packets": 28, "bytes": 29045}],
        },
        "stats:application": {
            "layer": "application",
            "rows": [{"protocol": "http", "packets": 2, "bytes": 910}],
        },
        "get_conversations": {
            "conversations": [
                {
                    "session_id": "S-000001",
                    "protocol": "tcp",
                    "src_ip": "127.0.0.1",
                    "src_port": 8080,
                    "dst_ip": "127.0.0.1",
                    "dst_port": 33412,
                    "packets": 28,
                    "bytes": 29045,
                    "state": "active",
                    "app_protocol": "http",
                    "direction_basis": "syn_first",
                }
            ]
        },
        "check_alerts": {"alerts": alerts or []},
        "findings": findings or {"findings": []},
        "sessions": {"sessions": []},
        "trace": {
            "trace": [
                {"_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH", "method": "get_capture_summary", "ts_unix_ns": "1"}
            ]
        },
        "get_task_artifacts": {"report_path": None, "alert_count": 0, "session_count": 1},
    }


def test_report_renders_nine_sections(tmp_path: Path) -> None:
    alerts = [
        {
            "alert_id": "alert_01J9Z4M8YQ2V7C1W3N5B6D8FGH",
            "rule_id": "NET-TCP-SYN-BURST-001",
            "rule_version": 1,
            "severity": "high",
            "first_packet": 8,
            "last_packet": 108,
            "evidence": {"value": 101.0, "threshold": 100.0, "sample_packets": [8, 108]},
        }
    ]
    client = FakeClient(payloads(alerts=alerts))
    generator = ReportGenerator(
        client,
        TASK,
        "run_1",
        model="mock",
        provider="mock",
        tokens_in=1244,
        tokens_out=204,
    )
    meta = generator.generate(tmp_path / "report.md")
    text = (tmp_path / "report.md").read_text(encoding="utf-8")
    for heading in (
        "## 1. Executive Summary",
        "## 2. Capture Overview",
        "## 3. Protocol Statistics",
        "## 4. Top Conversations",
        "## 5. Rule Alerts",
        "## 6. Agent Findings",
        "## 7. Evidence",
        "## 8. Limitations",
        "## 9. Analysis Trace",
    ):
        assert heading in text
    assert meta.status == "ok"
    assert meta.unverified_count == 0
    assert client.submitted and client.submitted[0]["status"] == "ok"
    # D3: the three fixed values travel with the report.
    assert "- model: `mock` · provider: `mock` · temperature: 0.0" in text
    assert "- usage: tokens_in 1244" in text
    assert client.submitted[0]["model"] == "mock"
    # The default prompt version is v2 (《Agent 系统提示词规格 v0.1》§0.3/S5).
    assert client.submitted[0]["prompt_version"] == "v2"
    # C2/C8: unambiguous column names.
    assert "packet_range" in text
    assert "client -> server" in text
    assert "endpoint_a" in text
    # C6: forward slashes only.
    assert "samples/synth" in text or "x.pcap" in text
    assert "\\" not in text.split("```")[0]


def test_integer_counts_are_not_rendered_as_floats(tmp_path: Path) -> None:
    alerts = [
        {
            "alert_id": "alert_01J9Z4M8YQ2V7C1W3N5B6D8FGH",
            "rule_id": "NET-TCP-SYN-BURST-001",
            "rule_version": 1,
            "severity": "high",
            "first_packet": 8,
            "last_packet": 108,
            "evidence": {"value": 101.0, "threshold": 100.0, "sample_packets": [8, 33, 58]},
        }
    ]
    client = FakeClient(payloads(alerts=alerts))
    generator = ReportGenerator(client, TASK, "run_1")
    meta = generator.generate(tmp_path / "report.md")
    text = (tmp_path / "report.md").read_text(encoding="utf-8")
    assert meta.status == "ok"
    assert "| 101 | 100 |" in text
    assert "101.0" not in text


def test_evidence_index_is_deduplicated(tmp_path: Path) -> None:
    findings = {
        "findings": [
            {
                "finding_id": "F-001",
                "title": "a",
                "severity": "high",
                "basis": "rule_match",
                "summary": "s",
                "validator_status": "accepted",
                "evidence": [
                    {"_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH", "method": "check_alerts"},
                    {"_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH", "method": "check_alerts"},
                ],
            },
            {
                "finding_id": "F-002",
                "title": "b",
                "severity": "low",
                "basis": "rule_match",
                "summary": "s",
                "validator_status": "accepted",
                "evidence": [
                    {"_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH", "method": "check_alerts"}
                ],
            },
        ]
    }
    client = FakeClient(payloads(findings=findings))
    generator = ReportGenerator(client, TASK, "run_1")
    generator.generate(tmp_path / "report.md")
    text = (tmp_path / "report.md").read_text(encoding="utf-8")
    index = text.split("Evidence index")[1]
    assert index.count("tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH | check_alerts") == 1


def test_trace_labels_agent_and_report_stages(tmp_path: Path) -> None:
    client = FakeClient(payloads())
    generator = ReportGenerator(client, TASK, "run_1")
    generator.generate(tmp_path / "report.md")
    text = (tmp_path / "report.md").read_text(encoding="utf-8")
    trace = text.split("## 9. Analysis Trace")[1]
    # The only ledger row is the summary call this generator made itself.
    assert "| tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH | report |" in trace
    assert "duration_ms" in trace


def test_report_carries_the_selected_prompt_version(tmp_path: Path) -> None:
    """`agent.prompt_version` travels with the report (§9, §0.2 bump discipline)."""
    client = FakeClient(payloads())
    generator = ReportGenerator(client, TASK, "run_1", prompt_version="v1")
    meta = generator.generate(tmp_path / "report.md")
    text = (tmp_path / "report.md").read_text(encoding="utf-8")
    assert meta.prompt_version == "v1"
    assert client.submitted[0]["prompt_version"] == "v1"
    assert "- template: `v1` · prompt: `v1`" in text


def test_executive_summary_hallucination_is_a_hard_failure(tmp_path: Path) -> None:
    findings = {
        "findings": [
            {
                "finding_id": "F-001",
                "title": "SYN burst",
                "severity": "high",
                "basis": "rule_match",
                "summary": "观察到 999999 个 SYN 数据包",
                "validator_status": "accepted",
                "evidence": [
                    {
                        "_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH",
                        "method": "check_alerts",
                        "ref_id": "NET-TCP-SYN-BURST-001",
                    }
                ],
            }
        ]
    }
    client = FakeClient(payloads(findings=findings))
    generator = ReportGenerator(client, TASK, "run_1")
    with pytest.raises(AntiHallucinationError) as excinfo:
        generator.generate(tmp_path / "report.md")
    assert "anti-hallucination" in str(excinfo.value)
    assert not (tmp_path / "report.md").exists()


def test_body_hallucination_degrades_the_report(tmp_path: Path) -> None:
    client = FakeClient(payloads())
    generator = ReportGenerator(client, TASK, "run_1")
    data = generator.collect()
    generator.legal.tokens.discard("29045")
    text, unverified, degraded = generator.render(data)
    assert degraded is True
    assert "[unverified by engine]" in text
    assert unverified >= 1


def test_lint_accepts_engine_facts_and_rejects_forgeries() -> None:
    legal = LegalFacts()
    legal.add_numbers(
        {"packets": 28, "session_id": "S-000001", "rule_id": "NET-TCP-SYN-BURST-001"}
    )
    assert lint("28 packets in S-000001", legal) == []
    offenders = lint("999 packets in S-99999", legal)
    assert "999" in offenders
    assert "S-99999" in offenders


def test_structural_identifiers_are_not_flagged() -> None:
    legal = LegalFacts()
    assert lint("见 ADR-007 与 V1 校验", legal) == []
