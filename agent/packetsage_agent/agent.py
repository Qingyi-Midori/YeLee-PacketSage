"""The evidence-first agent loop (M3~M6 §4.1/§4.4).

    observe -> hypothesise -> call tool -> collect evidence -> repeat -> finalize

Chain of thought is never stored (ADR-007): the trace keeps tool, args,
result summary, duration and status only.

The CLI contract of the Agent CLI 工程规格书 (§4/§5) needs two observers that
*do not* change the loop semantics: an optional ``observer`` (progress lines,
stderr only) and :meth:`PacketSageAgent.request_finalize` (first Ctrl-C: stop
after the current step and keep the findings already submitted).

The desktop sidecar (《GUI 工程规格书 v0.2》§4.5) needs a richer stream, so the
same observer gets five *additional* hooks that are purely additive — the CLI's
``progress.Progress`` implements only ``tool_call``/``llm_round`` and keeps
behaving exactly as before:

* ``tool_call_started(step, tool, args)`` — "正在调用 X"（主流 Agent 的观感来源）;
* ``tool_call_finished(ToolCallRecord)`` — 工具卡片定稿（字段 = 台账行）;
* ``finding_accepted({id, severity, basis, title, evidence_ids})``;
* ``finding_rejected(title, reason_code, reason)`` — V1–V4 的拒绝原因不再只进 stderr。
* ``llm_delta(step, channel, text, tool_name, tool_index)`` — token 级流式（§4.5）：
  只有实现了这个钩子的 observer 才会让 provider 走 SSE，CLI 因此保持单次 POST。
"""

from __future__ import annotations

import json
import time
import uuid
from dataclasses import dataclass, field
from typing import Any

from .engine_client import CallResult, EngineError, RpcCaller
from .models import FindingDraft, ToolCallRecord, ValidationError, parse_finding
from .policy import AgentBudget, AgentPolicy
from .preload import preload_facts
from .prompts import DEFAULT_PROMPT_VERSION, PROMPT_VERSION
from .provider import Decision, MockProvider, build_provider
from .tools import call_tool, wrap_result

#: Consecutive invalid-argument rounds tolerated before the run finalizes.
MAX_INVALID_ARG_ROUNDS = 3


@dataclass
class AgentRunResult:
    """Outcome of one agent run."""

    agent_run_id: str
    task_id: str
    status: str
    findings: list[FindingDraft] = field(default_factory=list)
    trace: list[ToolCallRecord] = field(default_factory=list)
    stop_reason: str | None = None
    malformed_output: bool = False
    tokens_in: int = 0
    tokens_out: int = 0
    cost_cents: int = 0
    model: str = "mock"
    prompt_version: str = PROMPT_VERSION
    notes: list[str] = field(default_factory=list)
    #: Findings the engine refused (V1-V4 rejection, submit rejection, unknown
    #: tc id): the ``submit_rejects`` counter of the run summary (§4).
    submit_rejects: int = 0
    #: Findings the engine actually stored, with the `F-{n:03}` ids it assigned.
    stored_findings: list[dict[str, Any]] = field(default_factory=list)
    #: 模型收尾时自己写的那段话（对话里的"它说了什么"）。**不作证据**，只是回答。
    summary: str = ""


class PacketSageAgent:
    """Runs the tool-calling loop against one analysed task."""

    def __init__(
        self,
        client: RpcCaller,
        provider: Any | None = None,
        budget: AgentBudget | None = None,
        model_name: str = "mock",
        provider_kind: str = "mock",
        observer: Any | None = None,
        mode: str = "run",
        prompt_version: str | None = None,
        preload: bool = True,
    ) -> None:
        self.client = client
        self.provider = provider or MockProvider()
        self.policy = AgentPolicy(budget)
        self.model_name = model_name
        self.provider_kind = provider_kind
        #: `run`（汇报结论）或 `chat`（回答追问）：决定收尾要求，也决定"没答上"时要不要兜底。
        self.mode = mode
        #: Progress sink (`progress.Progress`); it only ever writes to stderr.
        self.observer = observer
        #: Set by the first Ctrl-C of `run` (§4): finalize, keep partial results.
        self.interrupted = False
        #: Prompt version that produced this run (lands in `agent_runs`).
        self.prompt_version = prompt_version or DEFAULT_PROMPT_VERSION
        self.agent_run_id = f"run_{uuid.uuid4()}"
        #: Last finding-payload validation error, surfaced in the run notes.
        self.last_finding_error: str | None = None
        #: 预取确定性的抓包事实（性能改造 #2）；`False` 时回到纯探索式起步。
        self.preload = preload
        #: 上一次 LLM 往返的毫秒数（写进它触发的那条台账行）。
        self._last_llm_ms = 0
        #: 连续非法参数的次数（跨多工具的一轮也要计数）。
        self._invalid_args = 0

    def request_finalize(self) -> None:
        """Asks the loop to stop at the next boundary and keep what it has."""
        self.interrupted = True

    def _delta_hook(self) -> Any | None:
        """token 级流式的回调；observer 没有 `llm_delta` 钩子时返回 `None`。

        `None` 时 provider 走原来那条单次 POST——CLI 的字节级行为不变；只有桌面
        外壳（`EventSink`）实现了这个钩子，才开 SSE（§4.5）。
        """
        observer = self.observer
        if observer is None or getattr(observer, "llm_delta", None) is None:
            return None

        def emit(
            channel: str, text: str, tool_name: str | None, tool_index: int | None
        ) -> None:
            self._notify(
                "llm_delta", self.policy.state.steps, channel, text, tool_name, tool_index
            )

        return emit

    def _notify(self, hook: str, *args: Any) -> None:
        """Calls one observer hook; progress must never break a run."""
        observer = self.observer
        if observer is None:
            return
        callback = getattr(observer, hook, None)
        if callback is None:
            return
        try:
            callback(*args)
        except Exception:  # pragma: no cover - a broken sink is not a run failure
            self.observer = None

    # ------------------------------------------------------------ helpers
    def _record(
        self,
        call: CallResult,
        rendered: str,
    ) -> ToolCallRecord:
        summary = rendered.splitlines()[-1][:400] if rendered else ""
        return ToolCallRecord(
            step=len(self.policy.state.traces) + 1,
            tool_name=call.method,
            args_json=json.dumps(call.args, ensure_ascii=False),
            result_summary=summary,
            status="ok" if call.ok else "error",
            duration_ms=call.duration_ms,
            tc_id=call.tc_id,
        )

    def _collect_evidence(self, call: CallResult, evidence: dict[str, Any]) -> None:
        """Extracts the anchors a mock/final model needs from a tool result."""
        content = call.content
        if not isinstance(content, dict):
            return
        if call.method == "get_capture_summary":
            evidence["summary_tc"] = call.tc_id
            evidence["summary"] = content
        if call.method == "check_alerts":
            alerts = content.get("alerts") or []
            if alerts:
                evidence["alerts"] = alerts
                evidence["alert_tc"] = call.tc_id
        if call.method == "get_conversations":
            sessions = content.get("conversations") or []
            if sessions:
                evidence["sessions"] = sessions
                evidence["sessions_tc"] = call.tc_id
        if call.method == "filter_packets":
            indices = content.get("packet_indices") or []
            if indices:
                evidence["packet_indices"] = indices
                evidence["filter_tc"] = call.tc_id
        if call.method == "inspect_packets":
            evidence["inspected_tc"] = call.tc_id
        if call.method == "reconstruct_stream":
            evidence["stream_tc"] = call.tc_id

    def _finalize(self, decision: Decision, evidence: dict[str, Any]) -> tuple[list[FindingDraft], bool]:
        """Validates model findings; malformed payloads trigger one retry."""
        lookup: dict[str, str] = {}
        for key in (
            "summary_tc",
            "alert_tc",
            "sessions_tc",
            "filter_tc",
            "inspected_tc",
            "stream_tc",
        ):
            if evidence.get(key):
                lookup[str(evidence[key])] = key.replace("_tc", "")
        drafts: list[FindingDraft] = []
        malformed = False
        self.last_finding_error = None
        for payload in decision.findings or []:
            try:
                drafts.append(parse_finding(payload, lookup))
            except (ValidationError, KeyError, TypeError, ValueError) as exc:
                malformed = True
                self.last_finding_error = str(exc)
        return drafts, malformed

    # ---------------------------------------------------------------- run
    def run(self, task_id: str, user_goal: str = "分析该捕获并给出可追溯结论") -> AgentRunResult:
        """Executes the loop until the provider finalizes or a budget is hit."""
        result = AgentRunResult(
            agent_run_id=self.agent_run_id,
            task_id=task_id,
            status="ok",
            model=self.model_name,
            prompt_version=self.prompt_version,
        )
        evidence: dict[str, Any] = {}
        trace_payload: list[dict[str, Any]] = []
        retries = 0
        self._invalid_args = 0
        # 性能改造 #2：先把确定性的抓包事实取回来（同一批工具、同一本台账），
        # 让模型从"分析"起步而不是"探索"。预取不算模型步数，但如实计入工具。
        facts = ""
        if self.preload:
            facts, preloaded = preload_facts(self.client, task_id)
            for entry, call in preloaded:
                self._notify(
                    "tool_call_started",
                    len(self.policy.state.traces) + 1,
                    entry["tool_name"],
                    entry["args"],
                )
                self.policy.state.traces.append(entry)
                trace_payload.append(entry)
                # 和普通工具结果一样抽证据：后续 pivot 的依据不会因为"预取"而变弱。
                self._collect_evidence(call, evidence)
                self.policy.note_preloaded()
                self.policy.note_tool_time(entry.get("duration_ms") or 0)
                # 不占 `tool_calls` / 预算：那是**模型**的动作计数，预取的 4 次
                # 由 `preloaded` 单列（界面上写成"N 次工具调用（含 M 次预取）"），
                # 台账里当然一行都不少——审计看引擎，不看计数器。
                # CLI 的进度行也照实播（预取是真发生的调用，不是界面效果）。
                self._notify("tool_call", len(result.trace) + 1, entry["tool_name"], call.ok)
                self._notify(
                    "tool_call_finished",
                    ToolCallRecord(
                        step=len(self.policy.state.traces),
                        tool_name=entry["tool_name"],
                        args_json=json.dumps(entry["args"], ensure_ascii=False),
                        result_summary=entry["result_summary"],
                        status="preload",
                        duration_ms=int(entry.get("duration_ms") or 0),
                        tc_id=entry.get("tc_id"),
                    ),
                )
        while self.policy.can_continue():
            if self.interrupted and result.trace:
                # First Ctrl-C (§4): finalize here; already submitted findings
                # are never rolled back.
                result.status = "degraded"
                result.stop_reason = "interrupted"
                result.notes.append("run interrupted, partial results kept")
                break
            self.policy.note_step()
            llm_started = time.perf_counter()
            decision = self.provider.decide(
                task_id,
                trace_payload,
                evidence,
                user_goal,
                facts,
                on_delta=self._delta_hook(),
            )
            llm_ms = int((time.perf_counter() - llm_started) * 1000)
            # "先测再改"：LLM 往返与工具耗时分开记账，否则会把慢数据库当成慢模型。
            self.policy.note_llm_time(llm_ms)
            self._last_llm_ms = llm_ms
            # 一次记账：tokens + 缓存命中情况（DeepSeek 上下文硬盘缓存，
            # 别的 provider 不给就是 0——界面要能分辨"没数据"和"没命中"）。
            self.policy.note_llm_call(
                decision.tokens_in,
                decision.tokens_out,
                cache_hit_tokens=int(getattr(decision, "cache_hit_tokens", 0) or 0),
                cache_miss_tokens=int(getattr(decision, "cache_miss_tokens", 0) or 0),
            )
            if decision.usage_missing:
                # 流式端点没给 usage：不编数字，只把这件事记在 run 的 notes 里
                # （provider 已经把流式关掉，后面几轮回到精确计数）。
                result.notes.append("provider 未返回 usage：该轮 tokens 记 0")
            self._notify("llm_round", self.policy.state, self.policy.budget)
            if decision.malformed or (
                decision.kind == "tool" and decision.args is None
            ):
                retries += 1
                self.policy.state.malformed_retries = retries
                if retries > 1:
                    result.status = "degraded"
                    result.malformed_output = True
                    result.notes.append("model produced malformed output twice")
                    break
                continue
            if decision.kind == "final":
                result.summary = (decision.summary or "").strip()
                drafts, malformed = self._finalize(decision, evidence)
                if malformed and retries == 0:
                    # One retry with the same evidence, exactly like the
                    # "illegal JSON" path (M3~M6 §4.4).
                    retries += 1
                    result.notes.append(
                        "retried after an invalid finding payload: "
                        f"{self.last_finding_error or 'schema violation'}"
                    )
                    continue
                result.findings = drafts
                if malformed:
                    result.status = "degraded"
                    result.malformed_output = True
                    result.notes.append(
                        "model produced an invalid finding payload twice: "
                        f"{self.last_finding_error or 'schema violation'}"
                    )
                break
            if decision.kind == "give_up":
                result.status = "degraded"
                result.notes.append(decision.text or "model gave up")
                break
            # 一轮里模型可以请求多个工具（并行 function calling，性能改造 #3）：
            # 依次执行、逐个出卡片。省下的是 LLM 往返，工具本身仍是串行的
            # （引擎是单写者，ADR-018）。
            stop_here: str | None = None
            for tool, tool_args in decision.iter_calls():
                stop_here = self._run_tool(
                    tool,
                    tool_args,
                    result,
                    evidence,
                    trace_payload,
                    reasoning=decision.reasoning,
                )
                if stop_here:
                    break
            if stop_here:
                result.stop_reason = stop_here
                break
        else:
            result.stop_reason = self.policy.state.stop_reason or "budget"
        result.stop_reason = result.stop_reason or self.policy.state.stop_reason
        result.tokens_in = self.policy.state.tokens_in
        result.tokens_out = self.policy.state.tokens_out
        result.cost_cents = self.policy.state.cost_cents
        # 追问必须有答案：run 若被提前收尾（重复调用 / 预算），模型可能没机会写回答，
        # 这里补一次不带工具的小请求（失败就算了，界面会照实说"没答上"）。
        if self.mode == "chat" and not result.summary:
            fallback = getattr(self.provider, "answer", None)
            if callable(fallback):
                try:
                    result.summary = (fallback(task_id, user_goal, trace_payload) or "").strip()
                except Exception:  # noqa: BLE001 - 兜底不许反过来弄挂 run
                    result.summary = ""
        if not result.trace:
            result.status = "failed"
        self._submit(result)
        if result.status == "ok" and result.malformed_output:
            result.status = "degraded"
        return result

    def _run_tool(
        self,
        tool: str,
        tool_args: dict[str, Any],
        result: AgentRunResult,
        evidence: dict[str, Any],
        trace_payload: list[dict[str, Any]],
        reasoning: str | None = None,
    ) -> str | None:
        """Runs one tool call and records it; returns a stop reason or `None`.

        从 `run()` 里抽出来，是为了让"一轮多工具"能复用同一条路径：
        卡片、台账、证据、预算、去重规则都只有一份。

        `reasoning` 是这一轮模型的思维链（DeepSeek `reasoning_content`）：官方文档
        要求带 `tools` 时**在后续请求里回传**，所以它挂在这一轮的 `trace_payload`
        条目上——**只活在内存里**，不进 `result.trace`（台账里没有 CoT，ADR-007）。
        """
        try:
            # "正在调用 X" 先播出去（§4.5 tool_call_started），再真的打引擎：
            # 慢工具期间界面不会看起来没反应。
            self._notify("tool_call_started", len(self.policy.state.traces) + 1, tool, tool_args)
            call = call_tool(self.client, tool, tool_args)
        except ValidationError as exc:
            # Illegal tool arguments never reach the engine: the policy feeds the
            # error back to the model (M3~M6 §4.4) and stops the run if the model
            # keeps producing them.
            result.notes.append(f"invalid arguments for {tool}: {exc}")
            rejected_record = ToolCallRecord(
                step=len(self.policy.state.traces) + 1,
                tool_name=tool,
                args_json=json.dumps(tool_args, ensure_ascii=False),
                result_summary=f"invalid arguments: {exc}",
                status="invalid_args",
                duration_ms=0,
                tc_id=None,
                llm_ms=self._last_llm_ms,
            )
            result.trace.append(rejected_record)
            self._notify("tool_call", len(result.trace), tool, False)
            # 有 started 就必须有 finished：否则卡片会永远停在"正在调用"。
            self._notify("tool_call_finished", rejected_record)
            entry = {
                "tool_name": tool,
                "args": tool_args,
                "result_summary": (
                    f"[tool_error] invalid arguments: {exc}. "
                    "Fix the argument names and retry, or finalize."
                ),
                "status": "invalid_args",
                "duration_ms": 0,
                "tc_id": None,
                "llm_ms": self._last_llm_ms,
            }
            if reasoning:
                entry["reasoning"] = reasoning
            self.policy.state.traces.append(entry)
            trace_payload.append(entry)
            self._invalid_args += 1
            if self._invalid_args >= MAX_INVALID_ARG_ROUNDS:
                result.status = "degraded"
                return "invalid_arguments"
            if not self.policy.note_tool_call(tool, tool_args, str(entry["result_summary"])):
                return self.policy.state.stop_reason
            return None
        rendered = wrap_result(call)
        record = self._record(call, rendered)
        record.llm_ms = self._last_llm_ms
        result.trace.append(record)
        self._notify("tool_call", record.step, record.tool_name, call.ok)
        self._notify("tool_call_finished", record)
        self.policy.state.traces.append(
            {
                "tool_name": record.tool_name,
                "args": call.args,
                "result_summary": record.result_summary,
                "status": record.status,
                "duration_ms": record.duration_ms,
                "tc_id": record.tc_id,
                "llm_ms": record.llm_ms,
                # 只给"下一轮请求"用（官方要求回传）；台账那一份在上面，没有它。
                **({"reasoning": reasoning} if reasoning else {}),
            }
        )
        trace_payload.append(self.policy.state.traces[-1])
        self.policy.note_tool_time(record.duration_ms)
        self._collect_evidence(call, evidence)
        if not self.policy.note_tool_call(tool, call.args, record.result_summary):
            return self.policy.state.stop_reason
        return None

    def _submit(self, result: AgentRunResult) -> None:
        """Pushes findings through validate_finding + submit_finding (ADR-018)."""
        accepted: list[FindingDraft] = []
        rejected = 0
        for draft in result.findings:
            call = self.client.call_envelope(
                "validate_finding",
                {"task_id": result.task_id, "draft": draft.as_dict()},
            )
            if not call.ok:
                rejected += 1
                result.notes.append(f"validate_finding failed: {call.error}")
                self._notify(
                    "finding_rejected", draft.title, "VALIDATE_ERROR", str(call.error)
                )
                continue
            content = call.content or {}
            status = content.get("status")
            if status == "rejected":
                rejected += 1
                reasons = "; ".join(
                    f"{issue.get('check')}: {issue.get('message')}"
                    for issue in (content.get("issues") or [])
                )
                result.notes.append(
                    f"finding rejected by V1-V4 [{reasons or 'no detail'}]: {draft.title}"
                )
                self._notify(
                    "finding_rejected",
                    draft.title,
                    _first_check(content) or "V1_V4",
                    reasons or "no detail",
                )
                continue
            if status == "downgraded":
                reasons = "; ".join(
                    f"{issue.get('check')}: {issue.get('message')}"
                    for issue in (content.get("issues") or [])
                )
                result.notes.append(
                    f"finding downgraded to hypothesis [{reasons}]: {draft.title}"
                )
            accepted.append(draft)
        result.submit_rejects = rejected
        if not accepted:
            return
        submit = self.client.call_envelope(
            "submit_finding",
            {
                "task_id": result.task_id,
                "agent_run_id": self.agent_run_id,
                "drafts": [draft.as_dict() for draft in accepted],
            },
        )
        if not submit.ok:
            result.notes.append(f"submit_finding failed: {submit.error}")
            result.submit_rejects += len(accepted)
            result.findings = accepted
            return
        content = submit.content or {}
        refused = list(content.get("rejected") or [])
        unknown_tc = int(content.get("unknown_tc_id") or 0)
        result.submit_rejects += len(refused) + unknown_tc
        for refusal in refused:
            index = refusal.get("draft_index") if isinstance(refusal, dict) else None
            reason = str(
                (refusal.get("reason") if isinstance(refusal, dict) else refusal) or "rejected"
            )
            title = ""
            if isinstance(index, int) and 0 <= index < len(accepted):
                title = accepted[index].title
            self._notify("finding_rejected", title, "SUBMIT_REJECTED", reason)
        if unknown_tc:
            self._notify(
                "finding_rejected",
                "",
                "UNKNOWN_TC_ID",
                f"{unknown_tc} evidence anchor(s) were never issued by the engine",
            )
        stored = content.get("stored") or []
        if isinstance(stored, list):
            result.stored_findings = [item for item in stored if isinstance(item, dict)]
        for row in result.stored_findings:
            self._notify("finding_accepted", _accepted_row(row))
        result.findings = accepted


def _first_check(content: Any) -> str:
    """Validator check of the first issue (`V1`…`V4`), else an empty string."""
    for issue in (content.get("issues") if isinstance(content, dict) else None) or []:
        check = str(issue.get("check") or "")
        if check:
            return check
    return ""


def _accepted_row(row: dict[str, Any]) -> dict[str, Any]:
    """One stored finding → the `finding_accepted` event payload (§4.5)."""
    return {
        "id": str(row.get("finding_id") or ""),
        "severity": str(row.get("severity") or ""),
        "basis": str(row.get("basis") or ""),
        "title": str(row.get("title") or ""),
        "evidence_ids": [
            str(item.get("_id"))
            for item in (row.get("evidence") or [])
            if isinstance(item, dict) and item.get("_id")
        ],
    }


def build_agent(
    client: RpcCaller,
    provider_kind: str = "mock",
    model: str | None = None,
    scenario: str = "auto",
    budget: AgentBudget | None = None,
    observer: Any | None = None,
    mode: str = "run",
    prompt_version: str | None = None,
    base_url: str | None = None,
    api_key: str | None = None,
    preload: bool = True,
) -> PacketSageAgent:
    """Factory used by the CLI and the evaluation runner."""
    try:
        provider = build_provider(
            provider_kind,
            model,
            scenario,
            mode=mode,
            prompt_version=prompt_version,
            base_url=base_url,
            api_key=api_key,
        )
    except (RuntimeError, ValueError, ImportError) as exc:  # pragma: no cover
        raise EngineError(f"cannot build the {provider_kind} provider: {exc}") from exc
    return PacketSageAgent(
        client,
        provider=provider,
        budget=budget,
        model_name=model or provider_kind,
        provider_kind=provider_kind,
        observer=observer,
        mode=mode,
        prompt_version=prompt_version,
        preload=preload,
    )
