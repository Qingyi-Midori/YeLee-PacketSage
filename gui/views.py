"""界面渲染件（设计基线 §4）。

这里只做"把结构化数据画出来"，不取数、不改状态：每一件都能在
``tests/gui/app_cases.py`` 里用 ``streamlit.testing.v1.AppTest`` 直接断言。
本项目的四个一等公民（证据链、规则告警、预算常显、降级可见）与报告预览都在本模块，
故意不塞进设置面板（§4.2）。
"""

from __future__ import annotations

import json
from typing import Any

import streamlit as st
from packetsage_agent.progress import format_cost, format_tokens

UNVERIFIED_MARKER = "[unverified by engine]"

SEVERITY_ICON = {"high": "🔴", "medium": "🟠", "low": "🟡", "info": "⚪"}
SEVERITY_ORDER = {"high": 0, "medium": 1, "low": 2, "info": 3}

BASIS_LABEL = {
    "rule_match": "规则命中",
    "direct_observation": "直接观测",
    "correlated_observation": "交叉观测",
    "hypothesis": "待人工确认",
}

STATUS_LABEL = {
    "completed": "完成",
    "degraded": "降级收尾",
    "failed": "失败",
    "running": "进行中",
    "ok": "ok",
    "error": "错误",
    "invalid_args": "参数非法",
}

SUGGESTED_QUESTIONS = (
    "这份抓包里有扫描行为吗？",
    "哪些会话不完整？",
    "有没有可疑的 DNS 行为？",
    "端口 80 上都发生了什么？",
)


# --------------------------------------------------------------------------
# 小工具
# --------------------------------------------------------------------------
def mono(text: Any) -> str:
    """等宽引用：``tc_*`` / ``S-000001`` / 规则 id 里的连字符不能被 Markdown 吃掉。"""
    return f"`{text}`"


def severity_badge(severity: str) -> str:
    icon = SEVERITY_ICON.get(severity, "⚪")
    return f"{icon} {severity}"


def basis_label(basis: str) -> str:
    return BASIS_LABEL.get(basis, basis or "-")


def human_bytes(value: Any) -> str:
    try:
        size = float(value)
    except (TypeError, ValueError):
        return str(value)
    for unit in ("B", "KiB", "MiB", "GiB"):
        if size < 1024 or unit == "GiB":
            return f"{size:.0f} {unit}" if unit == "B" else f"{size:.1f} {unit}"
        size /= 1024
    return f"{size:.1f} GiB"


def record_to_dict(record: Any) -> dict[str, Any]:
    """``ToolCallRecord``（dataclass）→ 与实时 trace 同构的 dict。"""
    if isinstance(record, dict):
        return record
    return {
        "step": getattr(record, "step", 0),
        "tool_name": getattr(record, "tool_name", ""),
        "args_json": getattr(record, "args_json", "{}"),
        "result_summary": getattr(record, "result_summary", ""),
        "status": getattr(record, "status", ""),
        "duration_ms": getattr(record, "duration_ms", 0),
        "tc_id": getattr(record, "tc_id", None),
    }


def trace_args(entry: dict[str, Any]) -> str:
    """trace 条目里的参数：实时条目是对象，落库条目是 JSON 文本。"""
    args = entry.get("args")
    if args is None:
        raw = entry.get("args_json")
        if isinstance(raw, str):
            try:
                args = json.loads(raw)
            except ValueError:
                return raw
        else:
            args = raw
    try:
        return json.dumps(args, ensure_ascii=False, indent=2)
    except (TypeError, ValueError):
        return str(args)


# --------------------------------------------------------------------------
# 工具调用卡片（§4.1 中央流 / S58）
# --------------------------------------------------------------------------
def tool_card(entry: dict[str, Any], index: int, *, expanded: bool = False) -> None:
    """一次工具调用一张卡：工具名、参数、耗时、结果摘要、状态、tc_id。"""
    status = str(entry.get("status") or "ok")
    label = (
        f"#{index} {entry.get('tool_name', '?')} · {STATUS_LABEL.get(status, status)} · "
        f"{entry.get('duration_ms', 0)} ms"
    )
    with st.expander(label, expanded=expanded):
        left, right = st.columns([3, 2])
        with left:
            st.markdown("**参数**")
            st.code(trace_args(entry), language="json")
        with right:
            st.markdown("**证据锚点**")
            tc_id = entry.get("tc_id")
            if tc_id:
                st.markdown(mono(tc_id))
            else:
                st.caption("没有落到 tc 台账（多半是参数非法，引擎没被调用）")
        st.markdown("**结果摘要**")
        st.code(str(entry.get("result_summary") or "(空)"), language="text")


def tool_cards(entries: list[dict[str, Any]], *, limit: int = 40) -> None:
    total = len(entries)
    for index, entry in enumerate(entries[:limit], start=1):
        tool_card(record_to_dict(entry), index)
    if total > limit:
        st.caption(f"（只展开前 {limit} 条，共 {total} 条工具调用）")


# --------------------------------------------------------------------------
# 预算常显（§4.2 / M6）
# --------------------------------------------------------------------------
def budget_bar(state: dict[str, Any], budget: Any, *, live: bool = False) -> None:
    """``steps 3/12 · calls 5/24 · tokens · cost`` —— 数据是 ``PolicyState`` + ``AgentBudget``。"""
    if budget is None:
        return
    steps = int(state.get("steps", 0))
    calls = int(state.get("llm_calls", 0))
    tools = int(state.get("tool_calls", 0))
    tokens = int(state.get("tokens_in", 0)) + int(state.get("tokens_out", 0))
    cost = int(state.get("cost_cents", 0))
    columns = st.columns(5)
    columns[0].metric("步骤", f"{steps}/{budget.max_steps}")
    columns[1].metric("LLM 轮", f"{calls}/{budget.max_llm_calls}")
    columns[2].metric("工具调用", f"{tools}/{budget.max_tool_calls}")
    columns[3].metric("tokens", format_tokens(tokens), f"上限 {format_tokens(budget.max_tokens)}")
    columns[4].metric(
        "成本", format_cost(cost, budget.max_cost_cents), f"上限 {budget.max_cost_cents / 10:.0f}¢"
    )
    ratio = min(1.0, steps / budget.max_steps) if budget.max_steps else 0.0
    st.progress(ratio, text=("调查进行中…" if live else "本次 run 的预算占用"))


def capture_metrics(summary: dict[str, Any]) -> None:
    """任务摘要：包数 / 会话 / 告警 / 解码错误（M4 的"612 包、570 会话"）。"""
    if not summary:
        return
    columns = st.columns(4)
    columns[0].metric("packets", summary.get("packets", "-"))
    columns[1].metric("sessions", summary.get("sessions", "-"))
    columns[2].metric("alerts", summary.get("alerts", "-"))
    columns[3].metric("decode errors", summary.get("decode_errors", "-"))
    details = {
        "file": summary.get("source_path"),
        "format": summary.get("format"),
        "bytes": human_bytes(summary.get("bytes")) if summary.get("bytes") else "-",
        "duration_s": summary.get("duration_s"),
        "truncated_packets": summary.get("truncated_packets"),
        "incomplete_sessions": summary.get("incomplete_sessions"),
        "dropped_sessions": summary.get("dropped_sessions"),
        "sha256": summary.get("source_sha256"),
    }
    shown = {key: value for key, value in details.items() if value not in (None, "")}
    if shown:
        with st.expander("任务细节", expanded=False):
            for key, value in shown.items():
                st.markdown(f"- **{key}**: {mono(value)}")


# --------------------------------------------------------------------------
# 降级可见（§4.2 / S63）
# --------------------------------------------------------------------------
def degradation_notice(
    *,
    status: str,
    stop_reason: str | None = None,
    notes: list[str] | None = None,
    malformed: bool = False,
) -> None:
    """``degraded`` / ``stop_reason`` / 被拒 finding 必须在界面上显式标黄。"""
    reasons: list[str] = []
    if stop_reason:
        reasons.append(f"stop_reason = {mono(stop_reason)}")
    if malformed:
        reasons.append("模型两次产出非法 payload（malformed_output=true）")
    body = f"本次 run 以 {mono(status)} 收尾。" + (" " + " · ".join(reasons) if reasons else "")
    st.warning(body)
    for note in notes or []:
        st.warning(f"WARN {note}")


def marker_notice(text: str) -> None:
    """正文里出现 ``[unverified by engine]`` 或 ``sample_packets=[]`` 就标黄。"""
    hits: list[str] = []
    occurrences = text.count(UNVERIFIED_MARKER)
    if occurrences:
        hits.append(f"{UNVERIFIED_MARKER} ×{occurrences}")
    if "UNVERIFIED CONTENT" in text:
        hits.append("报告表头声明 UNVERIFIED CONTENT")
    if hits:
        st.warning("降级可见性：" + " · ".join(hits))


# --------------------------------------------------------------------------
# 规则告警（§4.2 / M10）
# --------------------------------------------------------------------------
def alerts_panel(alerts: list[dict[str, Any]]) -> None:
    """规则 id / 版本 / ``rule_content_hash`` / 窗口证据 / 样例包。"""
    if not alerts:
        st.info("规则引擎在本次 capture 上没有命中（或该任务尚未分析）。")
        return
    rows = []
    for alert in sorted(
        alerts, key=lambda item: (SEVERITY_ORDER.get(str(item.get("severity")), 9), item.get("rule_id"))
    ):
        evidence = alert.get("evidence") or {}
        rows.append(
            {
                "rule_id": alert.get("rule_id"),
                "severity": alert.get("severity"),
                "ver": alert.get("rule_version"),
                "hash": alert.get("rule_content_hash"),
                "packets": f"{alert.get('first_packet')}-{alert.get('last_packet')}",
                "metric": evidence.get("metric"),
                "value": evidence.get("value"),
                "threshold": evidence.get("threshold"),
                "算子": evidence.get("operator"),
                "窗口(ns)": evidence.get("window_ns"),
                "样例包": ", ".join(str(index) for index in evidence.get("sample_packets") or []) or "—",
                "degraded": bool(evidence.get("degraded")),
            }
        )
    st.dataframe(rows, width="stretch", hide_index=True)
    degraded = [row for row in rows if row["degraded"]]
    if degraded:
        st.warning(
            f"{len(degraded)} 条告警的窗口证据被预算裁剪（evidence.degraded=true）——"
            "样例包是采样而非全量。"
        )
    empty_samples = [row for row in rows if row["样例包"] == "—"]
    if empty_samples:
        st.warning(
            f"{len(empty_samples)} 条告警的 `sample_packets` 为空：窗口里没有可回溯的样例包。"
        )
    with st.expander("告警原文（分组的五元组 + 窗口证据）"):
        for alert in alerts:
            st.markdown(
                f"**{alert.get('rule_id')}** {severity_badge(str(alert.get('severity')))} "
                f"· {mono(alert.get('alert_id'))} · 包 {alert.get('first_packet')}-{alert.get('last_packet')}"
            )
            group = alert.get("group_key") or []
            if group:
                st.markdown(
                    "- 分组："
                    + " · ".join(f"{pairs[0]}={pairs[1]}" for pairs in group if len(pairs) == 2)
                )
            if alert.get("session_id"):
                st.markdown(f"- session: {mono(alert['session_id'])}")
            st.code(json.dumps(alert.get("evidence"), ensure_ascii=False, indent=2), language="json")
            st.divider()


# --------------------------------------------------------------------------
# 证据链（§4.2 一等公民 / M8、M9）
# --------------------------------------------------------------------------
def evidence_panel(findings: list[dict[str, Any]], trace_rows: list[dict[str, Any]]) -> None:
    """结论 → 证据锚点 ``tc_*`` → 具体工具调用（四段对拍且可点开）。"""
    if not findings:
        st.info("还没有 accepted 结论。先跑一次调查（run），或看引擎视角的规则告警。")
        return
    trace_index = {str(row.get("_id")): row for row in trace_rows if row.get("_id")}
    for finding in findings:
        title = f"{finding.get('finding_id', '?')} · {finding.get('title', '')}"
        with st.expander(title, expanded=True):
            st.markdown(
                f"{severity_badge(str(finding.get('severity')))} · "
                f"{basis_label(str(finding.get('basis')))} · 校验 {mono(finding.get('validator_status'))}"
            )
            st.markdown(str(finding.get("summary") or ""))
            references = finding.get("evidence") or []
            if not references:
                st.warning("这条结论没有证据锚点（V1 会拒绝，正常不该出现在这里）。")
                continue
            st.markdown("**证据锚点 → 工具调用**")
            for reference in references:
                tc_id = str(reference.get("_id"))
                entry = trace_index.get(tc_id, {})
                st.markdown(
                    f"- {mono(tc_id)} · `{reference.get('method')}`"
                    + (f" · ref `{reference.get('ref_id')}`" if reference.get("ref_id") else "")
                    + (
                        f" · {entry.get('duration_ms', '-')} ms · {mono(entry.get('status', '-'))}"
                        if entry
                        else " · （tc 台账里没有这一条：跨进程恢复了任务，先在 CLI 里跑一次同名工具）"
                    )
                )
                if entry:
                    st.code(
                        json.dumps(
                            {"args": entry.get("args"), "numbers": entry.get("numbers")},
                            ensure_ascii=False,
                            indent=2,
                        ),
                        language="json",
                    )


def evidence_anchor_picker(trace_rows: list[dict[str, Any]]) -> None:
    """按 tc_id 直接翻台账（M9 的"点开某个证据锚点"）。"""
    if not trace_rows:
        return
    labels = [
        f"{row.get('_id')} · {row.get('method')} · {row.get('duration_ms')} ms"
        for row in trace_rows
    ]
    choice = st.selectbox(
        "按 tc_id 查台账",
        options=list(range(len(trace_rows))),
        format_func=lambda index: labels[int(index)],
    )
    row = trace_rows[int(choice)]
    st.markdown(
        f"{mono(row.get('_id'))} · `{row.get('method')}` · {row.get('duration_ms')} ms · "
        f"{mono(row.get('status'))} · {mono(row.get('ts_unix_ns'))}"
    )
    st.code(str(row.get("args") or "{}"), language="json")
    numbers = row.get("numbers")
    if numbers:
        st.code(json.dumps(numbers, ensure_ascii=False, indent=2), language="json")


# --------------------------------------------------------------------------
# 报告（§4.2 / M12、S63）
# --------------------------------------------------------------------------
def report_checklist(text: str) -> None:
    """九节标题、降级标记、表头元信息 —— 报告能不能交出去，一眼看完。"""
    sections = [
        line.strip() for line in text.splitlines() if line.startswith("## ") and "."
    ]
    expected = 9
    if len(sections) >= expected:
        st.success(f"九节齐全（{len(sections)} 节）：" + " / ".join(s.split(". ", 1)[-1] for s in sections[:9]))
    else:
        st.error(f"报告只有 {len(sections)} 节，少于规范要求的 {expected} 节。")
    marker_notice(text)


def report_panel(report: dict[str, Any] | None) -> None:
    """报告预览 + 下载；``report`` 来自 ``ReportGenerator.generate``。"""
    if not report:
        st.info("还没有报告。跑完一次调查后点「生成报告」，或等它自动生成。")
        return
    text = str(report.get("text") or "")
    meta = report.get("meta") or {}
    columns = st.columns(4)
    columns[0].metric("模板", meta.get("template_version", "-"))
    columns[1].metric("提示词", meta.get("prompt_version", "-"))
    columns[2].metric("模型", meta.get("model", "-"))
    columns[3].metric(
        "usage",
        f"{format_tokens(int(meta.get('tokens_in', 0)) + int(meta.get('tokens_out', 0)))} tok",
        f"{int(meta.get('cost_cents', 0)) / 10:.1f}¢",
    )
    if meta.get("degraded"):
        st.warning(
            f"报告以 degraded 收尾：{meta.get('unverified_count', 0)} 处正文被替换为 "
            f"{UNVERIFIED_MARKER}。"
        )
    st.download_button(
        "⬇ 下载报告（Markdown）",
        data=text.encode("utf-8"),
        file_name=str(meta.get("file_name") or "packetsage-report.md"),
        mime="text/markdown",
    )
    st.caption(f"落盘路径：{mono(meta.get('path', '-'))} · sha256 {mono(meta.get('sha256', '-'))}")
    report_checklist(text)
    st.markdown(text)


# --------------------------------------------------------------------------
# 首屏：自检、空态、建议问题
# --------------------------------------------------------------------------
def doctor_panel(report: dict[str, Any]) -> None:
    """十项自检的紧凑视图；失败项直接把 ``repair_hint`` 摊开（§5.1）。"""
    items = report.get("items") or []
    if report.get("error"):
        st.error(f"自检没跑成：{report['error']}")
        if report.get("repair"):
            st.code(report["repair"], language="text")
        return
    if not items:
        st.warning("自检没有返回任何条目。")
        return
    for item in items:
        status = str(item.get("status"))
        icon = {"ok": "✅", "warn": "⚠", "fail": "❌", "skipped": "⏭"}.get(status, "•")
        st.markdown(f"{icon} **{item.get('name', item.get('id'))}** — {item.get('detail', '')}")
        if status in {"warn", "fail"} and item.get("repair_hint"):
            st.code(str(item["repair_hint"]), language="text")


def empty_state(*, has_task: bool = False) -> None:
    """空态：告诉用户下一步点哪里，并给几个建议问题（§4.1）。"""
    if has_task:
        st.info(
            "**任务已就绪，还没开始调查。** 左侧点「▶ 开始调查（run）」，"
            "或在下面的输入框里直接问一个问题（chat 模式）。"
            "调查过程中工具调用会一条条出现，最后给出可追溯的结论。"
        )
    else:
        st.info(
            "**还没有选中的任务。** 左侧选一份抓包（或上传自己的）→ 点「开始分析」→ "
            "再点「开始调查」。调查过程中工具调用会一条条出现，最后给出可追溯的结论。"
        )
    st.markdown("可以先问的问题：")
    for question in SUGGESTED_QUESTIONS:
        st.markdown(f"- {question}")


def trace_table(trace_rows: list[dict[str, Any]]) -> None:
    """§9 分析轨迹：工具 + 参数 + 耗时 + 状态（不含思维过程，ADR-007）。"""
    if not trace_rows:
        st.caption("台账还是空的。")
        return
    rows = []
    for row in trace_rows:
        args = str(row.get("args") or "{}")
        rows.append(
            {
                "tc_id": row.get("_id"),
                "method": row.get("method"),
                "args": args[:120] + ("…" if len(args) > 120 else ""),
                "duration_ms": row.get("duration_ms"),
                "status": row.get("status"),
                "ts_unix_ns": row.get("ts_unix_ns"),
            }
        )
    st.dataframe(rows, width="stretch", hide_index=True)
