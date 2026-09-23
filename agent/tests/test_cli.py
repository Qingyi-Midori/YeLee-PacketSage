"""The `packetsage-agent` command surface (Agent CLI 工程规格书 v0.1 §2–§11).

Every case here maps to one row of the §11 test matrix: entry, version, the
§8 exit-code matrix, the three streams, the REPL and the Python-side
configuration consumption.
"""

from __future__ import annotations

import io
import re
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from packetsage_agent import cli  # noqa: E402
from packetsage_agent import config as config_module  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
AGENT = ROOT / "agent"
FAKE = str(Path(__file__).resolve().parent / "fake_engine.py")
TASK = "task_01J9Z4M8YQ2V7C1W3N5B6D8FGH"


@pytest.fixture(autouse=True)
def _clean_env(monkeypatch):
    """No inherited PACKETSAGE_* / OPENAI_* may leak into a CLI case.

    The suite then configures the deterministic `mock` provider **explicitly**
    (that is the documented CI/demo switch). Cases that assert the *unset*
    behaviour delete the variable again; nothing relies on a silent default.
    """
    for name in (
        "PACKETSAGE_CONFIG",
        "PACKETSAGE_ENGINE",
        "PACKETSAGE_STORAGE_URL",
        "PACKETSAGE_DB",
        "PACKETSAGE_RULES_PATH",
        "PACKETSAGE_RULES",
        "PACKETSAGE_LLM_PROVIDER",
        "PACKETSAGE_PROVIDER",
        "PACKETSAGE_LLM_MODEL",
        "PACKETSAGE_LLM_API_KEY",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "DEEPSEEK_API_KEY",
    ):
        monkeypatch.delenv(name, raising=False)
    # Hermetic: a developer's real `agent/.env` must not decide a test outcome.
    monkeypatch.setenv(
        "PACKETSAGE_ENV_FILE", str(Path(tempfile.gettempdir()) / "packetsage-no-env")
    )
    monkeypatch.setenv("PACKETSAGE_LLM_PROVIDER", "mock")
    return monkeypatch


def engine_cmd(mode: str = "normal") -> str:
    """`--engine` value driving the fake worker in one mode."""
    return shlex.join([sys.executable, FAKE, mode])


def run_cli(args: list[str]) -> int:
    """In-process invocation: `main()` is the console script's target."""
    return cli.main(args)


def feed_inputs(monkeypatch, lines: list[str], prompts: list[str] | None = None) -> None:
    """Feeds the REPL; exhaustion is EOF (Ctrl-D), never StopIteration."""
    remaining = iter(lines)

    def _input(prompt: str = "") -> str:
        if prompts is not None:
            prompts.append(prompt)
        try:
            return next(remaining)
        except StopIteration as stop:  # pragma: no cover - defensive
            raise EOFError from stop

    monkeypatch.setattr("builtins.input", _input)


# --------------------------------------------------------------------------
# entry point, prog name, version
# --------------------------------------------------------------------------
def test_prog_name_is_pinned():
    assert cli.build_parser().prog == "packetsage-agent"
    assert cli.PROG == "packetsage-agent"


def test_module_and_console_entry_points_share_one_help():
    """§2: both forms are byte-for-byte identical."""
    # `stdin=DEVNULL` keeps the child from inheriting pytest's captured stdin
    # handle (invalid on some Windows hosts).
    module = subprocess.run(
        [sys.executable, "-m", "packetsage_agent", "--help"],
        capture_output=True,
        stdin=subprocess.DEVNULL,
        text=True,
        cwd=AGENT,
    )
    direct = subprocess.run(
        [
            sys.executable,
            "-c",
            "import sys; from packetsage_agent.cli import main; "
            "sys.exit(main(['--help']))",
        ],
        capture_output=True,
        stdin=subprocess.DEVNULL,
        text=True,
        cwd=AGENT,
    )
    assert module.returncode == 0 and direct.returncode == 0
    assert module.stdout == direct.stdout
    assert "usage: packetsage-agent" in module.stdout


def test_help_opens_with_the_ascii_title():
    """The project art heads the top-level help (not the sub-commands')."""
    from packetsage_agent.banner import art

    title = art()
    assert title, "the ASCII title must ship with the package"
    assert cli.build_parser().format_help().startswith(title)

    sub = cli.build_parser()._subparsers._group_actions[0].choices
    for name in ("run", "chat", "report"):
        assert not sub[name].format_help().startswith(title)


def test_version_is_a_single_line_fast_path(capsys):
    start = time.perf_counter()
    code = run_cli(["--version"])
    elapsed = time.perf_counter() - start
    captured = capsys.readouterr()
    assert code == 0
    assert captured.out.strip() == cli.version_line()
    assert re.fullmatch(r"packetsage-agent \S+", captured.out.strip())
    assert captured.err == ""
    # §2: the launcher probes with a 3 s budget; the CLI must stay well inside.
    assert elapsed < 1.0


def test_version_does_not_touch_configuration(monkeypatch, capsys):
    def explode(*_args, **_kwargs):  # pragma: no cover - must not be reached
        raise AssertionError("the --version path must not read the configuration")

    monkeypatch.setattr(config_module, "load", explode)
    monkeypatch.setattr(config_module, "load_dotenv", explode)
    assert run_cli(["--version"]) == 0
    assert capsys.readouterr().out.startswith("packetsage-agent ")


def test_version_matches_pyproject():
    """Single source of truth: the printed version is the PEP 621 one."""
    text = (AGENT / "pyproject.toml").read_text(encoding="utf-8")
    declared = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
    assert declared is not None
    assert cli.version_line() == f"packetsage-agent {declared.group(1)}"


def test_sub_commands_are_run_chat_report_setup_serve():
    """The command surface, including the desktop sidecar (GUI spec v0.2 U1)."""
    parser = cli.build_parser()
    actions = [action for action in parser._actions if action.dest == "command"]
    assert actions and list(actions[0].choices) == [
        "run",
        "chat",
        "report",
        "setup",
        "serve",
    ]


# --------------------------------------------------------------------------
# §8 exit matrix
# --------------------------------------------------------------------------
def test_missing_task_id_is_a_usage_error(capsys):
    assert run_cli(["run"]) == cli.EXIT_USAGE
    assert "--task-id" in capsys.readouterr().err


def test_unknown_flag_is_a_usage_error(capsys):
    assert run_cli(["run", "--task-id", TASK, "--capture", "x.pcap"]) == cli.EXIT_USAGE
    assert "unrecognized arguments" in capsys.readouterr().err


def test_missing_sub_command_is_a_usage_error():
    assert run_cli([]) == cli.EXIT_USAGE


def test_db_conflicting_with_the_engine_command_is_refused(capsys):
    code = run_cli(
        [
            "run",
            "--task-id",
            TASK,
            "--engine",
            "packetsage serve --db sqlite://other.db",
            "--db",
            "sqlite://packetsage.db",
        ]
    )
    assert code == cli.EXIT_USAGE
    assert "--db" in capsys.readouterr().err


def test_run_happy_path_exits_zero_and_prints_the_summary(capsys):
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd()])
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    assert "status=completed" in captured.out
    assert f"task={TASK}" in captured.out
    assert "accepted=1 submit_rejects=0 malformed_output=0" in captured.out
    assert "steps " in captured.out and " tok " in captured.out
    # The stored finding carries the engine-assigned id.
    assert "F-001" in captured.out
    # §7: no logs and no progress on stdout, nothing of the sort on stderr either
    # (stderr is not a terminal in this test, so progress is off by definition).
    assert "WARN" not in captured.err
    assert "#1 " not in captured.out


def test_run_json_is_one_line_and_covers_the_frozen_contract(capsys):
    """G1-3 / S42: `run --json` is the GUI's structured exit."""
    import json

    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd(), "--json"])
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    lines = [line for line in captured.out.splitlines() if line.strip()]
    assert len(lines) == 1, f"run --json must print exactly one line: {captured.out!r}"
    payload = json.loads(lines[0])
    assert {
        "run_id",
        "task_id",
        "status",
        "accepted",
        "submit_rejects",
        "malformed_output",
        "steps",
        "calls",
        "tokens",
        "cost_cents",
        "prompt_version",
        "findings",
    } <= set(payload)
    assert payload["task_id"] == TASK
    assert payload["status"] == "completed"
    assert payload["accepted"] == 1
    assert payload["submit_rejects"] == 0
    assert payload["malformed_output"] is False
    assert payload["prompt_version"] == "v2"
    assert isinstance(payload["steps"], int) and payload["steps"] >= 1
    assert isinstance(payload["calls"], int) and payload["calls"] >= 1
    assert set(payload["tokens"]) == {"in", "out", "total"}
    assert payload["tokens"]["total"] == payload["tokens"]["in"] + payload["tokens"]["out"]
    assert payload["findings"], "the happy path stores one finding"
    for finding in payload["findings"]:
        assert set(finding) == {"id", "severity", "basis", "title", "evidence_ids"}
        assert finding["id"] == "F-001"
        assert finding["evidence_ids"], "every finding cites at least one tc id"
    # The human summary must not be mixed into the machine stream.
    assert "status=completed" not in captured.out


def test_secrets_never_reach_streams_report_or_engine_log(
    capsys, monkeypatch, tmp_path
):
    """G5-3: doctor -vv, the report body, the engine log and error paths."""
    secret = "sk-live-hygiene-check-0123456789"
    monkeypatch.setenv("PACKETSAGE_LLM_API_KEY", secret)
    monkeypatch.setenv("PACKETSAGE_LLM_PROVIDER", "mock")
    log = tmp_path / "engine.log"
    monkeypatch.setattr(cli, "ENGINE_LOG_NAME", str(log))

    report = tmp_path / "report.md"
    code = run_cli(
        [
            "report",
            "--task-id",
            TASK,
            "--engine",
            engine_cmd(),
            "--report",
            str(report),
            "-vv",
        ]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    assert secret not in captured.out + captured.err, "the key reached a stream"
    assert "***" in captured.err, "the -vv dump must show the masked value"
    assert report.is_file()
    assert secret not in report.read_text(encoding="utf-8")
    if log.is_file():
        assert secret not in log.read_text(encoding="utf-8", errors="replace")

    # An engine that cannot even start is the other classic leak path: the
    # message prints the engine command and the effective configuration.
    code = run_cli(
        [
            "run",
            "--task-id",
            TASK,
            "--engine",
            "packetsage-not-installed-hygiene serve",
            "--json",
            "-vv",
        ]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_CONFIG
    assert secret not in captured.out + captured.err


def test_engine_that_does_not_know_the_task_exits_three(capsys):
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd("no-task")])
    captured = capsys.readouterr()
    assert code == cli.EXIT_CONFIG
    assert "task" in captured.err
    assert "packetsage analyze" in captured.err
    assert captured.out == ""


def test_engine_spawn_failure_exits_three(capsys):
    code = run_cli(["run", "--task-id", TASK, "--engine", "packetsage-not-installed-xyz serve"])
    captured = capsys.readouterr()
    assert code == cli.EXIT_CONFIG
    assert "cannot start the engine" in captured.err


def test_engine_spawn_failure_redacts_a_db_password(capsys):
    code = run_cli(
        [
            "run",
            "--task-id",
            TASK,
            "--engine",
            "packetsage-not-installed-xyz serve --db postgres://user:secret@host/db",
        ]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_CONFIG
    assert "secret" not in captured.err
    assert "postgres://user:***@host/db" in captured.err


@pytest.mark.parametrize(
    ("engine_mode", "expected"),
    [
        ("crash:2", 2),
        ("crash:3", 3),
        ("crash:4", 4),
        # §8: an engine that exits 0 without being asked to is a bug (4).
        ("crash:0", 4),
    ],
)
def test_engine_death_passes_the_exit_code_through(capsys, engine_mode, expected):
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd(engine_mode)])
    assert code == expected
    assert "engine died" in capsys.readouterr().err


def test_unknown_provider_is_a_config_error(capsys, monkeypatch):
    monkeypatch.setenv("PACKETSAGE_LLM_PROVIDER", "nope")
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd()])
    captured = capsys.readouterr()
    assert code == cli.EXIT_CONFIG
    assert "nope" in captured.err
    # The preflight message lists the three supported kinds.
    assert "mock, openai, deepseek or local" in config_module.provider_unavailable(
        _settings(provider="nope")
    )


def test_unset_provider_blocks_run_and_chat(capsys, monkeypatch):
    """A fresh install must configure a provider first (no silent mock)."""
    monkeypatch.delenv("PACKETSAGE_LLM_PROVIDER", raising=False)
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd()])
    err = capsys.readouterr().err
    assert code == cli.EXIT_CONFIG
    assert "packetsage-agent setup" in err
    assert "provider (unset)" in err

    monkeypatch.setattr(cli, "_interactive", lambda: True)
    code = run_cli(["chat", "--task-id", TASK, "--engine", engine_cmd()])
    assert code == cli.EXIT_CONFIG  # the provider gate fires before the tty gate
    assert "packetsage-agent setup" in capsys.readouterr().err


def test_mock_provider_must_be_named_explicitly(capsys, monkeypatch):
    monkeypatch.setenv("PACKETSAGE_LLM_PROVIDER", "mock")
    assert run_cli(["run", "--task-id", TASK, "--engine", engine_cmd()]) == cli.EXIT_SUCCESS


def test_unexpected_exception_is_exit_four_with_a_traceback(monkeypatch, capsys):
    def boom(*_args, **_kwargs):
        raise RuntimeError("boom")

    monkeypatch.setattr(cli, "cmd_run", boom)
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd()])
    captured = capsys.readouterr()
    assert code == cli.EXIT_INTERNAL
    assert "Traceback" in captured.err
    assert "RuntimeError: boom" in captured.err


# --------------------------------------------------------------------------
# three streams, progress, usage
# --------------------------------------------------------------------------
class TtyStream:
    """A stderr stand-in that claims to be a terminal."""

    def __init__(self) -> None:
        self.chunks: list[str] = []

    @property
    def lines(self) -> list[str]:
        """Whatever was written, split into lines (`print` writes two chunks)."""
        return "".join(self.chunks).splitlines()

    def isatty(self) -> bool:
        return True

    def write(self, text: str) -> None:
        self.chunks.append(text)

    def flush(self) -> None:  # noqa: D102 - io protocol
        return None


def test_progress_lines_exist_only_on_a_tty_and_are_closed_by_q():
    from packetsage_agent.policy import AgentBudget, PolicyState
    from packetsage_agent.progress import Progress, usage_line

    state = PolicyState(steps=3, llm_calls=5, tokens_in=4000, tokens_out=200, cost_cents=31)
    budget = AgentBudget()
    assert usage_line(state, budget) == "steps 3/12 · calls 5/24 · 4.2k tok · 3.1¢/50¢"
    # 有缓存命中数据时才补一段（DeepSeek 上下文硬盘缓存）：没有数据不显示，
    # 免得把"provider 不报"说成"命中 0%"。
    cached = PolicyState(
        steps=3,
        llm_calls=5,
        tokens_in=4000,
        tokens_out=200,
        cost_cents=31,
        cache_hit_tokens=3100,
        cache_miss_tokens=1900,
    )
    assert usage_line(cached, budget).endswith("· cache 62%")

    stream = TtyStream()
    progress = Progress(stream=stream, quiet=False)
    progress.tool_call(1, "get_capture_summary", True)
    progress.llm_round(state, budget)
    assert stream.lines[0].strip() == "#1 get_capture_summary ok"
    assert stream.lines[1].strip() == usage_line(state, budget)

    quiet = TtyStream()
    silent = Progress(stream=quiet, quiet=True)
    silent.tool_call(1, "get_capture_summary", True)
    assert quiet.lines == []
    # A pipe is not a terminal: same silence without -q.
    assert Progress(stream=io.StringIO()).enabled is False


def test_run_keeps_progress_off_stdout_but_writes_it_to_a_tty(capsys, monkeypatch):
    stream = TtyStream()
    from packetsage_agent.progress import Progress

    monkeypatch.setattr(cli, "_progress_for", lambda context: Progress(stream=stream))
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd()])
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    assert any(line.strip() == "#1 get_capture_summary ok" for line in stream.lines)
    assert "#1 get_capture_summary ok" not in captured.out


class _Reconfigurable:
    """Minimal non-tty stream that records the reconfigure() call (#43)."""

    def __init__(self, tty: bool) -> None:
        self._tty = tty
        self.kwargs: dict | None = None

    def isatty(self) -> bool:
        return self._tty

    def reconfigure(self, **kwargs) -> None:
        self.kwargs = kwargs


def test_redirected_output_is_forced_to_utf8_and_a_console_is_left_alone(monkeypatch):
    redirected = _Reconfigurable(tty=False)
    monkeypatch.setattr(cli.sys, "stdout", redirected)
    monkeypatch.setattr(cli.sys, "stderr", _Reconfigurable(tty=False))
    cli._make_streams_safe()
    assert redirected.kwargs == {"encoding": "utf-8", "errors": "replace"}

    console = _Reconfigurable(tty=True)
    monkeypatch.setattr(cli.sys, "stdout", console)
    cli._make_streams_safe()
    assert console.kwargs == {"errors": "replace"}


# --------------------------------------------------------------------------
# chat / REPL
# --------------------------------------------------------------------------
def test_chat_refuses_a_non_tty(capsys):
    code = run_cli(["chat", "--task-id", TASK, "--engine", engine_cmd()])
    captured = capsys.readouterr()
    assert code == cli.EXIT_USAGE
    assert "终端" in captured.err
    assert "packetsage-agent run" in captured.err


def test_chat_banner_reports_task_packets_alerts_and_rules():
    class FakeClient:
        def call(self, method, params):
            if method == "get_capture_summary":
                return {"content": {"packets": 612, "sessions": 570, "alerts": 2, "decode_errors": 0}}
            return {"rules": [{"id": "A"}, {"id": "B"}, {"id": "C"}, {"id": "D"}]}

    line = cli._banner(FakeClient(), TASK, _settings())
    assert "packets=612" in line
    assert "alerts=2" in line
    assert "rules=4" in line
    assert "provider=mock" in line


def test_content_unwraps_the_frozen_envelope():
    assert cli._content({"content": {"packets": 1}}) == {"packets": 1}
    assert cli._content({"packets": 1}) == {"packets": 1}


def test_repl_quit_is_exit_zero_and_keeps_streams_apart(capsys, monkeypatch):
    monkeypatch.setattr(cli, "_interactive", lambda: True)
    monkeypatch.setattr(cli, "_readline", lambda: (None, None))
    feed_inputs(monkeypatch, ["/help", "/quit"])

    code = run_cli(["chat", "--task-id", TASK, "--engine", engine_cmd()])
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    # Title, task banner and findings belong to stdout; the built-in help does not.
    assert captured.out.startswith(cli.banner())
    assert "packets=612" in captured.out
    assert "/quit" in captured.err and captured.err.count("/help") >= 1
    assert "/help" not in captured.out


def test_repl_prints_prompt_and_usage_line_per_turn(capsys, monkeypatch):
    from packetsage_agent.progress import Progress

    stream = TtyStream()
    monkeypatch.setattr(cli, "_interactive", lambda: True)
    monkeypatch.setattr(cli, "_readline", lambda: (None, None))
    monkeypatch.setattr(cli, "_progress_for", lambda context: Progress(stream=stream))
    prompts: list[str] = []
    feed_inputs(monkeypatch, ["看看这次捕获", "/quit"], prompts)

    code = run_cli(["chat", "--task-id", TASK, "--engine", engine_cmd()])
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    assert prompts[0] == f"packetsage·{TASK[:8]}>> "
    # The mock turn produced one finding on stdout and one usage line on stderr.
    assert "NET-TCP-SYN-BURST-001" in captured.out
    assert any(line.strip().startswith("steps ") for line in stream.lines)


def test_repl_survives_a_failing_turn(capsys, monkeypatch):
    monkeypatch.setattr(cli, "_interactive", lambda: True)
    monkeypatch.setattr(cli, "_readline", lambda: (None, None))

    from packetsage_agent import agent as agent_module

    class Exploding:
        def run(self, *_args, **_kwargs):
            raise RuntimeError("provider exploded")

    monkeypatch.setattr(agent_module, "build_agent", lambda *_a, **_k: Exploding())
    feed_inputs(monkeypatch, ["q1", "q2", "q3", "/quit"])

    code = run_cli(["chat", "--task-id", TASK, "--engine", engine_cmd()])
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS  # §5: errors never break the session
    assert captured.err.count("provider exploded") == 3
    assert "packetsage doctor" in captured.err


def test_chat_report_path_defaults_to_the_task_id():
    assert cli._report_path("", TASK) == Path(f"report-{TASK}.md")
    assert cli._report_path(None, TASK) is None
    assert cli._report_path("out/report.md", TASK) == Path("out/report.md")


def test_chat_report_writes_the_report_and_still_exits_zero(capsys, monkeypatch, tmp_path):
    monkeypatch.setattr(cli, "_interactive", lambda: True)
    monkeypatch.setattr(cli, "_readline", lambda: (None, None))
    feed_inputs(monkeypatch, ["/quit"])
    out = tmp_path / "report.md"

    code = run_cli(
        ["chat", "--task-id", TASK, "--engine", engine_cmd(), "--report", str(out)]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    assert out.is_file()
    assert "report written:" in captured.out


def test_interrupt_guard_hints_on_idle_and_warns_during_a_turn(capsys):
    guard = cli.InterruptGuard()
    guard._handle(2, None)  # idle prompt: never exits the session
    idle = capsys.readouterr().err
    assert "Ctrl-D" in idle or "Ctrl-D" in idle or "/quit" in idle

    guard.busy = True
    guard._handle(2, None)  # during a turn: ask for a clean stop, warn
    busy = capsys.readouterr().err
    assert "WARN" in busy
    assert guard.interrupts == 2


# --------------------------------------------------------------------------
# report
# --------------------------------------------------------------------------
def test_report_writes_one_line_and_backfills_the_engine(capsys, tmp_path):
    out = tmp_path / "report.md"
    code = run_cli(
        ["report", "--task-id", TASK, "--engine", engine_cmd(), "--report", str(out)]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    assert out.is_file()
    assert captured.out.startswith(f"report written: {out}")
    assert "sha256=" in captured.out and "degraded=0" in captured.out


def test_report_defaults_to_report_task_id_md(capsys, monkeypatch, tmp_path):
    monkeypatch.chdir(tmp_path)
    code = run_cli(["report", "--task-id", TASK, "--engine", engine_cmd()])
    assert code == cli.EXIT_SUCCESS
    assert (tmp_path / f"report-{TASK}.md").is_file()
    assert capsys.readouterr().out.startswith("report written:")


def test_report_lint_hard_failure_is_exit_four(capsys, monkeypatch, tmp_path):
    from packetsage_agent import report as report_module

    def explode(self, _path):
        raise report_module.AntiHallucinationError("anti-hallucination: cited 999")

    monkeypatch.setattr(report_module.ReportGenerator, "generate", explode)
    code = run_cli(
        ["report", "--task-id", TASK, "--engine", engine_cmd(), "--report", str(tmp_path / "r.md")]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_INTERNAL
    assert "anti-hallucination" in captured.err


# --------------------------------------------------------------------------
# configuration consumption (§9)
# --------------------------------------------------------------------------
def _settings(**overrides):
    from packetsage_agent.config import Settings

    base = {
        "provider": "mock",
        "model": None,
        "api_key": None,
        "base_url": None,
        "engine_path": None,
        "max_steps": 12,
        "max_llm_calls": 24,
        "max_tool_calls": 20,
        "max_same_tool_calls": 5,
        "max_tokens": 200_000,
        "max_cost_cents": 500,
        "scenario": "auto",
        "max_payload_bytes": 65_536,
        "prompt_version": None,
        "temperature": None,
    }
    base.update(overrides)
    return Settings(**base)


def test_rust_sections_are_ignored_but_agent_keys_are_strict(tmp_path, capsys):
    cfg = tmp_path / "packetsage.yaml"
    cfg.write_text(
        "engine:\n  max_sessions: 100000\n"
        "storage:\n  url: sqlite://packetsage.db\n  pool_size: 4\n"
        "rules:\n  path: ./rules\n"
        "agent:\n  max_steps: 4\n"
        "llm:\n  provider: mock\n",
        encoding="utf-8",
    )
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd(), "--config", str(cfg)])
    assert code == cli.EXIT_SUCCESS

    cfg.write_text("agent:\n  max_stesp: 3\n", encoding="utf-8")
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd(), "--config", str(cfg)])
    captured = capsys.readouterr()
    assert code == cli.EXIT_CONFIG
    assert "agent.max_stesp" in captured.err


def test_llm_section_unknown_key_is_fatal(tmp_path, capsys):
    cfg = tmp_path / "packetsage.yaml"
    cfg.write_text("llm:\n  provder: mock\n", encoding="utf-8")
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd(), "--config", str(cfg)])
    assert code == cli.EXIT_CONFIG
    assert "llm.provder" in capsys.readouterr().err


def test_token_budget_accepts_both_spellings(tmp_path):
    cfg = tmp_path / "packetsage.yaml"
    cfg.write_text("agent:\n  max_tokens_total: 1234\n", encoding="utf-8")
    loaded = config_module.load(cfg)
    assert config_module.settings(loaded).max_tokens == 1234

    cfg.write_text("agent:\n  max_tokens: 10\n  max_tokens_total: 20\n", encoding="utf-8")
    with pytest.raises(config_module.ConfigError):
        config_module.load(cfg)


def test_explicit_config_missing_is_exit_three(capsys, tmp_path):
    code = run_cli(["run", "--task-id", TASK, "--config", str(tmp_path / "absent.yaml")])
    assert code == cli.EXIT_CONFIG
    assert "absent.yaml" in capsys.readouterr().err


def test_credentials_in_the_config_are_refused(tmp_path):
    cfg = tmp_path / "packetsage.yaml"
    cfg.write_text("llm:\n  api_key: sk-123\n", encoding="utf-8")
    with pytest.raises(config_module.ConfigError):
        config_module.load(cfg)


def test_prompt_version_is_selectable_and_typos_are_fatal(tmp_path, capsys):
    """S5: the shipped default is v2; a wrong version is exit 3, never a silent fallback."""
    cfg = tmp_path / "packetsage.yaml"
    cfg.write_text("agent:\n  prompt_version: v1\n", encoding="utf-8")
    assert config_module.settings(config_module.load(cfg)).prompt_version == "v1"
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd(), "--config", str(cfg)])
    assert code == cli.EXIT_SUCCESS

    cfg.write_text("agent:\n  prompt_version: v3\n", encoding="utf-8")
    code = run_cli(["run", "--task-id", TASK, "--engine", engine_cmd(), "--config", str(cfg)])
    captured = capsys.readouterr()
    assert code == cli.EXIT_CONFIG
    assert "agent.prompt_version" in captured.err
    assert "v2, v1" in captured.err


def test_prompt_version_defaults_to_v2(tmp_path):
    cfg = tmp_path / "packetsage.yaml"
    cfg.write_text("agent:\n  max_steps: 3\n", encoding="utf-8")
    loaded = config_module.load(cfg)
    assert config_module.settings(loaded).prompt_version == "v2"
    assert "agent.prompt_version: v2" in config_module.dump(loaded, config_module.settings(loaded))


# --------------------------------------------------------------------------
# setup: the first step of a new installation
# --------------------------------------------------------------------------
def test_setup_writes_the_env_file(capsys, tmp_path):
    env_file = tmp_path / "agent.env"
    code = run_cli(
        [
            "setup",
            "--provider",
            "mock",
            "--no-verify",
            "--env-file",
            str(env_file),
        ]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    text = env_file.read_text(encoding="utf-8")
    assert "PACKETSAGE_LLM_PROVIDER=mock" in text
    assert "written" in captured.out
    assert "packetsage doctor" in captured.out
    # The credential file is the gitignored one by default (never the YAML).
    assert config_module.DEFAULT_ENV_FILE.name == ".env"
    assert config_module.DEFAULT_ENV_FILE.parent.name == "agent"


def test_setup_masks_the_key_when_printing(capsys, tmp_path):
    code = run_cli(
        [
            "setup",
            "--provider",
            "deepseek",
            "--api-key",
            "sk-super-secret",
            "--no-verify",
            "--print",
            "--env-file",
            str(tmp_path / "unused.env"),
        ]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    assert "sk-super-secret" not in captured.out
    assert "PACKETSAGE_LLM_API_KEY=***" in captured.out
    assert "PACKETSAGE_LLM_BASE_URL=https://api.deepseek.com/v1" in captured.out
    assert not (tmp_path / "unused.env").exists()


def test_setup_needs_a_terminal_or_explicit_flags(capsys, monkeypatch, tmp_path):
    monkeypatch.delenv("PACKETSAGE_LLM_PROVIDER", raising=False)
    code = run_cli(["setup", "--env-file", str(tmp_path / "unused.env")])
    captured = capsys.readouterr()
    assert code == cli.EXIT_USAGE
    assert "--provider" in captured.err


def test_setup_needs_a_key_for_keyed_providers(capsys, monkeypatch):
    monkeypatch.delenv("PACKETSAGE_LLM_API_KEY", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("DEEPSEEK_API_KEY", raising=False)
    code = run_cli(["setup", "--provider", "deepseek", "--no-verify", "--print"])
    captured = capsys.readouterr()
    assert code == cli.EXIT_CONFIG
    assert "API key" in captured.err


def test_setup_keeps_unrelated_env_lines(tmp_path):
    env_file = tmp_path / "agent.env"
    env_file.write_text("# keep me\nPACKETSAGE_LLM_MODEL=old\nSOMETHING_ELSE=1\n", encoding="utf-8")
    code = run_cli(
        ["setup", "--provider", "mock", "--no-verify", "--env-file", str(env_file)]
    )
    assert code == cli.EXIT_SUCCESS
    text = env_file.read_text(encoding="utf-8")
    assert "# keep me" in text
    assert "SOMETHING_ELSE=1" in text
    assert "PACKETSAGE_LLM_PROVIDER=mock" in text


def test_setup_reuses_the_configured_provider(tmp_path, capsys, monkeypatch):
    """Re-running `setup` (or `setup --print`) on a configured machine works."""
    env_file = tmp_path / "agent.env"
    env_file.write_text(
        "PACKETSAGE_LLM_PROVIDER=deepseek\nPACKETSAGE_LLM_MODEL=deepseek-flash\n"
        "PACKETSAGE_LLM_API_KEY=sk-existing\n",
        encoding="utf-8",
    )
    monkeypatch.delenv("PACKETSAGE_LLM_PROVIDER", raising=False)  # let the file decide
    monkeypatch.setenv("PACKETSAGE_ENV_FILE", str(env_file))
    code = run_cli(["setup", "--print", "--no-verify"])
    captured = capsys.readouterr()
    assert code == cli.EXIT_SUCCESS
    assert "provider        deepseek" in captured.out
    assert "deepseek-flash" in captured.out
    assert "*** (已存在)" in captured.out


def test_setup_rejects_a_multiline_key(tmp_path, capsys, monkeypatch):
    """A pasted key + note must not corrupt agent/.env."""
    monkeypatch.setenv("PACKETSAGE_LLM_API_KEY", "sk-good\nmodel: deepseek-flash")
    code = run_cli(
        [
            "setup",
            "--provider",
            "deepseek",
            "--no-verify",
            "--env-file",
            str(tmp_path / "agent.env"),
        ]
    )
    captured = capsys.readouterr()
    assert code == cli.EXIT_USAGE
    assert "含换行" in captured.err
    assert not (tmp_path / "agent.env").exists()


def test_empty_environment_variables_do_not_shadow_the_env_file(tmp_path, monkeypatch):
    """Launchers export empty variables; an empty one must mean "unset"."""
    env_file = tmp_path / "agent.env"
    env_file.write_text(
        "PACKETSAGE_LLM_PROVIDER=deepseek\nPACKETSAGE_LLM_API_KEY=sk-from-file\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("PACKETSAGE_ENV_FILE", str(env_file))
    monkeypatch.setenv("PACKETSAGE_LLM_PROVIDER", "")  # the Rust launcher's habit
    monkeypatch.delenv("PACKETSAGE_LLM_API_KEY", raising=False)

    config_module.load_dotenv()
    loaded = config_module.load(None)
    effective = config_module.settings(loaded)
    assert effective.provider == "deepseek"
    assert effective.api_key == "sk-from-file"


def test_deepseek_is_a_first_class_provider():
    from packetsage_agent.provider import (
        PROVIDER_DEFAULTS,
        PROVIDER_KINDS,
        default_base_url,
        default_model,
    )

    assert "deepseek" in PROVIDER_KINDS
    assert default_base_url("deepseek") == "https://api.deepseek.com/v1"
    # 2026-09-22：`deepseek-chat` / `deepseek-reasoner` 已下线，现役是 flash / v4-pro。
    assert default_model("deepseek") == "deepseek-flash"
    assert set(PROVIDER_DEFAULTS) == {"openai", "deepseek", "local"}


def test_probe_provider_reports_http_status(monkeypatch):
    httpx = pytest.importorskip("httpx")
    from packetsage_agent import provider as provider_module

    class Response:
        status_code = 200

        def json(self):
            return {"data": [{"id": "deepseek-chat"}]}

    monkeypatch.setattr(httpx, "get", lambda *a, **k: Response())
    ok, detail = provider_module.probe_provider("deepseek", api_key="sk-test")
    assert ok and "api.deepseek.com" in detail

    class Unauthorized(Response):
        status_code = 401

    monkeypatch.setattr(httpx, "get", lambda *a, **k: Unauthorized())
    ok, detail = provider_module.probe_provider("deepseek", api_key="sk-bad")
    assert not ok and "401" in detail

    ok, detail = provider_module.probe_provider("mock")
    assert ok and "不联网" in detail


def test_db_url_is_redacted_in_messages():
    assert config_module.redact_url("postgres://user:secret@host/db") == (
        "postgres://user:***@host/db"
    )
    assert config_module.redact_url("sqlite://packetsage.db") == "sqlite://packetsage.db"


def test_db_travels_to_the_engine_environment(monkeypatch):
    parser = cli.build_parser()
    args = parser.parse_args(["run", "--task-id", TASK, "--db", "sqlite://other.db"])
    env = cli.engine_env(args)
    assert env["PACKETSAGE_STORAGE_URL"] == "sqlite://other.db"
    assert env["PACKETSAGE_DB"] == "sqlite://other.db"


def test_engine_defaults_to_packetsage_serve(monkeypatch):
    parser = cli.build_parser()
    args = parser.parse_args(["run", "--task-id", TASK])
    assert cli.engine_argv(args) == ["packetsage", "serve"]
    monkeypatch.setenv("PACKETSAGE_ENGINE", "target/release/packetsage")
    assert cli.engine_argv(args) == ["target/release/packetsage", "serve"]


def test_global_flags_work_before_and_after_the_sub_command():
    parser = cli.build_parser()
    before = parser.parse_args(["-q", "-v", "run", "--task-id", TASK])
    after = parser.parse_args(["run", "--task-id", TASK, "-q", "-v"])
    assert (before.quiet, before.verbose) == (True, 1)
    assert (after.quiet, after.verbose) == (True, 1)


def test_budget_comes_from_the_effective_settings():
    budget = cli._budget(_settings(max_steps=5, max_tool_calls=7))
    assert (budget.max_steps, budget.max_tool_calls) == (5, 7)
