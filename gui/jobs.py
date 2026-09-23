"""后台线程 + 队列的渲染模型（U4/U5）。

``st.*`` 只能在脚本线程调用，而真模型一次 run 要 20-45s。所以：

* **analyze / run 都在工作线程里跑**，脚本线程只负责轮询与渲染；
* run 的过程事件来自 ``agent.policy.state.traces``（U1 路径 (1)：该列表在 run 期间
  实时 ``append``，字段与 ``ToolCallRecord`` 同构，零后端改动）；
* 终态（结果或异常）经 ``queue.Queue`` 交回脚本线程，异常永远不吞。

停止按钮走 ``PacketSageAgent.request_finalize()``（U5：已有能力接上，不新增后端）。
"""

from __future__ import annotations

import queue
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from packetsage_agent.engine_client import (
    EngineCrashed,
    EngineError,
    EngineSpawnError,
    RpcToolError,
    ToolTimeout,
)

from .engine import EngineSession, analyze_capture
from .settings import AgentEnv, budget_from

#: 轮询周期：够密（界面看起来是连续的）又够省（每周期只渲染一次）。
POLL_INTERVAL_S = 0.3


@dataclass
class Job:
    """一个后台任务：线程 + 完成事件 + 终态队列。"""

    kind: str
    started_at: float = field(default_factory=time.time)
    done: threading.Event = field(default_factory=threading.Event)
    outcome: queue.Queue = field(default_factory=queue.Queue)
    result: Any = None
    error: BaseException | None = None
    detail: str = ""
    thread: threading.Thread | None = None

    @property
    def elapsed_s(self) -> float:
        return time.time() - self.started_at

    def status(self) -> str:
        """``running`` / ``completed`` / ``failed``（§5.3 状态机的三个态）。"""
        if not self.done.is_set():
            return "running"
        return "failed" if self.error is not None else "completed"

    def _finish(self, kind: str, payload: Any) -> None:
        self.outcome.put((kind, payload))
        self.done.set()


@dataclass
class AnalyzeJob(Job):
    """``analyze_file`` 的一次后台执行。"""

    path: str = ""

    @property
    def task_id(self) -> str:
        if isinstance(self.result, dict):
            return str(self.result.get("task_id") or "")
        return ""

    @property
    def summary(self) -> dict[str, Any]:
        if isinstance(self.result, dict) and isinstance(self.result.get("summary"), dict):
            return self.result["summary"]
        return {}


@dataclass
class RunJob(Job):
    """一次 ``PacketSageAgent.run()`` 的后台执行。"""

    agent: Any = None
    task_id: str = ""
    goal: str = ""
    mode: str = "run"
    stop_requested: bool = False
    #: 已经渲染过的 trace 条数（实时工具卡片逐条出现，S58）。
    rendered: int = 0
    budget: Any = None

    def live_traces(self) -> list[dict[str, Any]]:
        """``policy.state.traces`` 的快照（只读，run 期间实时追加）。"""
        agent = self.agent
        if agent is None:
            return []
        state = getattr(getattr(agent, "policy", None), "state", None)
        traces = getattr(state, "traces", None)
        if not isinstance(traces, list):
            return []
        return list(traces)

    def live_state(self) -> dict[str, Any]:
        """预算条的实时计数（``PolicyState``）。"""
        state = getattr(getattr(self.agent, "policy", None), "state", None)
        if state is None:
            return {}
        return {
            "steps": int(getattr(state, "steps", 0)),
            "llm_calls": int(getattr(state, "llm_calls", 0)),
            "tool_calls": int(getattr(state, "tool_calls", 0)),
            "tokens_in": int(getattr(state, "tokens_in", 0)),
            "tokens_out": int(getattr(state, "tokens_out", 0)),
            "cost_cents": int(getattr(state, "cost_cents", 0)),
            "stop_reason": getattr(state, "stop_reason", None),
        }

    def request_stop(self) -> bool:
        """停止按钮：下一个边界收尾，已提交的 findings 保留（U5/S59）。"""
        agent = self.agent
        if agent is None or self.done.is_set():
            return False
        self.stop_requested = True
        self.detail = "已请求收尾：当前步骤跑完就停，已提交的结论保留"
        request = getattr(agent, "request_finalize", None)
        if callable(request):
            request()
            return True
        return False


def build_agent(
    session: EngineSession,
    env: AgentEnv,
    mode: str = "run",
    history: list[dict[str, Any]] | None = None,
) -> Any:
    """按有效配置装配 agent（provider / model / budget / prompt 版本都从配置来）。

    `history` 是上一轮的真实消息表：追问要接得住"上面已经查过什么"，否则模型只能
    从零再扫一遍抓包（2026-09-23 实测）。
    """
    from packetsage_agent.agent import build_agent as agent_factory

    settings = env.settings
    return agent_factory(
        session.agent_client(),
        provider_kind=settings.provider,
        model=settings.model,
        scenario=settings.scenario,
        budget=budget_from(settings),
        mode=mode,
        prompt_version=settings.prompt_version,
        base_url=settings.base_url,
        api_key=settings.api_key,
        history=history,
    )


def start_analyze(session: EngineSession, path: Path) -> AnalyzeJob:
    """后台跑一次 ``analyze_file``；解析阶段对用户就是"等一会儿 + 已用时间"。"""
    job = AnalyzeJob(kind="analyze", path=str(path))
    job.detail = f"解析 {Path(path).name}"

    def worker() -> None:
        try:
            job.result = analyze_capture(session, Path(path))
        except BaseException as exc:  # noqa: BLE001 - 异常必须回到界面
            job.error = exc
            job._finish("error", exc)
        else:
            job._finish("result", job.result)

    job.thread = threading.Thread(target=worker, name="packetsage-analyze", daemon=True)
    job.thread.start()
    return job


def start_run(
    session: EngineSession,
    env: AgentEnv,
    task_id: str,
    goal: str,
    mode: str = "run",
    history: list[dict[str, Any]] | None = None,
) -> RunJob:
    """后台跑一轮调查；过程事件由脚本线程轮询 ``policy.state.traces`` 取（U1 (1)）。"""
    agent = build_agent(session, env, mode=mode, history=history)
    job = RunJob(
        kind="run",
        agent=agent,
        task_id=task_id,
        goal=goal,
        mode=mode,
        budget=agent.policy.budget,
    )

    def worker() -> None:
        try:
            job.result = agent.run(task_id, goal)
        except BaseException as exc:  # noqa: BLE001 - 异常必须回到界面
            job.error = exc
            job._finish("error", exc)
        else:
            job._finish("result", job.result)

    job.thread = threading.Thread(target=worker, name="packetsage-run", daemon=True)
    job.thread.start()
    return job


def drain(job: Job) -> tuple[str, Any] | None:
    """取一次终态（不阻塞）；没有就返回 ``None``。"""
    try:
        return job.outcome.get_nowait()
    except queue.Empty:
        return None


def error_message(exc: BaseException) -> str:
    """把工作线程里的异常翻成给人看的一段话。"""
    if isinstance(exc, EngineCrashed):
        tail = "\n".join(f"  engine> {line}" for line in exc.stderr_tail(3))
        head = f"引擎中断：{exc}（exit={exc.returncode}）"
        return f"{head}\n{tail}".strip() if tail else head
    if isinstance(exc, ToolTimeout):
        return f"RPC 超时：{exc}"
    if isinstance(exc, RpcToolError):
        return f"引擎拒绝了这次调用：{exc.code} — {exc.message}"
    if isinstance(exc, EngineError):
        return f"{type(exc).__name__}: {exc}"
    return f"{type(exc).__name__}: {exc}"


def error_retryable(exc: BaseException) -> bool:
    """可重建重试（引擎类）还是必须先改配置/参数（§5.4 规矩 2）。"""
    if isinstance(exc, (EngineCrashed, EngineSpawnError, ToolTimeout)):
        return True
    if isinstance(exc, RpcToolError):
        # 引擎答了、但拒绝了这次调用：换参数或换任务，重建引擎没有意义。
        return exc.code in {"NOT_FOUND", "CAPTURE_UNAVAILABLE", "DATABASE_ERROR", "IO"}
    return False
