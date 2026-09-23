"""``packetsage-agent serve`` — the desktop Agent sidecar (通道 B).

《GUI 工程规格书 v0.2》§4.5 把这条协议定死。它只有一条铁律：

* **stdin 收命令、stdout 发 ack 与事件、stderr 只作诊断**；一帧 = 一行 JSON
  （UTF-8、``\\n`` 结尾、不允许内嵌裸换行、行内不得有 ANSI）。

```text
GUI  → {"id":"<uuid4>","type":"cmd","command":"run","params":{...}}
Agent→ {"id":"<uuid4>","type":"ack","ok":true,"result":{...}}
Agent→ {"type":"event","event":"tool_call_finished","run_id":"…","seq":7,
        "ts_unix_ms":1758342000123,"data":{…}}
```

几个必须记住的约定：

1. 第一帧必须是 ``hello``；``protocol_version`` 不等就回
   ``PROTOCOL_MISMATCH`` 并**拒绝后续命令**（不静默降级，S57/S70）。
2. ``seq`` 在**一次 run 内**从 1 起单调递增、无缺口；不属于任何 run 的事件
   （``welcome`` / ``report_written``）用空的 ``run_id`` 与进程级计数，两套规则不混。
3. 单条事件 ≤ 8 KiB，超限就地截断并加 ``"truncated": true``（S62）。
4. ``run_finished`` 与 ``packetsage-agent run --json`` **同构**（收口 §5.11），
   由 :func:`packetsage_agent.payloads.run_json_payload` 保证——GUI 不解析 CLI 文本。
5. ``cancel`` 只表示"已受理"：真正的停止是下一个边界收尾，界面在
   ``run_finished`` 之前必须显示"正在收尾…"（§4.7）。
6. ``shutdown`` 是命令表里的一条（P6）：ack 之后循环结束，收尾走 ``finally``；
   一行有上限（P9），超长帧就地丢弃并回 ``PROTOCOL_MISMATCH``。

错误码取自 §4.8；侧车另加一个 ``ANTI_HALLUCINATION``（报告硬失败，retryable=false），
因为报告的反幻觉门禁是**不可重试**的语义，混进 ``INTERNAL`` 会让界面给错按钮。
"""

from __future__ import annotations

import json
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, TextIO

from ._version import package_version
from .agent import build_agent
from .config import provider_unavailable
from .engine_client import (
    CallResult,
    EngineClient,
    EngineCrashed,
    EngineError,
    EngineSpawnError,
    RpcToolError,
    ToolTimeout,
)
from .payloads import SUMMARY_VERSION, run_json_payload
from .policy import budget_from_settings

#: 协议版本；字段只允许增加，改名或改语义必须 +1 并重做快照（§4.3 规则 2 / S70）。
PROTOCOL_VERSION = 1

#: 单条事件的上限（§4.5 规则 2）。
EVENT_MAX_BYTES = 8 * 1024

#: 单条**命令帧**的上限（P9：一行不再无上限）。事件是 8 KiB，命令更宽，
#: 但 4 MiB 已经远超任何合法帧（goal 是唯一的大字段），超过就是坏帧。
FRAME_MAX_CHARS = 4 * 1024 * 1024

#: 握手声明的能力集；GUI 不得调用未声明的能力（§4.3 规则 3）。
CAPABILITIES: tuple[str, ...] = ("run", "chat", "report", "cancel", "stream_events")

#: 引擎 schema 版本（与 `packetsage schema` 的 schema_version 对齐）；ping 拿不到
#: 时用它兜底，这样握手里的 schema_version 永远是数字。
DEFAULT_SCHEMA_VERSION = 2

#: 一次 run 的默认目标（与 CLI 的默认 goal 一致）。
DEFAULT_GOAL = "分析该捕获并给出可追溯结论"

#: `shutdown` 时等待正在收尾的 run 的上限（§4.9：命令级超时）。
SHUTDOWN_GRACE_S = 10.0

#: 报告生成的命令级超时预算（§4.9：`report 180s`）；报告内部走 RpcTimeouts。
REPORT_TIMEOUT_S = 180.0


class ProtocolError(RuntimeError):
    """一条命令失败，映射到 §4.8 的 ``error`` 对象。"""

    def __init__(
        self,
        code: str,
        message: str,
        *,
        retryable: bool = False,
        where: str = "sidecar",
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.retryable = retryable
        self.where = where

    def as_dict(self) -> dict[str, Any]:
        return {
            "code": self.code,
            "message": self.message,
            "retryable": self.retryable,
            "where": self.where,
        }


def classify_error(exc: BaseException) -> ProtocolError:
    """异常 → §4.8 的错误码（界面据此决定"重试"还是"去配 provider"）。"""
    if isinstance(exc, ProtocolError):
        return exc
    if isinstance(exc, EngineSpawnError):
        return ProtocolError(
            "ENGINE_SPAWN", str(exc.cause or exc), retryable=True, where="engine"
        )
    if isinstance(exc, EngineCrashed):
        return ProtocolError("ENGINE_CRASHED", str(exc), retryable=True, where="engine")
    if isinstance(exc, ToolTimeout):
        return ProtocolError("TOOL_TIMEOUT", str(exc), retryable=True, where="engine")
    if isinstance(exc, RpcToolError):
        code = str(exc.code).upper()
        if code in {"NOT_FOUND", "CAPTURE_UNAVAILABLE"}:
            return ProtocolError("TASK_NOT_FOUND", str(exc), retryable=False, where="engine")
        return ProtocolError("INTERNAL", str(exc))
    message = str(exc)
    if isinstance(exc, RuntimeError) and "no API key" in message:
        return ProtocolError("CONFIG", message, retryable=False, where="provider")
    if _looks_like_network(exc):
        return ProtocolError(
            "PROVIDER_UNREACHABLE",
            f"{type(exc).__name__}: {message}",
            retryable=True,
            where="provider",
        )
    if isinstance(exc, EngineError):
        return ProtocolError("INTERNAL", message, retryable=True, where="engine")
    return ProtocolError("INTERNAL", f"{type(exc).__name__}: {message}", retryable=True)


def _looks_like_network(exc: BaseException) -> bool:
    """httpx / 网络层异常（不 import httpx 也能判：看模块名与内建异常）。"""
    module = type(exc).__module__ or ""
    if module.startswith(("httpx", "httpcore")):
        return True
    return isinstance(exc, (ConnectionError, TimeoutError))


@dataclass
class ActiveRun:
    """一次进行中的 run：线程 + 序号 + 终态。"""

    run_id: str
    task_id: str
    mode: str
    goal: str
    agent: Any
    seq: int = 0
    done: threading.Event = field(default_factory=threading.Event)
    result: dict[str, Any] | None = None
    started_at: float = field(default_factory=time.time)
    cancel_requested: bool = False
    thread: threading.Thread | None = None


class LockedClient:
    """把 ``EngineClient`` 包成"同一时刻只有一个写者"。

    侧车有两类 RPC 发起方：run 线程（agent 的工具调用）与主线程（``status``、
    ``report``、任务存在性检查）。``EngineClient`` 把请求行写进同一个 stdin
    管道，所以两边必须共用一把锁——结论与原型 ``gui/engine.py`` 一致。
    """

    def __init__(self, client: EngineClient, lock: threading.RLock) -> None:
        self._client = client
        self._lock = lock

    def call(self, method: str, params: dict[str, Any], timeout_s: float | None = None) -> Any:
        with self._lock:
            return self._client.call(method, params, timeout_s)

    def call_envelope(self, method: str, params: dict[str, Any], step: int = 0) -> CallResult:
        with self._lock:
            return self._client.call_envelope(method, params, step)

    def __getattr__(self, name: str) -> Any:
        return getattr(self._client, name)


class EventSink:
    """agent 的过程钩子 → §4.5 事件（CLI 的 ``Progress`` 不受影响）。"""

    def __init__(self, sidecar: Sidecar, run: ActiveRun) -> None:
        self._sidecar = sidecar
        self._run = run

    def llm_round(self, state: Any, budget: Any) -> None:
        self._sidecar.emit(
            "llm_round",
            {
                "step": int(getattr(state, "steps", 0)),
                "llm_calls": int(getattr(state, "llm_calls", 0)),
                "tool_calls": int(getattr(state, "tool_calls", 0)),
                "tokens_in": int(getattr(state, "tokens_in", 0)),
                "tokens_out": int(getattr(state, "tokens_out", 0)),
                "cost_cents": int(getattr(state, "cost_cents", 0)),
                # 性能改造 #1：模型慢还是工具慢，界面和报告分得开。
                "llm_ms": int(getattr(state, "llm_ms", 0)),
                "tool_ms": int(getattr(state, "tool_ms", 0)),
                "preloaded": int(getattr(state, "preloaded", 0)),
                # 输入侧缓存命中情况（DeepSeek 上下文硬盘缓存）：没有数据就是 0，
                # 界面用"有没有这两个数"区分"这个 provider 不报"与"全没命中"。
                "cache_hit_tokens": int(getattr(state, "cache_hit_tokens", 0)),
                "cache_miss_tokens": int(getattr(state, "cache_miss_tokens", 0)),
            },
            self._run,
        )

    def tool_call_started(self, step: int, tool: str, args: Any) -> None:
        self._sidecar.emit(
            "tool_call_started",
            {
                "step": int(step),
                "tool_name": str(tool),
                "args": args if isinstance(args, dict) else {},
            },
            self._run,
        )

    def llm_delta(
        self,
        step: int,
        channel: str,
        text: str,
        tool_name: str | None = None,
        tool_index: int | None = None,
    ) -> None:
        """token 级流式的一段（§4.5）：合并后的碎块，不是单个 token。

        这是**装饰帧**：丢了不影响任何结论、证据与预算，所以外壳的队列满时
        优先丢它（`jsonl.rs` 的 EventQueue）。
        """
        self._sidecar.emit(
            "llm_delta",
            {
                "step": int(step),
                "channel": str(channel),
                "text": str(text),
                "tool_name": tool_name,
                "tool_index": tool_index,
            },
            self._run,
        )

    def tool_call_finished(self, record: Any) -> None:
        self._sidecar.emit(
            "tool_call_finished",
            {
                "step": int(getattr(record, "step", 0)),
                "tool_name": str(getattr(record, "tool_name", "")),
                "args": _parse_args(getattr(record, "args_json", "{}")),
                "status": str(getattr(record, "status", "")),
                "duration_ms": int(getattr(record, "duration_ms", 0)),
                "tc_id": getattr(record, "tc_id", None),
                "result_summary": str(getattr(record, "result_summary", "")),
            },
            self._run,
        )

    def finding_accepted(self, row: dict[str, Any]) -> None:
        self._sidecar.emit("finding_accepted", dict(row), self._run)

    def finding_rejected(self, title: str, reason_code: str, reason: str) -> None:
        self._sidecar.emit(
            "finding_rejected",
            {"title": str(title), "reason_code": str(reason_code), "reason": str(reason)},
            self._run,
        )


def _parse_args(raw: Any) -> Any:
    """``args_json`` → 对象；解析不了就原样给字符串（不编造结构）。"""
    if isinstance(raw, (dict, list)):
        return raw
    try:
        return json.loads(str(raw))
    except (TypeError, ValueError):
        return str(raw)


class Sidecar:
    """命令循环 + 事件流；一个进程一个实例。"""

    def __init__(
        self,
        client: EngineClient,
        settings: Any,
        *,
        out: TextIO | None = None,
        err: TextIO | None = None,
    ) -> None:
        self._engine = client
        self._settings = settings
        self._out = out if out is not None else sys.stdout
        self._err = err if err is not None else sys.stderr
        self._rpc_lock = threading.RLock()
        self._lock = threading.RLock()
        self._client = LockedClient(client, self._rpc_lock)
        self._welcome: dict[str, Any] | None = None
        self._fatal: ProtocolError | None = None
        self._active: ActiveRun | None = None
        self._session_seq = 0
        self._last_run_id = ""
        self._schema_version = DEFAULT_SCHEMA_VERSION

    # ------------------------------------------------------------- 输出
    @property
    def protocol_version(self) -> int:
        return PROTOCOL_VERSION

    def ack(self, request_id: str, result: dict[str, Any] | None = None) -> None:
        self._write({"id": request_id, "type": "ack", "ok": True, "result": result or {}})

    def ack_error(self, request_id: str, error: ProtocolError) -> None:
        self._write({"id": request_id, "type": "ack", "ok": False, "error": error.as_dict()})

    def _write(self, payload: dict[str, Any]) -> None:
        with self._lock:
            try:
                line = json.dumps(payload, ensure_ascii=False)
            except (TypeError, ValueError):  # pragma: no cover - 载荷都是 JSON 原生类型
                return
            self._out.write(line + "\n")
            self._out.flush()

    def emit(self, event: str, data: Any, run: ActiveRun | None = None) -> None:
        """写一条事件；``seq`` 的分配与写入在同一把锁里（不会跳号/乱序）。"""
        with self._lock:
            if run is not None:
                run.seq += 1
                seq, run_id = run.seq, run.run_id
            else:
                self._session_seq += 1
                seq, run_id = self._session_seq, ""
            payload = _fit_event(
                {
                    "type": "event",
                    "event": event,
                    "run_id": run_id,
                    "seq": seq,
                    "ts_unix_ms": int(time.time() * 1000),
                    "data": data,
                }
            )
            if event == "run_finished" and run is not None:
                self._last_run_id = run.run_id
            try:
                line = json.dumps(payload, ensure_ascii=False)
            except (TypeError, ValueError):  # pragma: no cover - 载荷已是 JSON 原生
                line = json.dumps(
                    {
                        "type": "event",
                        "event": event,
                        "run_id": run_id,
                        "seq": seq,
                        "ts_unix_ms": int(time.time() * 1000),
                        "data": {"note": "payload could not be serialised"},
                        "truncated": True,
                    },
                    ensure_ascii=False,
                )
            self._out.write(line + "\n")
            self._out.flush()

    # ------------------------------------------------------------- 握手
    def welcome(self) -> dict[str, Any]:
        """§4.3 的 ``welcome`` 载荷（``hello`` 的 ack 与 ``welcome`` 事件共用）。"""
        if self._welcome is None:
            self._welcome = {
                "protocol_version": PROTOCOL_VERSION,
                "agent_version": package_version(),
                "schema_version": self._schema_version,
                "capabilities": list(CAPABILITIES),
                "providers": {
                    "kind": str(getattr(self._settings, "provider", "") or ""),
                    "model": str(getattr(self._settings, "model", "") or ""),
                    "configured": self.provider_ready(),
                },
            }
        return dict(self._welcome)

    def provider_ready(self) -> bool:
        """provider 能不能开跑（未配置 = 引导首次运行向导，不静默 mock）。"""
        return provider_unavailable(self._settings) is None

    def provider_error(self) -> ProtocolError:
        reason = provider_unavailable(self._settings) or "provider is not configured"
        return ProtocolError("CONFIG", reason, retryable=False, where="provider")

    def ping_engine(self) -> None:
        """启动时探一次引擎：握手里的 ``schema_version`` 取真值（拿不到就兜底）。"""
        try:
            result = self._client.call("ping", {}, 5.0)
        except EngineError:
            return
        if isinstance(result, dict) and isinstance(result.get("schema_version"), int):
            self._schema_version = int(result["schema_version"])

    # ------------------------------------------------------------- 命令
    def handle_line(self, line: str) -> bool:
        """处理一帧；返回 ``False`` 表示循环应当结束（``shutdown``）。"""
        text = line.strip()
        if not text:
            return True
        try:
            request = json.loads(text)
        except ValueError as exc:
            self.ack_error(
                "",
                ProtocolError(
                    "PROTOCOL_MISMATCH", f"frame is not JSON: {exc}", where="protocol"
                ),
            )
            return True
        if not isinstance(request, dict):
            self.ack_error(
                "",
                ProtocolError(
                    "PROTOCOL_MISMATCH",
                    "a command frame must be a JSON object",
                    where="protocol",
                ),
            )
            return True
        request_id = str(request.get("id") or "")
        if request.get("type") != "cmd":
            self.ack_error(
                request_id,
                ProtocolError(
                    "PROTOCOL_MISMATCH", "only type=cmd frames are accepted", where="protocol"
                ),
            )
            return True
        command = str(request.get("command") or "")
        params = request.get("params")
        if not isinstance(params, dict):
            params = {}
        # `hello` opens the session and `shutdown` closes the process: both are
        # lifecycle, so neither is blocked by a fatal handshake failure (P6).
        if self._fatal is not None and command not in {"hello", "shutdown"}:
            self.ack_error(request_id, self._fatal)
            return True
        handler = {
            "hello": self._cmd_hello,
            "run": self._cmd_run,
            "chat": self._cmd_chat,
            "report": self._cmd_report,
            "cancel": self._cmd_cancel,
            "status": self._cmd_status,
            # P6: `shutdown` belongs to the handler table, not to `serve_forever`.
            # A host that embeds `Sidecar` and calls `handle_line` directly gets
            # the same ack + "stop the loop" contract the CLI path has.
            "shutdown": self._cmd_shutdown,
        }.get(command)
        if handler is None:
            self.ack_error(
                request_id,
                ProtocolError(
                    "PROTOCOL_MISMATCH",
                    f"unknown command {command!r}; expected one of {', '.join(_COMMANDS)}",
                    where="protocol",
                ),
            )
            return True
        if self._welcome is None and command not in {"hello", "shutdown"}:
            self.ack_error(
                request_id,
                ProtocolError(
                    "PROTOCOL_MISMATCH",
                    "the first frame must be `hello`",
                    where="protocol",
                ),
            )
            return True
        try:
            return bool(handler(request_id, params))
        except ProtocolError as exc:
            self.ack_error(request_id, exc)
        except BaseException as exc:  # noqa: BLE001 - 一条命令的失败不能带崩侧车
            self._warn(f"{type(exc).__name__}: {exc}")
            self.ack_error(request_id, classify_error(exc))
        return True

    def _warn(self, message: str) -> None:
        """stderr 只作诊断（§4.9）；界面永不解析它。"""
        try:
            print(f"packetsage-agent: {message}", file=self._err)
        except Exception:  # pragma: no cover - 诊断流坏了不影响协议
            pass

    # ---------------------------------------------------------- hello
    def _cmd_hello(self, request_id: str, params: dict[str, Any]) -> bool:
        version = params.get("protocol_version")
        if version != PROTOCOL_VERSION:
            mismatch = ProtocolError(
                "PROTOCOL_MISMATCH",
                f"protocol_version {version!r} is not supported (this sidecar speaks "
                f"{PROTOCOL_VERSION})",
                retryable=False,
                where="protocol",
            )
            self._fatal = mismatch
            self.ack_error(request_id, mismatch)
            return True
        payload = self.welcome()
        self.ack(request_id, payload)
        self.emit("welcome", payload)
        return True

    # ------------------------------------------------------------ run
    def _cmd_run(self, request_id: str, params: dict[str, Any]) -> bool:
        return self._start_run(request_id, params, mode="run")

    def _cmd_chat(self, request_id: str, params: dict[str, Any]) -> bool:
        return self._start_run(request_id, params, mode="chat")

    def _start_run(self, request_id: str, params: dict[str, Any], mode: str) -> bool:
        if not self.provider_ready():
            self.ack_error(request_id, self.provider_error())
            return True
        active = self._active
        if active is not None and not active.done.is_set():
            self.ack_error(
                request_id,
                ProtocolError(
                    "BUSY",
                    f"run {active.run_id} is still going; wait for run_finished",
                    where="sidecar",
                ),
            )
            return True
        task_id = str(params.get("task_id") or "")
        goal = str(
            params.get("goal")
            or params.get("question")
            or (DEFAULT_GOAL if mode == "run" else "")
        )
        if not task_id or not goal:
            raise ProtocolError(
                "INTERNAL",
                "run/chat needs `task_id` and a goal/question",
                where="sidecar",
            )
        self._require_task(task_id)
        agent = build_agent(
            self._client,
            provider_kind=self._settings.provider,
            model=self._settings.model,
            scenario=self._settings.scenario,
            budget=budget_from_settings(self._settings),
            mode=mode,
            prompt_version=params.get("prompt_version") or self._settings.prompt_version,
            base_url=self._settings.base_url,
            api_key=self._settings.api_key,
        )
        run = ActiveRun(
            run_id=agent.agent_run_id, task_id=task_id, mode=mode, goal=goal, agent=agent
        )
        agent.observer = EventSink(self, run)
        with self._lock:
            self._active = run
        self.ack(request_id, {"run_id": run.run_id})
        budget = agent.policy.budget
        self.emit(
            "run_started",
            {
                "run_id": run.run_id,
                "task_id": task_id,
                "mode": mode,
                "model": str(agent.model_name),
                "provider": str(agent.provider_kind),
                "prompt_version": str(agent.prompt_version),
                "budget": _budget_dict(budget),
            },
            run,
        )
        run.thread = threading.Thread(
            target=self._runner, args=(run,), name="agent-run", daemon=True
        )
        run.thread.start()
        return True

    def _runner(self, run: ActiveRun) -> None:
        try:
            result = run.agent.run(run.task_id, run.goal)
        except BaseException as exc:  # noqa: BLE001 - 终态必须回到 GUI
            error = classify_error(exc)
            self._warn(f"run {run.run_id} failed: {error.code}: {error.message}")
            self.emit("error", error.as_dict(), run)
            payload = _failure_payload(run, error)
        else:
            payload = run_json_payload(result, run.agent)
        run.result = payload
        self.emit("run_finished", payload, run)
        with self._lock:
            if self._active is run:
                self._active = None
        run.done.set()

    # ----------------------------------------------------------- cancel
    def _cmd_cancel(self, request_id: str, params: dict[str, Any]) -> bool:
        run_id = str(params.get("run_id") or "")
        active = self._active
        if active is not None and not active.done.is_set() and active.run_id == run_id:
            active.cancel_requested = True
            request = getattr(active.agent, "request_finalize", None)
            if callable(request):
                request()
            self._warn(f"cancel accepted for {run_id}: finalizing at the next boundary")
        self.ack(request_id, {})
        return True

    # --------------------------------------------------------- shutdown
    def _cmd_shutdown(self, request_id: str, params: dict[str, Any]) -> bool:
        """ack + 结束循环；真正的收尾在 :meth:`shutdown`（``serve_forever`` 的 finally）。"""
        self.ack(request_id, {})
        return False

    # ----------------------------------------------------------- status
    def _cmd_status(self, request_id: str, params: dict[str, Any]) -> bool:
        active = self._active
        running = active is not None and not active.done.is_set()
        state = None
        if running and active is not None:
            state = getattr(getattr(active.agent, "policy", None), "state", None)
        run_info: dict[str, Any] | None = None
        if running and active is not None and state is not None:
            run_info = {
                "run_id": active.run_id,
                "task_id": active.task_id,
                "mode": active.mode,
                "steps": int(getattr(state, "steps", 0)),
                "llm_calls": int(getattr(state, "llm_calls", 0)),
                "tool_calls": int(getattr(state, "tool_calls", 0)),
                "tokens_in": int(getattr(state, "tokens_in", 0)),
                "tokens_out": int(getattr(state, "tokens_out", 0)),
                "cost_cents": int(getattr(state, "cost_cents", 0)),
                "cancel_requested": bool(active.cancel_requested) if active else False,
            }
        self.ack(
            request_id,
            {
                "engine_alive": self.engine_alive(),
                "run": run_info,
                "budget": None if active is None else _budget_dict(active.agent.policy.budget),
            },
        )
        return True

    def engine_alive(self) -> bool:
        """引擎还活着吗；run 期间只做进程检查（避免与 run 抢 stdin 管道）。"""
        process = getattr(self._engine, "_process", None)
        if process is None or process.poll() is not None:
            return False
        if bool(getattr(self._engine, "unhealthy", False)):
            return False
        active = self._active
        if active is not None and not active.done.is_set():
            return True
        try:
            return bool(self._engine.heartbeat())
        except Exception:  # pragma: no cover - 探测不该抛
            return False

    # ----------------------------------------------------------- report
    def _cmd_report(self, request_id: str, params: dict[str, Any]) -> bool:
        from .report import AntiHallucinationError, ReportGenerator

        task_id = str(params.get("task_id") or "")
        if not task_id:
            raise ProtocolError("INTERNAL", "report needs `task_id`", where="sidecar")
        self._require_task(task_id)
        out_path = Path(str(params.get("out_path") or f"report-{task_id}.md"))
        artifacts = self._content("get_task_artifacts", {"task_id": task_id})
        meta = artifacts.get("report_meta") if isinstance(artifacts, dict) else None
        meta = meta if isinstance(meta, dict) else {}
        generator = ReportGenerator(
            self._client,
            task_id,
            str(meta.get("agent_run_id") or self._last_run_id),
            model=str(meta.get("model") or getattr(self._settings, "model", "") or "unknown"),
            provider=str(
                meta.get("provider") or getattr(self._settings, "provider", "") or "unknown"
            ),
            temperature=float(meta.get("temperature") or 0.0),
            tokens_in=int(meta.get("tokens_in") or 0),
            tokens_out=int(meta.get("tokens_out") or 0),
            cost_cents=int(meta.get("cost_cents") or 0),
            prompt_version=str(
                meta.get("prompt_version") or getattr(self._settings, "prompt_version", "") or ""
            ),
        )
        try:
            written = generator.generate(out_path)
        except AntiHallucinationError as exc:
            raise ProtocolError(
                "ANTI_HALLUCINATION", str(exc), retryable=False, where="report"
            ) from exc
        result = {
            "report_path": written.path,
            "sha256": written.sha256,
            "degraded": bool(written.degraded),
            "unverified_items": int(written.unverified_count),
            "status": str(written.status),
            "template_version": str(written.template_version),
            "prompt_version": str(written.prompt_version),
        }
        self.ack(request_id, result)
        self.emit("report_written", result)
        return True

    # ------------------------------------------------------------ 收尾
    def shutdown(self) -> None:
        """优雅退出：让正在收尾的 run 写完 ``run_finished``，再关引擎。"""
        active = self._active
        if active is not None and not active.done.is_set():
            request = getattr(active.agent, "request_finalize", None)
            if callable(request):
                request()
            active.done.wait(SHUTDOWN_GRACE_S)
        try:
            self._engine.close()
        except Exception:  # pragma: no cover - 关闭失败不影响退出
            pass

    # ------------------------------------------------------------ 工具
    def _content(self, method: str, params: dict[str, Any]) -> Any:
        """一次只读 RPC；方法自带裸结果时原样返回。"""
        result = self._client.call(method, params)
        if isinstance(result, dict) and "trusted_as_instruction" in result:
            return result.get("content")
        return result

    def _require_task(self, task_id: str) -> None:
        """task 不在就当场回 ``TASK_NOT_FOUND``（§4.8：提示重新分析）。"""
        if not task_id:
            raise ProtocolError("TASK_NOT_FOUND", "no task_id given", where="sidecar")
        try:
            self._content("get_capture_summary", {"task_id": task_id})
        except RpcToolError as exc:
            code = str(exc.code).replace("_", "").replace("-", "").upper()
            if code in {"NOTFOUND", "CAPTUREUNAVAILABLE"}:
                raise ProtocolError(
                    "TASK_NOT_FOUND", str(exc), retryable=False, where="engine"
                ) from exc
            raise classify_error(exc) from exc
        except EngineError as exc:
            raise classify_error(exc) from exc


#: §4.5 的命令表（`hello` 之外都要先握手）。
_COMMANDS: tuple[str, ...] = (
    "hello",
    "run",
    "chat",
    "report",
    "cancel",
    "status",
    "shutdown",
)


def _budget_dict(budget: Any) -> dict[str, Any]:
    return {
        "max_steps": int(budget.max_steps),
        "max_llm_calls": int(budget.max_llm_calls),
        "max_tool_calls": int(budget.max_tool_calls),
        "max_tokens": int(budget.max_tokens),
        "max_cost_cents": int(budget.max_cost_cents),
    }


def _failure_payload(run: ActiveRun, error: ProtocolError) -> dict[str, Any]:
    """失败 run 的 ``run_finished``：与 §5.11 同构 + 一个 additive 的 ``error``。"""
    state = run.agent.policy.state
    return {
        "summary_version": SUMMARY_VERSION,
        "run_id": run.run_id,
        "task_id": run.task_id,
        "status": "failed",
        "stop_reason": None,
        "accepted": 0,
        "submit_rejects": 0,
        "malformed_output": False,
        "steps": int(state.steps),
        "calls": int(state.llm_calls),
        "tool_calls": int(state.tool_calls),
        "tokens": {
            "in": int(state.tokens_in),
            "out": int(state.tokens_out),
            "total": int(state.tokens_in + state.tokens_out),
        },
        "cost_cents": int(state.cost_cents),
        "prompt_version": str(run.agent.prompt_version),
        "model": str(run.agent.model_name),
        "provider": str(run.agent.provider_kind),
        "findings": [],
        "error": error.as_dict(),
    }


def _event_size(payload: dict[str, Any]) -> int:
    return len(json.dumps(payload, ensure_ascii=False).encode("utf-8"))


def _clip(text: str, limit: int) -> str:
    return text if len(text) <= limit else text[:limit] + "…"


def _fit_event(payload: dict[str, Any]) -> dict[str, Any]:
    """把事件压进 8 KiB（§4.5 规则 2）：先截长文本，再兜底换一个说明体。"""
    if _event_size(payload) <= EVENT_MAX_BYTES:
        return payload
    data = payload.get("data")
    if isinstance(data, dict):
        fitted = dict(data)
        for key in ("result_summary", "reason", "message", "note"):
            value = fitted.get(key)
            if isinstance(value, str):
                fitted[key] = _clip(value, 512)
        args = fitted.get("args")
        if isinstance(args, (dict, list)):
            rendered = json.dumps(args, ensure_ascii=False)
            if len(rendered) > 512:
                fitted["args"] = _clip(rendered, 512)
        payload = {**payload, "data": fitted, "truncated": True}
        if _event_size(payload) <= EVENT_MAX_BYTES:
            return payload
    return {
        **{key: value for key, value in payload.items() if key != "data"},
        "data": {"note": "event dropped: it could not fit into 8 KiB", "truncated": True},
        "truncated": True,
    }


def _reconfigure_utf8(stream: TextIO | None) -> None:
    """把一条文本流切到 UTF-8（P10：管道 stdin 默认用 ANSI 代码页）。

    没有 ``reconfigure`` 的流（pytest 捕获、``StringIO``）原样跳过；真控制台
    也走同一条路——PEP 528 之后 Windows 控制台的编码已经是 UTF-8。
    """
    reconfigure = getattr(stream, "reconfigure", None)
    if reconfigure is None:
        return
    try:
        reconfigure(encoding="utf-8", errors="replace")
    except (ValueError, OSError, TypeError):  # pragma: no cover - 流已分离
        pass


def _discard_line(stream: TextIO, chunk: int = 64 * 1024) -> None:
    """丢掉一条超长帧的剩余部分，不把它读进内存（P9）。"""
    while True:
        try:
            rest = stream.readline(chunk)
        except (ValueError, OSError):  # pragma: no cover - 流已坏
            return
        if not rest or rest.endswith("\n"):
            return


def serve_forever(
    client: EngineClient,
    settings: Any,
    *,
    stdin: TextIO | None = None,
    stdout: TextIO | None = None,
    stderr: TextIO | None = None,
) -> int:
    """读 stdin 直到 EOF 或 ``shutdown``。返回进程退出码（桌面应用不看它）。

    三条与"手工起 sidecar"有关的历史坑在这里收口：

    * stdin 明确切到 UTF-8（P10）——``packetsage-agent serve`` 手工跑在管道上时
      默认是 ANSI 代码页（本机 GBK），中文 goal 会坏；
    * 单帧有上限（P9）——超长行就地丢弃并回 ``PROTOCOL_MISMATCH``，
     不再让一行把内存吃光；
    * ``shutdown`` 由 :meth:`Sidecar.handle_line` 处理（P6）——命令表里有一条，
      嵌进来的宿主也拿得到同样的 ack + 停机语义。
    """
    stream_in = stdin if stdin is not None else sys.stdin
    if stdin is None:
        _reconfigure_utf8(stream_in)
    sidecar = Sidecar(client, settings, out=stdout, err=stderr)
    sidecar.ping_engine()
    try:
        while True:
            try:
                line = stream_in.readline(FRAME_MAX_CHARS + 1)
            except (ValueError, OSError):  # pragma: no cover - 流已坏
                break
            if not line:
                break
            if len(line) > FRAME_MAX_CHARS:
                if not line.endswith("\n"):
                    _discard_line(stream_in)
                sidecar.ack_error(
                    "",
                    ProtocolError(
                        "PROTOCOL_MISMATCH",
                        f"frame is larger than {FRAME_MAX_CHARS} characters; it was dropped",
                        where="protocol",
                    ),
                )
                continue
            text = line.strip()
            if not text:
                continue
            if not sidecar.handle_line(text):
                break
    except KeyboardInterrupt:  # pragma: no cover - 桌面应用不发信号
        sidecar._warn("interrupted")  # noqa: SLF001 - 同模块内的收尾提示
    finally:
        sidecar.shutdown()
    return 0
