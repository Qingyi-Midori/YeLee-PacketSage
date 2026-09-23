"""Model providers (M3~M6 §4.6).

* ``mock``     — deterministic script replay, the only dependency of CI;
* ``openai``   — OpenAI compatible HTTP API (temperature forced to 0);
* ``deepseek`` — the same wire protocol at ``api.deepseek.com`` (the provider
  the project's configuration examples name);
* ``local``    — any OpenAI compatible endpoint (vLLM / ollama) over httpx.

Nothing here invents a default provider: the CLI refuses to run until one is
configured (`packetsage-agent setup`), so a silent ``mock`` can never be
mistaken for a real analysis.
"""

from __future__ import annotations

import json
import os
import time
from dataclasses import dataclass
from typing import Any, cast

#: Every provider kind `agent.llm.provider` may name.
PROVIDER_KINDS: tuple[str, ...] = ("mock", "openai", "deepseek", "local")

#: Endpoint and model defaults; all three HTTP kinds speak the OpenAI protocol,
#: so one client covers them.
PROVIDER_DEFAULTS: dict[str, dict[str, str]] = {
    "openai": {"base_url": "https://api.openai.com/v1", "model": "gpt-4o-mini"},
    # 2026-09-22 起 `deepseek-chat` / `deepseek-reasoner` 已下线，
    # 现役是两个都支持思考模式的 `deepseek-flash` 与 `deepseek-v4-pro`（见定价页）。
    "deepseek": {"base_url": "https://api.deepseek.com/v1", "model": "deepseek-flash"},
    "local": {"base_url": "http://localhost:8000/v1", "model": "local-model"},
}

#: Providers that need a credential.
PROVIDERS_NEEDING_KEY: tuple[str, ...] = ("openai", "deepseek")

#: 流式帧的合并粒度（《GUI 工程规格书 v0.2》§4.5 `llm_delta`）：先到者 flush。
#: 太碎会把 stdout 与外壳事件队列灌满，太粗就不像"正在打字"；这两个数是折中。
DELTA_FLUSH_SECONDS = 0.12
DELTA_FLUSH_CHARS = 256


class _StreamRejected(Exception):
    """端点拒绝了流式请求（4xx，还没吐出任何字节）：换一种写法再试。"""

    def __init__(self, status: int) -> None:
        super().__init__(f"HTTP {status}")
        self.status = status


def _cache_counts(usage: dict[str, Any]) -> tuple[int, int]:
    """`usage` 里的缓存命中/未命中 tokens（DeepSeek 上下文硬盘缓存）。

    字段名见 kv_cache 指南：`prompt_cache_hit_tokens` / `prompt_cache_miss_tokens`。
    别的 provider 不给就返回 `(0, 0)`——**不拿 tokens_in 去推算**，界面上"没有
    缓存数据"和"缓存全没命中"必须是两件事。
    """
    hit = usage.get("prompt_cache_hit_tokens")
    miss = usage.get("prompt_cache_miss_tokens")
    return (int(hit) if isinstance(hit, (int, float)) else 0, int(miss) if isinstance(miss, (int, float)) else 0)


def _clean_summary(value: Any) -> str | None:
    """模型自己写的那段总结；不是字符串或空串就当"没说"（不编）。"""
    if isinstance(value, str) and value.strip():
        return value.strip()
    return None


def _http_error(httpx: Any, response: Any, url: str) -> Exception:
    """把 HTTP 4xx/5xx 变成一条**能读懂**的错误（带 API 自己的说明）。

    错误只进日志与界面，不进报告；`raise_for_status()` 的原文只有状态码，
    用户看不到"是哪个字段不被接受"这类关键信息。
    """
    snippet = ""
    try:
        text = str(response.text or "").strip()
        if text:
            snippet = f" —— {text[:300]}"
    except Exception:  # pragma: no cover - 响应体读不出来就算了
        snippet = ""
    message = f"HTTP {response.status_code} from {url}{snippet}"
    return httpx.HTTPStatusError(message, request=response.request, response=response)


class DeltaCoalescer:
    """把 SSE 的碎块合并成 `llm_delta` 帧（§4.5）。

    同一 `channel` 的连续碎块拼在一起发出；`flush_s` 或 `flush_chars` 先到就先发。
    流结束时必须 `flush()` 一次，否则尾巴会留在缓冲里。

    `emit(channel, text, tool_name, tool_index)` 由调用方给（在 agent 里就是
    observer 的 `llm_delta` 钩子）。
    """

    def __init__(
        self,
        emit: Any,
        *,
        flush_s: float | None = None,
        flush_chars: int | None = None,
        clock: Any = time.monotonic,
    ) -> None:
        self._emit = emit
        # 读模块常量而不是写进默认值：测试与调优可以改常量，行为立刻跟着变。
        self._flush_s = DELTA_FLUSH_SECONDS if flush_s is None else flush_s
        self._flush_chars = DELTA_FLUSH_CHARS if flush_chars is None else flush_chars
        self._clock = clock
        self._text = ""
        self._channel = "content"
        self._tool_name: str | None = None
        self._tool_index: int | None = None
        self._since = clock()
        #: 实际发出去的帧数（测试与诊断用）。
        self.frames = 0

    def add(
        self,
        channel: str,
        text: str,
        tool_name: str | None = None,
        tool_index: int | None = None,
    ) -> None:
        """收一段碎块；换 channel / 换工具 / 换工具序号的先 flush，避免拼串。"""
        if not text:
            return
        if self._text and (
            channel != self._channel
            or tool_name != self._tool_name
            or tool_index != self._tool_index
        ):
            self.flush()
        self._channel = channel
        self._tool_name = tool_name
        self._tool_index = tool_index
        self._text += text
        if len(self._text) >= self._flush_chars or (self._clock() - self._since) >= self._flush_s:
            self.flush()

    def flush(self) -> None:
        """把缓冲里的这一段发出去（空缓冲什么也不做）。"""
        if not self._text:
            return
        text, self._text = self._text, ""
        self._since = self._clock()
        self.frames += 1
        self._emit(self._channel, text, self._tool_name, self._tool_index)


def default_base_url(kind: str) -> str | None:
    """Documented base URL of one provider kind (``None`` when it has none)."""
    return PROVIDER_DEFAULTS.get(kind, {}).get("base_url")


def default_model(kind: str) -> str | None:
    """Documented default model of one provider kind (``None`` for mock)."""
    return PROVIDER_DEFAULTS.get(kind, {}).get("model")


@dataclass
class Decision:
    """One model decision: call a tool, or finalize with findings."""

    kind: str
    tool: str | None = None
    args: dict[str, Any] | None = None
    #: 同一轮里模型请求的**全部**工具（并行 function calling）。`tool`/`args`
    #: 始终等于第一个，老调用方不受影响。
    calls: tuple[tuple[str, dict[str, Any]], ...] = ()
    findings: list[dict[str, Any]] | None = None
    text: str | None = None
    tokens_in: int = 0
    tokens_out: int = 0
    malformed: bool = False
    #: 流式端点没给 `usage`：这一轮的 tokens 记 0（不编数字），run 的 notes 里说明。
    usage_missing: bool = False
    #: 这一轮的思维链（DeepSeek `reasoning_content`）。**只在内存里**用于回传：
    #: 官方文档要求携带 `tools` 时把历史轮次的 reasoning 回传（会被拼进上下文），
    #: 但 ADR-007 不许把思维链写进台账，所以它到这里为止，不进 `traces`。
    reasoning: str | None = None
    #: 输入侧缓存命中/未命中 tokens（`usage.prompt_cache_hit_tokens` 等）。
    cache_hit_tokens: int = 0
    cache_miss_tokens: int = 0
    #: 收尾那一轮模型自己写的一段话（`summary`）：对话里"它到底说了什么"。
    summary: str | None = None

    def iter_calls(self) -> tuple[tuple[str, dict[str, Any]], ...]:
        """All tool calls of this round, in the model's order."""
        if self.calls:
            return self.calls
        if self.tool:
            return ((self.tool, self.args or {}),)
        return ()


class MockProvider:
    """Deterministic provider used by CI and by the demo mainline."""

    DEFAULT_PLAN: tuple[tuple[str, dict[str, Any]], ...] = (
        ("get_capture_summary", {}),
        ("get_protocol_stats", {"layer": "transport", "top": 5}),
        ("get_conversations", {"sort_by": "bytes", "limit": 10}),
        ("check_alerts", {}),
        # 演示主线 §38 ⑥⑦⑧：命中告警后再 pivot 到具体报文与会话，最后才是收尾。
        ("filter_packets", {"dst_port": 80, "limit": 20}),
        ("inspect_packets", {"packet_indices": [0], "preview_bytes": 256}),
        (
            "reconstruct_stream",
            {"direction": "client_to_server", "max_bytes": 65_536},
        ),
        ("get_task_artifacts", {}),
    )

    SCENARIOS: dict[str, tuple[tuple[str, dict[str, Any]], ...]] = {
        # `lean`：代表"事实已经在上下文里"的模型——不再做探索式拉取，
        # 只按需 pivot。用来量"削步数"的收益（agent/tests/bench_roundtrips.py）。
        "lean": (
            ("filter_packets", {"dst_port": 80, "limit": 20}),
            ("inspect_packets", {"packet_indices": [0], "preview_bytes": 256}),
            (
                "reconstruct_stream",
                {"direction": "client_to_server", "max_bytes": 65_536},
            ),
        ),
        "E1": (
            ("get_capture_summary", {}),
            ("get_conversations", {"sort_by": "bytes", "limit": 5}),
        ),
        "E2": (
            ("get_capture_summary", {}),
            ("get_conversations", {"sort_by": "bytes", "limit": 10}),
            ("check_alerts", {}),
            ("filter_packets", {"dst_port": 80, "limit": 20}),
            ("inspect_packets", {"packet_indices": [0], "preview_bytes": 128}),
            ("reconstruct_stream", {"direction": "client_to_server", "max_bytes": 65_536}),
        ),
        "E3": (
            ("get_capture_summary", {}),
            ("get_protocol_stats", {"layer": "transport", "top": 10}),
            ("get_conversations", {"limit": 10}),
            ("check_alerts", {}),
            ("inspect_packets", {"packet_indices": [0], "preview_bytes": 128}),
        ),
        "E4": (
            ("get_capture_summary", {}),
            ("check_alerts", {}),
            ("filter_packets", {"tcp_flags": ["SYN"], "limit": 20}),
            ("get_conversations", {"limit": 5}),
        ),
        "E5": (
            ("get_capture_summary", {}),
            ("get_protocol_stats", {"layer": "application", "top": 10}),
            ("check_alerts", {}),
        ),
        "E6": (
            ("get_capture_summary", {}),
            ("get_conversations", {"limit": 10}),
            ("check_alerts", {}),
            ("inspect_packets", {"packet_indices": [0], "preview_bytes": 256}),
        ),
    }

    def __init__(self, scenario: str = "auto") -> None:
        self.name = "mock"
        self.scenario = scenario
        self._cursor = 0
        #: 每次"模型往返"固定睡这么久（秒），用来量串行往返的代价。
        #: `scenario="slow:<s>"` 是同一个开关的老写法；环境变量让它与场景解耦，
        #: 于是 `lean + 慢往返` 也能一起测（agent/tests/bench_roundtrips.py）。
        self.latency_s = 0.0
        if scenario.startswith("slow:"):
            try:
                self.latency_s = float(scenario.split(":", 1)[1])
            except ValueError:
                self.latency_s = 0.0
        else:
            try:
                self.latency_s = float(os.environ.get("PACKETSAGE_MOCK_LATENCY_S", "") or 0)
            except ValueError:
                self.latency_s = 0.0

    def plan(self) -> tuple[tuple[str, dict[str, Any]], ...]:
        """Script for the configured scenario."""
        return self.SCENARIOS.get(self.scenario, self.DEFAULT_PLAN)

    def decide(
        self,
        task_id: str,
        trace: list[dict],
        evidence: dict,
        goal: str | None = None,
        facts: str | None = None,
        on_delta: Any | None = None,
    ) -> Decision:
        """Walks the script, skipping pivots the evidence does not justify.

        `goal` / `facts` are accepted for interface parity; the scripted provider
        decides by scenario, not by wording. `scenario = "slow:<seconds>"`（或
        `$PACKETSAGE_MOCK_LATENCY_S`）会在每次回答前睡一下，这就是"没有真 key
        也能量往返代价"的办法（见 `agent/tests/bench_roundtrips.py`）。

        `on_delta` 也照发：脚本回放没有 token 级流式，所以**一次给完整一段**——
        界面照样能走 `llm_delta` 那条路（S71），CI 与演示不必联网。
        """
        if self.latency_s > 0:
            time.sleep(self.latency_s)
        plan = self.plan()
        while self._cursor < len(plan):
            tool, args = plan[self._cursor]
            self._cursor += 1
            if tool == "filter_packets" and not evidence.get("alerts"):
                continue
            if tool == "inspect_packets" and not evidence.get("packet_indices"):
                continue
            if tool == "reconstruct_stream" and not evidence.get("sessions"):
                continue
            merged = {"task_id": task_id, **args}
            # Pivot arguments come from the evidence collected so far, which is
            # what makes the script behave like a decision tree rather than a
            # fixed call list.
            if tool == "filter_packets" and evidence.get("alerts"):
                ports = dict(evidence["alerts"][0].get("group_key") or [])
                if "dst_port" in ports:
                    merged["dst_port"] = int(ports["dst_port"])
                if "src_ip" in ports:
                    merged["src_ip"] = ports["src_ip"]
            if tool == "inspect_packets" and evidence.get("packet_indices"):
                merged["packet_indices"] = list(evidence["packet_indices"])[:3]
            if tool == "reconstruct_stream" and evidence.get("sessions"):
                merged["session_id"] = evidence["sessions"][0]["session_id"]
            self._emit_delta(
                on_delta, "tool_args", {"tool": tool, "args": merged}, tool, 0
            )
            return Decision(
                kind="tool",
                tool=tool,
                args=merged,
                calls=((tool, merged),),
                tokens_in=120,
                tokens_out=24,
            )
        findings = self._build_findings(evidence)
        self._emit_delta(on_delta, "content", {"findings": findings}, None, None)
        return Decision(
            kind="final",
            findings=findings,
            summary=self._summary(evidence, findings),
            tokens_in=200,
            tokens_out=180,
        )

    @staticmethod
    def _summary(evidence: dict[str, Any], findings: list[dict[str, Any]]) -> str:
        """脚本回放的"总结"：像模型那样说人话，但由已收集的证据决定。"""
        if findings:
            alerts = evidence.get("alerts") or []
            rules = "、".join(
                str(item.get("rule_id")) for item in alerts[:3] if item.get("rule_id")
            )
            detail = f"（{'、'.join(rules)}）" if rules else ""
            return (
                f"这份抓包里有点东西：确定性规则命中了 {len(findings)} 条{detail}，"
                "我已经逐条核对到工具台账里的证据锚点，结论在下面。"
            )
        return "这份抓包没查出需要确认的行为：规则没有命中，会话与协议统计也没有异常项。"

    @staticmethod
    def _emit_delta(
        on_delta: Any | None,
        channel: str,
        payload: Any,
        tool_name: str | None,
        tool_index: int | None,
    ) -> None:
        """脚本回放的一次性 delta（`json` 与真模型的 SSE 内容同一个 channel）。"""
        if on_delta is None:
            return
        on_delta(channel, json.dumps(payload, ensure_ascii=False), tool_name, tool_index)

    def answer(self, task_id: str, question: str, trace: list[dict]) -> str | None:
        """脚本回放的兜底回答（确定性，不联网）。"""
        if trace:
            return (
                f"（脚本回放）关于「{question}」：这次一共调了 {len(trace)} 次工具，"
                "抓包里没有规则命中，会话与协议统计也没有异常项。"
            )
        return f"（脚本回放）关于「{question}」：这次没有拿到可用的工具结果。"

    def _build_findings(self, evidence: dict) -> list[dict[str, Any]]:
        """Deterministic findings from the evidence actually collected."""
        findings: list[dict[str, Any]] = []
        alerts = evidence.get("alerts") or []
        summary_tc = evidence.get("summary_tc")
        alert_tc = evidence.get("alert_tc")
        for alert in alerts[:5]:
            alert_id = alert.get("alert_id")
            rule_id = alert.get("rule_id")
            refs: list[dict[str, Any]] = []
            # Evidence anchors must be engine-issued tc ids (M3~M6 §4.3); the
            # alert id / rule id travel as the referenced entity.
            if alert_tc:
                refs.append({"_id": alert_tc, "method": "check_alerts", "ref_id": alert_id})
            if summary_tc:
                refs.append({"_id": summary_tc, "method": "get_capture_summary"})
            window = alert.get("evidence", {})
            observed = window.get("value")
            threshold = window.get("threshold")

            def count(value: Any) -> str:
                """Counts are integers: `101`, not `101.0` (C1)."""
                if isinstance(value, float) and value.is_integer():
                    return str(int(value))
                return str(value)

            findings.append(
                {
                    "title": f"规则命中 {rule_id}",
                    "severity": str(alert.get("severity", "medium")),
                    "basis": "rule_match",
                    "summary": (
                        f"规则 {rule_id} 触发（告警 {alert_id}）：窗口内观测值 {count(observed)}，"
                        f"阈值 {count(threshold)}；证据为 tc 台账中的 check_alerts 结果"
                        f"{'，并与 get_capture_summary 的流量基线交叉' if summary_tc else ''}。"
                    ),
                    "evidence": refs,
                    "claims": [
                        {"_id": alert_tc, "field": "count", "value": float(len(alerts))}
                    ]
                    if alert_tc
                    else [],
                }
            )
        if not findings and summary_tc:
            findings.append(
                {
                    "title": "未发现规则命中",
                    "severity": "info",
                    "basis": "direct_observation",
                    "summary": "确定性引擎在本次捕获中未触发任何内置规则。",
                    "evidence": [{"_id": summary_tc, "method": "get_capture_summary"}],
                }
            )
        return findings


class OpenAIProvider:
    """OpenAI compatible provider (temperature forced to 0)."""

    def __init__(
        self,
        model: str,
        base_url: str | None = None,
        api_key: str | None = None,
        mode: str = "run",
        prompt_version: str | None = None,
        thinking: str | None = None,
    ) -> None:
        self.name = "openai"
        self.model = model
        #: `run` (batch investigation) or `chat` (REPL turn): picks the output
        #: block of the system prompt (《Agent 系统提示词规格》§2).
        self.mode = mode
        self.prompt_version = prompt_version
        #: `None` = 还没试过流式；`False` = 这个端点不吃（或没给 usage），以后整块。
        self.streaming: bool | None = None
        #: 思考模式（DeepSeek `{"thinking": {"type": ...}}`）：`auto` = 不显式传，
        #: 跟随厂商默认（DeepSeek 文档：思考模式默认打开）。想强制开关就设
        #: `PACKETSAGE_LLM_THINKING=enabled|disabled`。
        self.thinking = (thinking or os.environ.get("PACKETSAGE_LLM_THINKING") or "auto").lower()
        self.base_url = (
            base_url
            or os.environ.get("PACKETSAGE_LLM_BASE_URL")
            or os.environ.get("OPENAI_BASE_URL")
            or "https://api.openai.com/v1"
        )
        self.api_key = api_key or next(
            (
                os.environ[name]
                for name in ("PACKETSAGE_LLM_API_KEY", "OPENAI_API_KEY", "DEEPSEEK_API_KEY")
                if os.environ.get(name)
            ),
            "",
        )
        if not self.api_key:
            raise RuntimeError(
                "no API key: run `packetsage-agent setup`, or set "
                "$PACKETSAGE_LLM_API_KEY (agent/.env works too)"
            )

    def decide(
        self,
        task_id: str,
        trace: list[dict],
        evidence: dict,
        goal: str | None = None,
        facts: str | None = None,
        on_delta: Any | None = None,
    ) -> Decision:
        """One chat completion round trip (function calling + JSON mode).

        消息顺序是**按缓存排的**（#4）：system prompt、工具清单、预取事实、goal
        全部稳定，且都在前缀里；易变的对话历史一律追加在后面。这样 provider 的
        上下文缓存能命中前 N 轮的全部前缀，TTFT 与费用都降下来。

        `on_delta(channel, text, tool_name, tool_index)` 给了就走 SSE（§4.5
        `llm_delta`）；不给就是原来那条单次 POST——CLI 与 eval 字节级不变。
        """
        import httpx

        from .prompts import Mode, finalize_instruction, render_system_prompt, tool_listing
        from .tools import tool_schemas

        messages: list[dict[str, Any]] = [
            {
                "role": "system",
                "content": render_system_prompt(
                    task_id, cast("Mode", self.mode), self.prompt_version
                ),
            },
            {
                "role": "user",
                # 这条消息在一次 run 内逐字节不变（稳定前缀）。用户的 goal 是这里
                # 唯一的自由文本：没有它，模型会答一个泛泛的问题，而且（第一次真
                # 模型实测）会用 system prompt 的语言回答。
                "content": (
                    "Available tools:\n"
                    + tool_listing()
                    # 预取事实（#2）：引擎已经核实的确定性数据，含 tc 锚点，
                    # 让模型第一步就进入分析而不是探索。
                    + (f"\n\n{facts}" if facts else "")
                    + (f"\n\nTask goal: {goal}" if goal else "")
                    + "\n\nStart the analysis."
                    # 收尾指令放在**第一条消息里**，不放尾部：放在尾部会让每轮
                    # 都多出一条位置不同的消息，前缀缓存从那里往后全部失效。
                    # 放这里 = 每轮都在上一轮的基础上追加，缓存全程命中。
                    # run 与 chat 的收尾要求不同：run 要汇报结论，chat 要回答问题
                    # （用户实测：chat 用 run 的指令会答非所问）。
                    + f"\n\n{finalize_instruction(self.mode)}"
                ),
            },
        ]
        for entry in trace:
            assistant: dict[str, Any] = {
                "role": "assistant",
                "content": json.dumps(
                    {"tool": entry["tool_name"], "args": entry["args"]}, ensure_ascii=False
                ),
            }
            # 官方文档（thinking_mode）：请求带 `tools` 时，历史轮次的
            # `reasoning_content` **应当回传**，并会被拼进上下文。不回传 = 模型
            # 每轮都从零开始想，且与前缀缓存的实际内容不一致。
            if entry.get("reasoning"):
                assistant["reasoning_content"] = entry["reasoning"]
            messages.append(assistant)
            messages.append({"role": "user", "content": entry["result_summary"]})
        payload = {
            "model": self.model,
            "temperature": 0,
            "messages": messages,
            "tools": tool_schemas(),
            "response_format": {"type": "json_object"},
            **self._thinking_params(),
        }
        url = f"{self.base_url.rstrip('/')}/chat/completions"
        headers = {"Authorization": f"Bearer {self.api_key}"}
        if on_delta is not None and self.streaming is not False:
            streamed = self._decide_streaming(httpx, url, headers, payload, on_delta)
            if streamed is not None:
                return streamed
        response = httpx.post(url, headers=headers, json=payload, timeout=60)
        if response.status_code >= 400 and "thinking" in payload:
            # 端点不认 `thinking`（各家网关对未知字段的容忍度不同）：去掉它重发一次。
            # 思考模式是可选增强，**不该让整个调查挂掉**（用户 2026-09-22 实测）。
            self.thinking = "auto"
            retry = {key: value for key, value in payload.items() if key != "thinking"}
            response = httpx.post(url, headers=headers, json=retry, timeout=60)
        if response.status_code >= 400:
            raise _http_error(httpx, response, url)
        body = response.json()
        return self._decision_from_message(body["choices"][0]["message"], body.get("usage", {}))

    def answer(self, task_id: str, question: str, trace: list[dict]) -> str | None:
        """追问的**兜底回答**：不带工具再问一次"用几句话回答用户"。

        `chat` 若被策略提前收尾（重复调用 / 预算），模型可能压根没写回答——这一下补上，
        让"问了必须有答案"成立。失败就返回 `None`，界面照实说没答上（不编）。
        """
        import httpx

        from .prompts import CHAT_ANSWER_ONLY, render_system_prompt

        messages: list[dict[str, Any]] = [
            {
                "role": "system",
                "content": render_system_prompt(task_id, "chat", self.prompt_version),
            },
            {"role": "user", "content": f"用户的问题：{question}"},
        ]
        for entry in trace[-8:]:
            messages.append(
                {
                    "role": "user",
                    "content": f"[{entry.get('tool_name')}] {entry.get('result_summary')}",
                }
            )
        messages.append({"role": "user", "content": CHAT_ANSWER_ONLY})
        payload = {
            "model": self.model,
            "temperature": 0,
            "messages": messages,
            "response_format": {"type": "json_object"},
            **self._thinking_params(),
        }
        try:
            response = httpx.post(
                f"{self.base_url.rstrip('/')}/chat/completions",
                headers={"Authorization": f"Bearer {self.api_key}"},
                json=payload,
                timeout=60,
            )
            if response.status_code >= 400:
                return None
            content = response.json()["choices"][0]["message"].get("content") or ""
            parsed = json.loads(content)
        except Exception:  # noqa: BLE001 - 兜底失败不能反过来弄挂 run
            return None
        return _clean_summary(parsed.get("summary") if isinstance(parsed, dict) else None)

    def _thinking_params(self) -> dict[str, Any]:
        """思考模式开关（DeepSeek 专有字段，见 thinking_mode 指南）。

        `auto`（默认）：**不显式传**，跟随厂商默认——DeepSeek 文档写明"思考模式默认
        打开"，所以不加这一项也能拿到 `reasoning_content`；换成 OpenAI 之类的端点
        也不会因为多带一个不认识的字段被判 400。
        `enabled` / `disabled`：显式传 `{"thinking": {"type": ...}}`（用
        `PACKETSAGE_LLM_THINKING` 覆盖，适合"这次不算了我只要快"的场合）。
        """
        if self.thinking in ("enabled", "disabled"):
            return {"thinking": {"type": self.thinking}}
        return {}

    def _decide_streaming(
        self,
        httpx: Any,
        url: str,
        headers: dict[str, str],
        payload: dict[str, Any],
        on_delta: Any,
    ) -> Decision | None:
        """SSE 版的一次往返；`None` = 这个端点不吃流式，调用方改走整块。

        三条退路，都不重发已经流出去的内容：

        1. 端点不认 `stream_options`（4xx，此时一个字节都没吐）→ 去掉它再流一次；
        2. 端点根本不支持 `stream`（4xx）→ 关掉流式，改走整块（老路径）；
        3. 流完了但没给 `usage` → 这一轮照收（tokens 记 0，不编数字），
           同时关掉流式，后面几轮回到精确计数。
        """
        for extra in ({"stream_options": {"include_usage": True}}, {}):
            body = {**payload, "stream": True, **extra}
            try:
                return self._read_stream(httpx, url, headers, body, on_delta)
            except _StreamRejected:
                continue
        self.streaming = False
        return None

    def _read_stream(
        self,
        httpx: Any,
        url: str,
        headers: dict[str, str],
        body: dict[str, Any],
        on_delta: Any,
    ) -> Decision:
        """读一条 SSE，边读边发 `llm_delta`，最后拼成一次 Decision。"""
        coalescer = DeltaCoalescer(on_delta)
        content = ""
        reasoning_text = ""
        calls: dict[int, dict[str, Any]] = {}
        usage: dict[str, Any] = {}
        with httpx.stream("POST", url, headers=headers, json=body, timeout=60) as response:
            if response.status_code >= 400:
                raise _StreamRejected(response.status_code)
            for line in response.iter_lines():
                if not line or line.startswith(":"):
                    continue  # 空行与心跳注释
                if not line.startswith("data:"):
                    continue
                data = line[len("data:") :].strip()
                if data == "[DONE]":
                    break
                try:
                    chunk = json.loads(data)
                except json.JSONDecodeError:
                    continue  # 半行/脏帧：跳过，不打断这一轮
                if isinstance(chunk.get("usage"), dict):
                    usage = chunk["usage"]
                choices = chunk.get("choices") or []
                if not choices:
                    continue
                delta = choices[0].get("delta") or {}
                piece = delta.get("content")
                if piece:
                    content += piece
                    coalescer.add("content", piece)
                piece_reasoning = delta.get("reasoning_content")
                if piece_reasoning:
                    reasoning_text += piece_reasoning
                    coalescer.add("reasoning", piece_reasoning)
                for call in delta.get("tool_calls") or []:
                    index = int(call.get("index") or 0)
                    slot = calls.setdefault(index, {"id": None, "name": "", "arguments": ""})
                    if call.get("id"):
                        slot["id"] = call["id"]
                    function = call.get("function") or {}
                    if function.get("name"):
                        slot["name"] += str(function["name"])
                    if function.get("arguments"):
                        slot["arguments"] += str(function["arguments"])
                        coalescer.add(
                            "tool_args",
                            str(function["arguments"]),
                            slot["name"] or None,
                            index,
                        )
        coalescer.flush()
        message: dict[str, Any] = {"content": content}
        if reasoning_text:
            # 官方要求带 tools 时回传（见 decide 里的注释）；这里只放进这一次的
            # Decision，之后由 agent 挂在对应那一轮的 trace 上（不写台账）。
            message["reasoning_content"] = reasoning_text
        if calls:
            message["tool_calls"] = [
                {
                    "id": calls[index]["id"],
                    "type": "function",
                    "function": {
                        "name": calls[index]["name"],
                        "arguments": calls[index]["arguments"],
                    },
                }
                for index in sorted(calls)
            ]
        # 没有 usage 就等于预算里的 tokens 归零：不编数字，但把流式关掉止损。
        missing = not (usage.get("prompt_tokens") or usage.get("completion_tokens"))
        if missing:
            self.streaming = False
        decision = self._decision_from_message(message, usage)
        decision.usage_missing = missing
        return decision

    def _decision_from_message(self, message: dict[str, Any], usage: dict[str, Any]) -> Decision:
        """一次往返的应答 → Decision（整块与流式共用同一条解析）。"""
        # 缓存命中情况（DeepSeek 上下文硬盘缓存）：没有这个字段就是 0，不编数字。
        cache_hit, cache_miss = _cache_counts(usage)
        reasoning = message.get("reasoning_content")
        calls = message.get("tool_calls") or []
        if calls:
            # 一轮里模型可以请求多个工具（互不依赖的那些）。全部收下，循环里
            # 依次执行——省下的是 LLM 往返，不是工具时间。
            parsed_calls: list[tuple[str, dict[str, Any]]] = []
            for call in calls:
                function = call["function"]
                try:
                    parsed_args = json.loads(function.get("arguments") or "{}")
                except json.JSONDecodeError:
                    return Decision(
                        kind="tool",
                        tool=function.get("name"),
                        args=None,
                        malformed=True,
                        tokens_in=usage.get("prompt_tokens", 0),
                        tokens_out=usage.get("completion_tokens", 0),
                        reasoning=reasoning,
                        cache_hit_tokens=cache_hit,
                        cache_miss_tokens=cache_miss,
                    )
                parsed_calls.append((str(function.get("name")), parsed_args))
            first_tool, first_args = parsed_calls[0]
            return Decision(
                kind="tool",
                tool=first_tool,
                args=first_args,
                calls=tuple(parsed_calls),
                tokens_in=usage.get("prompt_tokens", 0),
                tokens_out=usage.get("completion_tokens", 0),
                reasoning=reasoning,
                cache_hit_tokens=cache_hit,
                cache_miss_tokens=cache_miss,
            )
        content = message.get("content") or "{}"
        try:
            parsed = json.loads(content)
        except json.JSONDecodeError:
            return Decision(
                kind="final",
                findings=[],
                malformed=True,
                tokens_in=usage.get("prompt_tokens", 0),
                tokens_out=usage.get("completion_tokens", 0),
                reasoning=reasoning,
                cache_hit_tokens=cache_hit,
                cache_miss_tokens=cache_miss,
            )
        return Decision(
            kind="final",
            findings=parsed.get("findings", []),
            summary=_clean_summary(parsed.get("summary")),
            tokens_in=usage.get("prompt_tokens", 0),
            tokens_out=usage.get("completion_tokens", 0),
            reasoning=reasoning,
            cache_hit_tokens=cache_hit,
            cache_miss_tokens=cache_miss,
        )


def build_provider(
    kind: str,
    model: str | None = None,
    scenario: str = "auto",
    mode: str = "run",
    prompt_version: str | None = None,
    base_url: str | None = None,
    api_key: str | None = None,
    thinking: str | None = None,
):  # noqa: ANN201 - provider interface is duck typed
    """Factory for the three supported provider kinds."""
    if kind == "mock":
        # The scripted provider is deterministic: the prompt does not steer it,
        # so the version is carried by the run result instead (agent.py).
        return MockProvider(scenario=scenario)
    if kind == "openai":
        return OpenAIProvider(
            model or default_model("openai") or "gpt-4o-mini",
            base_url=base_url,
            api_key=api_key,
            mode=mode,
            prompt_version=prompt_version,
            thinking=thinking,
        )
    if kind == "deepseek":
        return OpenAIProvider(
            model or default_model("deepseek") or "deepseek-flash",
            base_url=base_url or default_base_url("deepseek"),
            api_key=api_key,
            mode=mode,
            prompt_version=prompt_version,
            thinking=thinking,
        )
    if kind == "local":
        return OpenAIProvider(
            model or default_model("local") or "local-model",
            base_url=base_url or default_base_url("local"),
            api_key=api_key or os.environ.get("PACKETSAGE_API_KEY", "local"),
            mode=mode,
            prompt_version=prompt_version,
            thinking=thinking,
        )
    raise ValueError(f"unknown provider kind {kind!r}")


def probe_provider(
    kind: str,
    model: str | None = None,
    base_url: str | None = None,
    api_key: str | None = None,
    timeout: float = 15.0,
) -> tuple[bool, str]:
    """Live check used by `setup`: ``(ok, human readable detail)``.

    One `GET /models` is enough for OpenAI-compatible endpoints; when a gateway
    does not expose it (404), a one-token chat completion is tried instead. The
    key is never echoed — only the endpoint and the HTTP status.
    """
    if kind == "mock":
        return True, "mock：脚本回放，不联网、不需要 key"
    if kind not in PROVIDER_KINDS:
        return False, f"unknown provider kind {kind!r}"
    if kind in PROVIDERS_NEEDING_KEY and not api_key:
        return False, "没有 API key"

    import httpx

    url = (base_url or default_base_url(kind) or "").rstrip("/")
    if not url:
        return False, f"{kind}: 没有 base_url"
    headers = {"Authorization": f"Bearer {api_key}"} if api_key else {}
    try:
        response = httpx.get(f"{url}/models", headers=headers, timeout=timeout)
    except Exception as exc:  # network layer: DNS, TLS, proxy, timeout
        return False, f"无法连接 {url}：{type(exc).__name__}: {exc}"
    if response.status_code == 200:
        try:
            count = len(response.json().get("data") or [])
        except Exception:  # pragma: no cover - non-JSON 200
            count = 0
        suffix = f"，{count} 个模型可见" if count else ""
        return True, f"{url} 可达（HTTP 200{suffix}）"
    if response.status_code in (401, 403):
        return False, f"{url}: HTTP {response.status_code} —— key 无效或无权限"
    # Gateways without /models: fall back to the smallest possible completion.
    try:
        chat = httpx.post(
            f"{url}/chat/completions",
            headers=headers,
            json={
                "model": model or default_model(kind) or "unknown",
                "messages": [{"role": "user", "content": "ping"}],
                "max_tokens": 1,
            },
            timeout=timeout,
        )
    except Exception as exc:
        return False, f"无法连接 {url}：{type(exc).__name__}: {exc}"
    if chat.status_code == 200:
        return True, f"{url} 可达（HTTP 200，模型 {model or default_model(kind)}）"
    if chat.status_code in (401, 403):
        return False, f"{url}: HTTP {chat.status_code} —— key 无效或无权限"
    return False, f"{url}: HTTP {chat.status_code}（/models HTTP {response.status_code}）"


def list_models(
    kind: str,
    model: str | None = None,
    base_url: str | None = None,
    api_key: str | None = None,
    timeout: float = 15.0,
) -> list[str]:
    """`GET /models` 给出的模型名列表（桌面端"模型"下拉用它，见 list-models 指南）。

    拿不到就返回空列表——**界面照实说"拉不到"**，不猜、不编候选。
    """
    if kind == "mock":
        return []
    if kind not in PROVIDER_KINDS:
        return []
    import httpx

    url = (base_url or default_base_url(kind) or "").rstrip("/")
    if not url:
        return []
    headers = {"Authorization": f"Bearer {api_key}"} if api_key else {}
    try:
        response = httpx.get(f"{url}/models", headers=headers, timeout=timeout)
        if response.status_code != 200:
            return []
        payload = response.json()
    except Exception:  # noqa: BLE001 - 网络/解析失败都算"拉不到"
        return []
    rows = payload.get("data") if isinstance(payload, dict) else None
    if not isinstance(rows, list):
        return []
    names = {str(row.get("id")) for row in rows if isinstance(row, dict) and row.get("id")}
    return sorted(names)
