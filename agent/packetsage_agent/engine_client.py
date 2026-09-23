"""JSONL RPC client for `packetsage serve` (M3~M6 §4.2, Agent CLI §10).

Hard constraints implemented here:

* one request per line, one response per line (no packet framing guesses);
* `id` is a uuid4 used **only** for pending-map pairing and timeouts;
* timeouts use the per-method matrix from the spec;
* a crashed engine raises :class:`EngineCrashed` instead of restarting silently;
* engine stderr is forwarded to a local log file and never enters the model
  context (M2v0.2 §7.3): the CLI only ever shows the file path and its tail.
"""

from __future__ import annotations

import json
import os
import queue
import subprocess
import threading
import uuid
from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol


class EngineError(RuntimeError):
    """Base class for engine failures."""


class EngineSpawnError(EngineError):
    """The engine process could not be started at all."""

    def __init__(self, message: str, cause: str = "", cmd: Iterable[str] | None = None) -> None:
        super().__init__(message)
        #: The OS level reason, so a CLI can render its own (redacted) command.
        self.cause = cause or message
        self.cmd = list(cmd or [])


class EngineCrashed(EngineError):
    """The worker process exited or closed its pipes."""

    def __init__(
        self,
        message: str,
        returncode: int | None = None,
        client: EngineClient | None = None,
    ) -> None:
        super().__init__(message)
        self.returncode = returncode
        #: The client that lost its engine, so the CLI can print the stderr tail
        #: and the log path without a second registry (§10).
        self.client = client

    def stderr_tail(self, lines: int = 3) -> list[str]:
        """Last diagnostic lines of the dead engine (§10: tail of 3)."""
        if self.client is None:
            return []
        return self.client.stderr_tail(lines)


class RpcToolError(EngineError):
    """The engine answered with `ok: false`."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


class ToolTimeout(EngineError):
    """The engine did not answer within the method budget."""


@dataclass
class RpcTimeouts:
    """Per-method timeout matrix (seconds)."""

    ping: float = 5.0
    query: float = 10.0
    analyze_file: float = 60.0
    reconstruct_stream: float = 30.0
    default: float = 10.0
    overrides: dict[str, float] = field(default_factory=dict)

    def for_method(self, method: str) -> float:
        if method in self.overrides:
            return self.overrides[method]
        if method == "ping":
            return self.ping
        if method == "analyze_file":
            return self.analyze_file
        if method == "reconstruct_stream":
            return self.reconstruct_stream
        return self.query


@dataclass
class CallResult:
    """One tool call outcome, stored in the analysis trace."""

    method: str
    args: dict[str, Any]
    ok: bool
    envelope: dict[str, Any] | None
    error: str | None
    duration_ms: int

    @property
    def tc_id(self) -> str | None:
        if not self.envelope:
            return None
        return self.envelope.get("_id")

    @property
    def content(self) -> Any:
        if not self.envelope:
            return None
        return self.envelope.get("content")


class RpcCaller(Protocol):
    """The two methods every RPC consumer needs.

    `packetsage-agent serve` drives the agent through a wrapper that serialises
    calls (`serve.LockedClient`), so the loop and the report generator are typed
    against this protocol instead of the concrete ``EngineClient``.
    """

    def call(
        self, method: str, params: dict[str, Any], timeout_s: float | None = None
    ) -> Any:  # pragma: no cover - typing only
        ...

    def call_envelope(
        self, method: str, params: dict[str, Any], step: int = 0
    ) -> CallResult:  # pragma: no cover - typing only
        ...


class EngineClient:
    """Spawns and drives one `packetsage serve` worker."""

    HEARTBEAT_SECONDS = 60.0
    #: How many stderr lines are kept for the crash summary (§10: tail of 3).
    STDERR_KEEP = 200

    def __init__(
        self,
        cmd: Iterable[str],
        timeouts: RpcTimeouts | None = None,
        env: dict[str, str] | None = None,
        log_path: str | os.PathLike[str] | None = None,
    ) -> None:
        self.cmd = list(cmd)
        self.timeouts = timeouts or RpcTimeouts()
        self._pending: dict[str, queue.Queue] = {}
        self._lock = threading.Lock()
        self._unhealthy = False
        self._heartbeat_failures = 0
        self.stderr_lines: list[str] = []
        self.log_path: Path | None = Path(log_path) if log_path is not None else None
        self._log_file: Any = None
        if self.log_path is not None:
            try:
                self.log_path.parent.mkdir(parents=True, exist_ok=True)
                self._log_file = self.log_path.open("a", encoding="utf-8")
            except OSError:  # a read-only cwd must not break the run
                self.log_path = None
        try:
            self._process = subprocess.Popen(  # noqa: S603 - the command is explicit
                self.cmd,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                bufsize=1,
                env=env,
            )
        except OSError as exc:
            self._close_log()
            raise EngineSpawnError(
                f"cannot start the engine {self.command_line}: {exc}",
                cause=str(exc),
                cmd=self.cmd,
            ) from exc
        self._reader = threading.Thread(target=self._read_stdout, daemon=True)
        self._reader.start()
        self._stderr_reader = threading.Thread(target=self._read_stderr, daemon=True)
        self._stderr_reader.start()

    @property
    def command_line(self) -> str:
        """The engine command, for diagnostics (callers redact `--db` URLs)."""
        return " ".join(self.cmd)

    @property
    def exit_code(self) -> int | None:
        """The engine's exit code, or ``None`` while it is still running."""
        return self._process.poll()

    def _exit_code_after_death(self, grace: float = 2.0) -> int | None:
        """Exit code of a worker that is dying right now (§8 needs the number)."""
        code = self._process.poll()
        if code is not None:
            return code
        try:
            return self._process.wait(timeout=grace)
        except subprocess.TimeoutExpired:
            return None

    def stderr_tail(self, lines: int = 3) -> list[str]:
        """The last diagnostic lines, for the EngineCrashed message (§10)."""
        return list(self.stderr_lines[-lines:])

    def _close_log(self) -> None:
        if self._log_file is not None:
            try:
                self._log_file.close()
            except OSError:  # pragma: no cover
                pass
            self._log_file = None

    # ------------------------------------------------------------------ io
    def _read_stdout(self) -> None:
        assert self._process.stdout is not None
        for line in self._process.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                continue
            request_id = payload.get("id", "")
            with self._lock:
                waiter = self._pending.pop(request_id, None)
            if waiter is not None:
                waiter.put(payload)

    def _read_stderr(self) -> None:
        assert self._process.stderr is not None
        for line in self._process.stderr:
            # stderr is diagnostics only: it never enters the model context
            # (M0~M2 §7.3); it is forwarded to a local log file instead.
            text = line.rstrip()
            self.stderr_lines.append(text)
            if len(self.stderr_lines) > self.STDERR_KEEP:
                del self.stderr_lines[: len(self.stderr_lines) - self.STDERR_KEEP]
            if self._log_file is not None:
                try:
                    self._log_file.write(text + "\n")
                    self._log_file.flush()
                except OSError:  # pragma: no cover - the log file went away
                    self._log_file = None

    def close(self) -> None:
        """Shuts the worker down; stderr already read is flushed to the log."""
        if self._process.poll() is None:
            try:
                if self._process.stdin:
                    self._process.stdin.close()
            except OSError:
                pass
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._process.kill()
        self._stderr_reader.join(timeout=1)
        self._close_log()

    def wait(self, timeout: float = 5.0) -> int | None:
        """Waits for the engine to exit (no shutdown request); returns its code."""
        try:
            self._process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:  # pragma: no cover - the caller asked
            pass
        self._stderr_reader.join(timeout=timeout)
        return self._process.returncode

    def __enter__(self) -> EngineClient:
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    # --------------------------------------------------------------- calls
    def call(self, method: str, params: dict[str, Any], timeout_s: float | None = None) -> Any:
        """Calls one RPC method and returns the envelope (not the raw response)."""
        if self._unhealthy:
            raise EngineCrashed(
                "engine marked unhealthy after repeated failures", self._process.poll(), self
            )
        if self._process.poll() is not None:
            raise EngineCrashed(
                f"engine exited with code {self._process.returncode}",
                self._process.returncode,
                self,
            )
        request_id = str(uuid.uuid4())
        waiter: queue.Queue = queue.Queue(maxsize=1)
        with self._lock:
            self._pending[request_id] = waiter
        request = {"id": request_id, "method": method, "params": params}
        line = json.dumps(request, ensure_ascii=False)
        try:
            assert self._process.stdin is not None
            self._process.stdin.write(line + "\n")
            self._process.stdin.flush()
        except (OSError, ValueError) as exc:  # pragma: no cover - process died
            with self._lock:
                self._pending.pop(request_id, None)
            code = self._exit_code_after_death()
            raise EngineCrashed(
                f"cannot write to the engine: {exc}", code, self
            ) from exc
        budget = timeout_s if timeout_s is not None else self.timeouts.for_method(method)
        try:
            response = waiter.get(timeout=budget)
        except queue.Empty as exc:
            with self._lock:
                self._pending.pop(request_id, None)
            if self._process.poll() is not None:
                raise EngineCrashed(
                    f"engine exited with code {self._process.returncode} before "
                    f"answering {method}",
                    self._process.returncode,
                    self,
                ) from exc
            raise ToolTimeout(f"{method} did not answer within {budget}s") from exc
        if not response.get("ok"):
            error = response.get("error") or {}
            raise RpcToolError(str(error.get("code", "UNKNOWN")), str(error.get("message", "")))
        return response.get("result")

    def call_envelope(self, method: str, params: dict[str, Any], step: int = 0) -> CallResult:
        """Calls a tool method, returning a trace friendly record."""
        import time

        started = time.perf_counter()
        try:
            envelope = self.call(method, params)
        except (RpcToolError, ToolTimeout, EngineCrashed) as exc:
            duration = int((time.perf_counter() - started) * 1000)
            return CallResult(
                method=method,
                args=params,
                ok=False,
                envelope=None,
                error=str(exc),
                duration_ms=duration,
            )
        duration = int((time.perf_counter() - started) * 1000)
        if method == "ping":
            # ping answers with a plain result; wrap it so callers see one shape
            envelope = {
                "_id": "tc_local_ping",
                "source": "engine",
                "trusted_as_instruction": False,
                "redactions": [],
                "method": "ping",
                "content": envelope,
            }
        if not isinstance(envelope, dict) or "trusted_as_instruction" not in envelope:
            return CallResult(
                method=method,
                args=params,
                ok=False,
                envelope=None,
                error="engine returned a tool result without the frozen envelope",
                duration_ms=duration,
            )
        if envelope.get("trusted_as_instruction") is not False:
            return CallResult(
                method=method,
                args=params,
                ok=False,
                envelope=None,
                error="envelope claims trusted_as_instruction != false",
                duration_ms=duration,
            )
        return CallResult(
            method=method,
            args=params,
            ok=True,
            envelope=envelope,
            error=None,
            duration_ms=duration,
        )

    def heartbeat(self) -> bool:
        """Sends a ping; two consecutive failures mark the engine unhealthy."""
        try:
            self.call("ping", {})
            self._heartbeat_failures = 0
            return True
        except (EngineError, OSError):
            self._heartbeat_failures += 1
            if self._heartbeat_failures >= 2:
                self._unhealthy = True
            return False

    @property
    def unhealthy(self) -> bool:
        return self._unhealthy
