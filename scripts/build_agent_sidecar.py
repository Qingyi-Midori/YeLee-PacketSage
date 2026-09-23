#!/usr/bin/env python3
"""U2：把 Agent 打成**独立可执行文件**（桌面应用的 sidecar）。

    python scripts/build_agent_sidecar.py                 # 构建 + 自检
    python scripts/build_agent_sidecar.py --skip-build    # 只跑自检

为什么是 ``--onedir`` 而不是 ``--onefile``（《GUI 工程规格书 v0.2》§7.2）：

* 启动快：onefile 每次要把几百 MB 解到临时目录，桌面应用每次启动都要付这个代价；
* 少被 AV 折腾：onefile 的"自解压 + 执行"正是杀软最敏感的行为。

产物：``dist/agent-sidecar/packetsage-agent.exe``（连同 ``_internal/`` 一起分发）。
数据文件 ``prompts_text/*.txt`` 与 ``banner.txt`` 由 ``packagesage_agent`` 的
package-data 声明，这里显式 ``--add-data``，免得打包器"看起来成功、跑起来找不到提示词"。
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AGENT = ROOT / "agent"
PACKAGE = AGENT / "packetsage_agent"
DIST = ROOT / "dist" / "agent-sidecar"
EXE_NAME = "packetsage-agent"


def run(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess:
    print("+", " ".join(argv), flush=True)
    return subprocess.run(argv, text=True, encoding="utf-8", errors="replace", **kwargs)


def pyinstaller_argv(work: Path) -> list[str]:
    """The PyInstaller command; every include is explicit (see module docstring)."""
    return [
        sys.executable,
        "-m",
        "PyInstaller",
        "--noconfirm",
        "--clean",
        "--onedir",
        "--name",
        EXE_NAME,
        "--distpath",
        str(work / "dist"),
        "--workpath",
        str(work / "build"),
        "--specpath",
        str(work),
        # The prompt texts and the ASCII title ship as package data; a packaged
        # agent that cannot find them fails only at run time, so they are added
        # explicitly instead of trusting the collector.
        "--add-data",
        f"{PACKAGE / 'prompts_text'}{os.pathsep}packetsage_agent/prompts_text",
        "--add-data",
        f"{PACKAGE / 'banner.txt'}{os.pathsep}packetsage_agent",
        # Optional groups are not part of the sidecar (it never drives LangChain).
        "--exclude-module",
        "langchain",
        "--exclude-module",
        "langchain_openai",
        "--exclude-module",
        "tkinter",
        "--exclude-module",
        "pytest",
        "--exclude-module",
        "streamlit",
        # Hidden imports the CLI/agent reach lazily (kept short on purpose: every
        # entry is a module the sidecar really imports at run time).
        # `agent/` carries the package source; the editable install in this
        # workspace hides it behind a MetaPathFinder that PyInstaller cannot
        # follow, so the source dir is put on the analysis path explicitly.
        "--paths",
        str(AGENT),
        "--collect-submodules",
        "packetsage_agent",
        # `_version.py` answers through importlib.metadata; without the copied
        # dist-info a frozen sidecar would report `0.0.0+unknown`.
        "--copy-metadata",
        "packetsage-agent",
        str(ROOT / "scripts" / "agent_sidecar_entry.py"),
    ]


def build(work: Path) -> None:
    """Build into the work dir, then move the tree to its documented home.

    PyInstaller always emits ``<distpath>/<name>/``; the layout we promise is
    ``dist/agent-sidecar/packetsage-agent.exe``, so the directory is moved after
    the build instead of fighting the tool's naming.
    """
    if DIST.exists():
        shutil.rmtree(DIST, ignore_errors=True)
    done = run(pyinstaller_argv(work), cwd=str(ROOT))
    if done.returncode != 0:
        raise SystemExit(f"PyInstaller failed with exit {done.returncode}")
    produced = work / "dist" / EXE_NAME
    if not produced.is_dir():
        raise SystemExit(f"PyInstaller did not produce {produced}")
    DIST.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(str(produced), str(DIST))


def exe_path() -> Path:
    return DIST / f"{EXE_NAME}.exe" if os.name == "nt" else DIST / EXE_NAME


def smoke() -> None:
    """U2 判据的可自动化的那一半：不借开发机的 Python 也能跑。

    做法是把子进程环境擦干净（去掉 ``PYTHON*``、PATH 里不含解释器），
    模拟"目标机没装 Python"；真正的干净机器验证是 §8.2 的 M1–M4。
    """
    binary = exe_path()
    if not binary.is_file():
        raise SystemExit(f"sidecar not found: {binary}")
    env = {key: value for key, value in os.environ.items() if not key.startswith("PYTHON")}
    env["PATH"] = os.pathsep.join(
        part
        for part in env.get("PATH", "").split(os.pathsep)
        if "python" not in part.lower() and part
    )
    version = subprocess.run(
        [str(binary), "--version"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        timeout=120,
    )
    print(f"smoke --version -> exit {version.returncode}: {version.stdout.strip()!r}")
    if version.returncode != 0 or not version.stdout.strip().startswith("packetsage-agent "):
        raise SystemExit(f"--version failed: {version.stderr[-400:]}")

    # `--help` heads with the ASCII title, so it proves banner.txt shipped (a
    # bundle that "builds fine" but cannot read its own package data is the
    # failure mode this check exists for).
    helped = subprocess.run(
        [str(binary), "--help"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        timeout=120,
    )
    if helped.returncode != 0 or "###" not in helped.stdout:
        raise SystemExit(
            f"--help lost the ASCII title (package data missing): "
            f"{helped.stdout[:200]!r} {helped.stderr[-300:]!r}"
        )
    prompts = list((DIST / "_internal").rglob("prompts_text/*.txt"))
    if not prompts:
        raise SystemExit("prompts_text/*.txt is not in the bundle")
    print(f"smoke --help  -> ok: ASCII title + {len(prompts)} prompt file(s) bundled")

    # 握手自检：喂一帧 hello，必须拿到一帧 ack（不碰引擎、不需要 provider）。
    import json

    frame = json.dumps(
        {
            "id": "smoke-hello",
            "type": "cmd",
            "command": "hello",
            "params": {
                "protocol_version": 1,
                "client": "packetsage-desktop",
                "client_version": "4.0.0",
            },
        }
    )
    engine = _engine_binary()
    argv = [str(binary), "serve", "--db", f"sqlite://{work_dir() / 'smoke.db'}"]
    if engine is not None:
        argv += ["--engine", f'"{engine}" serve']
    else:
        print("smoke serve  -> skipped handshake: no packetsage engine found on this host")
        return
    served = subprocess.run(
        argv,
        input=frame + "\n",
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        timeout=180,
    )
    lines = [line for line in served.stdout.splitlines() if line.strip()]
    parsed = [json.loads(line) for line in lines if line.lstrip().startswith("{")]
    acks = [item for item in parsed if item.get("type") == "ack"]
    if not acks or not acks[0].get("ok"):
        raise SystemExit(
            f"handshake failed (exit {served.returncode}): stdout={served.stdout[:300]!r} "
            f"stderr={served.stderr[-400:]!r}"
        )
    result = acks[0]["result"]
    print(
        f"smoke serve  -> exit {served.returncode}: protocol={result['protocol_version']} "
        f"agent={result['agent_version']} capabilities={result['capabilities']}"
    )
    if not (DIST / "_internal").is_dir():
        raise SystemExit("--onedir layout is missing _internal/ (bundle is incomplete)")
    size_mb = _tree_size_mb(DIST)
    print(f"sidecar size: {size_mb:.0f} MB ({len(list(DIST.rglob('*')))} entries)")
    if size_mb > 400:
        print("WARN: sidecar bigger than the 40-80MB budget of §7.2 —— check excludes")


def _tree_size_mb(path: Path) -> float:
    total = sum(item.stat().st_size for item in path.rglob("*") if item.is_file())
    return total / (1024 * 1024)


def work_dir() -> Path:
    """A scratch dir for the smoke run's database (never the user's packetsage.db)."""
    scratch = Path(tempfile.gettempdir()) / "packetsage-sidecar-smoke"
    scratch.mkdir(parents=True, exist_ok=True)
    return scratch


def _engine_binary() -> str | None:
    """The engine to hand the sidecar: $PACKETSAGE_ENGINE → PATH → build tree."""
    from_env = os.environ.get("PACKETSAGE_ENGINE")
    if from_env:
        return from_env
    found = shutil.which("packetsage")
    if found:
        return found
    for candidate in (
        ROOT / "target" / "release" / "packetsage.exe",
        ROOT / "target" / "release" / "packetsage",
        ROOT / "target" / "debug" / "packetsage.exe",
        ROOT / "target" / "debug" / "packetsage",
    ):
        if candidate.is_file():
            return str(candidate)
    return None


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="build the Agent sidecar (U2)")
    parser.add_argument("--skip-build", action="store_true", help="only run the smoke checks")
    parser.add_argument("--keep-work", action="store_true", help="keep PyInstaller work files")
    args = parser.parse_args(argv)

    work = Path(tempfile.mkdtemp(prefix="packetsage-agent-build-"))
    try:
        if not args.skip_build:
            build(work)
        smoke()
    finally:
        if not args.keep_work:
            shutil.rmtree(work, ignore_errors=True)
    print(f"\nagent sidecar ready: {exe_path()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
