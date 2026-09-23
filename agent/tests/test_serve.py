"""通道 B 的三条收口：P6（`shutdown` 在命令表里）、P9（单帧有上限）、
P10（stdin 走 UTF-8）。

前两条是纯函数面（`Sidecar.handle_line` / `serve_forever`），第三条必须真起一个
子进程——只有"管道 stdin + 没有 `PYTHONUTF8`"才能复现 GBK 解码。
"""

from __future__ import annotations

import io
import json
import os
import queue
import subprocess
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent.serve import (  # noqa: E402
    FRAME_MAX_CHARS,
    PROTOCOL_VERSION,
    Sidecar,
    serve_forever,
)

REPO = Path(__file__).resolve().parents[2]
TASK_ID = "task_01J0000000000000000000000G"


class StubEngine:
    """够 `Sidecar` 用的最小引擎：只记下被问了什么。"""

    def __init__(self) -> None:
        self.closed = False
        self.calls: list[tuple[str, dict]] = []
        self.unhealthy = False

    def call(self, method: str, params: dict, timeout_s: float | None = None) -> object:
        self.calls.append((method, dict(params)))
        return {"task_id": params.get("task_id"), "packets": 612, "sessions": 570}

    def call_envelope(self, method: str, params: dict, step: int = 0) -> object:
        raise AssertionError("this stub only answers `call`")

    def heartbeat(self) -> bool:
        return True

    def close(self) -> None:
        self.closed = True


def settings() -> SimpleNamespace:
    """显式 mock：`provider_unavailable` 只在点名 mock 时放行。"""
    return SimpleNamespace(
        provider="mock",
        model="mock",
        api_key=None,
        base_url=None,
        scenario=None,
        prompt_version="v2",
    )


def sidecar(out: io.StringIO) -> Sidecar:
    return Sidecar(StubEngine(), settings(), out=out, err=io.StringIO())


def frames(out: io.StringIO) -> list[dict]:
    return [json.loads(line) for line in out.getvalue().splitlines() if line.strip()]


def command(name: str, params: dict | None = None, request_id: str = "1") -> str:
    return json.dumps(
        {"id": request_id, "type": "cmd", "command": name, "params": params or {}},
        ensure_ascii=False,
    )


# ---------------------------------------------------------------------- P6
def test_shutdown_is_handled_by_the_command_table() -> None:
    """P6：`handle_line` 直接嵌宿主要拿到 ack + "停"; 不再回 PROTOCOL_MISMATCH。"""
    out = io.StringIO()
    assert sidecar(out).handle_line(command("shutdown")) is False
    assert frames(out) == [{"id": "1", "type": "ack", "ok": True, "result": {}}]


def test_shutdown_does_not_need_a_handshake() -> None:
    out = io.StringIO()
    assert sidecar(out).handle_line(command("shutdown")) is False
    assert frames(out)[0]["ok"] is True


def test_shutdown_survives_a_fatal_protocol_mismatch() -> None:
    """版本不匹配后仍要能把侧车放下去（`hello` 是唯一另一条豁免的生命周期命令）。"""
    out = io.StringIO()
    side = sidecar(out)
    side.handle_line(command("hello", {"protocol_version": PROTOCOL_VERSION + 1}))
    assert frames(out)[0]["error"]["code"] == "PROTOCOL_MISMATCH"
    assert side.handle_line(command("status", request_id="2")) is True
    assert frames(out)[1]["error"]["code"] == "PROTOCOL_MISMATCH"
    assert side.handle_line(command("shutdown", request_id="3")) is False
    assert frames(out)[2] == {"id": "3", "type": "ack", "ok": True, "result": {}}


def test_other_commands_still_need_hello_and_unknown_ones_are_rejected() -> None:
    out = io.StringIO()
    side = sidecar(out)
    side.handle_line(command("status"))
    assert frames(out)[0]["error"]["code"] == "PROTOCOL_MISMATCH"
    side.handle_line(command("hello", {"protocol_version": PROTOCOL_VERSION}, "2"))
    side.handle_line(command("frobnicate", request_id="3"))
    assert "unknown command" in frames(out)[-1]["error"]["message"]


# ---------------------------------------------------------------------- P9
def test_oversized_line_is_dropped_and_the_stream_keeps_working() -> None:
    out = io.StringIO()
    stream = io.StringIO()
    stream.write("x" * (FRAME_MAX_CHARS + 10) + "\n")
    stream.write(command("hello", {"protocol_version": PROTOCOL_VERSION}))
    stream.write("\n")
    stream.write(command("shutdown", request_id="2"))
    stream.write("\n")
    stream.seek(0)

    assert serve_forever(StubEngine(), settings(), stdin=stream, stdout=out) == 0
    seen = frames(out)
    assert seen[0]["id"] == ""
    assert seen[0]["error"]["code"] == "PROTOCOL_MISMATCH"
    assert "dropped" in seen[0]["error"]["message"]
    assert seen[1]["ok"] is True and seen[1]["id"] == "1"
    assert seen[-1] == {"id": "2", "type": "ack", "ok": True, "result": {}}


# --------------------------------------------------------------------- P10
def _sidecar_env() -> dict[str, str]:
    """去掉一切"让 stdin 自动变 UTF-8"的变量，剩下的就是手工管道的真实环境。"""
    env = os.environ.copy()
    for name in ("PYTHONUTF8", "PYTHONIOENCODING", "PYTHONLEGACYWINDOWSSTDIO"):
        env.pop(name, None)
    for name in (
        "PACKETSAGE_LLM_API_KEY",
        "OPENAI_API_KEY",
        "DEEPSEEK_API_KEY",
        "PACKETSAGE_STORAGE_URL",
        "PACKETSAGE_DB",
    ):
        env.pop(name, None)
    env["PACKETSAGE_LLM_PROVIDER"] = "mock"
    return env


def test_hand_started_serve_reads_stdin_as_utf8(tmp_path: Path) -> None:
    """P10：手工起的 `serve` 必须按 UTF-8 解 stdin。

    两条判据：

    * **乱码**——发一条中文命令名，它会被原样回声进 `PROTOCOL_MISMATCH` 的
      message；按 GBK 解会变成「鑒嘸瀽」那种字（这正是 P10 的实测症状）；
    * **不崩**——中文 goal 里带一个非法字节，`errors="replace"` 之后仍然要
      正常 ack（旧路径会在读帧时就抛 `UnicodeDecodeError`）。
    """
    empty_env = tmp_path / "empty.env"
    empty_env.write_text("", encoding="utf-8")
    env = _sidecar_env()
    env["PACKETSAGE_ENV_FILE"] = str(empty_env)

    fake_engine = REPO / "agent" / "tests" / "fake_engine.py"
    process = subprocess.Popen(  # noqa: S603 - 固定命令，参数由测试控制
        [
            sys.executable,
            "-m",
            "packetsage_agent",
            "serve",
            "--engine",
            f'"{sys.executable}" "{fake_engine}"',
            "--db",
            f"sqlite://{tmp_path / 'serve.db'}",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=str(REPO / "agent"),
        env=env,
    )
    try:
        probe = json.dumps(
            {"id": "1", "type": "cmd", "command": "分析", "params": {}},
            ensure_ascii=False,
        ).encode("utf-8")
        # 中文 + 一个非法字节：GBK 严格解码会在这里抛 UnicodeDecodeError。
        goal = "分析该捕获\udcff并给出结论"
        payload = json.dumps(
            {
                "id": "2",
                "type": "cmd",
                "command": "chat",
                "params": {"task_id": TASK_ID, "goal": goal},
            },
            ensure_ascii=False,
        ).encode("utf-8", errors="surrogateescape")
        hello = json.dumps(
            {
                "id": "0",
                "type": "cmd",
                "command": "hello",
                "params": {"protocol_version": PROTOCOL_VERSION, "client": "pytest"},
            }
        ).encode("utf-8")
        assert process.stdin is not None
        process.stdin.write(hello + b"\n" + probe + b"\n" + payload + b"\n")
        process.stdin.flush()

        ack_ids = _wait_for_acks(process, {"0", "1", "2"}, timeout=60.0)
        message = ack_ids["1"]["error"]["message"]
        assert "分析" in message and "\ufffd" not in message, ascii(message)
        assert ack_ids["2"]["ok"] is True, ack_ids["2"]
        assert ack_ids["2"]["result"]["run_id"]
    finally:
        try:
            if process.poll() is None:
                process.kill()
        except OSError:  # pragma: no cover - 已经退出
            pass
        process.wait(timeout=30)


def _wait_for_acks(process: subprocess.Popen, wanted: set[str], timeout: float) -> dict:
    """读到 `wanted` 里每条 id 的 ack（事件跳过）；超时前进程死掉就算失败。"""
    assert process.stdout is not None
    lines: queue.Queue[bytes | None] = queue.Queue()

    def pump() -> None:
        assert process.stdout is not None
        for raw in process.stdout:
            lines.put(raw)
        lines.put(None)  # EOF：进程的 stdout 关了

    threading.Thread(target=pump, daemon=True).start()
    seen: dict[str, dict] = {}
    deadline = time.time() + timeout
    while time.time() < deadline and set(seen) != wanted:
        try:
            line = lines.get(timeout=max(0.1, deadline - time.time()))
        except queue.Empty:
            break
        if line is None:
            stderr = b""
            if process.stderr is not None:
                stderr = process.stderr.read() or b""
            raise AssertionError(
                f"侧车提前退出（returncode={process.poll()}）：{stderr.decode('utf-8', 'replace')[-800:]}"
            )
        text = line.decode("utf-8", errors="replace").strip()
        if not text:
            continue
        try:
            frame = json.loads(text)
        except ValueError:
            continue
        if frame.get("type") == "ack" and frame.get("id") in wanted:
            seen[frame["id"]] = frame
    assert set(seen) == wanted, f"缺少 ack：{sorted(wanted - set(seen))}"
    return seen
