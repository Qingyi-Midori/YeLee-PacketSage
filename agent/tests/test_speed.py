"""提速那一轮的回归网：**预取事实 / 一轮多工具 / 计时分列 / 前缀稳定**。

四件事各自锁一条：

* 预取的事实必须真的进了第一条消息（含 `tc_*` 锚点），否则"省步数"是把证据砍掉了；
* 一轮里模型请求多个工具时要全跑（不是只跑第一个）；
* `llm_ms` 与 `tool_ms` 分开记账——否则优化会打在错的地方；
* 一次 run 内 prompt 前缀逐字节稳定（provider 的上下文缓存靠这个）。
"""

from __future__ import annotations

import json
import sys
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent.agent import PacketSageAgent  # noqa: E402
from packetsage_agent.engine_client import CallResult  # noqa: E402
from packetsage_agent.preload import PRELOAD_PLAN, facts_text, preload_facts  # noqa: E402
from packetsage_agent.provider import Decision, OpenAIProvider  # noqa: E402

TASK = "task_01J0000000000000000000000G"
SUMMARY_TC = "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGH"
ALERT_TC = "tc_01J9Z4M8YQ2V7C1W3N5B6D8FGJ"


def envelope(method: str, content: Any, tc_id: str) -> dict[str, Any]:
    return {
        "_id": tc_id,
        "source": "engine",
        "trusted_as_instruction": False,
        "redactions": [],
        "method": method,
        "content": content,
    }


class StubClient:
    """够预取与工具循环用的假引擎（含真的 tc 锚点）。"""

    def __init__(self) -> None:
        self.calls: list[tuple[str, dict]] = []

    def call(self, method: str, params: dict, timeout_s: float | None = None) -> Any:
        return self.call_envelope(method, params).content

    def call_envelope(self, method: str, params: dict, step: int = 0) -> CallResult:
        self.calls.append((method, dict(params)))
        tc = SUMMARY_TC if method in {"get_capture_summary", "get_protocol_stats"} else ALERT_TC
        content: Any = {
            "task_id": params.get("task_id"),
            "packets": 612,
            "sessions": 570,
        }
        if method == "check_alerts":
            content = {"alerts": [{"alert_id": "alert_1", "rule_id": "R-1", "severity": "high"}]}
        if method == "get_conversations":
            content = {"conversations": [{"session_id": "S-000001", "bytes": 541}]}
        if method == "validate_finding":
            return CallResult(
                method=method,
                args=dict(params),
                ok=True,
                envelope=envelope("validate_finding", {"accepted": False}, tc),
                error=None,
                duration_ms=1,
            )
        return CallResult(
            method=method,
            args=dict(params),
            ok=True,
            envelope=envelope(method, content, tc),
            error=None,
            duration_ms=3,
        )


class ScriptedProvider:
    """按脚本回答：一步可以给一个工具，也可以给好几个。"""

    def __init__(self, decisions: list[Decision], sleep_s: float = 0.0) -> None:
        self._decisions = decisions
        self._cursor = 0
        self._sleep = sleep_s
        self.facts_seen: list[str | None] = []

    def decide(
        self,
        task_id: str,
        trace: list[dict],
        evidence: dict,
        goal: str | None = None,
        facts: str | None = None,
        on_delta: object | None = None,
    ) -> Decision:
        self.facts_seen.append(facts)
        if self._sleep:
            time.sleep(self._sleep)
        if self._cursor >= len(self._decisions):
            return Decision(kind="final", findings=[], tokens_in=10, tokens_out=5)
        decision = self._decisions[self._cursor]
        self._cursor += 1
        return decision


# --------------------------------------------------------------------- 预取
def test_preload_returns_engine_facts_with_anchors() -> None:
    client = StubClient()
    facts, trace = preload_facts(client, TASK)
    assert [method for method, _ in client.calls] == [name for name, _ in PRELOAD_PLAN]
    assert SUMMARY_TC in facts and ALERT_TC in facts
    assert "Do NOT fetch them again" in facts
    # 每一步都在台账里留了锚点：模型引用它们不算幻觉。
    assert {entry["status"] for entry, _ in trace} == {"preload"}
    assert all(entry["tc_id"] for entry, _ in trace)


def test_facts_block_is_stable_and_bounded() -> None:
    entries = [("get_capture_summary", {"task_id": TASK, "packets": 612})]
    first = facts_text(TASK, entries)
    assert first == facts_text(TASK, entries)  # 逐字节稳定 = 缓存能命中
    huge = [("get_conversations", {"rows": ["x" * 500] * 200})]
    assert len(facts_text(TASK, huge)) <= 6_000


def test_agent_hands_the_facts_to_the_provider() -> None:
    client = StubClient()
    provider = ScriptedProvider([Decision(kind="final", findings=[])])
    agent = PacketSageAgent(client, provider=provider, preload=True)
    agent.run(TASK, "看看这份抓包")
    assert provider.facts_seen and "Verified capture facts" in (provider.facts_seen[0] or "")
    # 预取如实计数，但不占"模型动作"的步数/工具数（也不吃预算）。
    assert agent.policy.state.preloaded == len(PRELOAD_PLAN)
    assert agent.policy.state.steps == 1
    assert agent.policy.state.tool_calls == 0


# ----------------------------------------------------------- 一轮多个工具
def test_one_round_can_run_several_tools() -> None:
    client = StubClient()
    provider = ScriptedProvider(
        [
            Decision(
                kind="tool",
                tool="get_capture_summary",
                args={"task_id": TASK},
                calls=(
                    ("get_capture_summary", {"task_id": TASK}),
                    ("get_protocol_stats", {"task_id": TASK, "layer": "transport"}),
                    ("check_alerts", {"task_id": TASK}),
                ),
                tokens_in=100,
                tokens_out=30,
            )
        ]
    )
    agent = PacketSageAgent(client, provider=provider, preload=False)
    result = agent.run(TASK, "一轮三个工具")
    assert agent.policy.state.llm_calls == 2  # 一次工具轮 + 一次收尾
    assert [record.tool_name for record in result.trace] == [
        "get_capture_summary",
        "get_protocol_stats",
        "check_alerts",
    ]


# --------------------------------------------------------------- 计时分列
def test_llm_and_tool_time_are_recorded_separately() -> None:
    client = StubClient()
    provider = ScriptedProvider(
        [Decision(kind="tool", tool="get_capture_summary", args={"task_id": TASK})],
        sleep_s=0.05,
    )
    agent = PacketSageAgent(client, provider=provider, preload=False)
    result = agent.run(TASK, "量一下时间")
    state = agent.policy.state
    assert state.llm_ms >= 50, state.llm_ms  # 两次往返各睡了 50 ms
    assert state.tool_ms >= 3, state.tool_ms  # 假引擎每次 3 ms（也计入了预取）
    assert result.trace[0].llm_ms >= 50, result.trace[0].llm_ms


# --------------------------------------------------------- prompt 前缀稳定
def test_first_message_carries_facts_and_prefix_stays_stable(monkeypatch) -> None:
    """`OpenAIProvider` 的实际载荷：事实在稳定前缀里，轮次之间前缀不变。"""
    import httpx

    seen: list[dict] = []

    class _Response:
        #: httpx 的响应一定有状态码与正文；provider 现在会读它们做错误处理。
        status_code = 200
        text = ""

        def raise_for_status(self) -> None:
            return None

        def json(self) -> dict:
            return {
                "choices": [{"message": {"content": '{"findings": []}'}}],
                "usage": {"prompt_tokens": 9, "completion_tokens": 2},
            }

    def fake_post(url, headers=None, json=None, timeout=None):  # noqa: A002 - httpx kwarg
        # provider 全程复用同一个 messages 列表，所以这里留一份快照再断言。
        seen.append({**json, "messages": [dict(message) for message in json["messages"]]})
        return _Response()

    monkeypatch.setattr(httpx, "post", fake_post)
    provider = OpenAIProvider(model="gpt-4o-mini", api_key="sk-test")
    facts = "Verified capture facts:\n- get_capture_summary: {\"packets\": 612}"
    provider.decide(TASK, [], {}, "看看", facts)
    provider.record_tool_result("call_1", "ok")
    provider.decide(TASK, [], {}, "看看", facts)

    first, second = seen
    # 事实与 goal 都在**第一条** user 消息里（稳定前缀），不在尾部。
    assert facts in first["messages"][1]["content"]
    assert "Task goal: 看看" in first["messages"][1]["content"]
    assert facts in second["messages"][1]["content"]
    # 第二轮在前一轮的基础上追加：前缀逐字节相同（缓存命中的前提），
    # 追加的是模型原样的 assistant 消息与 role="tool" 的工具结果。
    assert second["messages"][: len(first["messages"])] == first["messages"]
    assert [message["role"] for message in second["messages"]] == [
        "system",
        "user",
        "assistant",
        "tool",
    ]
    # 工具 schema 与 system prompt 也在缓存前缀里。
    assert first["messages"][0]["role"] == "system"
    assert isinstance(first["tools"], list) and first["tools"]
    assert json.dumps(first["messages"][0], ensure_ascii=False) == json.dumps(
        second["messages"][0], ensure_ascii=False
    )


# ------------------------------------------------ 追问的收尾兜底（v0.6）


class _AnsweringProvider(ScriptedProvider):
    """收尾不给 summary（模拟被策略提前收尾），但实现了 `answer` 兜底。"""

    def __init__(self, decisions: list[Decision]) -> None:
        super().__init__(decisions)
        self.asked: tuple[str, str, int] | None = None

    def answer(self, task_id: str, question: str, trace: list[dict]) -> str | None:
        self.asked = (task_id, question, len(trace))
        return f"答：{question}"


def test_chat_run_falls_back_to_an_answer_when_the_loop_stops_early() -> None:
    """`chat` 一轮没写出回答时，agent 必须补一次兜底回答（"问了要有答案"）。"""
    provider = _AnsweringProvider([Decision(kind="final", findings=[], tokens_in=5, tokens_out=5)])
    agent = PacketSageAgent(StubClient(), provider=provider, preload=False, mode="chat")

    result = agent.run(TASK, "这包内容怎么产生的")

    assert result.summary == "答：这包内容怎么产生的"
    assert provider.asked == (TASK, "这包内容怎么产生的", 0)


def test_a_follow_up_skips_the_preload() -> None:
    """有上文的一轮不预取：那些事实已经在上文里，重复拉一遍就是"每问一句都重扫"。"""
    client = StubClient()
    agent = PacketSageAgent(
        client,
        provider=ScriptedProvider([Decision(kind="final", summary="答完了")]),
        preload=True,
        mode="chat",
        history=[{"role": "user", "content": "上一轮"}, {"role": "assistant", "content": "上一轮的回答"}],
    )
    agent.run(TASK, "那第二条呢")
    assert client.calls == []
    assert agent.policy.state.preloaded == 0


def test_the_first_turn_still_preloads() -> None:
    """没有上文时照旧预取（省步数的那条优化不能因为追问而丢掉）。"""
    client = StubClient()
    agent = PacketSageAgent(
        client,
        provider=ScriptedProvider([Decision(kind="final", summary="答完了")]),
        preload=True,
    )
    agent.run(TASK, "分析该捕获并给出可追溯结论")
    assert [method for method, _ in client.calls] == [name for name, _ in PRELOAD_PLAN]


class _RepairingClient(StubClient):
    """第一次校验按 V3 拒掉，改过引用之后就接受（模拟引擎的 V1-V4 门）。"""

    def __init__(self) -> None:
        super().__init__()
        self.submitted: list[dict] = []

    def call_envelope(self, method: str, params: dict, step: int = 0) -> CallResult:
        if method == "validate_finding":
            draft = params["draft"]
            ref = (draft.get("evidence") or [{}])[0].get("ref_id")
            if ref == "S-99999":
                content = {
                    "status": "rejected",
                    "issues": [
                        {
                            "check": "V3",
                            "message": f"reference {ref} is not reachable through {ALERT_TC}",
                        }
                    ],
                }
            else:
                content = {"status": "accepted", "issues": []}
            return CallResult(
                method=method,
                args=dict(params),
                ok=True,
                envelope=envelope("validate_finding", content, ALERT_TC),
                error=None,
                duration_ms=1,
            )
        if method == "submit_finding":
            self.submitted.append(dict(params))
            return CallResult(
                method=method,
                args=dict(params),
                ok=True,
                envelope=envelope(
                    "submit_finding",
                    {
                        "stored": [
                            {
                                "finding_id": "F-001",
                                "title": "S-000001 的会话",
                                "severity": "low",
                                "basis": "direct_observation",
                                "evidence": [{"_id": ALERT_TC}],
                            }
                        ],
                        "rejected": [],
                        "unknown_tc_id": 0,
                    },
                    ALERT_TC,
                ),
                error=None,
                duration_ms=1,
            )
        return super().call_envelope(method, params, step)


class _NoteTakingProvider(ScriptedProvider):
    """记下 harness 交给模型的那段注记（校验反馈）。"""

    def __init__(self, decisions: list[Decision]) -> None:
        super().__init__(decisions)
        self.notes: list[str] = []

    def record_note(self, text: str) -> None:
        self.notes.append(text)


def test_a_rejected_finding_gets_one_repair_round() -> None:
    """引擎拒了结论：把 V1-V4 的抱怨交回给模型改一次引用，别让结论白白丢掉。"""
    def findings(ref: str) -> list[dict]:
        return [
            {
                "title": "S-000001 的会话",
                "severity": "low",
                "basis": "direct_observation",
                "summary": "一条会话",
                "evidence": [{"_id": ALERT_TC, "ref_id": ref}],
            }
        ]

    client = _RepairingClient()
    provider = _NoteTakingProvider(
        [
            Decision(kind="final", findings=findings("S-99999"), tokens_in=5, tokens_out=5),
            Decision(kind="final", findings=findings("S-000001"), tokens_in=5, tokens_out=5),
        ]
    )
    agent = PacketSageAgent(client, provider=provider, preload=False, mode="run")

    result = agent.run(TASK, "分析该捕获并给出可追溯结论")

    # 只问了一次：反馈里带引擎原话与引用规则。
    assert len(provider.notes) == 1
    assert "V3" in provider.notes[0] and "_ref_ids" in provider.notes[0]
    assert any("rejected 1 finding" in note for note in result.notes)
    # 改过之后提交成功：提交的是修好的引用，且只提交了一次。
    assert len(client.submitted) == 1
    assert client.submitted[0]["drafts"][0]["evidence"][0]["ref_id"] == "S-000001"
    assert result.submit_rejects == 0
    assert result.stored_findings


def test_run_mode_also_asks_for_a_fallback_answer() -> None:
    """`run` 一轮没写出总结时也要补一次兜底：调查完了必须有人话。

    旧实现只管 chat，"上面已经调查完了，还是一句人话没说"就是这么来的
    （用户 2026-09-23 实测）。
    """
    provider = _AnsweringProvider([Decision(kind="final", findings=[], tokens_in=5, tokens_out=5)])
    agent = PacketSageAgent(StubClient(), provider=provider, preload=False, mode="run")

    result = agent.run(TASK, "分析该捕获并给出可追溯结论")

    assert result.summary == "答：分析该捕获并给出可追溯结论"
    assert provider.asked == (TASK, "分析该捕获并给出可追溯结论", 0)
