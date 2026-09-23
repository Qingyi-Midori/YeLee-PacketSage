#!/usr/bin/env python3
"""Exit-code matrix and three-stream contract (CLI 收口工程规格书 §4, §14.1).

Run it after building the binary:

    python tests/cli/exit_codes.py --binary target/debug/packetsage

Every case is one row of the §4 matrix or one row of the §14.1 test matrix. The
script is deliberately hermetic: temporary directories, scrubbed environments
and injected fake agents, so it never depends on the developer PATH.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import sysconfig
import tempfile
from pathlib import Path
from typing import Callable, Optional


class Failure(AssertionError):
    """A failed expectation."""


class Case:
    """One executable assertion from the spec matrices."""

    def __init__(self, name: str, spec: str, fn: Callable[["Harness"], None]) -> None:
        self.name = name
        self.spec = spec
        self.fn = fn


CASES: list[Case] = []

#: Set by `--update-snapshot`; the help case writes the golden file instead of
#: comparing against it (the deliberate-refresh gate of §14.1).
_update_snapshots = False


def case(name: str, spec: str):
    """Registers a case (see the §4 matrix)."""

    def decorate(fn):
        CASES.append(Case(name, spec, fn))
        return fn

    return decorate


def expect_code(result: subprocess.CompletedProcess, expected: int) -> None:
    if result.returncode != expected:
        raise Failure(
            f"expected exit {expected}, got {result.returncode} | "
            f"stdout tail={result.stdout[-300:]!r} stderr tail={result.stderr[-300:]!r}"
        )


def expect_in(text: str, needle: str, where: str) -> None:
    if needle not in text:
        raise Failure(f"{where} does not contain {needle!r}: {text[-300:]!r}")


def no_ansi(text: str) -> bool:
    return "\x1b[" not in text


class Harness:
    """Runs the binary and hands out throwaway workspaces."""

    def __init__(self, binary: Path, root: Path) -> None:
        self.binary = binary
        self.root = root
        self.samples = root / "samples"
        self._tempdirs: list[tempfile.TemporaryDirectory] = []

    def workdir(self) -> Path:
        handle = tempfile.TemporaryDirectory(prefix="packetsage-cli-")
        self._tempdirs.append(handle)
        return Path(handle.name)

    def cleanup(self) -> None:
        for handle in self._tempdirs:
            handle.cleanup()

    def run(
        self,
        *args: str,
        cwd: Optional[Path] = None,
        env: Optional[dict] = None,
        stdin: Optional[str] = None,
    ) -> subprocess.CompletedProcess:
        base = os.environ.copy()
        for name in (
            "PACKETSAGE_CONFIG",
            "PACKETSAGE_LLM_PROVIDER",
            "PACKETSAGE_LLM_API_KEY",
            "PACKETSAGE_AGENT_BIN",
            "PACKETSAGE_STORAGE_URL",
        ):
            base.pop(name, None)
        # Hermetic: a developer's real `agent/.env` must not decide the outcome.
        base["PACKETSAGE_ENV_FILE"] = str(self.workdir() / "empty.env")
        if env:
            base.update(env)
        return subprocess.run(
            [str(self.binary), *args],
            cwd=str(cwd) if cwd else None,
            env=base,
            input=stdin,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=300,
        )

    def sample(self) -> Path:
        return self.samples / "synth-mixed.pcap"

    def agent_env(self, **extra: str) -> dict:
        """Environment for a directly spawned `python -m packetsage_agent`."""
        env = {
            "PYTHONPATH": str(self.root / "agent"),
            "PACKETSAGE_ENV_FILE": str(self.workdir() / "empty.env"),
        }
        env.update(extra)
        return env

    def fake_agent(self, exit_code: int, message: str = "") -> dict:
        """Injects a fake agent through `$PACKETSAGE_AGENT_BIN` (launcher step 1, T2).

        A file based injection keeps the case independent of whether the real
        `packetsage-agent` console script happens to be on PATH.
        """
        work = self.workdir()
        if os.name == "nt":
            script = work / "fake-agent.cmd"
            body = "@echo off\r\n"
            if message:
                body += f"echo {message} 1>&2\r\n"
            body += f"exit /b {exit_code}\r\n"
            script.write_text(body, encoding="ascii")
        else:
            script = work / "fake-agent.sh"
            body = "#!/bin/sh\n"
            if message:
                body += f"echo {message} >&2\n"
            body += f"exit {exit_code}\n"
            script.write_text(body, encoding="utf-8")
            script.chmod(0o755)
        return {"PACKETSAGE_AGENT_BIN": str(script)}


# --------------------------------------------------------------------------
# §4 exit-code matrix
# --------------------------------------------------------------------------
@case("analyze ok", "§4")
def _analyze_ok(h: Harness) -> None:
    expect_code(h.run("analyze", str(h.sample())), 0)


@case("analyze --jsonl ok", "§4")
def _analyze_jsonl_ok(h: Harness) -> None:
    result = h.run("analyze", str(h.sample()), "--jsonl")
    expect_code(result, 0)
    lines = [line for line in result.stdout.splitlines() if line.strip()]
    assert lines, "the JSONL stream must not be empty"
    for line in lines:
        parsed = json.loads(line)
        if parsed.get("schema_version") is not None:
            assert parsed["schema_version"] == 2, parsed


@case("analyze unknown magic", "§4")
def _analyze_unknown_magic(h: Harness) -> None:
    junk = h.workdir() / "not-a-capture.pcap"
    junk.write_bytes(b"definitely not a capture file" * 4)
    result = h.run("analyze", str(junk))
    expect_code(result, 2)
    expect_in(result.stderr, "no known magic", "stderr")


@case("analyze explicit config missing", "§4, §7.1")
def _analyze_config_missing(h: Harness) -> None:
    result = h.run(
        "analyze", str(h.sample()), "--config", str(h.workdir() / "absent.yaml")
    )
    expect_code(result, 3)


@case("analyze unknown config key", "§4, §7.1")
def _analyze_config_unknown_key(h: Harness) -> None:
    config = h.workdir() / "bad.yaml"
    config.write_text("engine:\n  max_stesp: 3\n", encoding="utf-8")
    result = h.run("analyze", str(h.sample()), "--config", str(config))
    expect_code(result, 3)
    expect_in(result.stderr, "max_stesp", "stderr")


@case("analyze broken working-directory config", "§4, §7.1 layer 3")
def _analyze_config_layer3(h: Harness) -> None:
    work = h.workdir()
    (work / "packetsage.yaml").write_text("nonsense: [1, 2\n", encoding="utf-8")
    expect_code(h.run("analyze", str(h.sample()), cwd=work), 3)


@case("analyze unknown flag", "§4")
def _analyze_unknown_flag(h: Harness) -> None:
    expect_code(h.run("analyze", str(h.sample()), "--nope"), 1)


@case("analyze missing positional", "§4")
def _analyze_missing_positional(h: Harness) -> None:
    expect_code(h.run("analyze"), 1)


@case("analyze --full agent failure degrades", "§4")
def _analyze_full_degraded(h: Harness) -> None:
    work = h.workdir()
    env = h.fake_agent(exit_code=1, message="boom")
    # The agent stage is faked here; the provider still has to be named, because
    # an unconfigured provider is exit 3 before anything runs.
    env["PACKETSAGE_LLM_PROVIDER"] = "mock"
    result = h.run(
        "analyze", str(h.sample()), "--full", "--report", str(work / "report.md"), env=env
    )
    expect_code(result, 0)
    expect_in(result.stderr, "WARN", "stderr")


@case("analyze --full lint hard failure", "§4, C9")
def _analyze_full_lint_failure(h: Harness) -> None:
    work = h.workdir()
    env = h.fake_agent(exit_code=4, message="anti-hallucination: cited 999 packets")
    env["PACKETSAGE_LLM_PROVIDER"] = "mock"
    result = h.run(
        "analyze", str(h.sample()), "--full", "--report", str(work / "report.md"), env=env
    )
    expect_code(result, 4)
    expect_in(result.stderr, "anti-hallucination", "stderr")


@case("analyze --full without a provider is exit 3", "Agent CLI §3, 收口 v0.2 §4")
def _analyze_full_unconfigured(h: Harness) -> None:
    """`--full` is the agent chain: no provider means configuration error, not WARN."""
    work = h.workdir()
    result = h.run(
        "analyze", str(h.sample()), "--full", "--report", str(work / "report.md")
    )
    expect_code(result, 3)
    expect_in(result.stderr, "packetsage-agent setup", "stderr")


@case("rules check invalid rule", "§4")
def _rules_check_invalid(h: Harness) -> None:
    rule = h.workdir() / "broken.yaml"
    rule.write_text("id: NET-BROKEN\n", encoding="utf-8")
    expect_code(h.run("rules", "check", str(rule)), 3)


@case("rules list ok", "§4")
def _rules_list(h: Harness) -> None:
    expect_code(h.run("rules", "list"), 0)


@case("doctor all green", "§4, §11")
def _doctor(h: Harness) -> None:
    result = h.run("doctor", "--no-net")
    expect_code(result, 0)
    expect_in(result.stdout, "all checks passed", "stdout")


@case("doctor failing check exits 3", "§4, §11")
def _doctor_failing(h: Harness) -> None:
    config = h.workdir() / "bad.yaml"
    config.write_text("engine:\n  max_stesp: 1\n", encoding="utf-8")
    result = h.run("doctor", "--no-net", "--config", str(config))
    expect_code(result, 3)
    expect_in(result.stdout, "\u274c", "stdout")


@case("db query --readonly ok", "§4, §8.1")
def _db_query_ok(h: Harness) -> None:
    work = h.workdir()
    url = f"sqlite://{work / 'q.db'}"
    expect_code(h.run("db", "migrate", "--db", url, "--yes"), 0)
    result = h.run(
        "db",
        "query",
        "--readonly",
        "--db",
        f"{url}?mode=ro",
        "--sql",
        "SELECT count(*) AS tables FROM sqlite_master",
    )
    expect_code(result, 0)
    expect_in(result.stdout, "tables", "stdout")


@case("db query --readonly connection failure", "§4, §8.1")
def _db_query_connection_failure(h: Harness) -> None:
    result = h.run(
        "db",
        "query",
        "--readonly",
        "--db",
        f"sqlite://{h.workdir() / 'missing.db'}?mode=ro",
        "--sql",
        "SELECT 1",
    )
    expect_code(result, 3)


@case("db query url without mode=ro", "§4, T9")
def _db_query_writable_url(h: Harness) -> None:
    result = h.run(
        "db",
        "query",
        "--readonly",
        "--db",
        "sqlite://packetsage.db",
        "--sql",
        "SELECT 1",
    )
    expect_code(result, 1)
    expect_in(result.stderr, "mode=ro", "stderr")


@case("db query non-select pre-check", "§4, T9")
def _db_query_non_select(h: Harness) -> None:
    result = h.run(
        "db",
        "query",
        "--readonly",
        "--db",
        "sqlite://packetsage.db?mode=ro",
        "--sql",
        "DELETE FROM alerts",
    )
    expect_code(result, 1)


@case("db query --readonly is mandatory", "§8.1")
def _db_query_without_readonly(h: Harness) -> None:
    result = h.run(
        "db", "query", "--db", "sqlite://packetsage.db?mode=ro", "--sql", "SELECT 1"
    )
    expect_code(result, 1)


@case("db query --jsonl rows", "§8.1, T4")
def _db_query_jsonl(h: Harness) -> None:
    work = h.workdir()
    url = f"sqlite://{work / 'q.db'}"
    expect_code(h.run("db", "migrate", "--db", url, "--yes"), 0)
    result = h.run(
        "db",
        "query",
        "--readonly",
        "--jsonl",
        "--db",
        f"{url}?mode=ro",
        "--sql",
        "SELECT name FROM sqlite_master ORDER BY name",
    )
    expect_code(result, 0)
    rows = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    assert rows, "expected at least one row"
    assert all("name" in row for row in rows)
    assert no_ansi(result.stdout)


@case("db migrate --yes ok", "§4, §8.2")
def _db_migrate_ok(h: Harness) -> None:
    url = f"sqlite://{h.workdir() / 'm.db'}"
    result = h.run("db", "migrate", "--db", url, "--yes")
    expect_code(result, 0)
    expect_in(result.stdout, "migrated", "stdout")


@case("db migrate non-interactive without --yes", "§4, §8.2")
def _db_migrate_fail_closed(h: Harness) -> None:
    url = f"sqlite://{h.workdir() / 'm.db'}"
    result = h.run("db", "migrate", "--db", url)
    expect_code(result, 1)
    expect_in(result.stderr, "--yes", "stderr")


@case("db migrate second run is a no-op", "§8.2")
def _db_migrate_idempotent(h: Harness) -> None:
    url = f"sqlite://{h.workdir() / 'm.db'}"
    expect_code(h.run("db", "migrate", "--db", url, "--yes"), 0)
    result = h.run("db", "migrate", "--db", url, "--yes")
    expect_code(result, 0)
    expect_in(result.stdout, "up to date", "stdout")


@case("query alerts ok", "§4")
def _query_alerts(h: Harness) -> None:
    url = f"sqlite://{h.workdir() / 'q.db'}"
    expect_code(h.run("analyze", str(h.sample()), "--db", url), 0)
    expect_code(h.run("query", "alerts", "--db", url), 0)


@case("query alerts missing database", "§4")
def _query_alerts_missing(h: Harness) -> None:
    url = f"sqlite://{h.workdir() / 'nothing.db'}"
    expect_code(h.run("query", "alerts", "--db", url), 3)


@case("chat provider unavailable", "§4")
def _chat_provider_unavailable(h: Harness) -> None:
    env = {"PACKETSAGE_LLM_PROVIDER": "openai"}
    env.pop("OPENAI_API_KEY", None)
    result = h.run("chat", str(h.sample()), env=env, stdin="")
    expect_code(result, 3)
    expect_in(result.stderr, "packetsage-agent setup", "stderr")


@case("unset provider is setup, not a silent mock", "Agent CLI §3, 收口 v0.2 §4")
def _unset_provider_is_a_setup_step(h: Harness) -> None:
    """A fresh machine must configure a provider before anything runs."""
    env = h.agent_env()
    chat = h.run("chat", str(h.sample()), env=env, stdin="")
    expect_code(chat, 3)
    expect_in(chat.stderr, "packetsage-agent setup", "stderr")

    work = h.workdir()
    url = f"sqlite://{work / 'unset.db'}"
    analyzed = h.run("analyze", str(h.sample()), "--db", url)
    expect_code(analyzed, 0)
    task = re.search(r"^task\s+(\S+)$", analyzed.stdout, re.MULTILINE)
    assert task, analyzed.stdout
    agent_env = h.agent_env(PACKETSAGE_ENGINE=str(Path(h.binary).resolve()))
    run = subprocess.run(
        [sys.executable, "-m", "packetsage_agent", "run", "--task-id", task.group(1), "--db", url],
        cwd=str(work),
        env={**os.environ, **agent_env},
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        stdin=subprocess.DEVNULL,
        timeout=300,
    )
    assert run.returncode == 3, f"expected exit 3, got {run.returncode}: {run.stdout}"
    expect_in(run.stderr, "packetsage-agent setup", "stderr")
    assert "status=completed" not in run.stdout, "no investigation may run unconfigured"


@case("setup writes agent/.env and masks the key", "Agent CLI §3 (setup)")
def _setup_writes_the_env_file(h: Harness) -> None:
    env = h.agent_env()
    target = h.workdir() / "setup.env"
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "packetsage_agent",
            "setup",
            "--provider",
            "deepseek",
            "--api-key",
            "sk-not-a-real-key",
            "--no-verify",
            "--env-file",
            str(target),
        ],
        cwd=str(h.root),
        env={**os.environ, **env},
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        stdin=subprocess.DEVNULL,
        timeout=120,
    )
    assert result.returncode == 0, result.stderr
    text = target.read_text(encoding="utf-8")
    assert "PACKETSAGE_LLM_PROVIDER=deepseek" in text, text
    assert "PACKETSAGE_LLM_API_KEY=sk-not-a-real-key" in text, text
    if "sk-not-a-real-key" in result.stdout:
        raise Failure("setup must never echo the key")
    expect_in(result.stdout, "packetsage-agent run", "stdout")


@case("chat launcher four steps fail", "§4, §2.3")
def _chat_launcher_failure(h: Harness) -> None:
    work = h.workdir()
    empty = work / "empty-path"
    empty.mkdir()
    env = {
        "PATH": str(empty),
        "PYTHONPATH": str(empty),
        "PACKETSAGE_AGENT_BIN": str(work / "does-not-exist"),
    }
    result = h.run("chat", str(h.sample()), env=env, stdin="")
    expect_code(result, 3)
    expect_in(result.stderr, "PACKETSAGE_AGENT_BIN", "stderr")


@case("chat: pty REPL on POSIX, documented non-tty refusal on Windows", "§4, §11, #43")
def _chat_end_to_end(h: Harness) -> None:
    """`packetsage chat <capture>` with the real agent.

    The agent only enters the REPL on a terminal (Agent CLI §5), so POSIX drives
    a real pty here; this harness has no pty on Windows (#43), where the
    contract to assert is the non-tty refusal (exit 1).
    """
    env = h.agent_env(PACKETSAGE_LLM_PROVIDER="mock")
    if os.name == "nt":
        result = h.run("chat", str(h.sample()), env=env, stdin="")
        expect_code(result, 1)
        return

    import pty
    import select
    import time

    work = h.workdir()
    base = os.environ.copy()
    for name in (
        "PACKETSAGE_CONFIG",
        "PACKETSAGE_LLM_PROVIDER",
        "PACKETSAGE_LLM_API_KEY",
        "PACKETSAGE_AGENT_BIN",
        "PACKETSAGE_STORAGE_URL",
    ):
        base.pop(name, None)
    base.update(env)

    master, slave = pty.openpty()
    process = subprocess.Popen(
        [str(h.binary), "chat", str(h.sample())],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        cwd=str(work),
        env=base,
    )
    os.close(slave)
    collected = b""
    try:
        time.sleep(3.0)  # analyse + persist + spawn the agent
        os.write(master, b"/quit\n")
        deadline = time.time() + 120
        while time.time() < deadline:
            ready, _, _ = select.select([master], [], [], 1.0)
            if ready:
                try:
                    chunk = os.read(master, 4096)
                except OSError:
                    break
                if not chunk:
                    break
                collected += chunk
            if process.poll() is not None:
                break
        code = process.wait(timeout=60)
    finally:
        if process.poll() is None:
            process.kill()
        os.close(master)

    text = collected.decode("utf-8", "replace")
    assert code == 0, f"chat exited {code}: {text}"
    assert "packets=" in text, f"the banner must report the task overview: {text}"
    assert "packetsage" in text, f"the REPL prompt must be visible: {text}"


@case("console script and python -m are interchangeable", "§2.2, T8")
def _help_parity(h: Harness) -> None:
    script = _console_script()
    if script is None:
        raise Failure("packetsage-agent is not installed (run `pip install -e ./agent`)")
    env = h.agent_env()
    console = subprocess.run(
        [script, "--help"], capture_output=True, text=True, env={**os.environ, **env}
    )
    module = subprocess.run(
        [sys.executable, "-m", "packetsage_agent", "--help"],
        capture_output=True,
        text=True,
        env={**os.environ, **env},
        cwd=str(h.root),
    )
    assert console.returncode == 0 and module.returncode == 0
    assert console.stdout == module.stdout, "both entry points must render the same help (T8)"
    expect_in(console.stdout, "packetsage-agent", "help")


@case("agent run over a persisted task (Agent CLI §1 path C)", "Agent CLI §1, §8")
def _agent_run_over_database(h: Harness) -> None:
    """The new agent contract: task from `analyze --db`, engine via env.

    Exercises ADR-019 cold recovery end to end — the agent spawns its own
    `serve` worker, which has to rebuild the task from the database row.
    """
    work = h.workdir()
    url = f"sqlite://{work / 'agent-e2e.db'}"
    analyzed = h.run("analyze", str(h.sample()), "--db", url)
    expect_code(analyzed, 0)
    match = re.search(r"^task\s+(\S+)$", analyzed.stdout, re.MULTILINE)
    if not match:
        raise Failure(f"analyze did not print a task id:\n{analyzed.stdout}")
    env = h.agent_env(
        PACKETSAGE_LLM_PROVIDER="mock",
        # Absolute: the agent runs with cwd = the temporary work directory.
        PACKETSAGE_ENGINE=str(Path(h.binary).resolve()),
    )
    result = subprocess.run(
        [sys.executable, "-m", "packetsage_agent", "run", "--task-id", match.group(1), "--db", url],
        cwd=str(work),
        env={**os.environ, **env},
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        stdin=subprocess.DEVNULL,
        timeout=300,
    )
    assert result.returncode == 0, f"agent run failed:\n{result.stdout}\n{result.stderr}"
    expect_in(result.stdout, "status=completed", "stdout")
    expect_in(result.stdout, "submit_rejects=0", "stdout")

    # The report runs in a *new* process: findings and the evidence facts they
    # cite have to survive in the database (ADR-019 + the ledger columns), or the
    # anti-hallucination lint rejects the whole Executive Summary.
    report_path = work / "agent-report.md"
    reported = subprocess.run(
        [
            sys.executable,
            "-m",
            "packetsage_agent",
            "report",
            "--task-id",
            match.group(1),
            "--report",
            str(report_path),
            "--db",
            url,
        ],
        cwd=str(work),
        env={**os.environ, **env},
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        stdin=subprocess.DEVNULL,
        timeout=300,
    )
    assert reported.returncode == 0, f"report failed:\n{reported.stdout}\n{reported.stderr}"
    text = report_path.read_text(encoding="utf-8")
    if "F-001" not in text:
        raise Failure("the report lost the findings of the earlier run")
    expect_in(reported.stdout, "report written:", "stdout")


def _console_script() -> Optional[str]:
    """Locates the `packetsage-agent` entry point, PATH or not."""
    found = shutil.which("packetsage-agent")
    if found:
        return found
    scripts = Path(sysconfig.get_path("scripts") or "")
    names = ("packetsage-agent.exe", "packetsage-agent") if os.name == "nt" else ("packetsage-agent",)
    for name in names:
        candidate = scripts / name
        if candidate.exists():
            return str(candidate)
    for name in names:
        candidate = Path(sys.executable).parent / name
        if candidate.exists():
            return str(candidate)
    return None


@case("version and completions", "§4")
def _version_and_completions(h: Harness) -> None:
    expect_code(h.run("version"), 0)
    expect_code(h.run("version", "--json"), 0)
    expect_code(h.run("--version"), 0)
    expect_code(h.run("completions", "bash"), 0)


@case("panic guard", "§4, #32")
def _panic_guard(h: Harness) -> None:
    result = h.run("version", env={"PACKETSAGE_PANIC_FOR_TEST": "1"})
    expect_code(result, 4)
    expect_in(result.stderr, "internal error (panic)", "stderr")


# --------------------------------------------------------------------------
# §14.1 three streams, determinism, help snapshot, completions, doctor --json
# --------------------------------------------------------------------------
@case("--jsonl stdout is pure, stderr never carries events", "§3.2, T7")
def _three_streams(h: Harness) -> None:
    result = h.run("analyze", str(h.sample()), "--jsonl")
    expect_code(result, 0)
    assert no_ansi(result.stdout), "piped stdout must not contain ANSI escapes"
    for line in result.stdout.splitlines():
        if line.strip():
            json.loads(line)
    assert '"event"' not in result.stderr, "stderr must not carry EngineEvent lines"
    assert "done:" in result.stderr, "progress belongs on stderr"


@case("progress on/off is byte-identical", "§3.3, T7")
def _progress_determinism(h: Harness) -> None:
    task = "task_01J00000000000000000000000"
    loud = h.run("analyze", str(h.sample()), "--jsonl", "--task-id", task)
    quiet = h.run("analyze", str(h.sample()), "--jsonl", "--task-id", task, "-q")
    expect_code(loud, 0)
    expect_code(quiet, 0)
    # Only the two wall-clock inputs can differ: the task id is pinned above,
    # and `started_at` plus the random alert ids are normalised here (§3.3 C9).
    pattern = re.compile(r'("started_at":"[^"]*"|"alert_id":"[^"]*")')
    assert pattern.sub('"dynamic"', loud.stdout) == pattern.sub('"dynamic"', quiet.stdout), (
        "progress must not influence the event stream"
    )
    assert "done:" in loud.stderr
    assert "done:" not in quiet.stderr, "-q mutes progress"


@case("pipes and NO_COLOR disable styling", "§3.2")
def _no_color(h: Harness) -> None:
    assert no_ansi(h.run("doctor", "--no-net").stdout), "a pipe must not receive ANSI escapes"
    assert no_ansi(h.run("doctor", "--no-net", "--no-color").stdout)


@case("help snapshot is stable", "§3.1, T8")
def _help_snapshot(h: Harness) -> None:
    first = h.run("--help")
    second = h.run("--help")
    expect_code(first, 0)
    assert first.stdout == second.stdout, "help output must be deterministic"
    for command in ("analyze", "doctor", "db", "completions", "version", "chat"):
        expect_in(first.stdout, command, "help")
    # The snapshot file is the deliberate-refresh gate: changing help text
    # means re-running with --update-snapshot and reviewing the diff.
    rendered = snapshot_text(h)
    snapshot = Path(__file__).with_name("help_snapshot.txt")
    if _update_snapshots:
        snapshot.write_text(rendered, encoding="utf-8")
        return
    if not snapshot.exists():
        raise Failure(
            "tests/cli/help_snapshot.txt is missing; run "
            "`python tests/cli/exit_codes.py --update-snapshot`"
        )
    expected = snapshot.read_text(encoding="utf-8")
    if expected != rendered:
        raise Failure(
            "help text changed; review it, then refresh with "
            "`python tests/cli/exit_codes.py --update-snapshot`"
        )


def _art_block(text: str) -> list[str]:
    """The leading ASCII art of a help page (lines up to the first blank one)."""
    lines: list[str] = []
    for line in text.splitlines():
        if not line.strip():
            if lines:
                break
            continue
        lines.append(line.rstrip())
    return lines


@case("ASCII title heads both --help pages", "§3.1, Agent CLI §2")
def _ascii_title(h: Harness) -> None:
    rust = h.run("--help")
    expect_code(rust, 0)
    art = _art_block(rust.stdout)
    if not art or not art[0].strip().startswith(":::"):
        raise Failure(f"`packetsage --help` must open with the title:\n{rust.stdout[:200]}")
    assert art[-1].startswith("###"), "the whole title block must be printed"

    env = h.agent_env()
    agent = subprocess.run(
        [sys.executable, "-m", "packetsage_agent", "--help"],
        cwd=str(h.root),
        env={**os.environ, **env},
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        stdin=subprocess.DEVNULL,
        timeout=120,
    )
    assert agent.returncode == 0, agent.stderr
    if _art_block(agent.stdout) != art:
        raise Failure("the agent's copy of the title drifted from the Rust one")


def snapshot_text(h: Harness) -> str:
    """Renders every help page that is part of the snapshot contract."""
    pages = [
        ["--help"],
        ["analyze", "--help"],
        ["doctor", "--help"],
        ["db", "--help"],
        ["db", "query", "--help"],
        ["chat", "--help"],
        ["version", "--help"],
        ["completions", "--help"],
    ]
    blocks = []
    for page in pages:
        result = h.run(*page)
        expect_code(result, 0)
        blocks.append(f"$ packetsage {' '.join(page)}\n{result.stdout.rstrip()}\n")
    return "\n".join(blocks)


@case("completions idempotent for 4 shells", "§6, §14.1")
def _completions(h: Harness) -> None:
    for shell in ("bash", "zsh", "fish", "powershell"):
        first = h.run("completions", shell)
        second = h.run("completions", shell)
        expect_code(first, 0)
        assert first.stdout == second.stdout, f"{shell} completions differ between runs"
        assert first.stdout.strip(), f"{shell} completions are empty"


@case("doctor --json is stable and side-effect free", "§11, T6")
def _doctor_json(h: Harness) -> None:
    work = h.workdir()
    before = sorted(p.name for p in work.iterdir())
    result = h.run("doctor", "--json", "--no-net", cwd=work)
    expect_code(result, 0)
    assert sorted(p.name for p in work.iterdir()) == before, "doctor must not touch files (T6)"
    items = json.loads(result.stdout)["items"]
    assert len(items) == 10, f"expected 10 checks, got {len(items)}"
    assert [item["id"] for item in items] == [
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
    ]
    for item in items:
        assert set(item) == {"id", "name", "status", "detail", "repair_hint"}
        assert item["status"] in {"ok", "warn", "fail", "skipped"}


@case("config dump is redacted", "§7.2, T1")
def _redaction(h: Harness) -> None:
    secret = "sk-do-not-print-me"
    result = h.run(
        "doctor",
        "--json",
        "--no-net",
        "-vv",
        env={"PACKETSAGE_LLM_API_KEY": secret, "PACKETSAGE_LLM_PROVIDER": "openai"},
    )
    expect_code(result, 0)
    assert secret not in (result.stdout + result.stderr), "the API key leaked"
    expect_in(result.stderr, "***", "stderr")


def main(argv: Optional[list] = None) -> int:
    global _update_snapshots
    # `§`/`·` in the case labels must not crash a legacy console (GBK).
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(errors="replace")
        except (AttributeError, ValueError, OSError):
            pass
    parser = argparse.ArgumentParser(description="CLI exit-code matrix (spec §4/§14.1)")
    parser.add_argument("--binary", default=None, help="path to the packetsage binary")
    parser.add_argument("--filter", default=None, help="only run cases containing this text")
    parser.add_argument(
        "--update-snapshot",
        action="store_true",
        help="refresh tests/cli/help_snapshot.txt instead of comparing against it",
    )
    args = parser.parse_args(argv)
    _update_snapshots = args.update_snapshot

    root = Path(__file__).resolve().parents[2]
    binary = Path(args.binary) if args.binary else root / "target" / "debug" / "packetsage"
    if not binary.exists() and binary.with_suffix(".exe").exists():
        binary = binary.with_suffix(".exe")
    if not binary.exists():
        print(f"binary not found: {binary}", file=sys.stderr)
        return 2
    if not (root / "samples" / "synth-mixed.pcap").exists():
        print("samples/synth-mixed.pcap is missing; run scripts/gen_traffic.py", file=sys.stderr)
        return 2

    harness = Harness(binary, root)
    selected = [c for c in CASES if not args.filter or args.filter in c.name]
    results: list = []
    try:
        for spec in selected:
            try:
                spec.fn(harness)
            except Exception as error:  # noqa: BLE001 - every failure is reported
                results.append((spec.name, False, f"{type(error).__name__}: {error}"))
            else:
                results.append((spec.name, True, spec.spec))
    finally:
        harness.cleanup()

    width = max((len(name) for name, _, _ in results), default=0)
    failures = 0
    for name, ok, detail in results:
        if not ok:
            failures += 1
        print(f"{'ok  ' if ok else 'FAIL'} {name:<{width}}  {detail}")
    print(f"\n{len(results) - failures}/{len(results)} case(s) passed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
