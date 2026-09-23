"""Policy gates and FindingDraft validation (M3~M6 §4.4/§4.9)."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent.models import (  # noqa: E402
    EvidenceRef,
    FindingDraft,
    ValidationError,
    parse_finding,
)
from packetsage_agent.policy import AgentBudget, AgentPolicy  # noqa: E402


def draft(**overrides) -> FindingDraft:
    """Valid draft with optional overrides."""
    base = FindingDraft(
        title="t",
        severity="high",
        basis="rule_match",
        summary="s",
        evidence=[EvidenceRef(_id="tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH", method="check_alerts", ref_id="RULE-001")],
    )
    for key, value in overrides.items():
        setattr(base, key, value)
    return base


def test_severity_outside_the_whitelist_is_rejected() -> None:
    with pytest.raises(ValidationError) as excinfo:
        draft(severity="critical").validate()
    assert "critical" in str(excinfo.value)


def test_basis_outside_the_whitelist_is_rejected() -> None:
    with pytest.raises(ValidationError):
        draft(basis="guess").validate()


def test_findings_need_evidence() -> None:
    with pytest.raises(ValidationError):
        draft(evidence=[]).validate()


def test_parse_finding_resolves_evidence_method() -> None:
    parsed = parse_finding(
        {
            "title": "x",
            "severity": "medium",
            "basis": "correlated_observation",
            "summary": "y",
            "evidence": [{"_id": "tc_01J9Z4M8YQ2V7C1W3N5B6D8FZZ", "ref_id": "S-000001"}],
        },
        {"tc_01J9Z4M8YQ2V7C1W3N5B6D8FZZ": "get_conversations"},
    )
    assert parsed.evidence[0].method == "get_conversations"
    assert parsed.evidence[0].ref_id == "S-000001"


def test_parse_finding_rejects_a_bad_severity() -> None:
    with pytest.raises(ValidationError):
        parse_finding(
            {
                "title": "x",
                "severity": "critical",
                "basis": "rule_match",
                "summary": "y",
                "evidence": [{"_id": "tc-1"}],
            },
            {},
        )


def test_step_budget_stops_the_loop() -> None:
    policy = AgentPolicy(AgentBudget(max_steps=2))
    assert policy.can_continue()
    policy.note_step()
    policy.note_step()
    assert not policy.can_continue()
    assert policy.state.stop_reason == "step_budget"


def test_tool_call_budget_stops_the_loop() -> None:
    policy = AgentPolicy(AgentBudget(max_tool_calls=2, max_same_tool_calls=9))
    assert policy.note_tool_call("get_capture_summary", {"task_id": "t"}, "r1")
    assert not policy.note_tool_call("get_conversations", {"task_id": "t"}, "r2")
    assert policy.state.stop_reason == "tool_call_budget"


def test_homogeneous_calls_stop_the_loop() -> None:
    policy = AgentPolicy(AgentBudget(max_tool_calls=20, max_same_tool_calls=3))
    for _ in range(2):
        assert policy.note_tool_call("check_alerts", {"task_id": "t"}, "same")
    assert not policy.note_tool_call("check_alerts", {"task_id": "t"}, "same")
    assert policy.state.stop_reason == "homogenisation"


def test_token_budget_stops_the_loop() -> None:
    policy = AgentPolicy(AgentBudget(max_tokens=100))
    policy.note_llm_call(tokens_in=80, tokens_out=30)
    assert not policy.can_continue()
    assert policy.state.stop_reason == "token_budget"


def test_cost_budget_stops_the_loop() -> None:
    policy = AgentPolicy(AgentBudget(max_cost_cents=5))
    policy.note_llm_call(cost_cents=6)
    assert not policy.can_continue()
    assert policy.state.stop_reason == "cost_budget"
