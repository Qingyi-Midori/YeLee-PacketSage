"""预取"确定性事实"，让模型第一步就开始分析（性能改造 #2 / #4）。

为什么值得做：一次 run 的墙钟时间几乎全在**串行的 LLM 往返**上，而典型流程的头
几步是"探索"——`get_capture_summary` / `get_protocol_stats` / `check_alerts` /
`get_conversations`。这些数据引擎侧早就有，而且是确定性的（不依赖模型判断）。
把它们在**第一次 LLM 调用之前**取回来、放进第一条消息，模型就从"分析"起步，
典型流程从 8–9 步降到 3–4 步。

三条纪律：

* 预取用的是**同一批工具**（`tools.call_tool`），所以每一步都真在引擎台账里留下
  `tc_*` 锚点——模型引用它们不算幻觉（反幻觉 lint 照旧）；
* 预取**不算模型步数**（`state.steps` / `llm_calls` 不变），但算工具调用，
  并在摘要里单列 `preloaded`，免得"省了步数"变成"偷偷多干活"；
* 事实块有上限（`:data:`FACTS_MAX_CHARS``），超了就按优先级截断：
  摘要 → 告警 → 协议统计 → 会话。

顺带满足缓存友好的那条要求（#4）：事实块在一次 run 内**逐字节稳定**，
所以它落在 prompt 前缀里，能被 provider 的上下文缓存命中。
"""

from __future__ import annotations

import json
from typing import Any

from .models import ValidationError
from .tools import call_tool

#: 事实块的上限；超了按优先级截断（不能让"预取"把上下文吃光）。
FACTS_MAX_CHARS = 6_000

#: 预取哪些（顺序 = 重要性 = 截断优先级）。
PRELOAD_PLAN: tuple[tuple[str, dict[str, Any]], ...] = (
    ("get_capture_summary", {}),
    ("check_alerts", {}),
    ("get_protocol_stats", {"layer": "transport"}),
    ("get_conversations", {"sort_by": "bytes", "limit": 8}),
)


def facts_text(
    task_id: str,
    entries: list[tuple[str, dict[str, Any]]],
    anchors: dict[str, str] | None = None,
) -> str:
    """把预取结果拼成模型读的那一段（含 `tc_*` 锚点，逐字节稳定）。"""
    blocks: list[str] = [
        "Verified capture facts (already fetched from the engine before this round):",
        f"- task_id: {task_id}",
    ]
    for method, payload in entries:
        body = json.dumps(payload, ensure_ascii=False, sort_keys=True)
        # 锚点必须写进事实块：模型只能引用**出现在它上下文里**的 tc id，
        # 否则反幻觉 lint 会（正确地）把结论判成伪造引用。
        tc = (anchors or {}).get(method)
        blocks.append(f"- {method}{f' (tc_id={tc})' if tc else ''}: {body}")
    blocks.append(
        "These are engine facts, not guesses. Do NOT fetch them again — start from "
        "them, pivot only where you need packet-level detail, then finalize. Every "
        "_id above is a valid evidence anchor."
    )
    text = "\n".join(blocks)
    if len(text) <= FACTS_MAX_CHARS:
        return text
    kept: list[str] = []
    used = len(blocks[0]) + len(blocks[-1]) + 64
    for line in blocks[1:-1]:
        if used + len(line) > FACTS_MAX_CHARS:
            kept.append(f"- (truncated: {len(blocks) - len(kept) - 2} more fact blocks)")
            break
        kept.append(line)
        used += len(line) + 1
    return "\n".join([blocks[0], *kept, blocks[-1]])


def preload_facts(
    client: Any,
    task_id: str,
    plan: tuple[tuple[str, dict[str, Any]], ...] = PRELOAD_PLAN,
) -> tuple[str, list[tuple[dict[str, Any], Any]]]:
    """Runs the plan against the engine.

    Returns ``(facts_text, [(trace_entry, CallResult), ...])``——把 `CallResult`
    一起交回去，调用方才能像普通工具结果那样抽证据（`_collect_evidence`），
    于是后续 pivot 的依据和探索式起步时完全一样。

    A failing pre-fetch is never fatal: the loop simply starts without that fact
    (and the model can still ask for it the old way).
    """
    entries: list[tuple[str, dict[str, Any]]] = []
    trace: list[tuple[dict[str, Any], Any]] = []
    anchors: dict[str, str] = {}
    for method, args in plan:
        # `task_id` 由调用方补齐（模型平时也是这么给参数的）。
        call_args = {"task_id": task_id, **args}
        try:
            call = call_tool(client, method, call_args)
        except ValidationError:
            continue
        if not call.ok or call.content is None:
            continue
        if call.tc_id:
            anchors[method] = str(call.tc_id)
        payload: Any = call.content
        if method == "check_alerts":
            alerts = payload.get("alerts") if isinstance(payload, dict) else None
            # 没有告警也要如实说：这是"确定性事实"，不是"没查"。
            payload = {"alerts": alerts or [], "count": len(alerts or [])}
            if not alerts:
                entries.append((method, payload))
                trace.append((_trace_entry(method, call, "no alerts"), call))
                continue
        entries.append((method, payload))
        trace.append((_trace_entry(method, call, None), call))
    return facts_text(task_id, entries, anchors), trace


def _trace_entry(method: str, call: Any, note: str | None) -> dict[str, Any]:
    summary = note or json.dumps(call.content, ensure_ascii=False)[:200]
    return {
        "tool_name": method,
        "args": dict(call.args),
        "result_summary": summary,
        "status": "preload",
        "duration_ms": call.duration_ms,
        "tc_id": call.tc_id,
    }
