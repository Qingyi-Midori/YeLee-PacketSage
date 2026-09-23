#!/usr/bin/env python3
"""把两个 sidecar 摆到 Tauri 要求的位置（U9 的打包前置，§7.1/§7.2）。

    python scripts/stage_desktop_sidecars.py            # 从现有产物摆放
    python scripts/stage_desktop_sidecars.py --build-agent   # 先跑 U2 再摆放

Tauri 的两条规矩：

* ``bundle.externalBin`` 的文件必须叫 ``<name>-<target-triple>[.exe]``，
  打包时会去掉三元组后缀装到应用目录；
* 目录型资源（Agent 的 ``--onedir`` 树）走 ``bundle.resources``。

于是引擎（单文件 exe）走 externalBin，Agent（onedir）整棵树走 resources：
``desktop/src-tauri/binaries/agent-sidecar/`` → 安装后的 ``<资源目录>/agent-sidecar/``。
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DESKTOP_BIN = ROOT / "desktop" / "src-tauri" / "binaries"

#: Windows 上的三元组；Tauri 用 `rustc -vV` 的 host，shell 是 MSVC（§7.7）。
TRIPLE = "x86_64-pc-windows-msvc"


def engine_source() -> Path:
    name = "packetsage.exe" if os.name == "nt" else "packetsage"
    for candidate in (
        ROOT / "target" / "release" / name,
        ROOT / "target" / "debug" / name,
    ):
        if candidate.is_file():
            return candidate
    raise SystemExit(
        "engine binary not found; build it first: "
        "powershell -ExecutionPolicy Bypass -File scripts/build.ps1 -CargoArgs --release"
    )


def agent_source() -> Path:
    name = "packetsage-agent.exe" if os.name == "nt" else "packetsage-agent"
    candidate = ROOT / "dist" / "agent-sidecar" / name
    if not candidate.is_file():
        raise SystemExit(
            "agent sidecar not found; build it first: python scripts/build_agent_sidecar.py"
        )
    return candidate


def stage(rebuild_agent: bool) -> None:
    if rebuild_agent:
        done = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "build_agent_sidecar.py")], cwd=str(ROOT)
        )
        if done.returncode != 0:
            raise SystemExit("building the agent sidecar failed")

    DESKTOP_BIN.mkdir(parents=True, exist_ok=True)
    engine = engine_source()
    target_engine = DESKTOP_BIN / f"packetsage-{TRIPLE}{engine.suffix}"
    shutil.copyfile(engine, target_engine)
    print(f"engine: {engine} -> {target_engine.relative_to(ROOT)}")

    source_tree = agent_source().parent
    target_tree = DESKTOP_BIN / "agent-sidecar"
    if target_tree.exists():
        shutil.rmtree(target_tree)
    shutil.copytree(source_tree, target_tree)
    size = sum(item.stat().st_size for item in target_tree.rglob("*") if item.is_file())
    print(
        f"agent:  {source_tree} -> {target_tree.relative_to(ROOT)} "
        f"({size / (1024 * 1024):.0f} MB, onedir tree)"
    )
    print("\nstaged for `npm run tauri build`")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="stage sidecars for the desktop bundle")
    parser.add_argument(
        "--build-agent",
        action="store_true",
        help="run scripts/build_agent_sidecar.py before staging",
    )
    args = parser.parse_args(argv)
    stage(rebuild_agent=bool(args.build_agent))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
