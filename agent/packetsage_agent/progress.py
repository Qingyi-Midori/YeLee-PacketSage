"""Progress and usage lines (Agent CLI 工程规格书 §4/§5/§7).

Three stream discipline: everything here goes to **stderr**, is printed only
when stderr is a terminal, and disappears under ``-q``. stdout stays reserved
for the product (run summary, REPL, report line).
"""

from __future__ import annotations

import sys
from typing import Any, TextIO

from .policy import AgentBudget, PolicyState


def format_tokens(total: int) -> str:
    """``4200`` → ``4.2k``; counts below 1000 stay exact."""
    if total < 1000:
        return str(total)
    return f"{total / 1000:.1f}k".replace(".0k", "k")


def format_cost(cost_cents: int, max_cost_cents: int) -> str:
    """Renders the budget in the notation of §4 (``3.1¢/50¢``).

    The engine prices a run in ``cost_cents``; the progress line shows ten of
    those units as one cent so the default budget of 500 reads ``50¢``.
    """
    return f"{cost_cents / 10:.1f}\u00a2/{max_cost_cents / 10:.0f}\u00a2"


def usage_line(state: PolicyState, budget: AgentBudget) -> str:
    """One budget line: ``steps 3/12 · calls 5/24 · 4.2k tok · 3.1¢/50¢``.

    输入侧有缓存命中数据时（DeepSeek 上下文硬盘缓存）在末尾补一段
    ``· cache 62%``——它是这家 API 里**最直接影响费用与首字延迟**的数字。
    provider 不报就不显示（不是"命中 0%"，是"没有这个数据"）。
    """
    line = (
        f"steps {state.steps}/{budget.max_steps} \u00b7 "
        f"calls {state.llm_calls}/{budget.max_llm_calls} \u00b7 "
        f"{format_tokens(state.tokens_in + state.tokens_out)} tok \u00b7 "
        f"{format_cost(state.cost_cents, budget.max_cost_cents)}"
    )
    cached = state.cache_hit_tokens + state.cache_miss_tokens
    if cached > 0:
        percent = round(state.cache_hit_tokens * 100 / cached)
        line += f" \u00b7 cache {percent}%"
    return line


def tool_line(step: int, tool: str, ok: bool) -> str:
    """One finished tool call: ``#3 check_alerts ok``."""
    return f"#{step} {tool} {'ok' if ok else 'err'}"


class Progress:
    """stderr progress channel of one run/session."""

    def __init__(
        self,
        stream: TextIO | None = None,
        quiet: bool = False,
        enabled: bool | None = None,
    ) -> None:
        self.stream = stream if stream is not None else sys.stderr
        self.quiet = quiet
        if enabled is None:
            enabled = not quiet and _is_tty(self.stream)
        self.enabled = enabled

    def write(self, line: str) -> None:
        if not self.enabled:
            return
        print(line, file=self.stream)
        try:
            self.stream.flush()
        except (AttributeError, ValueError):  # pragma: no cover - closed stream
            pass

    # ------------------------------------------------------- observer hooks
    def tool_call(self, step: int, tool: str, ok: bool) -> None:
        self.write(tool_line(step, tool, ok))

    def llm_round(self, state: PolicyState, budget: AgentBudget) -> None:
        self.write(usage_line(state, budget))


def _is_tty(stream: Any) -> bool:
    try:
        return bool(stream.isatty())
    except (AttributeError, ValueError):
        return False
