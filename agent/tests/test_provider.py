"""The real providers must actually send the system prompt (§5, S5–S6)."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

httpx = pytest.importorskip("httpx")

from packetsage_agent import provider as provider_mod  # noqa: E402
from packetsage_agent.provider import (  # noqa: E402
    DeltaCoalescer,
    MockProvider,
    OpenAIProvider,
    build_provider,
)

TASK = "task_01J9Z4M8YQ2V7C1W3N5B6D8FGH"


class _Response:
    """Minimal httpx response stand-in carrying one final answer."""

    def __init__(self, body: dict, status_code: int = 200, text: str = "") -> None:
        self._body = body
        self.status_code = status_code
        self.text = text
        self.request = None

    def raise_for_status(self) -> None:
        return None

    def json(self) -> dict:
        return self._body


def capture_post(monkeypatch, content: str = '{"findings": []}') -> dict:
    """Replaces `httpx.post`, returning the dict that receives the payload."""
    seen: dict = {}

    def fake_post(url, headers=None, json=None, timeout=None):  # noqa: A002 - httpx kwarg
        seen["url"] = url
        # provider 全程复用同一个 messages 列表：留一份快照，断言才不会被下一轮追加污染。
        seen["payload"] = {
            **json,
            "messages": [dict(message) for message in json.get("messages", [])],
        }
        return _Response(
            {
                "choices": [{"message": {"content": content}}],
                "usage": {"prompt_tokens": 11, "completion_tokens": 3},
            }
        )

    monkeypatch.setattr(httpx, "post", fake_post)
    return seen


class _StreamResponse:
    """Minimal httpx streaming response stand-in carrying SSE lines."""

    def __init__(self, lines: list[str], status_code: int = 200) -> None:
        self._lines = lines
        self.status_code = status_code

    def iter_lines(self):
        yield from self._lines

    def __enter__(self) -> _StreamResponse:
        return self

    def __exit__(self, *exc: object) -> bool:
        return False


def sse(*chunks: dict) -> list[str]:
    """SSE 正文：每个 chunk 一行 `data:`，最后一条 `[DONE]`。"""
    return [f"data: {json.dumps(chunk, ensure_ascii=False)}" for chunk in chunks] + [
        "data: [DONE]"
    ]


def capture_stream(monkeypatch, responses: list[_StreamResponse]) -> dict:
    """Replaces `httpx.stream`; records the payload of every attempt."""
    seen: dict = {"calls": []}

    def fake_stream(method, url, headers=None, json=None, timeout=None):  # noqa: A002
        seen["calls"].append({"method": method, "url": url, "payload": json})
        return responses[min(len(seen["calls"]) - 1, len(responses) - 1)]

    monkeypatch.setattr(httpx, "stream", fake_stream)
    # 合并窗口拉到 60 s：这一组用例只看"碎块怎么拼"，不看墙钟。
    monkeypatch.setattr(provider_mod, "DELTA_FLUSH_SECONDS", 60.0)
    return seen


def test_openai_provider_sends_the_chat_prompt(monkeypatch):
    seen = capture_post(monkeypatch)
    provider = OpenAIProvider("test-model", api_key="sk-test", mode="chat")
    decision = provider.decide(TASK, [], {}, goal="最可疑的会话是哪一个？")

    messages = seen["payload"]["messages"]
    system = messages[0]["content"]
    assert messages[0]["role"] == "system"
    # The user's goal is the only free text that reaches the model, and it must
    # actually arrive (a first live run answered in the wrong language without it).
    assert messages[1]["role"] == "user"
    assert "最可疑的会话是哪一个？" in messages[1]["content"]
    assert "## Output \u2014 chat mode" in system
    assert "## Output \u2014 run mode" not in system
    assert TASK in system
    # temperature stays pinned at 0 (M3~M6 §4.6) and usage is accounted.
    assert seen["payload"]["temperature"] == 0
    assert (decision.tokens_in, decision.tokens_out) == (11, 3)


def test_openai_provider_sends_the_run_prompt_and_honours_a_pinned_version(monkeypatch):
    seen = capture_post(monkeypatch)
    provider = OpenAIProvider("test-model", api_key="sk-test", mode="run", prompt_version="v1")
    provider.decide(TASK, [], {}, goal="分析该捕获并给出可追溯结论")

    system = seen["payload"]["messages"][0]["content"]
    assert system.startswith("You are PacketSage, a network traffic analysis agent.")
    assert "## Output \u2014 run mode" not in system  # v1 has no blocks
    assert "分析该捕获并给出可追溯结论" in seen["payload"]["messages"][1]["content"]


def test_factory_forwards_mode_and_version(monkeypatch):
    seen = capture_post(monkeypatch)
    provider = build_provider("local", model="local-model", mode="chat", prompt_version="v1")
    provider.decide(TASK, [], {})
    system = seen["payload"]["messages"][0]["content"]
    assert "## Output \u2014 chat mode" not in system
    assert system.startswith("You are PacketSage, a network traffic analysis agent.")


# ------------------------------------------------------------------ U11 流式


def test_streaming_round_trip_emits_deltas_and_parses_tool_calls(monkeypatch):
    lines = sse(
        {
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_1",
                                "function": {"name": "check_alerts", "arguments": '{"rule'},
                            }
                        ]
                    }
                }
            ]
        },
        {
            "choices": [
                {"delta": {"tool_calls": [{"index": 0, "function": {"arguments": '_id": "X"}'}}]}}
            ]
        },
        {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]},
        {"choices": [], "usage": {"prompt_tokens": 21, "completion_tokens": 7}},
    )
    seen = capture_stream(monkeypatch, [_StreamResponse(lines)])
    frames: list[tuple] = []
    provider = OpenAIProvider("test-model", api_key="sk-test")

    decision = provider.decide(TASK, [], {}, on_delta=lambda *args: frames.append(args))

    assert seen["calls"][0]["payload"]["stream"] is True
    assert seen["calls"][0]["payload"]["stream_options"] == {"include_usage": True}
    assert decision.kind == "tool"
    assert decision.tool == "check_alerts"
    assert decision.args == {"rule_id": "X"}
    assert (decision.tokens_in, decision.tokens_out) == (21, 7)
    assert decision.usage_missing is False
    # 两段参数拼成一帧（合并窗口内），channel 与工具名跟着走。
    assert frames == [("tool_args", '{"rule_id": "X"}', "check_alerts", 0)]
    assert provider.streaming is None  # 端点没拒绝，也没缺 usage：保持流式


def test_streaming_content_round_trip_returns_findings(monkeypatch):
    lines = sse(
        {"choices": [{"delta": {"content": '{"findings": ['}}]},
        {"choices": [{"delta": {"content": "]}"}}]},
        {"choices": [], "usage": {"prompt_tokens": 9, "completion_tokens": 4}},
    )
    capture_stream(monkeypatch, [_StreamResponse(lines)])
    frames: list[tuple] = []
    decision = OpenAIProvider("m", api_key="sk").decide(
        TASK, [], {}, on_delta=lambda *args: frames.append(args)
    )

    assert decision.kind == "final" and decision.findings == []
    assert frames == [("content", '{"findings": []}', None, None)]


def test_streaming_retries_without_stream_options_when_rejected(monkeypatch):
    lines = sse(
        {"choices": [{"delta": {"content": '{"findings": []}'}}]},
        {"choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 2}},
    )
    seen = capture_stream(
        monkeypatch, [_StreamResponse([], status_code=400), _StreamResponse(lines)]
    )
    decision = OpenAIProvider("m", api_key="sk").decide(TASK, [], {}, on_delta=lambda *a: None)

    assert len(seen["calls"]) == 2
    assert "stream_options" in seen["calls"][0]["payload"]
    assert "stream_options" not in seen["calls"][1]["payload"]
    assert decision.kind == "final"


def test_streaming_falls_back_to_bulk_when_the_endpoint_rejects_stream(monkeypatch):
    seen = capture_stream(monkeypatch, [_StreamResponse([], status_code=400)])
    posted = capture_post(monkeypatch)
    provider = OpenAIProvider("m", api_key="sk")

    decision = provider.decide(TASK, [], {}, on_delta=lambda *a: None)

    assert len(seen["calls"]) == 2  # stream_options 一次 + 裸 stream 一次
    assert "stream" not in posted["payload"]  # 退路是原来那条单次 POST
    assert decision.kind == "final"
    assert provider.streaming is False


def test_streaming_without_usage_marks_the_round_and_keeps_streaming(monkeypatch):
    lines = sse({"choices": [{"delta": {"content": '{"findings": []}'}}]})  # 没有 usage
    capture_stream(monkeypatch, [_StreamResponse(lines)])
    provider = OpenAIProvider("m", api_key="sk")

    first = provider.decide(TASK, [], {}, on_delta=lambda *a: None)

    assert first.usage_missing is True
    assert (first.tokens_in, first.tokens_out) == (0, 0)  # 不编数字
    # 流式保持开着：关掉之后每一轮在界面上都"什么都不发生"，那正是卡住的观感。
    assert provider.streaming is None
    provider.decide(TASK, [], {}, on_delta=lambda *a: None)
    assert provider.streaming is None


def test_delta_coalescer_flushes_by_size():
    frames: list[tuple] = []
    coalescer = DeltaCoalescer(lambda *args: frames.append(args), flush_chars=6, flush_s=60)
    for piece in ("abc", "def", "ghi", "jkl"):
        coalescer.add("content", piece)
    coalescer.flush()

    # 到 6 个字符就发一帧；尾巴靠 flush() 兜住（不 flush 就会丢）。
    assert [text for _, text, _, _ in frames] == ["abcdef", "ghijkl"]


def test_delta_coalescer_splits_when_the_tool_changes():
    frames: list[tuple] = []
    coalescer = DeltaCoalescer(lambda *args: frames.append(args), flush_chars=999, flush_s=60)
    coalescer.add("tool_args", '{"a"', "first", 0)
    coalescer.add("tool_args", ":1}", "second", 1)
    # 同一个工具在同一轮里被要了两次（index 不同）：两段参数不能拼成一段。
    coalescer.add("tool_args", '{"b"', "first", 1)
    coalescer.flush()

    assert frames == [
        ("tool_args", '{"a"', "first", 0),
        ("tool_args", ":1}", "second", 1),
        ("tool_args", '{"b"', "first", 1),
    ]


def test_mock_provider_emits_one_delta_per_round():
    frames: list[tuple] = []
    decision = MockProvider().decide(TASK, [], {}, on_delta=lambda *args: frames.append(args))

    assert decision.kind == "tool"
    channel, text, tool_name, index = frames[0]
    assert (channel, tool_name, index) == ("tool_args", decision.tool, 0)
    assert json.loads(text)["tool"] == decision.tool


def test_mock_provider_stays_silent_without_a_delta_sink():
    frames: list[tuple] = []
    MockProvider().decide(TASK, [], {})  # CLI 路径：不给 on_delta
    assert frames == []


# ------------------------------------------- 思考模式（thinking_mode）与上下文缓存


def test_streaming_captures_reasoning_and_cache_hits(monkeypatch):
    """思考模式：推理内容与正文同级返回；usage 里带缓存命中情况。"""
    lines = sse(
        {"choices": [{"delta": {"reasoning_content": "先看告警"}}]},
        {"choices": [{"delta": {"reasoning_content": "，再看会话。"}}]},
        {
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_9",
                                "function": {"name": "check_alerts", "arguments": "{}"},
                            }
                        ]
                    }
                }
            ]
        },
        {
            "choices": [],
            "usage": {
                "prompt_tokens": 1200,
                "completion_tokens": 90,
                "prompt_cache_hit_tokens": 1024,
                "prompt_cache_miss_tokens": 176,
            },
        },
    )
    capture_stream(monkeypatch, [_StreamResponse(lines)])
    frames: list[tuple] = []
    decision = OpenAIProvider("deepseek-flash", api_key="sk").decide(
        TASK, [], {}, on_delta=lambda *args: frames.append(args)
    )

    assert decision.reasoning == "先看告警，再看会话。"
    assert (decision.cache_hit_tokens, decision.cache_miss_tokens) == (1024, 176)
    # 思考也走 llm_delta 的 reasoning 泳道（界面上的「思考」）；同路碎块合成一帧。
    assert frames[0] == ("reasoning", "先看告警，再看会话。", None, None)
    assert frames[1][0] == "tool_args" and frames[1][2] == "check_alerts"


def test_reasoning_is_echoed_back_on_the_next_round(monkeypatch):
    """官方 thinking_mode：带 tools 时，历史轮次的 reasoning_content 必须回传。

    回传的是**模型自己的那条 assistant 消息**（含 tool_calls），工具结果则是
    `role="tool"` + `tool_call_id` 的那条——旧实现往里塞的是"伪造的工具记录"，
    模型既拿不到 id，也看不到自己真的调过什么。
    """
    posted = capture_post(monkeypatch, content='{"summary": "只有一条告警", "findings": []}')
    tool_round = sse(
        {"choices": [{"delta": {"reasoning_content": "先确认规则命中。"}}]},
        {
            "choices": [
                {
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_7",
                                "function": {"name": "check_alerts", "arguments": "{}"},
                            }
                        ]
                    }
                }
            ]
        },
        {"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 4}},
    )
    capture_stream(monkeypatch, [_StreamResponse(tool_round)])
    provider = OpenAIProvider("deepseek-flash", api_key="sk")

    first = provider.decide(TASK, [], {}, on_delta=lambda *a: None)
    assert first.kind == "tool"
    assert first.call_ids == ("call_7",)
    # 工具跑完 → agent 用同一个 id 回填结果（`_run_tool` 干的就是这件事）。
    provider.record_tool_result(first.call_ids[0], '{"alerts": []}')
    # 第二轮不带 delta → 走整块 POST，便于直接看载荷。
    provider.decide(TASK, [], {})

    messages = posted["payload"]["messages"]
    assistant, tool_message = messages[2], messages[3]
    assert assistant["role"] == "assistant"
    assert assistant["reasoning_content"] == "先确认规则命中。"
    assert assistant["tool_calls"][0]["id"] == "call_7"
    assert tool_message == {
        "role": "tool",
        "tool_call_id": "call_7",
        "content": '{"alerts": []}',
    }


def test_thinking_params_follow_the_knob(monkeypatch):
    """`auto`（默认）不显式传；显式开关才带上 DeepSeek 的 thinking 字段。"""
    monkeypatch.delenv("PACKETSAGE_LLM_THINKING", raising=False)
    seen = capture_post(monkeypatch)
    OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {})
    assert "thinking" not in seen["payload"]

    monkeypatch.setenv("PACKETSAGE_LLM_THINKING", "enabled")
    seen = capture_post(monkeypatch)
    OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {})
    assert seen["payload"]["thinking"] == {"type": "enabled"}


def test_missing_cache_fields_are_not_invented(monkeypatch):
    """provider 不报缓存时留 0——"没有这个数据"不能和"一次都没命中"混为一谈。"""
    capture_post(monkeypatch)  # 默认 usage 里没有 prompt_cache_*
    decision = OpenAIProvider("gpt-test", api_key="sk").decide(TASK, [], {})
    assert (decision.cache_hit_tokens, decision.cache_miss_tokens) == (0, 0)


def test_final_round_carries_the_models_own_summary(monkeypatch):
    """收尾那一轮的 `summary` 就是对话里"模型说了什么"；空白/缺字段当没说。"""
    capture_post(
        monkeypatch, content='{"summary": "  先说结论：只有一条 SYN 突发。 ", "findings": []}'
    )
    decision = OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {})
    assert decision.summary == "先说结论：只有一条 SYN 突发。"

    capture_post(monkeypatch, content='{"summary": "   ", "findings": []}')
    assert OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {}).summary is None


def test_mock_provider_always_has_a_summary():
    """脚本回放也要给一句人话，否则演示与 CI 里对话是空的。"""
    provider = MockProvider()
    assert provider.decide(TASK, [], {}).kind == "tool"
    # 脚本按计划一步步给工具；走到最后一步就是收尾那一轮。
    final = provider.decide(TASK, [], {})
    for _ in range(20):
        if final.kind == "final":
            break
        final = provider.decide(TASK, [], {})
    assert final.kind == "final" and final.summary


def test_chat_mode_tells_the_model_to_answer_the_question(monkeypatch):
    """chat 追问的收尾指令必须是"回答问题"，不是"汇报有没有可疑行为"。"""
    seen = capture_post(monkeypatch)
    OpenAIProvider("deepseek-flash", api_key="sk", mode="chat").decide(
        TASK, [], {}, goal="这包的内容和什么有关系"
    )
    user = seen["payload"]["messages"][1]["content"]
    assert "就是你的回答" in user
    assert "这包的内容和什么有关系" in user


def test_reasoning_effort_is_sent_only_when_it_is_chosen(monkeypatch):
    """强度是档位（`reasoning_effort`），不是开关；自动时不显式传。"""
    monkeypatch.delenv("PACKETSAGE_LLM_EFFORT", raising=False)

    seen = capture_post(monkeypatch)
    OpenAIProvider("deepseek-flash", api_key="sk", effort="max").decide(TASK, [], {})
    assert seen["payload"]["reasoning_effort"] == "max"

    seen = capture_post(monkeypatch)
    OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {})
    assert "reasoning_effort" not in seen["payload"]

    monkeypatch.setenv("PACKETSAGE_LLM_EFFORT", "low")
    seen = capture_post(monkeypatch)
    OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {})
    assert seen["payload"]["reasoning_effort"] == "low"


def test_a_follow_up_tells_the_model_not_to_refetch(monkeypatch):
    """追问那一轮明确告诉模型：上文已经有工具结果，别再查一遍。"""
    seen = capture_post(monkeypatch)
    provider = OpenAIProvider(
        "deepseek-flash",
        api_key="sk",
        mode="chat",
        history=[{"role": "user", "content": "上一轮的问题"}],
    )
    provider.decide(TASK, [], {}, goal="那第二条呢")

    messages = seen["payload"]["messages"]
    assert "already in the messages above" in messages[-1]["content"]


def test_prose_answer_is_kept_as_the_summary(monkeypatch):
    """模型写了一段人话（不是 JSON）：这段就是回答，不再判 malformed 丢掉。

    旧实现要求整轮 `content` 必须是带 `summary` 的 JSON，散文被当成"非法输出"
    重试一次后整轮作废——用户看到的是"调查完了却一句人话都没有"。
    """
    capture_post(monkeypatch, content="这份抓包有 76836 个包，绝大多数是 TCP 重传。")
    decision = OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {})

    assert decision.kind == "final"
    assert decision.malformed is False
    assert decision.summary == "这份抓包有 76836 个包，绝大多数是 TCP 重传。"
    assert decision.findings == []


def test_json_with_literal_newlines_still_yields_the_summary(monkeypatch):
    """模型把换行直接敲进 JSON 字符串里：那是格式瑕疵，不该把回答整段丢掉。"""
    capture_post(monkeypatch, content='{"summary": "第一行\n第二行", "findings": []}')
    decision = OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {})

    assert decision.malformed is False
    assert decision.summary == "第一行\n第二行"


def test_json_wrapped_in_a_code_fence_is_unwrapped(monkeypatch):
    """```json … ``` 这层围栏要拆掉，否则用户看到的就是一大坨 JSON。"""
    capture_post(monkeypatch, content='```json\n{"summary": "答完了", "findings": []}\n```')
    decision = OpenAIProvider("deepseek-flash", api_key="sk").decide(TASK, [], {})

    assert decision.summary == "答完了"


def test_a_fallback_answer_reaches_the_next_turn(monkeypatch):
    """兜底补上的回答也要进消息表：用户看到了它，下一轮就必须看得到。"""
    seen = capture_post(monkeypatch, content='{"summary": "第一次回答", "findings": []}')
    provider = OpenAIProvider("deepseek-flash", api_key="sk")
    provider.decide(TASK, [], {})  # 建表
    provider.record_answer("兜底补上的回答")
    provider.decide(TASK, [], {}, goal="再问一句")

    messages = seen["payload"]["messages"]
    assert [message["role"] for message in messages] == [
        "system",
        "user",
        "assistant",
        "assistant",
    ]
    assert messages[3]["content"] == "兜底补上的回答"


def test_history_from_the_previous_turn_is_replayed(monkeypatch):
    """跨回合记忆：追问要带上上一轮的提问与回答（system 用**当前**这份）。"""
    seen = capture_post(monkeypatch)
    history = [
        {"role": "system", "content": "stale system prompt"},
        {"role": "user", "content": "第一次提问"},
        {"role": "assistant", "content": "第一次回答"},
    ]
    OpenAIProvider(
        "deepseek-flash", api_key="sk", mode="chat", history=history
    ).decide(TASK, [], {}, goal="那第二条呢")

    messages = seen["payload"]["messages"]
    assert [message["role"] for message in messages] == [
        "system",
        "user",
        "assistant",
        "user",
    ]
    assert messages[1]["content"] == "第一次提问"
    assert messages[2]["content"] == "第一次回答"
    assert "那第二条呢" in messages[3]["content"]
    assert messages[0]["content"] != "stale system prompt"


def test_a_stalled_round_raises_a_visible_timeout(monkeypatch):
    """整轮超过墙钟上限就抛一条能读懂的错，而不是界面永远不动。"""
    capture_stream(
        monkeypatch,
        [_StreamResponse(sse({"choices": [{"delta": {"content": "……"}}]}))],
    )
    provider = OpenAIProvider("m", api_key="sk", round_timeout_s=1e-6)

    with pytest.raises(provider_mod.LlmRoundTimeout):
        provider.decide(TASK, [], {}, on_delta=lambda *a: None)


def test_endpoint_rejecting_the_thinking_field_does_not_kill_the_run(monkeypatch):
    """端点不认 `thinking` 就去掉重发——思考模式是可选增强，不该让整个调查挂掉。"""
    attempts: list[dict] = []

    def fake_post(url, headers=None, json=None, timeout=None):  # noqa: A002 - httpx kwarg
        attempts.append(json)
        if "thinking" in json:
            return _Response({}, status_code=400, text='{"error":{"message":"unknown field"}}')
        return _Response(
            {
                "choices": [{"message": {"content": '{"summary": "答完了", "findings": []}'}}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 2},
            }
        )

    monkeypatch.setattr(httpx, "post", fake_post)
    provider = OpenAIProvider("deepseek-flash", api_key="sk", thinking="enabled")
    decision = provider.decide(TASK, [], {})

    assert decision.summary == "答完了"
    assert "thinking" in attempts[0]
    assert "thinking" not in attempts[1]
    assert provider.thinking == "auto"  # 之后几轮不再试


def test_non_json_envelope_keeps_only_the_prose(monkeypatch):
    """实测（2026-09-23）：收尾信封偶尔写成 `[summary]: "…" [findings]: […]`——
    键名带方括号，不是合法 JSON。回答里只该留那段人话，不能把 JSON 一起端到界面上。"""
    envelope = (
        '[summary]: "这份抓包记录了一次 iperf MPTCP 吞吐测试。"\n'
        '[findings]: [{"title": "全包解码失败", "severity": "low", "basis": "hypothesis"}]'
    )
    lines = sse(
        {"choices": [{"delta": {"content": envelope}}]},
        {"choices": [], "usage": {"prompt_tokens": 9, "completion_tokens": 4}},
    )
    capture_stream(monkeypatch, [_StreamResponse(lines)])

    # 给了 `on_delta` 才走 SSE（`httpx.stream`）；不给的话是那条单次 POST。
    decision = OpenAIProvider("deepseek-flash", api_key="sk").decide(
        TASK, [], {}, on_delta=lambda *args: None
    )

    assert decision.kind == "final"
    assert decision.summary == "这份抓包记录了一次 iperf MPTCP 吞吐测试。"
    assert decision.findings == []


def test_prose_that_mentions_the_fields_is_left_alone(monkeypatch):
    """反例：回答里**提到** `summary` / `findings` 这两个词很正常，不许把回答从中间截断。"""
    answer = "模型那段话放在 summary 里，findings 是机器字段，两者都由收尾信封给出。"
    lines = sse(
        {"choices": [{"delta": {"content": answer}}]},
        {"choices": [], "usage": {"prompt_tokens": 9, "completion_tokens": 4}},
    )
    capture_stream(monkeypatch, [_StreamResponse(lines)])

    decision = OpenAIProvider("deepseek-flash", api_key="sk").decide(
        TASK, [], {}, on_delta=lambda *args: None
    )

    assert decision.summary == answer
