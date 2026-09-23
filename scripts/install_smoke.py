#!/usr/bin/env python3
"""Creates a local *install test directory* and exercises both install paths.

    python scripts/install_smoke.py                 # default: ./install-test
    python scripts/install_smoke.py --dir /tmp/ps   # any empty directory
    python scripts/install_smoke.py --skip-source-install --binary target/debug/packetsage.exe

What it produces inside the directory:

```text
install-test/
├── source/          cargo install --root … (源码路径, §2.1)
├── dist/            packetsage-<ver>-<os>-<arch>.tar.gz (脚本产出)
├── tarball/         解包后的发行版
├── work/            运行时工作目录（样本、ci.db、事件流）
├── bin/             仅用于"Agent 不可用"场景的桩件
└── RESULTS.md       本目录的演练记录（对应规格 #34）
```

This is the local counterpart of CI's `install-from-scratch` and
`release-tarball` jobs, and the evidence file for CLI 收口工程规格书 #34
("tarball 在干净容器的 install-from-scratch 提前演练结果记录").
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path
from typing import Optional

ROOT = Path(__file__).resolve().parents[1]


def toolchain_env() -> tuple[dict, list[str]]:
    """Mirrors `scripts/build.ps1` so this script also works from a bare shell.

    A bare shell on this host has neither `cargo` nor MinGW on PATH: cargo lives
    in `%USERPROFILE%\\.cargo\\bin`, the assembler (`as.exe`) only ships with
    MinGW, and the linker driver must be one whose own path has no spaces
    (rustup's self-contained gcc). Without all three, `cargo build` fails with
    `dlltool.exe` not found or `cannot find C:/Users/...`.
    """
    if not sys.platform.startswith("win"):
        return {}, []
    env: dict[str, str] = {}
    notes: list[str] = []
    prepends: list[str] = []

    cargo_bin = Path(os.environ.get("USERPROFILE", "")) / ".cargo" / "bin"
    if (cargo_bin / "cargo.exe").exists():
        prepends.append(str(cargo_bin))
        notes.append(f"cargo={cargo_bin}")

    candidates = [
        Path("C:/mingw64"),
        Path(os.environ.get("LOCALAPPDATA", "")) / "Programs/mingw64",
        Path("C:/msys64/mingw64"),
        Path("C:/MinGW"),
        Path(os.environ.get("ProgramFiles", "")) / "mingw64",
    ]
    for root in candidates:
        bin_dir = root / "bin"
        if (bin_dir / "as.exe").exists() and (bin_dir / "dlltool.exe").exists():
            prepends.append(str(bin_dir))
            notes.append(f"mingw={bin_dir}")
            break
    if prepends:
        env["PATH"] = os.pathsep.join(prepends + [os.environ.get("PATH", "")])

    toolchains = Path(os.environ.get("USERPROFILE", "")) / ".rustup/toolchains"
    if toolchains.is_dir():
        gccs = sorted(
            toolchains.glob(
                "*windows-gnu*/lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained/"
                "x86_64-w64-mingw32-gcc.exe"
            )
        )
        if gccs:
            env["CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER"] = str(gccs[-1])
            notes.append("linker=rustup self-contained gcc")
    return env, notes


#: Applied to every child process: cargo needs it, the CLI itself does not.
TOOLCHAIN_ENV, TOOLCHAIN_NOTES = toolchain_env()


def find_cargo() -> str:
    """Absolute cargo path: Windows resolves the program with the *caller's*
    PATH, so passing a PATH in `env` alone cannot locate cargo.exe."""
    candidates: list[Path] = []
    if os.environ.get("USERPROFILE"):
        candidates.append(Path(os.environ["USERPROFILE"]) / ".cargo" / "bin" / "cargo.exe")
    if os.environ.get("CARGO_HOME"):
        candidates.append(Path(os.environ["CARGO_HOME"]) / "bin" / "cargo.exe")
    for candidate in candidates:
        if candidate.exists():
            return str(candidate)
    return shutil.which("cargo") or "cargo"


CARGO = find_cargo()


def force_utf8_console() -> None:
    """Console code pages (GBK on this host) must not crash the runner (§0.1)."""
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            try:
                reconfigure(encoding="utf-8", errors="replace")
            except (ValueError, OSError):  # pragma: no cover - stream without support
                pass


class Recorder:
    """Collects one row per check so RESULTS.md can be regenerated cheaply."""

    def __init__(self) -> None:
        self.rows: list[tuple[str, str, str]] = []

    def ok(self, name: str, detail: str = "") -> None:
        self.rows.append(("pass", name, detail))
        print(f"ok   {name}" + (f"  {detail}" if detail else ""))

    def fail(self, name: str, detail: str = "") -> None:
        self.rows.append(("FAIL", name, detail))
        print(f"FAIL {name}  {detail}", file=sys.stderr)

    def skip(self, name: str, detail: str = "") -> None:
        self.rows.append(("skip", name, detail))
        print(f"skip {name}  {detail}")

    @property
    def failures(self) -> int:
        return sum(1 for status, _, _ in self.rows if status == "FAIL")


def run(
    args: list[str],
    *,
    cwd: Optional[Path] = None,
    env: Optional[dict] = None,
    binary: Optional[Path] = None,
    stdin: str = "",
    timeout: int = 600,
) -> subprocess.CompletedProcess:
    """Runs a command, optionally replacing argv[0] with a binary path."""
    argv = [str(binary), *args] if binary else args
    merged = os.environ.copy()
    merged.update(TOOLCHAIN_ENV)
    for name in ("PACKETSAGE_CONFIG", "PACKETSAGE_AGENT_BIN", "PACKETSAGE_LLM_PROVIDER"):
        merged.pop(name, None)
    # Children must speak UTF-8 even when the console code page is GBK, or the
    # `packetsage·task_…>> ` prompt arrives as replacement characters.
    merged.setdefault("PYTHONUTF8", "1")
    merged.setdefault("PYTHONIOENCODING", "utf-8")
    if env:
        merged.update(env)
    return subprocess.run(
        argv,
        cwd=str(cwd) if cwd else None,
        env=merged,
        input=stdin,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def locate_binary(explicit: Optional[str], release: bool = True) -> Path:
    if explicit:
        candidate = Path(explicit)
        if not candidate.is_absolute():
            candidate = ROOT / candidate
        if candidate.exists():
            return candidate
        raise SystemExit(f"binary not found: {candidate}")
    profile = "release" if release else "debug"
    candidate = ROOT / "target" / profile / "packetsage"
    if not candidate.exists() and candidate.with_suffix(".exe").exists():
        candidate = candidate.with_suffix(".exe")
    return candidate


def build_release(recorder: Recorder) -> Path:
    """Builds the release binary (incremental: it is a no-op when fresh).

    Always invoking cargo keeps the packaged binary in sync with the sources —
    reusing whatever sits in `target/release` would silently ship a stale CLI.
    """
    binary = locate_binary(None)
    started = dt.datetime.now()
    result = run([CARGO, "build", "--release", "-p", "packetsage-cli"], cwd=ROOT)
    if result.returncode != 0:
        tail = (result.stderr or result.stdout).strip().splitlines()[-6:]
        hint = ""
        if any("dlltool" in line for line in tail):
            hint = (
                "  hint: GNU host needs MinGW's dlltool plus an assembler on PATH; see "
                "docs/ux-walkthrough.md §0.1 for the working linker combination."
        )
        raise SystemExit("cargo build --release failed:\n  " + "\n  ".join(tail) + "\n" + hint)
    if not binary.exists():
        raise SystemExit(f"cargo build --release did not produce {binary}")
    elapsed = (dt.datetime.now() - started).total_seconds()
    recorder.ok(
        "cargo build --release",
        f"{binary.relative_to(ROOT)} in {elapsed:.1f}s",
    )
    return binary


def ensure_sample(work: Path, recorder: Recorder) -> Path:
    """Copies (or generates) the capture the smoke tests analyse."""
    source = ROOT / "samples" / "synth-mixed.pcap"
    if not source.exists():
        generated = run(
            [
                sys.executable,
                "scripts/gen_traffic.py",
                "--out",
                "samples/synth-mixed.pcap",
                "--packets",
                "600",
                "--profile",
                "mixed",
            ],
            cwd=ROOT,
        )
        if generated.returncode != 0:
            raise SystemExit("samples/synth-mixed.pcap is missing and could not be generated")
        recorder.ok("generate sample", "samples/synth-mixed.pcap")
    target = work / "sample.pcap"
    shutil.copy2(source, target)
    return target


def check_source_install(args: argparse.Namespace, directory: Path, recorder: Recorder) -> None:
    """§2.1: `cargo install --path … --root …`, then `version` from elsewhere."""
    root = directory / "source"
    result = run(
        [
            CARGO,
            "install",
            "--path",
            "crates/packetsage-cli",
            "--root",
            str(root),
            "--locked",
        ],
        cwd=ROOT,
    )
    if result.returncode != 0:
        # `--locked` can fail on a stale lockfile; retry once without it.
        result = run(
            [CARGO, "install", "--path", "crates/packetsage-cli", "--root", str(root)],
            cwd=ROOT,
        )
    if result.returncode != 0:
        recorder.fail("cargo install --path (源码安装)", result.stderr.strip().splitlines()[-1])
        return
    binary = root / "bin" / ("packetsage.exe" if os.name == "nt" else "packetsage")
    if not binary.exists():
        recorder.fail("cargo install --path (源码安装)", f"{binary} missing after install")
        return
    version = run(["version"], binary=binary, cwd=directory)
    if version.returncode != 0 or "schema_version=" not in version.stdout:
        recorder.fail("installed binary answers `version`", version.stderr.strip())
        return
    recorder.ok(
        "cargo install --path (源码安装)",
        f"{binary.relative_to(directory)} -> {version.stdout.strip()}",
    )
    # §2.1 acceptance: usable from an unrelated directory.
    elsewhere = run(["version", "--json"], binary=binary, cwd=directory / "work")
    payload = json.loads(elsewhere.stdout)
    expected = {"version", "schema_version", "git_hash", "build_time", "profile", "rustc", "target"}
    if set(payload) != expected:
        recorder.fail("version --json key set (json 输出格式 v1)", str(sorted(payload)))
    else:
        recorder.ok("version --json key set (json 输出格式 v1)", f"profile={payload['profile']}")


def check_tarball(
    args: argparse.Namespace, directory: Path, binary: Path, recorder: Recorder
) -> Optional[Path]:
    """§12: build the tarball, unpack it, verify SHA256SUMS, return its root."""
    dist = directory / "dist"
    packed = run(
        [
            sys.executable,
            "scripts/package_release.py",
            "--binary",
            str(binary),
            "--out-dir",
            str(dist),
        ],
        cwd=ROOT,
    )
    if packed.returncode != 0:
        recorder.fail("package release tarball", packed.stderr.strip().splitlines()[-1])
        return None
    tarballs = sorted(dist.glob("packetsage-*.tar.gz"))
    if not tarballs:
        recorder.fail("package release tarball", "no tarball produced")
        return None
    tarball = tarballs[-1]
    recorder.ok("package release tarball", f"{tarball.name} sha256={sha256(tarball)[:12]}")

    unpacked = directory / "tarball"
    if unpacked.exists():
        shutil.rmtree(unpacked)
    unpacked.mkdir(parents=True)
    with tarfile.open(tarball, "r:gz") as archive:
        archive.extractall(unpacked)  # noqa: S202 - our own artifact
    roots = [entry for entry in unpacked.iterdir() if entry.is_dir()]
    if len(roots) != 1:
        recorder.fail("unpack tarball", f"expected exactly one top-level directory, got {len(roots)}")
        return None
    root = roots[0]
    for required in ("INSTALL.md", "SHA256SUMS", "rules/builtin"):
        if not (root / required).exists():
            recorder.fail("tarball contents", f"{required} is missing")
            return None
    recorder.ok("tarball contents", "binary + rules/builtin/ + INSTALL.md + SHA256SUMS")

    # SHA256SUMS verification, exactly as INSTALL.md tells the user to do it.
    mismatched = []
    for line in (root / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        digest, _, name = line.partition("  ")
        target = root / name
        if not target.exists() or sha256(target) != digest:
            mismatched.append(name)
    if mismatched:
        recorder.fail("SHA256SUMS", f"mismatched: {mismatched}")
    else:
        recorder.ok("SHA256SUMS", "every listed file matches")
    return root


def check_capabilities(
    args: argparse.Namespace, root: Path, work: Path, sample: Path, recorder: Recorder
) -> None:
    """The tarball capability matrix of INSTALL.md, run in its own directory."""
    binary = root / ("packetsage.exe" if os.name == "nt" else "packetsage")

    version = run(["version"], binary=binary, cwd=work)
    if version.returncode != 0:
        recorder.fail("tarball `version`", version.stderr.strip())
    else:
        recorder.ok("tarball `version`", version.stdout.strip())

    analysed = run(["analyze", str(sample)], binary=binary, cwd=work)
    if analysed.returncode != 0 or "packets" not in analysed.stdout:
        recorder.fail("tarball `analyze`", analysed.stderr.strip()[-160:])
    else:
        packets = next(
            (line for line in analysed.stdout.splitlines() if line.startswith("packets")), ""
        )
        recorder.ok("tarball `analyze`", packets.strip())
        recorder.ok("progress goes to stderr", analysed.stderr.strip().splitlines()[-1])

    jsonl = run(["analyze", str(sample), "--jsonl", "-q"], binary=binary, cwd=work)
    lines = [line for line in jsonl.stdout.splitlines() if line.strip()]
    parsed = all(json.loads(line) for line in lines)
    if jsonl.returncode != 0 or not lines or not parsed:
        recorder.fail("tarball `analyze --jsonl`", "stdout is not pure JSONL")
    else:
        versions = {
            json.loads(line).get("schema_version") for line in lines if "schema_version" in line
        }
        recorder.ok("tarball `analyze --jsonl`", f"{len(lines)} event(s), schema_version={versions}")

    rules = run(["rules", "list"], binary=binary, cwd=work)
    builtin = run(["rules", "list", "--dir", str(root / "rules" / "builtin")], binary=binary, cwd=work)
    if rules.returncode != 0 or builtin.returncode != 0:
        recorder.fail("tarball `rules list`", builtin.stderr.strip()[-160:])
    else:
        recorder.ok("tarball `rules list`", f"embedded + {len(builtin.stdout.splitlines())} file rule(s)")

    doctor = run(["doctor", "--no-net", "--rules-dir", str(root / "rules"), "--samples-dir", str(work)],
                 binary=binary, cwd=work)
    if doctor.returncode != 0:
        recorder.fail("tarball `doctor --no-net`", doctor.stderr.strip()[-160:])
    else:
        recorder.ok("tarball `doctor --no-net`", "all checks passed")
    doctor_json = run(["doctor", "--json", "--no-net"], binary=binary, cwd=work)
    items = json.loads(doctor_json.stdout)["items"] if doctor_json.returncode == 0 else []
    if len(items) != 10:
        recorder.fail("tarball `doctor --json`", f"expected 10 items, got {len(items)}")
    else:
        recorder.ok("tarball `doctor --json`", "10 items, stable ids")

    db_url = f"sqlite://{work / 'install.db'}"
    migrated = run(["db", "migrate", "--db", db_url, "--yes"], binary=binary, cwd=work)
    queried = run(
        [
            "db",
            "query",
            "--readonly",
            "--db",
            f"{db_url}?mode=ro",
            "--sql",
            "SELECT name FROM sqlite_master ORDER BY name",
        ],
        binary=binary,
        cwd=work,
    )
    refused = run(
        ["db", "query", "--readonly", "--db", db_url, "--sql", "SELECT 1"],
        binary=binary,
        cwd=work,
    )
    if migrated.returncode != 0 or queried.returncode != 0:
        recorder.fail("tarball `db migrate` / `db query`", queried.stderr.strip()[-160:])
    elif refused.returncode != 1:
        recorder.fail("db query 写库护栏", f"expected exit 1, got {refused.returncode}")
    else:
        recorder.ok("tarball `db migrate` + `db query --readonly`", "read-only guard refused the writable URL")

    first = run(["completions", "bash"], binary=binary, cwd=work)
    second = run(["completions", "bash"], binary=binary, cwd=work)
    if first.returncode != 0 or first.stdout != second.stdout:
        recorder.fail("tarball `completions`", "output is not idempotent")
    else:
        recorder.ok("tarball `completions`", "bash script, idempotent")

    # Tarball path must fail closed on `chat`: the agent is not shipped with it.
    stub_dir = args.directory / "bin"
    stub_dir.mkdir(parents=True, exist_ok=True)
    if os.name == "nt":
        stub = stub_dir / "python.cmd"
        stub.write_text("@echo off\r\necho No module named packetsage_agent 1>&2\r\nexit /b 1\r\n", encoding="ascii")
    else:
        stub = stub_dir / "python"
        stub.write_text("#!/bin/sh\necho 'No module named packetsage_agent' >&2\nexit 1\n", encoding="utf-8")
        stub.chmod(0o755)
    env = {
        "PATH": str(stub_dir) + os.pathsep + os.environ.get("PATH", ""),
        "PYTHONPATH": "",
    }
    chat = run(["chat", str(sample)], binary=binary, cwd=work, env=env)
    if chat.returncode != 3:
        recorder.fail("tarball `chat` fails closed", f"expected exit 3, got {chat.returncode}")
    elif "PACKETSAGE_AGENT_BIN" not in chat.stderr:
        recorder.fail("tarball `chat` repair hint", chat.stderr.strip()[-160:])
    else:
        recorder.ok(
            "tarball `chat` fails closed (exit 3 + 四级路径提示)",
            "simulated with a python stub: no agent shipped in the tarball",
        )


def check_source_chat(directory: Path, recorder: Recorder) -> None:
    """Source path: the real agent is found and refuses a non-terminal stdin.

    The REPL itself needs a tty (Agent CLI §5), so a piped stdin ends with exit 1
    and the "use `packetsage-agent run`" hint; the launcher must pass that code
    through untouched (§1).
    """
    binary = directory / "source" / "bin" / ("packetsage.exe" if os.name == "nt" else "packetsage")
    if not binary.exists():
        recorder.skip("source path `chat` (non-tty)", "source install was skipped or failed")
        return
    sample = directory / "work" / "sample.pcap"
    # `mock` is named explicitly here: this check is about the tty gate, and a
    # fresh machine without a provider is covered by its own check below.
    env = {
        "PYTHONPATH": str(ROOT / "agent"),
        "PACKETSAGE_LLM_PROVIDER": "mock",
        # Hermetic: a real agent/.env must not decide this check.
        "PACKETSAGE_ENV_FILE": str(directory / "work" / "empty.env"),
    }
    result = run(["chat", str(sample)], binary=binary, cwd=directory / "work", env=env, stdin="")
    if result.returncode != 1:
        recorder.fail("source path `chat` (non-tty)", result.stderr.strip()[-200:])
    else:
        recorder.ok("source path `chat` (non-tty refusal)", "exit 1, agent found")


def check_unconfigured_provider(directory: Path, recorder: Recorder) -> None:
    """A machine with no provider configured must be told to run `setup` (exit 3)."""
    binary = directory / "source" / "bin" / ("packetsage.exe" if os.name == "nt" else "packetsage")
    if not binary.exists():
        recorder.skip("unset provider → setup hint", "source install was skipped or failed")
        return
    sample = directory / "work" / "sample.pcap"
    env = {
        "PYTHONPATH": str(ROOT / "agent"),  # no PACKETSAGE_LLM_PROVIDER on purpose
        "PACKETSAGE_ENV_FILE": str(directory / "work" / "empty.env"),
    }
    result = run(["chat", str(sample)], binary=binary, cwd=directory / "work", env=env, stdin="")
    if result.returncode != 3 or "packetsage-agent setup" not in result.stderr:
        recorder.fail("unset provider → setup hint", result.stderr.strip()[-200:])
    else:
        recorder.ok("unset provider → setup hint", "exit 3, no silent mock")


def write_report(
    directory: Path, recorder: Recorder, binary_source: Path, args: argparse.Namespace
) -> None:
    stamp = dt.datetime.now().astimezone().strftime("%Y-%m-%d %H:%M:%S %z")
    passed = sum(1 for status, _, _ in recorder.rows if status == "pass")
    lines = [
        "# 本地 install 测试目录 · 演练记录",
        "",
        "> 由 `python scripts/install_smoke.py` 生成（CLI 收口工程规格书 §2.1/§12，待验证项 #34）。",
        "> 本目录整体在 `.gitignore` 中：它是可随时删除、可随时重建的产物。",
        "",
        f"- 时间：{stamp}",
        f"- 平台：{platform.platform()} / {platform.machine()}",
        f"- Python：{platform.python_version()}（{sys.executable}）",
        f"- 用于打包的二进制：`{binary_source}`（sha256={sha256(binary_source)[:16]}…）",
        f"- 结果：**{passed}/{len(recorder.rows)} 通过**"
        + (f"，{recorder.failures} 失败" if recorder.failures else "，无失败"),
        "",
        "## 两条安装路径",
        "",
        "| 路径 | 落点 | 验收 |",
        "|---|---|---|",
        "| 源码（`cargo install --path crates/packetsage-cli --root`） | `source/bin/packetsage` | 任意目录 `version` / `version --json` |",
        "| tarball（`scripts/package_release.py` 产出） | `tarball/packetsage-…/` | SHA256SUMS + 能力矩阵（不含 chat） |",
        "",
        "## 逐项结果",
        "",
        "| 状态 | 检查 | 细节 |",
        "|---|---|---|",
    ]
    for status, name, detail in recorder.rows:
        mark = {"pass": "✅", "FAIL": "❌", "skip": "⏭"}[status]
        safe_detail = detail.replace("|", "\\|")
        lines.append(f"| {mark} | {name} | {safe_detail} |")
    lines += [
        "",
        "## 复现",
        "",
        "```bash",
        f"python scripts/install_smoke.py --dir {args.directory.name}"
        + (" --skip-source-install" if args.skip_source_install else ""),
        "```",
        "",
        "两份上游证据在 CI 里是独立 job（`install-from-scratch`、`release-tarball`）；",
        "本目录是它们的本机对应物，用于在推 tag 之前提前发现问题（#34）。",
        "",
    ]
    (directory / "RESULTS.md").write_text("\n".join(lines), encoding="utf-8")


def main(argv: Optional[list[str]] = None) -> int:
    force_utf8_console()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", default="install-test", help="the directory to create/populate")
    parser.add_argument("--binary", default=None, help="binary to package (default: target/release)")
    parser.add_argument(
        "--skip-source-install",
        action="store_true",
        help="skip `cargo install` (much faster; still tests the tarball path)",
    )
    parser.add_argument(
        "--run-cli",
        action="store_true",
        help="after installing, run the installed CLI and write RUN.md",
    )
    args = parser.parse_args(argv)

    directory = Path(args.dir)
    if not directory.is_absolute():
        directory = ROOT / directory
    args.directory = directory
    directory.mkdir(parents=True, exist_ok=True)
    work = directory / "work"
    work.mkdir(exist_ok=True)
    print(f"install test directory: {directory}")

    recorder = Recorder()
    if TOOLCHAIN_ENV:
        recorder.ok("toolchain env (Windows/GNU)", "; ".join(TOOLCHAIN_NOTES))
    binary_source = build_release(recorder)
    sample = ensure_sample(work, recorder)

    if args.skip_source_install:
        recorder.skip("cargo install --path (源码安装)", "--skip-source-install")
    else:
        check_source_install(args, directory, recorder)

    root = check_tarball(args, directory, binary_source, recorder)
    if root is None:
        recorder.fail("capability matrix", "no unpacked tarball")
    else:
        check_capabilities(args, root, work, sample, recorder)

    check_source_chat(directory, recorder)
    check_unconfigured_provider(directory, recorder)

    if args.run_cli:
        demo = run(
            [sys.executable, "scripts/cli_demo.py", "--from-install", str(directory)],
            cwd=ROOT,
        )
        print(demo.stdout, end="")
        if demo.returncode != 0:
            recorder.fail("run the installed CLI (RUN.md)", demo.stderr.strip()[-200:])
        else:
            recorder.ok("run the installed CLI (RUN.md)", "every step matched its expected exit code")

    # Double-click friendly entry point, next to the installed binaries.
    launcher = ROOT / "scripts" / "run_cli.cmd"
    if launcher.exists():
        shutil.copy2(launcher, directory / "RUN-CLI.cmd")
        recorder.ok(
            "double-click launcher",
            f"{directory.name}\\RUN-CLI.cmd (title + most capable build + version/analyze, then pauses)",
        )

    write_report(directory, recorder, binary_source, args)

    print(f"\n{len(recorder.rows) - recorder.failures}/{len(recorder.rows)} check(s) passed")
    print(f"report: {directory / 'RESULTS.md'}")
    return 1 if recorder.failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
