#!/usr/bin/env python3
"""GUI 层验收用例（《GUI 工程规格书 v0.1》§6.1 的可脚本化部分）。

    python tests/gui/app_cases.py --binary target/release/packetsage.exe

这批用例**真跑** ``gui/app.py``（``streamlit.testing.v1.AppTest`` 会在进程内执行
脚本、并允许像浏览器一样点按钮），不是静态检查。三条纪律：

* 只用 mock provider（确定性脚本回放）与临时数据库，不碰网络、不碰真 key；
* 需要真模型、真终端、真上传对话框的条目（S61 的真模型长 run、M13–M18）留在
  §6.2 的人工清单里，脚本不假装覆盖；
* 每条都断言**行为**（界面元素、状态、进程号），不断言内部实现。

覆盖：S57、S58、S59、S60、S62、S63、S64、S65 与 M4、M8、M10、M12。
"""

from __future__ import annotations

import argparse
import contextlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any


class Failure(AssertionError):
    """一条没过的判据。"""


class Case:
    """§6.1 里的一行，可执行版本。"""

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
# harness
# --------------------------------------------------------------------------
class Harness:
    """一个进程内的测试台：临时数据库 + mock provider + 仓库里的真二进制。"""

    def __init__(self, binary: Path, root: Path) -> None:
        self.binary = binary
        self.root = root
        self.work = Path(tempfile.mkdtemp(prefix="packetsage-gui-cases-"))
        self.db_url = f"sqlite://{self.work / 'gui.db'}"
        self.empty_env = self.work / "empty.env"
        self.empty_env.write_text("", encoding="utf-8")
        self.sample = root / "samples" / "synth-mixed.pcap"
        self.app_path = root / "gui" / "app.py"
        self._gui: Any = None

    # ------------------------------------------------------------- 环境
    def install_env(self, *, provider: str | None = "mock") -> None:
        """把界面要用到的环境变量钉死（在 import gui.* 之前调用）。"""
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
            "PACKETSAGE_GUI_DB",
        ):
            os.environ.pop(name, None)
        os.environ["PACKETSAGE_ENV_FILE"] = str(self.empty_env)
        os.environ["PACKETSAGE_ENGINE"] = str(self.binary)
        os.environ["PACKETSAGE_GUI_DB"] = self.db_url
        if provider:
            os.environ["PACKETSAGE_LLM_PROVIDER"] = provider
        else:
            os.environ.pop("PACKETSAGE_LLM_PROVIDER", None)
        sys.path.insert(0, str(self.root))
        sys.path.insert(0, str(self.root / "agent"))

    @property
    def gui(self) -> Any:
        """``gui`` 包（import 之后才生效的环境变量决定了它的 DB_URL）。"""
        if self._gui is None:
            import gui.engine
            import gui.jobs
            import gui.settings
            import gui.views

            self._gui = gui
        return self._gui

    # -------------------------------------------------------------- 工具
    def clear_caches(self) -> None:
        """doctor（30s）与历史 task（5s）的缓存，跨条目必须清掉。"""
        self.gui.engine.doctor_report.clear()
        try:
            from gui import app as app_module

            app_module._task_rows.clear()
        except Exception:  # pragma: no cover - app 还没被 import
            pass

    def new_app(self, timeout: float = 600.0) -> Any:
        """一个新的 AppTest 会话（等价于刷新一次浏览器）。"""
        from streamlit.testing.v1 import AppTest

        return AppTest.from_file(str(self.app_path), default_timeout=timeout)

    def app_with_task(self, timeout: float = 600.0) -> Any:
        """起界面 → 选 ``synth-mixed.pcap`` → 分析 → 任务就绪。"""
        self.clear_caches()
        at = self.new_app(timeout)
        at.run()
        self.assert_no_exception(at)
        self.pick_sample(at, "synth-mixed")
        at.run()
        at.button(key="start_analyze").click().run()
        self.assert_no_exception(at)
        task_id = at.session_state["task_id"]
        if not task_id:
            raise Failure("analyze 之后没有拿到 task_id")
        return at

    def pick_sample(self, at: Any, needle: str) -> None:
        labels = list(at.selectbox(key="sample_pick").options)
        match = [index for index, label in enumerate(labels) if needle in label]
        if not match:
            raise Failure(f"samples/ 里找不到 {needle}（现有：{labels}）")
        at.selectbox(key="sample_pick").select(match[0])

    def assert_no_exception(self, at: Any) -> None:
        if len(at.exception):
            raise Failure(
                "界面抛异常了：" + " | ".join(str(item.value) for item in at.exception)
            )

    def engine_pid(self, at: Any) -> int | None:
        """从侧栏的 "pid `n`" 读引擎进程号（界面显示什么就断言什么）。"""
        for element in at.markdown:
            found = re.search(r"pid `(\d+)`", element.value)
            if found:
                return int(found.group(1))
        return None

    def kill_engine(self, pid: int) -> None:
        if os.name == "nt":
            subprocess.run(
                ["taskkill", "/F", "/PID", str(pid)], capture_output=True, check=False
            )
        else:
            os.kill(pid, 9)
        time.sleep(0.5)

    def sample_with_odd_path(self) -> Path:
        """中文 + 空格目录里的抓包（S62：路径不许拖累分析）。"""
        target_dir = self.work / "带 空格 的 目录"
        target_dir.mkdir(parents=True, exist_ok=True)
        target = target_dir / "synth mixed 抓包.pcap"
        shutil.copyfile(self.sample, target)
        return target

    # ------------------------------------------------------- 后台任务的直用
    def session(self) -> Any:
        """一个裸 EngineSession（非 AppTest 路径的用例用）。"""
        return self.gui.engine.EngineSession()

    def analyze_blocking(self, session: Any, path: Path) -> Any:
        job = self.gui.jobs.start_analyze(session, path)
        if not job.done.wait(300):
            raise Failure("analyze 300s 还没结束")
        if job.error is not None:
            raise Failure(f"analyze 失败：{job.error}")
        return job


def _slow_provider_agent(session: Any, delay: float) -> Any:
    """注入一个"每一步都慢"的 mock（真模型 20-45s 的替身，S58/S59 的桩）。"""
    from packetsage_agent.agent import PacketSageAgent
    from packetsage_agent.provider import MockProvider

    class SlowMock(MockProvider):
        def decide(self, *args: Any, **kwargs: Any) -> Any:
            time.sleep(delay)
            return super().decide(*args, **kwargs)

    return PacketSageAgent(
        session.agent_client(), provider=SlowMock(), model_name="slow-mock", provider_kind="mock"
    )


@contextlib.contextmanager
def _injected_agent(h: Harness, session: Any, delay: float = 0.25):
    """把 ``jobs.build_agent`` 换成慢速桩，退出时一定要还原（用例之间必须隔离）。"""
    original = h.gui.jobs.build_agent
    h.gui.jobs.build_agent = lambda *_a, **_k: _slow_provider_agent(session, delay)
    try:
        yield
    finally:
        h.gui.jobs.build_agent = original


# --------------------------------------------------------------------------
# S60：provider 未配置 → setup 引导，禁用开始分析
# --------------------------------------------------------------------------
@case("S60 provider 未配置时给 setup 引导并禁用开始分析", "§6.1 S60 / U7")
def _s60(h: Harness) -> None:
    os.environ.pop("PACKETSAGE_LLM_PROVIDER", None)
    try:
        h.clear_caches()
        at = h.new_app().run()
        h.assert_no_exception(at)
        errors = " ".join(str(item.value) for item in at.error)
        if "provider" not in errors or "setup" not in errors:
            raise Failure(f"首屏没有给出 setup 引导：{errors[:200]!r}")
        codes = " ".join(str(item.value) for item in at.code)
        if "packetsage-agent setup" not in codes:
            raise Failure("没有把 setup 命令摆出来给用户复制")
        if not at.button(key="start_analyze").disabled:
            raise Failure("provider 未配置时「开始分析」必须禁用（不得静默 mock）")
        if any("任务面板" in element.value for element in at.markdown):
            raise Failure("provider 未配置时不该进入主界面")
    finally:
        os.environ["PACKETSAGE_LLM_PROVIDER"] = "mock"


# --------------------------------------------------------------------------
# S57 / S65：引擎单例 + 20 次交互
# --------------------------------------------------------------------------
@case("S57/S65 连续 20 次交互：引擎只有一个进程、不卡死", "§6.1 S57/S65 / U3")
def _s57(h: Harness) -> None:
    at = h.app_with_task()
    first = h.engine_pid(at)
    if first is None:
        raise Failure("界面上读不到引擎 pid")
    if first and not _process_alive(first):
        raise Failure(f"引擎进程 {first} 不在运行")
    for index in range(20):
        at.run()
        h.assert_no_exception(at)
        pid = h.engine_pid(at)
        if pid != first:
            raise Failure(f"第 {index + 1} 次交互后 pid 变了：{first} → {pid}")
    if len(at.chat_input) != 1 or at.chat_input[0].disabled:
        raise Failure("任务就绪且没有 run 在跑时，追问输入框应可用")


# --------------------------------------------------------------------------
# S64：引擎崩溃 → 可读错误 + 一键重建
# --------------------------------------------------------------------------
@case("S64 杀掉引擎：界面给可读错误并能重建", "§6.1 S64 / U3")
def _s64(h: Harness) -> None:
    at = h.app_with_task()
    pid = h.engine_pid(at)
    if pid is None:
        raise Failure("界面上读不到引擎 pid")
    h.kill_engine(pid)
    at.run()
    h.assert_no_exception(at)
    text = " ".join(str(item.value) for item in at.warning)
    if "已重建引擎" not in text:
        raise Failure(f"引擎死后没有走重建分支：{text[:200]!r}")
    rebuilt = h.engine_pid(at)
    if rebuilt is None or rebuilt == pid:
        raise Failure(f"重建后 pid 没有变化：{pid} → {rebuilt}")
    if not _process_alive(rebuilt):
        raise Failure(f"重建出来的引擎进程 {rebuilt} 不在运行")


# --------------------------------------------------------------------------
# M4 / S62：analyze 走 RPC，路径含中文与空格也照样出 612 包 / 570 会话
# --------------------------------------------------------------------------
@case("M4/S62 analyze：中文+空格路径，摘要 612 包 / 570 会话", "§6.1 S62 / U6")
def _m4(h: Harness) -> None:
    session = h.session()
    try:
        job = h.analyze_blocking(session, h.sample_with_odd_path())
        summary = job.summary
        if summary.get("packets") != 612 or summary.get("sessions") != 570:
            raise Failure(f"任务摘要不对：{summary}")
        if summary.get("alerts") != 2:
            raise Failure(f"synth-mixed 应该命中两条规则：{summary}")
        if not job.task_id.startswith("task_"):
            raise Failure(f"task_id 形状不对：{job.task_id}")
    finally:
        session.close()


# --------------------------------------------------------------------------
# S58：run 结束之前工具卡片就逐条出现
# --------------------------------------------------------------------------
@case("S58 工具调用在 run 结束前逐条可见", "§6.1 S58 / U1+U4")
def _s58(h: Harness) -> None:
    session = h.session()
    try:
        analyzed = h.analyze_blocking(session, h.sample)
        with _injected_agent(h, session):
            env = h.gui.settings.load_agent_env()
            job = h.gui.jobs.start_run(session, env, analyzed.task_id, "分析该捕获", mode="run")
            counts: list[int] = []
            while not job.done.wait(0.15):
                counts.append(len(job.live_traces()))
            counts.append(len(job.live_traces()))
        partial = [count for count in counts[:-1] if count > 0]
        if len(partial) < 2:
            raise Failure(f"run 期间没有观察到逐步出现的工具卡片：{counts[:10]}")
        if max(counts) != len(job.result.trace):
            raise Failure("实时 trace 的最终条数与 result.trace 不一致")
        if job.error is not None:
            raise Failure(f"run 失败：{job.error}")
        entry = job.live_traces()[0]
        expected = {"tool_name", "args", "result_summary", "status", "duration_ms", "tc_id"}
        if set(entry) != expected:
            missing = sorted(expected - set(entry))
            raise Failure(f"实时 trace 字段与 ToolCallRecord 不同构（缺 {missing}）")
        if not job.result.findings:
            raise Failure("mock provider 应当提交 findings")
    finally:
        session.close()


# --------------------------------------------------------------------------
# S59：停止按钮
# --------------------------------------------------------------------------
@case("S59 中途停止：下一步收尾、保留已得结论", "§6.1 S59 / U5")
def _s59(h: Harness) -> None:
    session = h.session()
    try:
        analyzed = h.analyze_blocking(session, h.sample)
        with _injected_agent(h, session):
            env = h.gui.settings.load_agent_env()
            job = h.gui.jobs.start_run(session, env, analyzed.task_id, "分析", mode="run")
            deadline = time.time() + 30
            while len(job.live_traces()) < 2 and time.time() < deadline and not job.done.is_set():
                time.sleep(0.05)
            if job.done.is_set():
                raise Failure("run 太快结束了，没能测到中途停止")
            if not job.request_stop():
                raise Failure("request_finalize() 没有接上")
            if not job.done.wait(60):
                raise Failure("停止之后 60s 还没收尾")
            result = job.result
        if result is None:
            raise Failure(f"停止之后拿不到结果：{job.error}")
        if result.status != "degraded" or result.stop_reason != "interrupted":
            raise Failure(f"收尾状态不对：status={result.status} stop_reason={result.stop_reason}")
        if not result.trace:
            raise Failure("已产生的工具调用被回滚了（部分结果必须保留）")
    finally:
        session.close()


# --------------------------------------------------------------------------
# M8 / M10 / M12 / S63：结论、告警、报告
# --------------------------------------------------------------------------
@case("M8/M10 结论 F-001/F-002 + 两条规则告警都在界面上", "§6.1 M8/M10 / §4.2")
def _m8(h: Harness) -> None:
    at = h.app_with_task()
    at.button(key="start_run").click().run()
    h.assert_no_exception(at)
    log = at.session_state["log"]
    assistant = [entry for entry in log if entry.get("role") == "assistant"]
    if not assistant:
        raise Failure("run 之后没有结论消息")
    findings = assistant[-1]["findings"]
    ids = [finding["finding_id"] for finding in findings]
    if ids[:2] != ["F-001", "F-002"]:
        raise Failure(f"结论编号不对：{ids}")
    titles = " ".join(str(finding["title"]) for finding in findings)
    if "NET-TCP-SYN-BURST-001" not in titles or "NET-TCP-PORT-SWEEP-001" not in titles:
        raise Failure(f"两条内置规则的结论缺失：{titles}")
    if not all(finding["evidence"] for finding in findings):
        raise Failure("有结论没有证据锚点")
    rendered = " ".join(str(element.value) for element in at.markdown)
    for needle in ("F-001", "F-002", "证据锚点", "规则告警"):
        if needle not in rendered:
            raise Failure(f"界面上找不到 {needle!r}")
    tables = list(getattr(at, "dataframe", []))
    if not tables:
        raise Failure("规则告警表没有渲染")
    rows = str(getattr(tables[0], "value", ""))
    if "NET-TCP-SYN-BURST-001" not in rows:
        raise Failure("告警表里没有 SYN 突发规则")


@case("M12/S63 报告：九节齐全、可下载、表头含模板/提示词/模型", "§6.1 M12/S63 / §4.2")
def _m12(h: Harness) -> None:
    at = h.app_with_task()
    at.button(key="start_run").click().run()
    at.button(key="make_report").click().run()
    h.assert_no_exception(at)
    report = at.session_state["report"]
    if not report:
        raise Failure("界面没有生成报告")
    meta = report["meta"]
    for key in ("template_version", "prompt_version", "model", "provider", "sha256"):
        if not meta.get(key):
            raise Failure(f"报告表头缺 {key}")
    text = report["text"]
    sections = [line for line in text.splitlines() if line.startswith("## ")]
    if len(sections) != 9:
        raise Failure(f"报告不是九节：{sections}")
    if "F-001" not in text or "tc_" not in text:
        raise Failure("报告里没有结论编号或证据锚点")
    success = " ".join(str(item.value) for item in at.success)
    if "九节齐全" not in success:
        raise Failure(f"界面上没有给出九节自检：{success[:120]!r}")
    if not at.get("download_button"):
        raise Failure("报告没有下载按钮")


def _process_alive(pid: int) -> bool:
    """进程还在吗（Windows 用 tasklist，POSIX 用 kill -0）。"""
    if os.name == "nt":
        done = subprocess.run(
            ["tasklist", "/FI", f"PID eq {pid}", "/NH"],
            capture_output=True,
            text=True,
            check=False,
        )
        return str(pid) in done.stdout
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def main(argv: list[str] | None = None) -> int:
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace")
        except (AttributeError, ValueError, OSError):
            pass
    parser = argparse.ArgumentParser(description="GUI cases S57-S65 / M4-M12")
    parser.add_argument("--binary", default=None)
    parser.add_argument("--filter", default=None)
    args = parser.parse_args(argv)

    root = Path(__file__).resolve().parents[2]
    default = root / "target" / "debug" / ("packetsage.exe" if os.name == "nt" else "packetsage")
    binary = (Path(args.binary) if args.binary else default).resolve()
    if not binary.is_file():
        print(f"app_cases: binary not found: {binary}", file=sys.stderr)
        return 2
    if not (root / "samples" / "synth-mixed.pcap").is_file():
        print(
            "app_cases: samples/synth-mixed.pcap is missing; generate it with "
            "`python scripts/gen_traffic.py --out samples/synth-mixed.pcap --packets 600 "
            "--profile mixed`",
            file=sys.stderr,
        )
        return 2

    harness = Harness(binary, root)
    harness.install_env()

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
    print(f"\n{len(selected) - len(failures)}/{len(selected)} GUI cases passed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
