#!/usr/bin/env python3
"""Runs the installed CLI end to end and writes a transcript.

    python scripts/cli_demo.py --from-install install-test
    python scripts/cli_demo.py --binary target/release/packetsage

Every step is a real subprocess against the binary under test: the transcript
(`install-test/RUN.md` by default) records the exact command, its stdout/stderr
and its exit code, so the run is evidence rather than a claim. Expected exit
codes are asserted, so the script doubles as a smoke test of the installed CLI.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Optional

ROOT = Path(__file__).resolve().parents[1]


def force_utf8_console() -> None:
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            try:
                reconfigure(encoding="utf-8", errors="replace")
            except (ValueError, OSError):
                pass


class Step:
    """One CLI invocation plus its expectation."""

    def __init__(
        self,
        title: str,
        argv: list[str],
        *,
        binary: Optional[Path] = None,
        stdin: str = "",
        expect: int = 0,
        head: int = 0,
        env: Optional[dict] = None,
        check=None,
    ) -> None:
        self.title = title
        self.argv = argv
        self.binary = binary
        self.stdin = stdin
        self.expect = expect
        self.head = head
        self.env = env or {}
        self.check = check


def locate_binaries(args: argparse.Namespace) -> tuple[Path, Path]:
    """Returns (primary, chat-capable) binaries."""
    if args.binary:
        primary = Path(args.binary)
        if not primary.is_absolute():
            primary = ROOT / primary
        if not primary.exists() and primary.with_suffix(".exe").exists():
            primary = primary.with_suffix(".exe")
        if not primary.exists():
            raise SystemExit(f"binary not found: {primary}")
        return primary, primary

    directory = Path(args.from_install)
    if not directory.is_absolute():
        directory = ROOT / directory
    source = directory / "source" / "bin" / ("packetsage.exe" if os.name == "nt" else "packetsage")
    tarballs = sorted((directory / "tarball").glob("*/packetsage.exe" if os.name == "nt" else "*/packetsage"))
    if tarballs:
        primary = tarballs[-1]
    elif source.exists():
        primary = source
    else:
        raise SystemExit(
            f"no installed CLI under {directory}; run `python scripts/install_smoke.py` first"
        )
    return primary, source if source.exists() else primary


def build_steps(primary: Path, chatty: Path, work: Path, sample: Path) -> list[Step]:
    db_url = f"sqlite://{work / 'demo.db'}"
    readonly = f"{db_url}?mode=ro"
    fresh_url = f"sqlite://{work / 'migrate.db'}"
    fresh_path = work / "migrate.db"
    if fresh_path.exists():
        fresh_path.unlink()
    agent_env = {"PYTHONPATH": str(ROOT / "agent")}
    return [
        Step("version", ["version"], binary=primary),
        Step("version --json", ["version", "--json"], binary=primary),
        # The transcript shows the ASCII title *and* the usage block.
        Step("--help (top: ASCII 标题 + usage)", ["--help"], binary=primary, head=34),
        Step(
            "doctor --no-net",
            ["doctor", "--no-net", "--rules-dir", str(primary.parent / "rules"), "--samples-dir", str(work)],
            binary=primary,
        ),
        Step("analyze (human summary)", ["analyze", str(sample)], binary=primary, head=14),
        Step(
            "analyze --jsonl (machine stream)",
            ["analyze", str(sample), "--jsonl", "-q"],
            binary=primary,
            head=2,
            check=lambda out: len(out.splitlines()) > 100,
        ),
        Step("db migrate --yes (全新库：打印 current→target→pending)", ["db", "migrate", "--db", fresh_url, "--yes"], binary=primary, head=8),
        Step("analyze --db (落库，含规则告警)", ["analyze", str(sample), "--db", db_url], binary=primary, head=6),
        Step("query alerts", ["query", "alerts", "--db", db_url], binary=primary, head=6),
        Step(
            "db query --readonly (table)",
            ["db", "query", "--readonly", "--db", readonly, "--sql", "SELECT id, packet_count FROM analysis_tasks"],
            binary=primary,
        ),
        Step(
            "db query --readonly --jsonl",
            ["db", "query", "--readonly", "--jsonl", "--db", readonly, "--sql", "SELECT name FROM sqlite_master ORDER BY name"],
            binary=primary,
            check=lambda out: all(json.loads(line) for line in out.splitlines() if line.strip()),
        ),
        Step(
            "db query 护栏：写库 URL 被拒",
            ["db", "query", "--readonly", "--db", db_url, "--sql", "SELECT 1"],
            binary=primary,
            expect=1,
        ),
        Step("completions bash", ["completions", "bash"], binary=primary, head=3),
        Step("schema", ["schema"], binary=primary, head=10),
        Step(
            "chat 非 tty：Agent 拒绝交互（退出码 1，提示改用 `packetsage-agent run`）",
            ["chat", str(sample), "--provider", "mock"],
            binary=chatty,
            stdin="最可疑的会话\n/quit\n",
            env=agent_env,
            expect=1,
            head=6,
        ),
    ]


def main(argv: Optional[list[str]] = None) -> int:
    force_utf8_console()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=None, help="single binary to exercise")
    parser.add_argument("--from-install", default="install-test", help="install test directory")
    parser.add_argument("--out", default=None, help="transcript path (default <dir>/RUN.md)")
    parser.add_argument("--capture", default=None, help="capture file to analyse")
    args = parser.parse_args(argv)

    directory = Path(args.from_install)
    if not directory.is_absolute():
        directory = ROOT / directory
    work = directory / "work"
    work.mkdir(parents=True, exist_ok=True)
    sample = Path(args.capture) if args.capture else work / "sample.pcap"
    if not sample.exists():
        sample = ROOT / "samples" / "synth-mixed.pcap"
    if not sample.exists():
        raise SystemExit("no capture to analyse; run scripts/install_smoke.py first")
    out_path = Path(args.out) if args.out else directory / "RUN.md"

    primary, chatty = locate_binaries(args)
    print(f"CLI under test : {primary}")
    print(f"chat-capable   : {chatty}")
    print(f"transcript     : {out_path}\n")

    blocks: list[str] = [
        "# 已安装 CLI 的实际运行记录",
        "",
        f"- 时间：{dt.datetime.now().astimezone().strftime('%Y-%m-%d %H:%M:%S %z')}",
        f"- 主二进制：`{primary}`",
        f"- chat 用二进制：`{chatty}`",
        f"- 样本：`{sample}`",
        f"- 工作目录：`{work}`",
        "",
        "> 由 `python scripts/cli_demo.py` 生成：每条都是真实子进程，含命令、输出与退出码。",
        "",
    ]
    steps = build_steps(primary, chatty, work, sample)
    failures = 0
    for step in steps:
        binary = step.binary or primary
        env = os.environ.copy()
        for name in ("PACKETSAGE_CONFIG", "PACKETSAGE_AGENT_BIN", "PACKETSAGE_LLM_PROVIDER"):
            env.pop(name, None)
        env.setdefault("PYTHONIOENCODING", "utf-8")
        env.setdefault("PYTHONUTF8", "1")
        env.update(step.env)
        result = subprocess.run(
            [str(binary), *step.argv],
            cwd=str(work),
            env=env,
            input=step.stdin,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=600,
        )
        ok = result.returncode == step.expect
        detail = ""
        if ok and step.check is not None:
            try:
                ok = bool(step.check(result.stdout))
            except Exception as error:  # noqa: BLE001 - reported as a failure
                ok, detail = False, f"{type(error).__name__}: {error}"
        if not ok:
            failures += 1
            detail = detail or f"exit {result.returncode} != {step.expect}"

        print(f"{'ok  ' if ok else 'FAIL'} {step.title}  (exit {result.returncode})")
        for line in (result.stdout or "").splitlines()[: step.head]:
            print("     " + line)
        if step.head:
            print("     ...")
        for line in (result.stderr or "").strip().splitlines()[-2:]:
            print("  e| " + line)

        blocks.append(f"## {step.title}")
        blocks.append("")
        blocks.append("```console")
        blocks.append(f"$ packetsage {' '.join(step.argv)}".rstrip())
        if step.stdin:
            blocks.append(f"< (stdin) {step.stdin.strip()!r}")
        body = (result.stdout or "").rstrip()
        if step.head:
            body = "\n".join(body.splitlines()[: step.head]) + "\n... (truncated)"
        if body:
            blocks.append(body)
        stderr = (result.stderr or "").rstrip()
        if stderr:
            blocks.append("[stderr]")
            blocks.append("\n".join(stderr.splitlines()[-12:]))
        blocks.append(f"[exit {result.returncode}]" + ("" if ok else f"   <-- EXPECTED {step.expect}  {detail}"))
        blocks.append("```")
        blocks.append("")

    blocks.append(f"**结果：{len(steps) - failures}/{len(steps)} 步符合预期。**")
    blocks.append("")
    out_path.write_text("\n".join(blocks), encoding="utf-8")
    print(f"\n{failures} step(s) failed; transcript -> {out_path}")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
