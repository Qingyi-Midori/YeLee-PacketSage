"""引擎与会话生命周期（U3），以及界面唯一的 RPC 入口。

三件事必须同时成立，否则 Streamlit 的重跑模型会把引擎搞崩：

* **单例**：``packetsage serve`` 只 spawn 一次，放进 ``@st.cache_resource``
  （每次交互都重跑脚本，见 §4.4 约束 1/4）；
* **单写者**：``EngineClient`` 把请求行写进同一个 stdin 管道，所以所有 RPC 都
  串行通过一把锁——界面线程与后台工作线程不会交错写坏 JSONL 流（U4）；
* **可重建**：``EngineCrashed`` / ``EngineSpawnError`` 走到重建分支，界面给可读
  错误（含 ``stderr_tail(3)``）而不是白屏（S57/S64/S65）。
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import streamlit as st
from packetsage_agent import config as agent_config
from packetsage_agent.engine_client import (
    CallResult,
    EngineClient,
    EngineCrashed,
    EngineError,
    EngineSpawnError,
    RpcTimeouts,
    RpcToolError,
    ToolTimeout,
)

from .paths import DB_URL, ENGINE_LOG_PATH, ROOT, engine_candidates

#: analyze 的 RPC 预算：``RpcTimeouts.analyze_file`` 默认 60s，界面留足余量
#: （大抓包），但不会无限等下去。
ANALYZE_TIMEOUT_S = 900.0

#: ``packetsage doctor --json`` 的十项 id（顺序被 Rust 测试锁定，见收口 §5.8）。
DOCTOR_IDS = (
    "binary",
    "config",
    "rules",
    "samples",
    "database",
    "paths",
    "python",
    "provider",
    "e2e",
    "schema",
)


class EngineUnavailable(EngineError):
    """连引擎可执行文件都找不到——只有 U8 的依赖/构建问题会走到这里。"""


# --------------------------------------------------------------------------
# 命令行出口（只读，两个用途：doctor 与 db query）
# --------------------------------------------------------------------------
def engine_command() -> list[str]:
    """``packetsage serve`` 的命令前缀：``$PACKETSAGE_ENGINE`` → PATH → 构建产物。"""
    override = os.environ.get("PACKETSAGE_ENGINE")
    if override:
        return [override, "serve"]
    found = shutil.which("packetsage")
    if found:
        return [found, "serve"]
    for candidate in engine_candidates():
        if candidate.is_file():
            return [str(candidate), "serve"]
    raise EngineUnavailable(
        "找不到 packetsage 可执行文件：把 `packetsage` 放进 PATH，或设 $PACKETSAGE_ENGINE，"
        "或在仓库里构建（powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release）"
    )


def engine_binary() -> str:
    """引擎可执行文件路径（doctor / db query 用）。"""
    return engine_command()[0]


def run_cli(*args: str, timeout: float = 180.0) -> subprocess.CompletedProcess:
    """跑一次 ``packetsage <args...>``，工作目录固定在仓库根。

    界面只允许两个只读用途（``doctor --json`` 与 ``db query --readonly --jsonl``）：
    分析走 RPC，查询走 RPC，人读表格一律不解析（§5.9）。
    """
    return subprocess.run(
        [engine_binary(), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        cwd=str(ROOT),
        check=False,
    )


def doctor_failure_hint() -> str:
    """引擎不可用时的 §5.1 修法提示。"""
    return (
        "先在仓库根构建引擎：`powershell -ExecutionPolicy Bypass -File "
        "scripts/build.ps1 -CargoArgs --release`，或用 `scripts/install_smoke.py` 装一份。"
    )


@st.cache_data(ttl=30, show_spinner=False)
def doctor_report() -> dict[str, Any]:
    """``packetsage doctor --json --no-net``（U7 的前置检查，缓存 30s）。

    ``--no-net`` 的理由：GUI 启动不能被外网探测拖住（provider 的连通性由
    ``packetsage-agent setup`` 与 run 自己的错误呈现负责）。
    """
    try:
        done = run_cli("doctor", "--json", "--no-net", timeout=180.0)
    except EngineUnavailable as exc:
        return {"items": [], "error": str(exc), "repair": doctor_failure_hint()}
    except subprocess.TimeoutExpired:
        return {"items": [], "error": "packetsage doctor 超时（180s）", "repair": ""}
    # `doctor --json` is a pretty printed *document* (not JSONL), so the whole
    # stdout is parsed first; the line scan is only a fallback for wrappers that
    # append noise around it.
    payload: dict[str, Any] | None = None
    try:
        candidate = json.loads(done.stdout.strip())
        payload = candidate if isinstance(candidate, dict) else None
    except ValueError:
        payload = None
    if payload is None:
        for line in reversed(done.stdout.splitlines()):
            text = line.strip()
            if not text.startswith("{"):
                continue
            try:
                candidate = json.loads(text)
            except ValueError:
                continue
            if isinstance(candidate, dict) and "items" in candidate:
                payload = candidate
                break
    if not isinstance(payload, dict):
        return {
            "items": [],
            "error": (done.stderr.strip() or "packetsage doctor 没有输出 JSON"),
            "repair": doctor_failure_hint(),
        }
    items = payload.get("items")
    return {
        "items": items if isinstance(items, list) else [],
        "exit_code": done.returncode,
        "error": "",
        "repair": "",
    }


def doctor_item(report: dict[str, Any], item_id: str) -> dict[str, Any]:
    """按 id 取一项自检结果；缺失时给一个 ``skipped`` 形状的空项。"""
    for item in report.get("items") or []:
        if isinstance(item, dict) and item.get("id") == item_id:
            return item
    return {"id": item_id, "name": item_id, "status": "skipped", "detail": "", "repair_hint": ""}


# --------------------------------------------------------------------------
# 引擎会话
# --------------------------------------------------------------------------
@dataclass
class RpcStats:
    """本会话的 RPC 计数（排障用：卡死时能看出停在哪一步）。"""

    calls: int = 0
    errors: int = 0
    last_method: str = ""
    last_error: str = ""
    last_duration_ms: int = 0


class EngineSession:
    """一个 ``packetsage serve`` 进程 + 串行化的 RPC 入口。"""

    def __init__(self, db_url: str = DB_URL, log_path: Path = ENGINE_LOG_PATH) -> None:
        self.db_url = db_url
        self.log_path = Path(log_path)
        #: RLock：``call_envelope`` 内部会调 ``call``，重入不能死锁。
        self._lock = threading.RLock()
        self._client: EngineClient | None = None
        self.restarts = 0
        self.last_error: str | None = None
        self.started_at = time.time()
        self.stats = RpcStats()
        self._start()

    # ------------------------------------------------------------ 生命周期
    def _start(self) -> None:
        env = dict(os.environ)
        # serve 从环境读存储 URL（没有 --db 旗标），两个名字都设上（cli.py 同款）。
        env["PACKETSAGE_STORAGE_URL"] = self.db_url
        env["PACKETSAGE_DB"] = self.db_url
        client = EngineClient(
            engine_command(),
            timeouts=RpcTimeouts(),
            env=env,
            log_path=str(self.log_path),
        )
        try:
            client.call("ping", {})
        except BaseException:
            client.close()
            raise
        self._client = client
        self.started_at = time.time()

    def restart(self, reason: str = "") -> None:
        """关掉旧进程、起一个新进程；重启次数会显示在侧栏（U3）。"""
        with self._lock:
            self.last_error = reason or self.last_error
            self._close_locked()
            self._start()
            self.restarts += 1

    def _close_locked(self) -> None:
        client, self._client = self._client, None
        if client is None:
            return
        try:
            client.close()
        except Exception:  # pragma: no cover - 关闭失败不影响重建
            pass

    def close(self) -> None:
        with self._lock:
            self._close_locked()

    # ---------------------------------------------------------------- 诊断
    @property
    def pid(self) -> int | None:
        """引擎进程号（S57/S65 的判据：连续交互后必须稳定）。"""
        client = self._client
        process = getattr(client, "_process", None)
        return getattr(process, "pid", None)

    @property
    def command_line(self) -> str:
        client = self._client
        return getattr(client, "command_line", " ".join(engine_command()))

    @property
    def alive(self) -> bool:
        client = self._client
        process = getattr(client, "_process", None)
        return process is not None and process.poll() is None

    def stderr_tail(self, lines: int = 3) -> list[str]:
        """引擎崩溃时的排障尾巴（§5.3 ``EngineCrashed.stderr_tail(3)``）。"""
        client = self._client
        if client is None:
            return []
        try:
            return list(client.stderr_tail(lines))
        except Exception:  # pragma: no cover - 引擎已关闭
            return []

    def display_url(self) -> str:
        """脱敏后的存储 URL（sqlite 一般没密码，但规矩不例外）。"""
        return agent_config.redact_url(self.db_url)

    def heartbeat(self) -> bool:
        """一次 ping；连续两次失败会被 ``EngineClient`` 标记 unhealthy。"""
        with self._lock:
            client = self._client
            if client is None:
                return False
            try:
                return bool(client.heartbeat())
            except Exception:  # pragma: no cover - 探测本身不该抛
                return False

    # ------------------------------------------------------------------ RPC
    def call(self, method: str, params: dict[str, Any], timeout_s: float | None = None) -> Any:
        """一次 RPC；返回信封（或方法自带的裸结果）。"""
        with self._lock:
            client = self._client
            if client is None:
                raise EngineCrashed("engine session is closed", None, None)
            started = time.perf_counter()
            self.stats.calls += 1
            self.stats.last_method = method
            try:
                result = client.call(method, params, timeout_s)
            except (EngineCrashed, EngineSpawnError, RpcToolError, ToolTimeout, OSError) as exc:
                self.stats.errors += 1
                self.stats.last_error = f"{type(exc).__name__}: {exc}"
                self.last_error = self.stats.last_error
                raise
            finally:
                self.stats.last_duration_ms = int((time.perf_counter() - started) * 1000)
            return result

    def call_envelope(
        self, method: str, params: dict[str, Any], step: int = 0
    ) -> CallResult:
        """一次工具调用；返回 ``CallResult``（``tool_calls`` 台账的形状）。"""
        with self._lock:
            client = self._client
            if client is None:
                raise EngineCrashed("engine session is closed", None, None)
            return client.call_envelope(method, params, step)

    def content(self, method: str, params: dict[str, Any], timeout_s: float | None = None) -> Any:
        """取信封里的 ``content``；方法返回裸结果时原样返回（``analyze_file`` 等）。"""
        return unwrap_envelope(self.call(method, params, timeout_s))

    def agent_client(self) -> SerializedClient:
        """交给 ``PacketSageAgent`` 的客户端：同一个锁，仍然只有一个写者。"""
        return SerializedClient(self)


class SerializedClient:
    """把 ``EngineSession`` 包成 ``EngineClient`` 形状（agent 只用到两个方法）。

    工作线程跑 ``agent.run()`` 时，界面线程绝不能同时写同一个 stdin 管道；
    锁在 :class:`EngineSession` 上，所以两条路径天然互斥（U4/§4.4 约束 2）。
    """

    def __init__(self, session: EngineSession) -> None:
        self._session = session

    def call(self, method: str, params: dict[str, Any], timeout_s: float | None = None) -> Any:
        return self._session.call(method, params, timeout_s)

    def call_envelope(self, method: str, params: dict[str, Any], step: int = 0) -> CallResult:
        return self._session.call_envelope(method, params, step)


def unwrap_envelope(result: Any) -> Any:
    """信封 → ``content``；裸结果原样返回。"""
    if isinstance(result, dict) and "trusted_as_instruction" in result:
        return result.get("content")
    return result


@st.cache_resource(show_spinner=False)
def get_engine(db_url: str = DB_URL) -> EngineSession:
    """进程级单例：Streamlit 每次交互重跑脚本，引擎只 spawn 一次（U3/S57）。"""
    return EngineSession(db_url)


def engine_or_rebuild(db_url: str = DB_URL) -> tuple[EngineSession | None, str]:
    """拿单例；引擎死了就重建一次并返回可读说明（§5.3 ``failed`` 分支）。

    返回 ``(session, message)``：``session`` 为 ``None`` 时 ``message`` 是给用户看的
    原因（含 ``stderr_tail(3)``），界面照原样渲染，不白屏。
    """
    try:
        session = get_engine(db_url)
    except EngineSpawnError as exc:
        return None, f"引擎起不来：{exc.cause or exc}"
    except EngineUnavailable as exc:
        return None, str(exc)
    except EngineError as exc:
        return None, f"引擎握手失败：{exc}"
    if session.alive:
        return session, ""
    tail = "\n".join(f"  engine> {line}" for line in session.stderr_tail(3))
    detail = f"引擎进程已退出（exit={_exit_code(session)}）\n{tail}".strip()
    try:
        session.restart(detail)
    except EngineError as exc:
        return None, f"{detail}\n重建失败：{exc}"
    return session, f"{detail}\n已重建引擎（第 {session.restarts} 次）。"


def _exit_code(session: EngineSession) -> int | None:
    process = getattr(session._client, "_process", None)  # noqa: SLF001 - 只读诊断
    return process.poll() if process is not None else None


# --------------------------------------------------------------------------
# 数据访问（全部走 §5.12 的 Python 面）
# --------------------------------------------------------------------------
def analyze_capture(session: EngineSession, path: Path) -> dict[str, Any]:
    """``analyze_file``：落库 + 留在内存，返回 ``{task_id, summary}``。

    这是本项目的分析入口：一次调用同时满足"任务可被后续 run 使用"（同一个
    serve 进程内存里就有）与"任务可被别的进程冷恢复"（ADR-019 的持久化行）。
    """
    result = session.call("analyze_file", {"path": str(path)}, timeout_s=ANALYZE_TIMEOUT_S)
    if not isinstance(result, dict) or not result.get("task_id"):
        raise EngineError(f"analyze_file 返回了非预期结构：{result!r}")
    return result


def capture_summary(session: EngineSession, task_id: str) -> dict[str, Any]:
    """``get_capture_summary``：M4 的"612 包 / 570 会话"就来自这里。"""
    content = session.content("get_capture_summary", {"task_id": task_id})
    return content if isinstance(content, dict) else {}


def protocol_stats(session: EngineSession, task_id: str, layer: str, top: int = 10) -> list[dict]:
    """``get_protocol_stats`` 的一层。"""
    content = session.content("get_protocol_stats", {"task_id": task_id, "layer": layer, "top": top})
    rows = content.get("rows") if isinstance(content, dict) else None
    return rows if isinstance(rows, list) else []


def conversations(session: EngineSession, task_id: str, limit: int = 20) -> list[dict]:
    """``get_conversations``：Top 会话表（报告 §4 的数据源）。"""
    content = session.content(
        "get_conversations", {"task_id": task_id, "sort_by": "bytes", "limit": limit}
    )
    rows = content.get("conversations") if isinstance(content, dict) else None
    return rows if isinstance(rows, list) else []


def alerts(session: EngineSession, task_id: str, limit: int = 100) -> list[dict]:
    """``check_alerts``：规则告警面板（§4.2 的一等公民）。"""
    content = session.content("check_alerts", {"task_id": task_id})
    rows = content.get("alerts") if isinstance(content, dict) else None
    if not isinstance(rows, list):
        return []
    return rows[:limit]


def findings(session: EngineSession, task_id: str, limit: int = 50) -> list[dict]:
    """``query_history kind=findings``：结论 + 证据锚点（证据链的入口）。"""
    content = session.content("query_history", {"task_id": task_id, "kind": "findings", "limit": limit})
    rows = content.get("findings") if isinstance(content, dict) else None
    return rows if isinstance(rows, list) else []


def trace_entries(session: EngineSession, task_id: str, limit: int = 200) -> list[dict]:
    """``query_history kind=trace``：tc 台账（证据锚点的落点）。"""
    content = session.content("query_history", {"task_id": task_id, "kind": "trace", "limit": limit})
    rows = content.get("trace") if isinstance(content, dict) else None
    return rows if isinstance(rows, list) else []


def artifacts(session: EngineSession, task_id: str) -> dict[str, Any]:
    """``get_task_artifacts``：报告路径 + per-task 计数。"""
    content = session.content("get_task_artifacts", {"task_id": task_id})
    return content if isinstance(content, dict) else {}


def rules(session: EngineSession) -> dict[str, Any]:
    """``list_rules``（裸结果，不是信封）：规则 id / 版本 / 内容哈希。"""
    result = session.call("list_rules", {})
    return result if isinstance(result, dict) else {}


def list_tasks(db_url: str = DB_URL, limit: int = 30) -> list[dict[str, Any]]:
    """历史 task 列表：``db query --readonly --jsonl``（唯一只读 SQL 出口）。

    库文件不存在时返回空列表：首启的界面应该是空态 + 建议问题，而不是报错。
    """
    from .paths import readonly_url, sqlite_path

    path = sqlite_path(db_url)
    if path is not None and not path.is_file():
        return []
    sql = (
        "SELECT id, status, source_path, packet_count, byte_count, started_at, finished_at "
        "FROM analysis_tasks ORDER BY started_at DESC"
    )
    try:
        done = run_cli(
            "db",
            "query",
            "--readonly",
            "--db",
            readonly_url(db_url),
            "--jsonl",
            "--limit",
            str(limit),
            "--sql",
            sql,
            timeout=60.0,
        )
    except (EngineUnavailable, subprocess.TimeoutExpired):
        return []
    rows: list[dict[str, Any]] = []
    for line in done.stdout.splitlines():
        text = line.strip()
        if not text.startswith("{"):
            continue
        try:
            payload = json.loads(text)
        except ValueError:
            continue
        if isinstance(payload, dict):
            rows.append(payload)
    return rows


def samples() -> list[Path]:
    """``samples/`` 里的抓包（按名字排序，界面下拉框用）。"""
    from .paths import SAMPLES_DIR

    if not SAMPLES_DIR.is_dir():
        return []
    return sorted(
        (path for path in SAMPLES_DIR.iterdir() if path.suffix.lower() in {".pcap", ".pcapng"}),
        key=lambda path: path.name,
    )
