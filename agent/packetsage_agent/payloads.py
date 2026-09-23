"""Machine payloads shared by the CLI and the sidecar.

`run_json_payload` is the **frozen** `packetsage-agent run --json` object
(收口文档 §5.11). The desktop sidecar emits the very same object as its
`run_finished` event (《GUI 工程规格书 v0.2》§4.5), so the two paths can never
drift into "two parsers for one truth" (§2.2 / P6).

Both callers read it by field name; changes are additive only, and the shape is
locked by `tests/cli/gui_contract.py` (S42) on the CLI side and by
`tests/sidecar/protocol_cases.py` (S61) on the sidecar side.
"""

from __future__ import annotations

from typing import Any

SUMMARY_VERSION = 1


def run_status(result: Any) -> str:
    """`ok` → completed; anything else keeps its own name (§4)."""
    return "completed" if result.status == "ok" else str(result.status)


def finding_summaries(result: Any) -> list[dict[str, Any]]:
    """`id / severity / basis / title / evidence_ids` for the machine summary.

    Stored findings carry the engine-assigned `F-{n:03}` id; the accepted
    drafts are paired with them by position. A draft that was never stored
    keeps an empty id rather than inventing one.
    """
    stored = [row for row in (result.stored_findings or []) if isinstance(row, dict)]
    summaries: list[dict[str, Any]] = []
    for index, draft in enumerate(result.findings):
        row = stored[index] if index < len(stored) else {}
        summaries.append(
            {
                "id": str(row.get("finding_id") or ""),
                "severity": str(draft.severity),
                "basis": str(draft.basis),
                "title": str(draft.title),
                "evidence_ids": [str(ref._id) for ref in draft.evidence],
            }
        )
    for row in stored[len(result.findings) :]:
        evidence = row.get("evidence") or []
        summaries.append(
            {
                "id": str(row.get("finding_id") or ""),
                "severity": str(row.get("severity") or ""),
                "basis": str(row.get("basis") or ""),
                "title": str(row.get("title") or ""),
                "evidence_ids": [
                    str(item.get("_id"))
                    for item in evidence
                    if isinstance(item, dict) and item.get("_id")
                ],
            }
        )
    return summaries


def run_json_payload(result: Any, agent: Any) -> dict[str, Any]:
    """The frozen `run --json` object (收口文档 §5.11)."""
    state = agent.policy.state
    return {
        "summary_version": SUMMARY_VERSION,
        "run_id": str(result.agent_run_id),
        "task_id": str(result.task_id),
        "status": run_status(result),
        "stop_reason": result.stop_reason,
        "accepted": len(result.findings),
        "submit_rejects": int(result.submit_rejects),
        "malformed_output": bool(result.malformed_output),
        "steps": int(state.steps),
        "calls": int(state.llm_calls),
        "tool_calls": int(state.tool_calls),
        "tokens": {
            "in": int(result.tokens_in),
            "out": int(result.tokens_out),
            "total": int(result.tokens_in + result.tokens_out),
        },
        # 输入侧缓存（DeepSeek 上下文硬盘缓存）；provider 不报这两个数时都是 0。
        "cache": {
            "hit": int(getattr(state, "cache_hit_tokens", 0)),
            "miss": int(getattr(state, "cache_miss_tokens", 0)),
        },
        "cost_cents": int(result.cost_cents),
        "prompt_version": str(result.prompt_version),
        "model": str(result.model),
        "provider": str(getattr(agent, "provider_kind", "")),
        # 模型自己写的一段话（对话里的"它到底说了什么"）；模型没给就是空串。
        "summary": str(getattr(result, "summary", "") or ""),
        "findings": finding_summaries(result),
    }
