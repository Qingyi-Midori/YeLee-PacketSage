#!/usr/bin/env python3
"""量一次 run 到底慢在哪：**LLM 往返 vs 工具**，以及"削步数"能省多少。

    cd agent && python tests/bench_roundtrips.py
    cd agent && python tests/bench_roundtrips.py --ttft 3.0

它跑两组对照，引擎用 `tests/fake_engine.py`（毫秒级），provider 用 mock +
``slow:<ttft>``——每次"模型往返"固定睡 `ttft` 秒，**模拟真实 provider 的
首字延迟**。这样两组之间唯一的差别就是往返次数：

* **探索式**：`preload=off` + 默认脚本（summary/stats/conversations/alerts 都要
  模型自己点）——就是现在的线上行为；
* **预取式**：`preload=on` + `lean` 脚本（事实已经在第一条消息里，只按需 pivot）
  ——代表一个"看到事实就不再重复拉取"的模型。

两者差的不是模型速度，而是**串行往返的乘数**：真实 provider 下这部分就是
墙钟时间的主要来源。真 key 的 E1–E6 复测要单独跑（需要预算）。
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent.agent import build_agent  # noqa: E402
from packetsage_agent.engine_client import EngineClient  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
TASK_ID = "task_01J0000000000000000000000G"


def engine_command() -> list[str]:
    """真引擎优先（`target/release/packetsage[.exe]`），否则用假引擎。"""
    name = "packetsage.exe" if os.name == "nt" else "packetsage"
    for candidate in (
        ROOT / "target" / "release" / name,
        ROOT / "target" / "debug" / name,
    ):
        if candidate.is_file():
            return [str(candidate), "serve"]
    return [sys.executable, str(ROOT / "agent" / "tests" / "fake_engine.py"), "normal"]


def run_once(
    preload: bool, scenario: str, ttft: float, task_id: str, db_url: str
) -> dict:
    """一次 run；返回计数与耗时（引擎由 `EngineClient` 起，和 CLI 同一条路）。"""
    # 引擎的库地址只从环境读（`serve` 没有 `--db`），所以这里显式注入。
    # 往返延迟走环境变量：场景名因此保持干净（`default` vs `lean`）。mock provider
    # 跑在**本进程**里，所以这个变量设在本进程；库地址才是给引擎子进程的。
    os.environ["PACKETSAGE_MOCK_LATENCY_S"] = str(ttft)
    env = {**os.environ, "PACKETSAGE_STORAGE_URL": db_url, "PACKETSAGE_DB": db_url}
    with EngineClient(engine_command(), env=env) as engine:
        agent = build_agent(
            engine,
            provider_kind="mock",
            model="mock",
            scenario=scenario,
            preload=preload,
        )
        started = time.perf_counter()
        result = agent.run(task_id, "分析该捕获并给出可追溯结论")
        wall_ms = int((time.perf_counter() - started) * 1000)
        state = agent.policy.state
        return {
            "preload": preload,
            "scenario": scenario,
            "wall_ms": wall_ms,
            "llm_ms": state.llm_ms,
            "tool_ms": state.tool_ms,
            "llm_calls": state.llm_calls,
            "tool_calls": state.tool_calls,
            "preloaded": state.preloaded,
            "steps": state.steps,
            "findings": len(result.findings),
            "status": result.status,
        }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="LLM round trips vs tool time")
    parser.add_argument("--ttft", type=float, default=1.5, help="simulated seconds per LLM round")
    parser.add_argument("--engine", help="engine command (default: auto)")
    args = parser.parse_args(argv)

    global engine_command  # noqa: PLW0603 - 命令行覆盖默认解析
    if args.engine:
        argv_engine = shlex.split(args.engine)
        engine_command = lambda: argv_engine  # noqa: E731

    work = Path(tempfile.mkdtemp(prefix="packetsage-bench-"))
    db_url = f"sqlite://{work / 'bench.db'}"
    sample = ROOT / "samples" / "synth-mixed.pcap"
    engine_bin = engine_command()[0]
    done = subprocess.run(
        [
            engine_bin,
            "analyze",
            str(sample),
            "--db",
            db_url,
            "--task-id",
            TASK_ID,
            "-q",
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if done.returncode != 0:
        print(f"analyze failed: {done.stderr[-400:]}", file=sys.stderr)
        return 1

    rows = [
        run_once(False, "default", args.ttft, TASK_ID, db_url),
        run_once(True, "lean", args.ttft, TASK_ID, db_url),
    ]
    print(json.dumps(rows, ensure_ascii=False, indent=2))
    before, after = rows
    saved = before["wall_ms"] - after["wall_ms"]
    print(
        f"\n探索式 {before['llm_calls']} 次往返 / {before['wall_ms']} ms  →  "
        f"预取式 {after['llm_calls']} 次往返 / {after['wall_ms']} ms  "
        f"（省 {saved} ms，{(saved / max(1, before['wall_ms'])) * 100:.0f}%）"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
