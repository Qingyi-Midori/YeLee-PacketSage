"""Hard policy gates for one agent run (M3~M6 §4.4).

Everything in this module is a *hard* limit: when a limit is hit the run is
forced to finalize with the evidence collected so far.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass, field
from typing import Any


@dataclass
class AgentBudget:
    """Budget of one run (开发文档 §15.1 + M2v0.2 §9.3)."""

    max_steps: int = 12
    max_llm_calls: int = 24
    max_tool_calls: int = 20
    max_same_tool_calls: int = 5
    max_tokens: int = 200_000
    max_cost_cents: int = 500


@dataclass
class PolicyState:
    """Mutable counters of one run."""

    steps: int = 0
    llm_calls: int = 0
    tool_calls: int = 0
    tokens_in: int = 0
    tokens_out: int = 0
    cost_cents: int = 0
    #: 累计 LLM 往返墙钟毫秒（"先测再改"要的那一列：模型慢还是工具慢）。
    llm_ms: int = 0
    #: 累计工具耗时毫秒（台账 duration_ms 之和，不含 LLM）。
    tool_ms: int = 0
    #: 预取事实用掉的工具调用数（不算模型步数，但用户该看得见）。
    preloaded: int = 0
    #: 输入侧缓存命中/未命中的 tokens（DeepSeek 上下文硬盘缓存，`usage` 里给的；
    #: 别的 provider 不给就一直是 0——不编数字）。
    cache_hit_tokens: int = 0
    cache_miss_tokens: int = 0
    repeated: int = 0
    last_signature: str | None = None
    stop_reason: str | None = None
    malformed_retries: int = 0
    traces: list[dict] = field(default_factory=list)


class PolicyViolation(RuntimeError):
    """Raised when the policy forces the run to stop."""


def call_signature(tool: str, args: dict, result: str) -> str:
    """`(tool, params_hash, result_hash)` used by the homogenisation check."""
    params = json.dumps(args, sort_keys=True, ensure_ascii=False)
    digest = hashlib.sha256(f"{tool}|{params}|{result}".encode()).hexdigest()
    return digest[:32]


class AgentPolicy:
    """Enforces the budget and the homogenisation rule."""

    def __init__(self, budget: AgentBudget | None = None) -> None:
        self.budget = budget or AgentBudget()
        self.state = PolicyState()

    def note_llm_call(
        self,
        tokens_in: int = 0,
        tokens_out: int = 0,
        cost_cents: int = 0,
        cache_hit_tokens: int = 0,
        cache_miss_tokens: int = 0,
    ) -> None:
        """Records one model call and its usage (含缓存命中情况，如果有的话)."""
        self.state.llm_calls += 1
        self.state.tokens_in += tokens_in
        self.state.tokens_out += tokens_out
        self.state.cost_cents += cost_cents
        self.state.cache_hit_tokens += cache_hit_tokens
        self.state.cache_miss_tokens += cache_miss_tokens

    def note_step(self) -> None:
        """Records one agent step."""
        self.state.steps += 1

    def note_llm_time(self, milliseconds: int) -> None:
        """Records how long one model round trip took (wall clock)."""
        self.state.llm_ms += max(0, int(milliseconds))

    def note_tool_time(self, milliseconds: int) -> None:
        """Records how long tool calls took (the engine side of the same run)."""
        self.state.tool_ms += max(0, int(milliseconds))

    def note_preloaded(self, count: int = 1) -> None:
        """Records the deterministic pre-fetch calls made before the first LLM round."""
        self.state.preloaded += max(0, int(count))

    def note_tool_call(self, tool: str, args: dict, result: str) -> bool:
        """Records a tool call; returns False when the loop must stop."""
        self.state.tool_calls += 1
        signature = call_signature(tool, args, result)
        if signature == self.state.last_signature:
            self.state.repeated += 1
        else:
            self.state.repeated = 1
            self.state.last_signature = signature
        if self.state.repeated >= self.budget.max_same_tool_calls:
            self.state.stop_reason = "homogenisation"
            return False
        if self.state.tool_calls >= self.budget.max_tool_calls:
            self.state.stop_reason = "tool_call_budget"
            return False
        return True

    def can_continue(self) -> bool:
        """True while every budget still has room."""
        if self.state.stop_reason is not None:
            return False
        if self.state.steps >= self.budget.max_steps:
            self.state.stop_reason = "step_budget"
            return False
        if self.state.llm_calls >= self.budget.max_llm_calls:
            self.state.stop_reason = "llm_call_budget"
            return False
        if self.state.tokens_in + self.state.tokens_out >= self.budget.max_tokens:
            self.state.stop_reason = "token_budget"
            return False
        if self.state.cost_cents >= self.budget.max_cost_cents:
            self.state.stop_reason = "cost_budget"
            return False
        return True


def budget_from_settings(effective: Any) -> AgentBudget:
    """``Settings`` → ``AgentBudget`` (Agent CLI 工程规格书 §9).

    One definition for both callers (CLI `run`/`chat` and the desktop sidecar):
    the budget the GUI paints in `run_started.budget` is the budget the loop
    actually enforces.
    """
    return AgentBudget(
        max_steps=effective.max_steps,
        max_llm_calls=effective.max_llm_calls,
        max_tool_calls=effective.max_tool_calls,
        max_same_tool_calls=effective.max_same_tool_calls,
        max_tokens=effective.max_tokens,
        max_cost_cents=effective.max_cost_cents,
    )
