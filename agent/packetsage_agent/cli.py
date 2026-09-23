"""``packetsage-agent`` — the agent command surface (Agent CLI 工程规格书 v0.1).

```text
packetsage-agent run    --task-id <id> [--engine <cmd>] [--db <url>]
packetsage-agent chat   --task-id <id> [--engine <cmd>] [--db <url>] [--report [<path>]]
packetsage-agent report --task-id <id> [--engine <cmd>] [--db <url>] [--report <path>]
packetsage-agent --version | --help
```

The task must already exist: this process never analyses a capture and never
touches the database itself (ADR-018) — `--db` only travels to the engine it
spawns. Three streams are kept apart (§7): stdout carries the product (run
summary, REPL, report line), stderr carries logs, progress, usage lines, WARNs
and errors. ``PROG`` is pinned so ``packetsage-agent --help`` and
``python -m packetsage_agent --help`` are byte-for-byte identical.
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import shlex
import signal
import sys
import traceback
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, NoReturn

PROG = "packetsage-agent"

#: ASCII title (`banner.txt`, hardcoded package data): shown by
#: `--help` and at the start of the REPL, never on the machine-facing streams.
from .banner import banner  # noqa: E402 - a constant import, kept next to PROG

#: The frozen machine payloads (收口 §5.11); one definition for CLI and sidecar.
from .payloads import run_json_payload, run_status  # noqa: E402

_run_status = run_status

EXIT_SUCCESS = 0
EXIT_USAGE = 1
EXIT_CAPTURE = 2
EXIT_CONFIG = 3
EXIT_INTERNAL = 4
EXIT_UNSUPPORTED = 5

#: Engine command used when neither ``--engine`` nor ``$PACKETSAGE_ENGINE`` say
#: otherwise (§3). The Rust launcher hands its own binary over through the
#: environment, so a development tree drives the matching engine.
DEFAULT_ENGINE: tuple[str, ...] = ("packetsage", "serve")

#: Engine stderr is forwarded here (M2v0.2 §7.3); the path is never fed to the
#: model, only printed when the engine dies (§10).
ENGINE_LOG_NAME = "packetsage-agent.engine.log"

#: Built-ins inside `chat` (§5: no other slash command).
CHAT_COMMANDS = ("/help", "/quit")

CHAT_TTY_REQUIRED = (
    "交互模式需要终端；批式请用 `packetsage-agent run`"
)
SETUP_HINT = (
    "先运行 `packetsage-agent setup` 配置 provider 与 API key（写入 agent/.env，"
    "不进 config、不进 git），再 `packetsage doctor` 复核"
)
TASK_HINT = (
    "先 `packetsage analyze` 或 `packetsage chat` 创建该 task，"
    "再用 --task-id 引用它"
)


class UsageError(RuntimeError):
    """An argument combination argparse cannot express (exit 1)."""


class TaskNotFound(RuntimeError):
    """The engine does not know this task id (exit 3 + hint, §1/§8)."""


class HardInterrupt(BaseException):
    """Second Ctrl-C of `run`: leave immediately with exit 4 (§4)."""


class _Parser(argparse.ArgumentParser):
    """argparse with the project's usage exit code (§8: usage errors are 1)."""

    def __init__(self, *args: Any, show_banner: bool = True, **kwargs: Any) -> None:
        #: The ASCII title heads the top-level help, not every sub-command's.
        self.show_banner = show_banner
        super().__init__(*args, **kwargs)

    def format_help(self) -> str:
        """`--help` opens with the project art (sub-commands stay text only)."""
        text = super().format_help()
        if not self.show_banner:
            return text
        title = banner()
        return f"{title}\n\n{text}" if title else text

    def error(self, message: str) -> NoReturn:  # noqa: D102 - argparse hook
        self.print_usage(sys.stderr)
        self.exit(EXIT_USAGE, f"{self.prog}: error: {message}\n")


@dataclass
class Context:
    """Everything one invocation needs after parsing."""

    loaded: Any
    settings: Any
    verbose: int = 0
    quiet: bool = False

    @property
    def provider(self) -> str:
        """Effective provider kind."""
        return str(self.settings.provider)

    @property
    def model(self) -> str | None:
        """Effective model name, when the provider needs one."""
        return self.settings.model if isinstance(self.settings.model, str) else None


# --------------------------------------------------------------------------
# entry point
# --------------------------------------------------------------------------
def version_line() -> str:
    """``packetsage-agent <version>`` — the single line the launcher probes."""
    from ._version import package_version

    return f"{PROG} {package_version()}"


def _is_version_fast_path(argv: Sequence[str]) -> bool:
    """True for a bare ``--version``: no config, no spawn, no .env, no network."""
    return list(argv) == ["--version"]


def _make_streams_safe() -> None:
    """Best effort UTF-8 output on a legacy console (#43, 收口 v0.2 #29).

    Three problems are solved here:

    * a redirected stream would otherwise use the ANSI code page (GBK on this
      host) while the engine writes UTF-8 — ``packetsage-agent run > out.txt``
      must produce the same bytes the Rust CLI produces;
    * the progress/summary lines carry ``·`` and ``¢``, which a legacy console
      cannot encode — with ``errors="replace"`` that costs one glyph instead of
      turning a good run into exit 4.
    * **stdin too** (P10): ``packetsage-agent serve`` reading a pipe would
      otherwise decode the JSONL command frames as GBK, so a hand-started
      sidecar mangled any non-ASCII goal. The shell path was already fine
      because it injects ``PYTHONUTF8``/``PYTHONIOENCODING``; this closes the
      manual path.

    On a real console (PEP 528) the encoding is left alone: Python already
    writes through the Unicode console API there.
    """
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:  # pytest capture or another wrapper
            continue
        try:
            if _isatty(stream):
                reconfigure(errors="replace")
            else:
                reconfigure(encoding="utf-8", errors="replace")
        except (ValueError, OSError):  # pragma: no cover - detached stream
            pass


def main(argv: Sequence[str] | None = None) -> int:
    """Console-script / ``python -m`` entry point."""
    _make_streams_safe()
    arguments = list(sys.argv[1:] if argv is None else argv)
    if _is_version_fast_path(arguments):
        print(version_line())
        return EXIT_SUCCESS
    parser = build_parser()
    try:
        args = parser.parse_args(arguments)
    except SystemExit as exit_request:  # --help / --version / usage error
        code = exit_request.code
        return EXIT_SUCCESS if code is None else int(code)
    try:
        return _dispatch(parser, args)
    except UsageError as exc:
        print(f"{PROG}: {exc}", file=sys.stderr)
        return EXIT_USAGE
    except TaskNotFound as exc:
        print(f"{PROG}: {exc}", file=sys.stderr)
        print(f"{PROG}: {TASK_HINT}", file=sys.stderr)
        return EXIT_CONFIG
    except _config_module().ConfigError as exc:
        print(f"{PROG}: {exc}", file=sys.stderr)
        return EXIT_CONFIG
    except _engine_module().EngineSpawnError as exc:
        # §10: the full engine command, with any URL password masked.
        print(
            f"{PROG}: cannot start the engine {_safe_engine_line(args)}: {exc.cause}",
            file=sys.stderr,
        )
        print(
            f"{PROG}: check that the engine is installed and on PATH, or point "
            f"$PACKETSAGE_ENGINE / --engine at it",
            file=sys.stderr,
        )
        return EXIT_CONFIG
    except _engine_module().EngineCrashed as exc:
        return _engine_crash_exit(exc)
    except _engine_module().EngineError as exc:
        print(f"{PROG}: {exc}", file=sys.stderr)
        return EXIT_CONFIG
    except FileNotFoundError as exc:
        print(f"{PROG}: {exc}", file=sys.stderr)
        return EXIT_CAPTURE
    except HardInterrupt:
        # Second Ctrl-C of `run` (§4): the engine is closed by its context
        # manager on the way out, no partial findings are rolled back.
        return EXIT_INTERNAL
    except KeyboardInterrupt:  # pragma: no cover - handlers cover the REPL
        return EXIT_INTERNAL
    except Exception:  # §10: the audience is developers, keep the traceback
        traceback.print_exc()
        return EXIT_INTERNAL


def _config_module():  # noqa: ANN202 - lazy import keeps `--version` fast
    from . import config

    return config


def _engine_module():  # noqa: ANN202 - lazy import keeps `--version` fast
    from . import engine_client

    return engine_client


def _dispatch(parser: argparse.ArgumentParser, args: argparse.Namespace) -> int:
    """Loads the configuration chain and runs the sub-command (§9)."""
    config = _config_module()
    # `.env` is a Python-side concern only (§7.1): the Rust engine never reads it.
    config.load_dotenv()
    _validate_command(args)
    loaded = config.load(getattr(args, "config", None))
    settings = config.settings(loaded)
    context = Context(
        loaded=loaded,
        settings=settings,
        verbose=int(getattr(args, "verbose", 0) or 0),
        quiet=bool(getattr(args, "quiet", False)),
    )
    if context.verbose >= 1:
        print(
            f"{PROG}: info: config source={loaded.source} provider={settings.provider} "
            f"model={settings.model or '-'} engine={' '.join(engine_argv(args))}",
            file=sys.stderr,
        )
    if context.verbose >= 2:
        print(f"{PROG}: info: effective configuration (redacted)\n{config.dump(loaded, settings)}",
              file=sys.stderr)
    return int(args.func(args, context))


def _validate_command(args: argparse.Namespace) -> None:
    """Fail fast on flag combinations that cannot mean one thing (§3)."""
    url = getattr(args, "db", None)
    raw_engine = getattr(args, "engine", None)
    if url and raw_engine:
        try:
            tokens = split_command(raw_engine)
        except ValueError:
            tokens = raw_engine.split()
        if any(token == "--db" or token.startswith("--db=") for token in tokens):
            raise UsageError(
                "--db and an --engine command that already carries --db cannot be "
                "combined: they would name two storage URLs"
            )


def _engine_crash_exit(exc: Any) -> int:
    """EngineCrashed → pass the engine's exit code through (§8)."""
    code = getattr(exc, "returncode", None)
    print(f"{PROG}: engine died: {exc}", file=sys.stderr)
    log_path = getattr(getattr(exc, "client", None), "log_path", None)
    if log_path is not None:
        print(f"{PROG}: engine log: {Path(log_path).resolve()}", file=sys.stderr)
    tail_source = getattr(exc, "stderr_tail", None)
    tail = list(tail_source(3)) if callable(tail_source) else []
    for line in tail:
        print(f"{PROG}: engine> {line}", file=sys.stderr)
    if code in (EXIT_CAPTURE, EXIT_CONFIG, EXIT_INTERNAL):
        return int(code)
    return EXIT_INTERNAL


# --------------------------------------------------------------------------
# parser
# --------------------------------------------------------------------------
def _add_global_flags(parser: argparse.ArgumentParser, *, inherit: bool = False) -> None:
    """`--config` / `-v` / `-q` (§7); accepted before *and* after the sub-command.

    ``inherit=True`` marks the sub-command copy: its defaults are suppressed so
    the root parser's values survive when the flag is not repeated.
    """
    default: Any = argparse.SUPPRESS if inherit else None
    parser.add_argument(
        "--config",
        metavar="PATH",
        default=default,
        help="explicit configuration file (default: $PACKETSAGE_CONFIG, then ./packetsage.yaml)",
    )
    parser.add_argument(
        "-v",
        dest="verbose",
        action="count",
        default=argparse.SUPPRESS if inherit else 0,
        help="stderr log level: -v info, -vv debug (default warn)",
    )
    parser.add_argument(
        "-q",
        dest="quiet",
        action="store_true",
        default=argparse.SUPPRESS if inherit else False,
        help="silence progress and usage lines (errors and WARNs still print)",
    )


def _add_task_flags(parser: argparse.ArgumentParser) -> None:
    """`--task-id` / `--engine` / `--db` — shared by the three sub-commands."""
    parser.add_argument(
        "--task-id",
        required=True,
        metavar="ID",
        help="task to work on (created by `packetsage analyze`; existence is an engine question)",
    )
    parser.add_argument(
        "--engine",
        metavar="CMD",
        help='engine command prefix, shlex split (default: "packetsage serve", or $PACKETSAGE_ENGINE)',
    )
    parser.add_argument(
        "--db",
        metavar="URL",
        help="storage URL for the spawned engine (never opened by this process, ADR-018)",
    )


def build_parser() -> argparse.ArgumentParser:
    """Builds the parser; `prog` is pinned for the two-form help contract (§2)."""
    parser = _Parser(
        prog=PROG,
        description=(
            "YeLee' PacketSage agent: evidence-first analysis over the JSONL RPC engine"
        ),
    )
    _add_global_flags(parser)
    parser.add_argument(
        "--version",
        action="version",
        version=version_line(),
        help="print the agent version and exit",
    )
    sub = parser.add_subparsers(
        dest="command", required=True, metavar="{run,chat,report,setup,serve}"
    )

    run = sub.add_parser(
        "run", help="batch investigation: run the loop once and summarise", show_banner=False
    )
    _add_global_flags(run, inherit=True)
    _add_task_flags(run)
    run.add_argument(
        "--json",
        action="store_true",
        help="emit one JSON object on stdout instead of the human summary (GUI contract §5.3)",
    )
    run.set_defaults(func=cmd_run)

    chat = sub.add_parser(
        "chat", help="interactive session over an analysed task", show_banner=False
    )
    _add_global_flags(chat, inherit=True)
    _add_task_flags(chat)
    chat.add_argument(
        "--report",
        nargs="?",
        const="",
        default=None,
        metavar="PATH",
        help="write a report when the session ends (default: report-<task_id>.md)",
    )
    chat.set_defaults(func=cmd_chat)

    report = sub.add_parser(
        "report", help="render the report of an analysed task", show_banner=False
    )
    _add_global_flags(report, inherit=True)
    _add_task_flags(report)
    report.add_argument(
        "--report",
        nargs="?",
        const="",
        default=None,
        metavar="PATH",
        help="output path (default: report-<task_id>.md)",
    )
    report.set_defaults(func=cmd_report)

    setup = sub.add_parser(
        "setup",
        help="first step: choose the LLM provider and store its API key",
        description=(
            "Configures the provider the agent talks to and writes the credential to "
            "agent/.env (never to packetsage.yaml, never to git). Run it once per "
            "machine; `packetsage-agent run`/`chat` refuse to start until it is done."
        ),
        show_banner=False,
    )
    _add_global_flags(setup, inherit=True)
    setup.add_argument(
        "--provider",
        choices=_provider_kinds(),
        help="mock | openai | deepseek | local (interactive choice when omitted)",
    )
    setup.add_argument("--model", help="model name (default: the provider's documented one)")
    setup.add_argument("--base-url", dest="base_url", help="OpenAI-compatible endpoint")
    setup.add_argument(
        "--api-key",
        dest="api_key",
        help="API key to store (omit it and setup asks, without echoing)",
    )
    setup.add_argument(
        "--env-file",
        dest="env_file",
        help="file to write (default: agent/.env, which is gitignored)",
    )
    setup.add_argument(
        "--no-verify",
        dest="verify",
        action="store_false",
        help="skip the live endpoint/key check",
    )
    setup.add_argument(
        "--print",
        dest="print_only",
        action="store_true",
        help="show the environment block instead of writing a file",
    )
    setup.add_argument(
        "--list-models",
        dest="list_models",
        action="store_true",
        help="print {\"models\": [...]} from GET /models and exit (desktop model picker)",
    )
    setup.set_defaults(func=cmd_setup)

    serve = sub.add_parser(
        "serve",
        # Help text stays ASCII: `packetsage-agent --help` must survive a legacy
        # (GBK) pipe byte-for-byte, which is what the CLI contract test checks.
        help="desktop sidecar: stdio JSONL commands and events (GUI spec v0.2 s4.5)",
        description=(
            "Long-lived child process for the desktop application: commands arrive on "
            "stdin ({hello,run,chat,report,cancel,status,shutdown}), acks and events leave "
            "on stdout, stderr stays diagnostics-only. See GUI spec v0.2 section 4.5."
        ),
        show_banner=False,
    )
    _add_global_flags(serve, inherit=True)
    serve.add_argument(
        "--engine",
        metavar="CMD",
        help='engine command prefix, shlex split (default: "packetsage serve", or $PACKETSAGE_ENGINE)',
    )
    serve.add_argument(
        "--db",
        metavar="URL",
        help="storage URL for the spawned engine (never opened by this process, ADR-018)",
    )
    serve.set_defaults(func=cmd_serve)
    return parser


def _provider_kinds() -> tuple[str, ...]:
    """Provider kinds, imported lazily so `--version` stays a fast path."""
    from .provider import PROVIDER_KINDS

    return PROVIDER_KINDS


# --------------------------------------------------------------------------
# engine plumbing
# --------------------------------------------------------------------------
def engine_argv(args: argparse.Namespace) -> list[str]:
    """The engine command prefix (§3): `--engine`, else `$PACKETSAGE_ENGINE serve`."""
    raw = getattr(args, "engine", None)
    if raw:
        return split_command(raw)
    from_env = os.environ.get("PACKETSAGE_ENGINE")
    if from_env:
        return [from_env, "serve"]
    return list(DEFAULT_ENGINE)


def split_command(raw: str) -> list[str]:
    """Splits an `--engine` command line (§3).

    POSIX `shlex.split` eats the backslashes of a Windows path
    (``target\\debug\\packetsage.exe`` → ``targetdebugpacketsage.exe``), so on
    Windows the non-POSIX mode is used and surrounding quotes are dropped.
    """
    if os.name != "nt":
        return shlex.split(raw)
    return [_strip_quotes(token) for token in shlex.split(raw, posix=False)]


def _strip_quotes(token: str) -> str:
    if len(token) >= 2 and token[0] == token[-1] and token[0] in "\"'":
        return token[1:-1]
    return token


def engine_env(args: argparse.Namespace) -> dict[str, str]:
    """Environment of the engine child; `--db` travels here (#41)."""
    env = dict(os.environ)
    url = getattr(args, "db", None)
    if url:
        # `packetsage serve` has no `--db` flag: the storage URL is read from the
        # environment (serve.rs `env_first(["PACKETSAGE_STORAGE_URL",
        # "PACKETSAGE_DB"])`). Both names are set so `--db` beats a stale shell
        # variable, and the agent itself never opens the URL (ADR-018).
        env["PACKETSAGE_STORAGE_URL"] = url
        env["PACKETSAGE_DB"] = url
    return env


def redacted_engine_line(args: argparse.Namespace) -> str:
    """Engine command for error messages, with any URL password masked (§10)."""
    config = _config_module()
    return " ".join(config.redact_url(token) for token in engine_argv(args))


def _safe_engine_line(args: argparse.Namespace) -> str:
    """`redacted_engine_line` that never fails while reporting a failure."""
    try:
        return redacted_engine_line(args)
    except ValueError:  # unbalanced quotes in --engine
        return "<unparsable --engine command>"


def _open_engine(args: argparse.Namespace) -> Any:
    """Spawns the engine, forwards its stderr to a local log, pings it."""
    engine_client = _engine_module()
    client = engine_client.EngineClient(
        engine_argv(args),
        env=engine_env(args),
        log_path=ENGINE_LOG_NAME,
    )
    try:
        client.call("ping", {})
    except BaseException:
        client.close()
        raise
    return client


def _require_task(client: Any, task_id: str) -> dict[str, Any]:
    """The engine decides whether the task exists (§1/§8)."""
    engine_client = _engine_module()
    try:
        content = _content(client.call("get_capture_summary", {"task_id": task_id}))
    except engine_client.RpcToolError as exc:
        # Engine codes are SCREAMING_SNAKE_CASE; normalise before comparing so
        # `NOT_FOUND`, `NotFound` and `not_found` all mean the same thing.
        code = str(getattr(exc, "code", "")).replace("_", "").replace("-", "").lower()
        if code in ("notfound", "captureunavailable"):
            raise TaskNotFound(f"引擎查无 task {task_id}: {exc}") from exc
        raise
    if not isinstance(content, dict):
        raise engine_client.EngineError(
            f"get_capture_summary returned {type(content).__name__}, expected an object"
        )
    return content


def _budget(effective: Any) -> Any:
    """AgentBudget from the effective settings (§9)."""
    from .policy import budget_from_settings

    return budget_from_settings(effective)


def _content(call: Any) -> Any:
    """Unwraps the frozen tool envelope when there is one."""
    if isinstance(call, dict) and "content" in call:
        return call["content"]
    return call


def _banner(client: Any, task_id: str, effective: Any) -> str:
    """One line of task overview for the REPL start (§5)."""
    engine_error = _engine_module().EngineError
    packets = sessions = alerts = decode_errors = "?"
    try:
        summary = _content(client.call("get_capture_summary", {"task_id": task_id}))
        if isinstance(summary, dict):
            packets = summary.get("packets", packets)
            sessions = summary.get("sessions", sessions)
            alerts = summary.get("alerts", alerts)
            decode_errors = summary.get("decode_errors", decode_errors)
    except engine_error:
        pass
    rules: Any = "?"
    try:
        # No `path`: the engine answers with the embedded rule set, which is
        # what `analyze` actually evaluates when no rules directory overrides it.
        listed = _content(client.call("list_rules", {}))
        if isinstance(listed, dict):
            rule_rows = listed.get("rules")
            if isinstance(rule_rows, list):
                rules = len(rule_rows)
            elif isinstance(listed.get("loaded"), int):
                rules = listed["loaded"]
    except engine_error:
        pass
    return (
        f"task {task_id} | packets={packets} sessions={sessions} "
        f"alerts={alerts} decode_errors={decode_errors} rules={rules} "
        f"| provider={effective.provider} model={effective.model or '-'}"
    )


def _degraded_warnings(result: Any) -> None:
    """Run diagnostics on stderr (§4): notes always, the degraded line when due.

    A run can finish `completed` and still have something to report — a rejected
    finding (`submit_rejects>0`) is exactly the case a user must see, otherwise
    the one-line stdout summary silently hides why a finding is missing.
    """
    for note in result.notes:
        print(f"{PROG}: WARN {note}", file=sys.stderr)
    if result.status != "ok" and not result.notes:
        print(
            f"{PROG}: WARN run finished as {result.status} "
            f"({result.stop_reason or 'no stop reason recorded'})",
            file=sys.stderr,
        )
    elif result.submit_rejects and not any("reject" in note for note in result.notes):
        print(
            f"{PROG}: WARN {result.submit_rejects} finding(s) were refused by the "
            "engine's evidence checks (V1-V4)",
            file=sys.stderr,
        )


# --------------------------------------------------------------------------
# run
# --------------------------------------------------------------------------
def _provider_gate(context: Context) -> int | None:
    """Refuses to run an investigation without a configured provider.

    `mock` is allowed, but only when it is named explicitly (CI, demos, E2E);
    an unset provider is a *setup* problem, not an invitation to fake results.
    """
    config = _config_module()
    unavailable = config.provider_unavailable(context.settings)
    if not unavailable:
        return None
    print(
        f"{PROG}: provider {context.provider or '(unset)'}: {unavailable}",
        file=sys.stderr,
    )
    print(f"{PROG}: {SETUP_HINT}", file=sys.stderr)
    return EXIT_CONFIG


class RunInterrupts:
    """Ctrl-C semantics of `run` (§4).

    First SIGINT asks the loop to finalize (partial findings are kept); the
    second one leaves immediately with exit 4.
    """

    def __init__(self, agent: Any | None = None) -> None:
        self.agent = agent
        self.count = 0
        self._previous: Any = None
        self.installed = False

    def __enter__(self) -> RunInterrupts:
        try:
            self._previous = signal.signal(signal.SIGINT, self._handle)
            self.installed = True
        except (ValueError, OSError):  # not the main thread: keep the default
            self.installed = False
        return self

    def __exit__(self, *_exc: object) -> None:
        if self.installed:
            signal.signal(signal.SIGINT, self._previous)

    def _handle(self, _signum: int, _frame: Any) -> None:
        self.count += 1
        if self.count == 1 and self.agent is not None:
            self.agent.request_finalize()
            print(f"{PROG}: run interrupted, partial results kept", file=sys.stderr)
            return
        raise HardInterrupt()


def _print_run_summary(result: Any, agent: Any, as_json: bool = False) -> None:
    """One stdout line: status, findings counters and budget usage (§4)."""
    from .progress import usage_line

    if as_json:
        # Exactly one line: the GUI contract (S42) forbids extra stdout lines.
        print(json.dumps(run_json_payload(result, agent), ensure_ascii=False))
        return
    print(
        f"run {result.agent_run_id} task={result.task_id} status={_run_status(result)} "
        f"accepted={len(result.findings)} submit_rejects={result.submit_rejects} "
        f"malformed_output={1 if result.malformed_output else 0} "
        f"{usage_line(agent.policy.state, agent.policy.budget)}"
    )
    for finding in result.stored_findings or []:
        print(
            f"  {str(finding.get('severity', '?')):<6} "
            f"{str(finding.get('basis', '?')):<24} "
            f"{finding.get('finding_id', '')} {finding.get('title', '')}".rstrip()
        )
    if not result.stored_findings:
        for draft in result.findings:
            print(f"  {draft.severity:<6} {draft.basis:<24} {draft.title}")


def cmd_run(args: argparse.Namespace, context: Context) -> int:
    """Batch investigation: engine → agent loop → stdout summary (§4)."""
    from .agent import build_agent

    blocked = _provider_gate(context)
    if blocked is not None:
        return blocked
    progress = _progress_for(context)
    with _open_engine(args) as client:
        _require_task(client, args.task_id)
        agent = build_agent(
            client,
            provider_kind=context.provider,
            model=context.model,
            scenario=context.settings.scenario,
            budget=_budget(context.settings),
            observer=progress,
            mode="run",
            prompt_version=context.settings.prompt_version,
            base_url=context.settings.base_url,
            api_key=context.settings.api_key,
        )
        with RunInterrupts(agent):
            result = agent.run(args.task_id)
        _print_run_summary(result, agent, as_json=bool(getattr(args, "json", False)))
        _degraded_warnings(result)
    return EXIT_SUCCESS


# --------------------------------------------------------------------------
# chat
# --------------------------------------------------------------------------
class InterruptGuard:
    """Ctrl-C semantics of the REPL (§5).

    * idle prompt: print the hint, never end the session;
    * during a turn: ask the run to stop at the next boundary and warn that the
      current turn may finish first (#28: providers cannot be cancelled cleanly);
    * the session survives either way.
    """

    def __init__(self) -> None:
        self.busy = False
        self.agent: Any | None = None
        self.interrupts = 0
        self._previous: Any = None
        self.installed = False

    def __enter__(self) -> InterruptGuard:
        try:
            self._previous = signal.signal(signal.SIGINT, self._handle)
            self.installed = True
        except (ValueError, OSError):  # not the main thread: keep default behaviour
            self.installed = False
        return self

    def __exit__(self, *_exc: object) -> None:
        if self.installed:
            signal.signal(signal.SIGINT, self._previous)

    def _handle(self, _signum: int, _frame: Any) -> None:
        self.interrupts += 1
        if self.busy:
            if self.agent is not None:
                self.agent.request_finalize()
            print(
                f"{PROG}: WARN Ctrl-C during a turn — stopping after the current step; "
                "findings already submitted are kept",
                file=sys.stderr,
            )
        else:
            print(f"{PROG}: （/quit 或 Ctrl-D 退出）", file=sys.stderr)


def _interactive() -> bool:
    """Both stdin and stdout must be terminals (§5)."""
    return _isatty(sys.stdin) and _isatty(sys.stdout)


def _isatty(stream: Any) -> bool:
    try:
        return bool(stream.isatty())
    except (AttributeError, ValueError):
        return False


def _chat_help() -> str:
    return (
        "built-ins: /help (this text), /quit (same as Ctrl-D). "
        "Anything else is a goal for the agent."
    )


def _chat_prompt(task_id: str) -> str:
    return f"packetsage\u00b7{task_id[:8]}>> "


def _readline() -> tuple[Any, Path | None]:
    """Enables the stdlib history if the platform provides `readline` (§5)."""
    try:
        import readline  # type: ignore
    except ImportError:  # Windows: best-effort, no extra dependency
        return None, None
    # The stdlib `readline` stubs are not available on every platform; the
    # calls below are the documented POSIX/GNU readline API.
    module: Any = readline
    candidate = Path.home() / ".packetsage_chat_history"
    history: Path | None = candidate
    try:
        module.set_history_length(500)
        if candidate.is_file():
            module.read_history_file(str(candidate))
    except OSError:
        history = None
    return module, history


def _save_history(readline: Any, history: Path | None) -> None:
    if readline is None or history is None:
        return
    try:
        readline.write_history_file(str(history))
    except OSError:
        pass


def _report_path(argument: str | None, task_id: str) -> Path | None:
    """`--report [<path>]` → the path to write, or None when not requested."""
    if argument is None:
        return None
    if argument == "":
        return Path(f"report-{task_id}.md")
    return Path(argument)


def _write_report(
    client: Any,
    context: Context,
    task_id: str,
    path: Path,
    run_result: Any | None,
) -> tuple[int, Any | None]:
    """Renders the report for `chat --report` (§5: failures keep exit 0)."""
    from .report import AntiHallucinationError, ReportGenerator

    generator = ReportGenerator(
        client,
        task_id,
        getattr(run_result, "agent_run_id", "") or f"chat_{task_id}",
        model=getattr(run_result, "model", None) or context.model or "unknown",
        provider=context.provider,
        temperature=context.settings.temperature or 0.0,
        tokens_in=getattr(run_result, "tokens_in", 0),
        tokens_out=getattr(run_result, "tokens_out", 0),
        cost_cents=getattr(run_result, "cost_cents", 0),
        prompt_version=context.settings.prompt_version,
    )
    try:
        meta = generator.generate(path)
    except AntiHallucinationError as exc:
        # §8: the anti-hallucination gate is a hard failure everywhere.
        print(f"{PROG}: {exc}", file=sys.stderr)
        return EXIT_INTERNAL, None
    except (OSError, _engine_module().EngineError) as exc:
        print(f"{PROG}: WARN report 生成失败（不影响退出码）: {exc}", file=sys.stderr)
        return EXIT_SUCCESS, None
    print(f"report written: {meta.path} sha256={meta.sha256} degraded={meta.unverified_count}")
    return EXIT_SUCCESS, meta


def cmd_chat(args: argparse.Namespace, context: Context) -> int:
    """Interactive REPL over an already analysed task (§5)."""
    blocked = _provider_gate(context)
    if blocked is not None:
        return blocked
    if not _interactive():
        print(f"{PROG}: {CHAT_TTY_REQUIRED}", file=sys.stderr)
        return EXIT_USAGE

    readline, history = _readline()
    progress = _progress_for(context)
    last_result: Any = None
    with _open_engine(args) as client:
        _require_task(client, args.task_id)
        # The banner is product output, not a log: it belongs to stdout (§7).
        title = banner()
        if title:
            print(title)
        print(_banner(client, args.task_id, context.settings))
        prompt = _chat_prompt(args.task_id)
        failures = 0
        with InterruptGuard() as guard:
            while True:
                try:
                    line = input(prompt)
                except EOFError:
                    break
                except KeyboardInterrupt:  # pragma: no cover - handler installed
                    continue
                text = line.strip()
                if not text:
                    continue
                if text == "/quit":
                    break
                if text == "/help":
                    print(_chat_help(), file=sys.stderr)
                    continue
                if text.startswith("/"):
                    print(f"{PROG}: unknown command {text!r}; {_chat_help()}", file=sys.stderr)
                    continue
                guard.busy = True
                try:
                    # The agent exists before the turn runs, so a Ctrl-C during
                    # it can ask *this* run to stop (guard.agent).
                    agent = _chat_agent(client, context)
                    guard.agent = agent
                    result = agent.run(args.task_id, text)
                    last_result = result
                    failures = 0
                    for finding in result.findings:
                        print(f"{finding.severity:<6} {finding.title}: {finding.summary}")
                    _degraded_warnings(result)
                    progress.llm_round(agent.policy.state, agent.policy.budget)
                except HardInterrupt:
                    raise
                except KeyboardInterrupt:  # pragma: no cover - handler installed
                    continue
                except Exception as exc:  # a broken turn must not kill the session
                    failures += 1
                    print(f"{PROG}: {type(exc).__name__}: {exc}", file=sys.stderr)
                    if failures >= 3:
                        print(
                            f"{PROG}: 3 turns failed in a row — run `packetsage doctor` "
                            "to check the environment",
                            file=sys.stderr,
                        )
                finally:
                    guard.busy = False
                    guard.agent = None
        _save_history(readline, history)
        path = _report_path(args.report, args.task_id)
        if path is not None:
            code, _meta = _write_report(client, context, args.task_id, path, last_result)
            if code != EXIT_SUCCESS:
                return code
    return EXIT_SUCCESS


def _chat_agent(client: Any, context: Context) -> Any:
    """One REPL turn gets its own agent: fresh budget, own agent_run_id (§5)."""
    from .agent import build_agent

    return build_agent(
        client,
        provider_kind=context.provider,
        model=context.model,
        scenario=context.settings.scenario,
        budget=_budget(context.settings),
        mode="chat",
        prompt_version=context.settings.prompt_version,
        base_url=context.settings.base_url,
        api_key=context.settings.api_key,
    )


def _progress_for(context: Context) -> Any:
    from .progress import Progress

    return Progress(quiet=context.quiet)


# --------------------------------------------------------------------------
# setup (the first step of a new installation)
# --------------------------------------------------------------------------
def _env_path(argument: str | None) -> Path:
    """Where the credentials go: `--env-file`, else `agent/.env` (gitignored).

    `$PACKETSAGE_ENV_FILE` is honoured so a deployment can keep the file outside
    the source tree; whatever it points at is also what the loader reads.
    """
    if argument:
        return Path(argument)
    override = os.environ.get("PACKETSAGE_ENV_FILE")
    if override:
        return Path(override)
    config = _config_module()
    return Path(config.DEFAULT_ENV_FILE)


def _write_env_file(path: Path, values: dict[str, str]) -> None:
    """Merges `values` into `path`, preserving comments and unrelated keys."""
    for key, value in values.items():
        if "\n" in value or "\r" in value:
            raise UsageError(
                f"{key} 的值里含换行（多半是把注释/第二行一起复制进来了）——"
                "只贴 key 本身，或分两次运行 setup"
            )
    existing: list[str] = []
    if path.is_file():
        try:
            existing = path.read_text(encoding="utf-8").splitlines()
        except OSError:  # pragma: no cover
            existing = []
    written: set[str] = set()
    out: list[str] = []
    for line in existing:
        text = line.strip()
        if text and not text.startswith("#") and "=" in text:
            key = text.partition("=")[0].strip()
            if key in values:
                out.append(f"{key}={values[key]}")
                written.add(key)
                continue
        out.append(line)
    missing = [key for key in values if key not in written]
    if missing:
        if out and out[-1].strip():
            out.append("")
        out.extend(f"{key}={values[key]}" for key in missing)
    header = [
        "# PacketSage agent credentials - written by `packetsage-agent setup`.",
        "# Never commit this file (it is gitignored); API keys do not belong in packetsage.yaml.",
        "",
    ]
    text = "\n".join(header + [line for line in out if line is not None]) + "\n"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    if os.name != "nt":
        try:
            os.chmod(path, 0o600)
        except OSError:  # pragma: no cover - exotic filesystem
            pass


def _ask(prompt: str, default: str | None = None) -> str:
    """One interactive line; empty input keeps the default."""
    suffix = f" [{default}]" if default else ""
    answer = input(f"{prompt}{suffix}: ").strip()
    return answer or (default or "")


def _ask_provider(current: str) -> str:
    kinds = _provider_kinds()
    listing = "  ".join(
        f"{index + 1}) {kind}" for index, kind in enumerate(kinds)
    )
    print(listing, file=sys.stderr)
    default = current if current in kinds else "deepseek"
    answer = _ask("provider (输入序号或名字)", default)
    if answer.isdigit() and 1 <= int(answer) <= len(kinds):
        return kinds[int(answer) - 1]
    return answer


def cmd_setup(args: argparse.Namespace, context: Context) -> int:
    """First step of a new installation: provider + key, stored in agent/.env."""
    from .provider import (
        PROVIDERS_NEEDING_KEY,
        default_base_url,
        default_model,
        list_models,
        probe_provider,
    )

    config = _config_module()
    interactive = _interactive()
    existing_provider = (context.settings.provider or "").strip()

    provider = (args.provider or existing_provider).strip()
    if not provider:
        if not interactive:
            raise UsageError(
                "非交互模式下必须给 --provider（mock|openai|deepseek|local）；"
                "想在终端里交互选择，就直接运行 `packetsage-agent setup`"
            )
        provider = _ask_provider(existing_provider)
    if provider not in _provider_kinds():
        raise UsageError(
            f"unknown provider {provider!r} — expected {', '.join(_provider_kinds())}"
        )

    # 桌面端的"模型"下拉：拉一次 /models 就退出，不写任何文件。
    if getattr(args, "list_models", False):
        resolved_key = args.api_key or context.settings.api_key
        names = list_models(
            provider,
            model=args.model,
            base_url=args.base_url or context.settings.base_url or default_base_url(provider),
            api_key=resolved_key,
        )
        print(json.dumps({"models": names}, ensure_ascii=False))
        return EXIT_SUCCESS

    same_provider = provider == existing_provider
    model = (
        args.model
        or (context.settings.model if same_provider else None)
        or default_model(provider)
    )
    base_url = (
        args.base_url
        or (context.settings.base_url if same_provider else None)
        or default_base_url(provider)
    )
    # A key is a key: an exported `$PACKETSAGE_LLM_API_KEY` (or one already in
    # agent/.env) is reused whatever provider is being configured — that is how
    # the non-interactive path is meant to work.
    api_key = args.api_key or context.settings.api_key
    key_source = "命令行" if args.api_key else ("已存在" if api_key else "")
    if provider in PROVIDERS_NEEDING_KEY and not api_key:
        if not interactive:
            print(
                f"{PROG}: {provider} 需要 API key：非交互模式请加 --api-key，"
                "或先 export PACKETSAGE_LLM_API_KEY",
                file=sys.stderr,
            )
            return EXIT_CONFIG
        api_key = getpass.getpass(f"{provider} API key（不回显，留空=跳过）: ").strip()
        key_source = "新输入"
        if not api_key:
            print(f"{PROG}: 没有 key，将只写入 provider/model 设置", file=sys.stderr)

    values: dict[str, str] = {"PACKETSAGE_LLM_PROVIDER": provider}
    if model:
        values["PACKETSAGE_LLM_MODEL"] = model
    if base_url:
        values["PACKETSAGE_LLM_BASE_URL"] = base_url
    if api_key:
        values["PACKETSAGE_LLM_API_KEY"] = api_key

    verified: bool | None = None
    detail = "跳过（--no-verify）"
    if args.verify:
        verified, detail = probe_provider(provider, model, base_url, api_key)

    print(f"provider        {provider}")
    print(f"model           {model or '-'}")
    print(f"base_url        {base_url or '-'}")
    key_desc = f"*** ({key_source})" if api_key and key_source else ("***" if api_key else "-")
    print(f"api key         {key_desc}")
    print(f"verify          {'✅ ' if verified else '⚠ ' if verified is None else '❌ '}{detail}")

    if args.print_only:
        print()
        print(f"# would be written to {_env_path(args.env_file)}")
        for key, value in values.items():
            shown = config.MASK if "API_KEY" in key else value
            print(f"{key}={shown}")
        return EXIT_SUCCESS

    path = _env_path(args.env_file)
    _write_env_file(path, values)
    print(f"written         {path}")
    print()
    print("下一步:")
    print("  packetsage doctor                                  # 复核生效配置")
    print("  packetsage chat samples/synth-mixed.pcap            # 交互式调查")
    print("  packetsage-agent run --task-id <task_…>             # 批式调查")
    if verified is False:
        print()
        print(f"{PROG}: WARN 校验没过（{detail}）—— 配置已写入，修好 key/端点后重跑 setup", file=sys.stderr)
        return EXIT_CONFIG
    return EXIT_SUCCESS


# --------------------------------------------------------------------------
# report
# --------------------------------------------------------------------------
def cmd_report(args: argparse.Namespace, context: Context) -> int:
    """Renders the report of an analysed task from engine data (§6)."""
    from .report import AntiHallucinationError, ReportGenerator

    path = _report_path(args.report, args.task_id) or Path(f"report-{args.task_id}.md")
    with _open_engine(args) as client:
        _require_task(client, args.task_id)
        artifacts = _content(client.call("get_task_artifacts", {"task_id": args.task_id}))
        meta = artifacts.get("report_meta") if isinstance(artifacts, dict) else None
        meta = meta if isinstance(meta, dict) else {}
        generator = ReportGenerator(
            client,
            args.task_id,
            str(meta.get("agent_run_id") or ""),
            model=str(meta.get("model") or context.model or "unknown"),
            provider=str(meta.get("provider") or context.provider),
            temperature=float(meta.get("temperature") or context.settings.temperature or 0.0),
            tokens_in=int(meta.get("tokens_in") or 0),
            tokens_out=int(meta.get("tokens_out") or 0),
            cost_cents=int(meta.get("cost_cents") or 0),
            prompt_version=context.settings.prompt_version,
        )
        try:
            written = generator.generate(path)
        except AntiHallucinationError as exc:
            print(f"{PROG}: {exc}", file=sys.stderr)
            return EXIT_INTERNAL
    print(
        f"report written: {written.path} sha256={written.sha256} "
        f"degraded={written.unverified_count}"
    )
    return EXIT_SUCCESS


# --------------------------------------------------------------------------
# serve (desktop sidecar, 通道 B)
# --------------------------------------------------------------------------
def cmd_serve(args: argparse.Namespace, context: Context) -> int:
    """长驻侧车：把 Agent 变成 GUI 的子进程（《GUI 工程规格书 v0.2》§4.5）。

    这里**故意不过** `_provider_gate`：未配置 provider 由握手的
    `providers.configured=false` 表达，界面据此进首次运行向导（S67）；
    只有在真的收到 `run`/`chat` 时才回 `CONFIG` 错误。
    """
    from .serve import serve_forever

    with _open_engine(args) as client:
        return serve_forever(client, context.settings)


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
