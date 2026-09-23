#!/usr/bin/env python3
"""通道 B（GUI ↔ Agent sidecar）验收用例：《GUI 工程规格书 v0.2》§8.1。

    python tests/sidecar/protocol_cases.py --binary target/release/packetsage[.exe]
    python tests/sidecar/protocol_cases.py --binary <engine> --filter S61
    python tests/sidecar/protocol_cases.py --write-protocol   # 重做 §4.5 快照

纪律与其它验收脚本一致：

* 只喂 `PACKETSAGE_LLM_PROVIDER=mock`（确定性回放），不联网、不用真 key；
* 引擎可以用真二进制（默认），也可以用 `agent/tests/fake_engine.py`（L2 桩）——
  需要"慢到能 cancel / 并发"的用后者，因为真引擎跑 mock 一次 run 只有几十毫秒；
* 每条断言都打在协议面上（帧、seq、字段、错误码），不碰内部实现。

覆盖：S57、S60、S61、S62、S63、S67、S70；另加 §4.5 的 report 命令预演。
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import shlex
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from collections.abc import Callable, Iterable
from pathlib import Path
from typing import Any

#: 与 §4.5 的表一一对应；改动即失败（S70）。
PROTOCOL_SNAPSHOT = {
    "protocol_version": 1,
    "capabilities": ["run", "chat", "report", "cancel", "stream_events"],
    "commands": {
        "hello": {"params": ["protocol_version", "client", "client_version"], "result": ["protocol_version", "agent_version", "schema_version", "capabilities", "providers"]},
        "run": {"params": ["task_id", "goal", "mode", "prompt_version"], "result": ["run_id"]},
        "chat": {"params": ["task_id", "question"], "result": ["run_id"]},
        "report": {"params": ["task_id", "out_path"], "result": ["report_path", "sha256", "degraded", "unverified_items", "status", "template_version", "prompt_version"]},
        "cancel": {"params": ["run_id"], "result": []},
        "status": {"params": [], "result": ["engine_alive", "run", "budget"]},
        "shutdown": {"params": [], "result": []},
    },
    "events": {
        "welcome": ["protocol_version", "agent_version", "schema_version", "capabilities", "providers"],
        "run_started": ["run_id", "task_id", "mode", "model", "provider", "prompt_version", "budget"],
        # token 级流式（U11）：`channel` = content / reasoning / tool_args；
        # 合并后的碎块，**装饰帧**——丢了不影响任何结论、证据与预算。
        "llm_delta": ["step", "channel", "text", "tool_name", "tool_index"],
        # llm_ms / tool_ms / preloaded 是 P1 提速那一轮的**追加**字段（字段只允许
        # 增加，见 §4.3 规则 2）：模型慢还是工具慢，界面与验收都分得开。
        "llm_round": [
            "step",
            "llm_calls",
            "tool_calls",
            "tokens_in",
            "tokens_out",
            "cost_cents",
            "llm_ms",
            "tool_ms",
            "preloaded",
            # v0.4：输入侧缓存命中情况（DeepSeek 上下文硬盘缓存）。
            "cache_hit_tokens",
            "cache_miss_tokens",
        ],
        "tool_call_started": ["step", "tool_name", "args"],
        "tool_call_finished": ["step", "tool_name", "args", "status", "duration_ms", "tc_id", "result_summary"],
        "finding_accepted": ["id", "severity", "basis", "title", "evidence_ids"],
        "finding_rejected": ["title", "reason_code", "reason"],
        "report_written": ["report_path", "sha256", "degraded", "unverified_items", "status", "template_version", "prompt_version"],
        # v0.4 追加 `cache`（输入侧缓存命中情况）与 `summary`（模型自己写的那段话）。
        "run_finished": ["summary_version", "run_id", "task_id", "status", "stop_reason", "accepted", "submit_rejects", "malformed_output", "steps", "calls", "tool_calls", "tokens", "cache", "cost_cents", "prompt_version", "model", "provider", "summary", "findings"],
        "error": ["code", "message", "retryable", "where"],
    },
    #: §4.8 的错误码 + 侧车自己的 `ANTI_HALLUCINATION`（见 serve.py 模块说明）。
    "error_codes": [
        "PROTOCOL_MISMATCH",
        "TASK_NOT_FOUND",
        "CONFIG",
        "PROVIDER_UNREACHABLE",
        "ENGINE_CRASHED",
        "ENGINE_SPAWN",
        "TOOL_TIMEOUT",
        "BUSY",
        "INTERNAL",
        "ANTI_HALLUCINATION",
    ],
    "event_max_bytes": 8192,
    "envelope": {
        "ack": ["id", "type", "ok", "result|error"],
        "event": ["type", "event", "run_id", "seq", "ts_unix_ms", "data"],
    },
}


class Failure(AssertionError):
    """一条没过的判据。"""


class Case:
    def __init__(self, name: str, spec: str, fn: Callable[[Harness], None]) -> None:
        self.name = name
        self.spec = spec
        self.fn = fn


CASES: list[Case] = []


def case(name: str, spec: str):
    def decorate(fn):
        CASES.append(Case(name, spec, fn))
        return fn

    return decorate


# --------------------------------------------------------------------------
# 驱动 sidecar 的最小客户端
# --------------------------------------------------------------------------
class SidecarClient:
    """`python -m packetsage_agent serve` 的子进程客户端（写一行、读一行）。"""

    def __init__(self, harness: Harness, engine: list[str] | None = None) -> None:
        command = engine or harness.engine_command()
        argv = [
            sys.executable,
            "-m",
            "packetsage_agent",
            "serve",
            "--engine",
            shlex.join(command) if os.name != "nt" else _windows_join(command),
            "--db",
            harness.db_url,
        ]
        self._process = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
            cwd=str(harness.root / "agent"),
            env=harness.env(),
        )
        self._frames: queue.Queue[dict[str, Any]] = queue.Queue()
        self._stderr: list[str] = []
        threading.Thread(target=self._read_stdout, daemon=True).start()
        threading.Thread(target=self._read_stderr, daemon=True).start()

    def _read_stdout(self) -> None:
        assert self._process.stdout is not None
        for line in self._process.stdout:
            text = line.strip()
            if not text:
                continue
            try:
                self._frames.put(json.loads(text))
            except ValueError:
                self._frames.put({"type": "garbage", "raw": text})

    def _read_stderr(self) -> None:
        assert self._process.stderr is not None
        for line in self._process.stderr:
            self._stderr.append(line.rstrip())

    def send(self, command: str, params: dict[str, Any] | None = None) -> str:
        request_id = str(uuid.uuid4())
        frame = {"id": request_id, "type": "cmd", "command": command, "params": params or {}}
        assert self._process.stdin is not None
        self._process.stdin.write(json.dumps(frame, ensure_ascii=False) + "\n")
        self._process.stdin.flush()
        return request_id

    def call(self, command: str, params: dict[str, Any] | None = None, timeout: float = 60.0) -> dict:
        """发一条命令并等到它的 ack（事件跳过，交给 :meth:`next_event`）。"""
        request_id = self.send(command, params)
        while True:
            frame = self.next_frame(timeout)
            if frame.get("type") == "ack" and frame.get("id") == request_id:
                return frame

    def next_frame(self, timeout: float = 60.0) -> dict:
        try:
            return self._frames.get(timeout=timeout)
        except queue.Empty as exc:
            raise Failure(
                f"sidecar 在 {timeout}s 内没有输出帧；stderr={self.stderr_tail()!r}"
            ) from exc

    def next_event(self, timeout: float = 60.0, *, run_id: str | None = None) -> dict:
        """下一帧必须是事件；``run_id`` 给出时跳过不属于它的帧。"""
        deadline = time.time() + timeout
        while True:
            frame = self.next_frame(max(0.1, deadline - time.time()))
            if frame.get("type") != "event":
                continue
            if run_id is not None and frame.get("run_id") != run_id:
                continue
            return frame

    def collect_until(self, event: str, timeout: float = 120.0) -> list[dict]:
        """收集事件直到（含）``event``。"""
        deadline = time.time() + timeout
        seen: list[dict] = []
        while time.time() < deadline:
            frame = self.next_event(max(0.1, deadline - time.time()))
            seen.append(frame)
            if frame.get("event") == event:
                return seen
        raise Failure(f"没等到事件 {event!r}；已收到 {[f.get('event') for f in seen]}")

    def handshake(self) -> dict:
        ack = self.call(
            "hello",
            {
                "protocol_version": 1,
                "client": "packetsage-desktop",
                "client_version": "3.8.1",
            },
        )
        if not ack.get("ok"):
            raise Failure(f"hello 被拒：{ack}")
        return ack

    def stderr_tail(self, lines: int = 5) -> str:
        return "\n".join(self._stderr[-lines:])

    def close(self) -> None:
        try:
            if self._process.poll() is None:
                self.call("shutdown", {}, timeout=30.0)
        except Exception:  # noqa: BLE001 - 收尾阶段不掩盖真正的失败
            pass
        try:
            if self._process.stdin is not None:
                self._process.stdin.close()
        except OSError:
            pass
        try:
            self._process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self._process.kill()

    def __enter__(self) -> SidecarClient:
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()


def _windows_join(command: Iterable[str]) -> str:
    """`--engine` 的 Windows 形式：含空格的路径要带引号（cli.split_command 认这个）。"""
    return " ".join(f'"{token}"' if " " in token else token for token in command)


# --------------------------------------------------------------------------
# harness
# --------------------------------------------------------------------------
class Harness:
    """临时库 + mock provider + 真引擎（或 fake engine）。"""

    TASK_ID = "task_01J0000000000000000000000G"

    def __init__(self, binary: Path, root: Path, *, provider: str | None = "mock") -> None:
        self.binary = binary
        self.root = root
        self.provider = provider
        self.work = Path(tempfile.mkdtemp(prefix="packetsage-sidecar-"))
        self.db_url = f"sqlite://{self.work / 'sidecar.db'}"
        self.empty_env = self.work / "empty.env"
        self.empty_env.write_text("", encoding="utf-8")
        self.sample = root / "samples" / "synth-mixed.pcap"
        self._analysed = False

    def env(self) -> dict[str, str]:
        env = os.environ.copy()
        for name in (
            "PACKETSAGE_CONFIG",
            "PACKETSAGE_STORAGE_URL",
            "PACKETSAGE_DB",
            "PACKETSAGE_LLM_MODEL",
            "PACKETSAGE_LLM_BASE_URL",
            "PACKETSAGE_LLM_API_KEY",
            "PACKETSAGE_PROVIDER",
            "OPENAI_API_KEY",
            "DEEPSEEK_API_KEY",
        ):
            env.pop(name, None)
        env["PACKETSAGE_ENV_FILE"] = str(self.empty_env)
        if self.provider:
            env["PACKETSAGE_LLM_PROVIDER"] = self.provider
        return env

    def engine_command(self) -> list[str]:
        return [str(self.binary), "serve"]

    def fake_engine_command(self, mode: str = "normal") -> list[str]:
        return [sys.executable, str(self.root / "agent" / "tests" / "fake_engine.py"), mode]

    def analyze(self) -> str:
        """落库 + 拿到 task id（分析本身不是本文件的验收对象）。"""
        if self._analysed:
            return self.TASK_ID
        done = subprocess.run(
            [
                str(self.binary),
                "analyze",
                str(self.sample),
                "--db",
                self.db_url,
                "--task-id",
                self.TASK_ID,
                "-q",
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=self.env(),
            timeout=300,
        )
        if done.returncode != 0:
            raise Failure(f"analyze 失败（exit {done.returncode}）：{done.stderr[-300:]}")
        self._analysed = True
        return self.TASK_ID


# --------------------------------------------------------------------------
# S57 / S70：握手、版本协商、协议清单
# --------------------------------------------------------------------------
@case("S57 握手：匹配进主界面，不匹配拒绝后续命令", "§8.1 S57 / §4.3")
def _s57(h: Harness) -> None:
    h.analyze()
    with SidecarClient(h) as client:
        ack = client.handshake()
        result = ack["result"]
        expected = {
            "protocol_version",
            "agent_version",
            "schema_version",
            "capabilities",
            "providers",
        }
        if set(result) != expected:
            raise Failure(f"welcome 字段不对：{sorted(result)}")
        if result["protocol_version"] != 1 or result["schema_version"] != 2:
            raise Failure(f"版本声明不对：{result}")
        if result["capabilities"] != PROTOCOL_SNAPSHOT["capabilities"]:
            raise Failure(f"能力集与 §4.5 不一致：{result['capabilities']}")
        if result["providers"].get("configured") is not True:
            raise Failure(f"mock provider 应当算「已配置」：{result['providers']}")
        # welcome 事件与 ack 同载荷（事件表里也列了 welcome）。
        event = client.next_event(timeout=10)
        if event["event"] != "welcome" or event["seq"] != 1 or event["run_id"] != "":
            raise Failure(f"welcome 事件不对：{event}")
    with SidecarClient(h) as bad:
        ack = bad.call(
            "hello",
            {"protocol_version": 999, "client": "packetsage-desktop", "client_version": "9"},
        )
        if ack.get("ok") is not False:
            raise Failure("版本不匹配必须 ack.ok=false")
        error = ack.get("error") or {}
        if error.get("code") != "PROTOCOL_MISMATCH" or error.get("retryable") is not False:
            raise Failure(f"错误码不对：{error}")
        follow = bad.call("status", {})
        if follow.get("ok") is not False or (follow.get("error") or {}).get("code") != "PROTOCOL_MISMATCH":
            raise Failure(f"版本不匹配后必须拒绝后续命令：{follow}")


@case("S70 协议清单快照：命令/事件/字段/错误码冻结", "§8.1 S70 / §4.10")
def _s70(h: Harness) -> None:
    snapshot_path = h.root / "tests" / "sidecar" / "protocol_v1.json"
    current = _protocol_snapshot()
    if _WRITE_PROTOCOL:
        snapshot_path.write_text(
            json.dumps(current, ensure_ascii=False, indent=2, sort_keys=False) + "\n",
            encoding="utf-8",
        )
        return
    if not snapshot_path.is_file():
        raise Failure(f"缺少协议快照 {snapshot_path}；用 --write-protocol 生成")
    stored = json.loads(snapshot_path.read_text(encoding="utf-8"))
    if stored != current:
        raise Failure(
            "协议清单变了（S70）：改协议 = 升 protocol_version + 重做快照。\n"
            f"  快照: {json.dumps(stored, ensure_ascii=False)[:300]}\n"
            f"  现在: {json.dumps(current, ensure_ascii=False)[:300]}"
        )
    # 再用一次真 run 对拍：事件名与字段必须与清单一致（多字段 = 未冻结的改动）。
    task_id = h.analyze()
    with SidecarClient(h) as client:
        client.handshake()
        ack = client.call("run", {"task_id": task_id, "goal": "对拍协议字段", "mode": "run"})
        if not ack.get("ok"):
            raise Failure(f"run 被拒：{ack}")
        events = client.collect_until("run_finished", timeout=180)
    table = PROTOCOL_SNAPSHOT["events"]
    for event in events:
        name = event["event"]
        if name not in table:
            raise Failure(f"出现了未声明的事件 {name!r}")
        keys = set(event["data"])
        declared = set(table[name])
        extra = keys - declared
        if extra and not (name == "run_finished" and extra <= {"error"}):
            raise Failure(f"{name} 多出未声明字段 {sorted(extra)}")
        missing = declared - keys
        if missing:
            raise Failure(f"{name} 缺字段 {sorted(missing)}")


def _protocol_snapshot() -> dict[str, Any]:
    """运行时代码里的协议面（与 §4.5 的手写表对拍）。"""
    sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "agent"))
    from packetsage_agent import serve

    snapshot = json.loads(json.dumps(PROTOCOL_SNAPSHOT))
    snapshot["protocol_version"] = serve.PROTOCOL_VERSION
    snapshot["capabilities"] = list(serve.CAPABILITIES)
    snapshot["event_max_bytes"] = serve.EVENT_MAX_BYTES
    return snapshot


# --------------------------------------------------------------------------
# S61：事件顺序与完整性
# --------------------------------------------------------------------------
@case("S61 mock run：seq 单调无缺口、run_started 首 / run_finished 末", "§8.1 S61 / §4.5")
def _s61(h: Harness) -> None:
    task_id = h.analyze()
    with SidecarClient(h) as client:
        client.handshake()
        ack = client.call("run", {"task_id": task_id, "goal": "找出扫描与突发", "mode": "run"})
        if not ack.get("ok"):
            raise Failure(f"run 被拒：{ack}")
        run_id = ack["result"]["run_id"]
        events = client.collect_until("run_finished", timeout=180)
    names = [event["event"] for event in events]
    if names[0] != "run_started" or names[-1] != "run_finished":
        raise Failure(f"首末事件不对：{names[0]} … {names[-1]}")
    seqs = [event["seq"] for event in events]
    if seqs != list(range(1, len(seqs) + 1)):
        raise Failure(f"seq 有缺口：{seqs}")
    if any(event["run_id"] != run_id for event in events):
        raise Failure("事件的 run_id 与 ack 不一致")
    if len(events) < 3:
        raise Failure(f"事件太少：{names}")
    started = events[0]["data"]
    if started["budget"] != {
        "max_steps": 12,
        "max_llm_calls": 24,
        "max_tool_calls": 20,
        "max_tokens": 200000,
        "max_cost_cents": 500,
    }:
        raise Failure(f"预算不是 §4.5 的 12/24/20/200k/500：{started['budget']}")
    finished = events[-1]["data"]
    # 与 `packetsage-agent run --json` 同构（收口 §5.11）。
    for key in (
        "summary_version",
        "run_id",
        "task_id",
        "status",
        "stop_reason",
        "accepted",
        "submit_rejects",
        "malformed_output",
        "steps",
        "calls",
        "tool_calls",
        "tokens",
        "cost_cents",
        "prompt_version",
        "model",
        "provider",
        "findings",
    ):
        if key not in finished:
            raise Failure(f"run_finished 缺 {key!r}（与 §5.11 不同构）")
    if finished["status"] != "completed" or not finished["findings"]:
        raise Failure(f"mock run 应当 completed 且带 findings：{finished['status']}")
    if set(finished["findings"][0]) != {"id", "severity", "basis", "title", "evidence_ids"}:
        raise Failure(f"findings[] 字段不对：{sorted(finished['findings'][0])}")
    if "tool_call_finished" not in names or "tool_call_started" not in names:
        raise Failure(f"缺少工具调用事件：{names}")
    started_calls = [event for event in events if event["event"] == "tool_call_started"]
    finished_calls = [event for event in events if event["event"] == "tool_call_finished"]
    if len(started_calls) != len(finished_calls):
        raise Failure(
            f"tool_call_started/finished 不成对：{len(started_calls)} vs {len(finished_calls)}"
        )
    if not all(event["data"].get("tc_id") for event in finished_calls):
        raise Failure("有工具卡片没有 tc_id（证据锚点必须落台账）")
    # U11：token 级流式。mock provider 每轮给一帧，所以两种 channel 都该出现。
    deltas = [event for event in events if event["event"] == "llm_delta"]
    channels = {event["data"]["channel"] for event in deltas}
    if not deltas:
        raise Failure(f"没有 llm_delta：mock provider 也该走 token 级流式（{names}）")
    if not {"tool_args", "content"} <= channels:
        raise Failure(f"llm_delta 的 channel 不全：{sorted(channels)}")
    if any(not event["data"].get("text") for event in deltas):
        raise Failure("llm_delta 有空帧（合并逻辑不该发空段）")
    if any(event["data"]["step"] < 1 for event in deltas):
        raise Failure("llm_delta 的 step 不是从 1 起")


# --------------------------------------------------------------------------
# S60：取消（需要"慢到能 cancel"的引擎桩）
# --------------------------------------------------------------------------
@case("S60 cancel：受理 → 收尾 → degraded + stop_reason", "§8.1 S60 / §4.7")
def _s60(h: Harness) -> None:
    h.analyze()
    with SidecarClient(h, engine=h.fake_engine_command("slow:0.2")) as client:
        client.handshake()
        ack = client.call("run", {"task_id": Harness.TASK_ID, "goal": "慢一点", "mode": "run"})
        if not ack.get("ok"):
            raise Failure(f"run 被拒：{ack}")
        run_id = ack["result"]["run_id"]
        # 等到第二个工具调用才开始取消：一定在 run 中途。
        seen = 0
        deadline = time.time() + 60
        while seen < 2 and time.time() < deadline:
            event = client.next_event(timeout=60, run_id=run_id)
            if event["event"] == "tool_call_started":
                seen += 1
        if seen < 2:
            raise Failure("没等到第二个工具调用，无法验证中途取消")
        cancel = client.call("cancel", {"run_id": run_id})
        if not cancel.get("ok"):
            raise Failure(f"cancel 必须被受理：{cancel}")
        events = client.collect_until("run_finished", timeout=120)
    finished = events[-1]["data"]
    if finished["status"] != "degraded":
        raise Failure(f"取消后应当 degraded：{finished['status']}")
    if not finished.get("stop_reason"):
        raise Failure("取消后 stop_reason 必须有值")
    if finished["tool_calls"] < 1:
        raise Failure("已产生的工具调用被回滚了（部分结果必须保留）")


# --------------------------------------------------------------------------
# S63：并发命令 → BUSY
# --------------------------------------------------------------------------
@case("S63 run 进行中再发 run：回 BUSY，不排队", "§8.1 S63 / §4.5")
def _s63(h: Harness) -> None:
    h.analyze()
    with SidecarClient(h, engine=h.fake_engine_command("slow:0.2")) as client:
        client.handshake()
        ack = client.call("run", {"task_id": Harness.TASK_ID, "goal": "第一个", "mode": "run"})
        if not ack.get("ok"):
            raise Failure(f"第一条 run 被拒：{ack}")
        second = client.call("run", {"task_id": Harness.TASK_ID, "goal": "第二个", "mode": "run"})
        if second.get("ok") is not False or (second.get("error") or {}).get("code") != "BUSY":
            raise Failure(f"并发 run 必须回 BUSY：{second}")
        status = client.call("status", {})
        run_info = (status.get("result") or {}).get("run") or {}
        if run_info.get("run_id") != ack["result"]["run_id"]:
            raise Failure(f"status 应当报出正在跑的 run：{status}")
        if not (status.get("result") or {}).get("engine_alive"):
            raise Failure(f"status 应当报 engine_alive=true：{status}")
        client.call("cancel", {"run_id": ack["result"]["run_id"]})
        client.collect_until("run_finished", timeout=120)


# --------------------------------------------------------------------------
# S62：事件体积上限（对同一保证做单元级断言，见文件头说明）
# --------------------------------------------------------------------------
@case("S62 单条事件 ≤8 KiB：超限截断并标 truncated", "§8.1 S62 / §4.5 规则 2")
def _s62(h: Harness) -> None:
    sys.path.insert(0, str(h.root / "agent"))
    from packetsage_agent import serve

    big_args = {"packet_indices": list(range(4000))}
    payload = {
        "type": "event",
        "event": "tool_call_finished",
        "run_id": "run_x",
        "seq": 3,
        "ts_unix_ms": 0,
        "data": {
            "step": 1,
            "tool_name": "inspect_packets",
            "args": big_args,
            "status": "ok",
            "duration_ms": 1,
            "tc_id": "tc_x",
            "result_summary": "x" * 40000,
        },
    }
    fitted = serve._fit_event(payload)
    size = len(json.dumps(fitted, ensure_ascii=False).encode("utf-8"))
    if size > serve.EVENT_MAX_BYTES:
        raise Failure(f"截断后仍然 {size} 字节 > {serve.EVENT_MAX_BYTES}")
    if fitted.get("truncated") is not True:
        raise Failure("截断后必须标 truncated=true")
    if len(str(fitted["data"].get("result_summary"))) >= 40000:
        raise Failure("result_summary 没有被截断")


# --------------------------------------------------------------------------
# S67：provider 未配置 → 握手告知 + 拒绝 run
# --------------------------------------------------------------------------
@case("S67 provider 未配置：握手报 unconfigured，run 回 CONFIG", "§8.1 S67 / U7")
def _s67(h: Harness) -> None:
    h.provider = None
    try:
        with SidecarClient(h) as client:
            ack = client.handshake()
            providers = ack["result"]["providers"]
            if providers.get("configured") is not False:
                raise Failure(f"未配置时应当 configured=false：{providers}")
            run = client.call("run", {"task_id": Harness.TASK_ID, "goal": "x", "mode": "run"})
            if run.get("ok") is not False or (run.get("error") or {}).get("code") != "CONFIG":
                raise Failure(f"未配置 provider 时 run 必须回 CONFIG：{run}")
            if (run.get("error") or {}).get("retryable") is not False:
                raise Failure("CONFIG 不可重试（要先去向导）")
    finally:
        h.provider = "mock"


# --------------------------------------------------------------------------
# report 命令（§4.5 / M13 的自动化预演）
# --------------------------------------------------------------------------
@case("report 命令：九节报告 + report_written 事件", "§4.5 report / M13")
def _s61b(h: Harness) -> None:
    task_id = h.analyze()
    out_path = h.work / "报告 输出.md"
    with SidecarClient(h) as client:
        client.handshake()
        ack = client.call("run", {"task_id": task_id, "goal": "分析", "mode": "run"})
        if not ack.get("ok"):
            raise Failure(f"run 被拒：{ack}")
        client.collect_until("run_finished", timeout=180)
        report = client.call("report", {"task_id": task_id, "out_path": str(out_path)}, timeout=180)
        if not report.get("ok"):
            raise Failure(f"report 被拒：{report}")
        result = report["result"]
        if Path(result["report_path"]).name != out_path.name:
            raise Failure(f"报告的落盘路径不对：{result['report_path']}")
        event = client.next_event(timeout=30)
        if event["event"] != "report_written" or event["run_id"] != "":
            raise Failure(f"report_written 事件不对：{event}")
    text = out_path.read_text(encoding="utf-8")
    sections = [line for line in text.splitlines() if line.startswith("## ")]
    if len(sections) != 9:
        raise Failure(f"报告不是九节：{sections}")
    if result["sha256"] and len(result["sha256"]) != 64:
        raise Failure(f"sha256 形状不对：{result['sha256']}")
    if "degraded" not in result or "unverified_items" not in result:
        raise Failure(f"report ack 字段不全：{sorted(result)}")


_WRITE_PROTOCOL = False


def main(argv: list[str] | None = None) -> int:
    global _WRITE_PROTOCOL
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace")
        except (AttributeError, ValueError, OSError):
            pass
    parser = argparse.ArgumentParser(description="sidecar protocol cases (通道 B)")
    parser.add_argument("--binary", default=None)
    parser.add_argument("--filter", default=None)
    parser.add_argument(
        "--write-protocol",
        action="store_true",
        help="重做 tests/sidecar/protocol_v1.json（改协议时才用）",
    )
    args = parser.parse_args(argv)
    _WRITE_PROTOCOL = bool(args.write_protocol)

    root = Path(__file__).resolve().parents[2]
    default = root / "target" / "debug" / ("packetsage.exe" if os.name == "nt" else "packetsage")
    binary = (Path(args.binary) if args.binary else default).resolve()
    if not binary.is_file():
        print(f"protocol_cases: binary not found: {binary}", file=sys.stderr)
        return 2
    harness = Harness(binary, root)

    failures: list[tuple[str, str]] = []
    selected = [entry for entry in CASES if not args.filter or args.filter in entry.name]
    for entry in selected:
        try:
            entry.fn(harness)
        except Exception as exc:  # noqa: BLE001 - 失败的用例就是报告
            failures.append((entry.name, str(exc)))
            print(f"FAIL {entry.name}  [{entry.spec}]\n     {exc}")
        else:
            print(f"ok   {entry.name}  [{entry.spec}]")
    print(f"\n{len(selected) - len(failures)}/{len(selected)} sidecar protocol cases passed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
