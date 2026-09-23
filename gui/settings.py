"""Agent 侧配置与门禁（U6/U7）。

界面不自己解析 ``packetsage.yaml``，也不自己找 key：它调用 agent 自己的
``config`` 链（``.env`` → 环境变量 → 文件 → 默认值），拿到与 CLI 完全相同的一份
``Settings``。这样"界面能不能跑 run"与"CLI 能不能跑 run"永远是同一个答案
（U6：同一信息一套解析）。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from packetsage_agent import config as agent_config
from packetsage_agent.policy import AgentBudget

#: 未配置 provider 时给用户的唯一动作（U7/S60：不静默 mock）。
SETUP_COMMAND = "packetsage-agent setup"


@dataclass
class AgentEnv:
    """一次读取的结果：``loaded``/``settings`` 为 ``None`` 时看 ``error``。"""

    loaded: Any = None
    settings: Any = None
    error: str = ""
    #: provider 未配置的原因（``None`` = 可以开跑）。
    blocked: str | None = None

    @property
    def ok(self) -> bool:
        return self.settings is not None and not self.error

    @property
    def provider(self) -> str:
        return str(getattr(self.settings, "provider", "") or "(unset)")

    @property
    def model(self) -> str:
        return str(getattr(self.settings, "model", "") or "-")

    @property
    def prompt_version(self) -> str:
        return str(getattr(self.settings, "prompt_version", "") or "-")


def load_agent_env() -> AgentEnv:
    """读一次有效配置；配置坏了也返回可读原因，不抛给 Streamlit。"""
    try:
        # `.env` 只被 Python 侧读（§7.1），且环境变量优先，所以这一步幂等。
        agent_config.load_dotenv()
        loaded = agent_config.load(None)
        effective = agent_config.settings(loaded)
    except agent_config.ConfigError as exc:
        return AgentEnv(error=str(exc))
    return AgentEnv(
        loaded=loaded,
        settings=effective,
        blocked=agent_config.provider_unavailable(effective),
    )


def budget_from(effective: Any) -> AgentBudget:
    """``Settings`` → ``AgentBudget``（与 CLI 的 ``_budget`` 逐字段一致）。"""
    return AgentBudget(
        max_steps=effective.max_steps,
        max_llm_calls=effective.max_llm_calls,
        max_tool_calls=effective.max_tool_calls,
        max_same_tool_calls=effective.max_same_tool_calls,
        max_tokens=effective.max_tokens,
        max_cost_cents=effective.max_cost_cents,
    )


def provider_blocked(env: AgentEnv, doctor_provider_status: str = "") -> str | None:
    """provider 门禁：Python 侧的事实 + ``doctor`` 那一项的状态。

    ``doctor`` 的 ``provider`` 项在 ``--no-net`` 下对"已配 key 的 openai/deepseek"会
    报 ``skipped``（网络探测被跳过），那不是阻塞项；``warn`` / ``fail`` 才是。
    """
    if not env.ok:
        return env.error or "配置读取失败"
    if doctor_provider_status in {"warn", "fail"}:
        return env.blocked or f"packetsage doctor 的 provider 项为 {doctor_provider_status}"
    return env.blocked
