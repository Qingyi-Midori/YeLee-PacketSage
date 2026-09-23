"""Configuration discovery chain and redaction (CLI 收口工程规格书 §7).

Python owns the ``agent`` and ``llm`` sections of the same file the Rust
engine reads, so this module mirrors three things exactly: the file discovery
chain (``--config`` then ``$PACKETSAGE_CONFIG`` then ``./packetsage.yaml`` then
built-in defaults), the key precedence chain (CLI then environment then file
then defaults), and the redaction rule — a credential is recognised by *name
segments*, never by substring, so ``max_tokens_total`` stays readable while
``llm.api_key`` and ``db_password`` are masked (§7.2, T1 negative case).

``.env`` is loaded here and only here (§7.1): the Rust side never reads it, so
the two runtimes cannot drift.

Agent CLI 工程规格书 §9 narrows *what this process validates*: the ``engine``,
``reassembly``, ``emit``, ``storage`` and ``rules`` sections belong to the Rust
engine and are legal here but ignored — unknown keys are fail-fast only inside
``agent`` and ``llm``, so a legal Rust-side key can never be mistaken for a
Python-side typo.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .prompts import DEFAULT_PROMPT_VERSION, available_versions

#: Marker used for every masked value; the length is never revealed.
MASK = "***"

#: Where ``setup`` writes credentials (and ``load_dotenv`` reads them from):
#: ``agent/.env`` next to the package's parent directory. Never a YAML file —
#: §7.1 forbids credentials in the configuration file, and this path is
#: gitignored.
DEFAULT_ENV_FILE = Path(__file__).resolve().parent.parent / ".env"

#: An unset ``llm.provider`` is not "mock": the agent refuses to run until the
#: user has chosen a provider (see :func:`provider_unavailable`).
PROVIDER_UNSET = ""

#: Sections this process consumes and therefore validates strictly (§9).
AGENT_KEYS: tuple[str, ...] = (
    "max_steps",
    "max_llm_calls",
    "max_tool_calls",
    "max_same_tool_calls",
    "max_tokens",
    "max_tokens_total",
    "max_payload_bytes",
    "max_cost_cents",
    "scenario",
    "temperature",
    "prompt_version",
)
LLM_KEYS: tuple[str, ...] = ("provider", "model", "base_url", "temperature", "timeout_s")

#: Sections owned by the Rust engine: legal in the same file, ignored here.
IGNORED_SECTIONS: tuple[str, ...] = (
    "engine",
    "reassembly",
    "emit",
    "storage",
    "rules",
)

#: Kept for backwards compatibility with callers that inspected the old table.
SCHEMA: dict[str, tuple[str, ...]] = {
    "agent": AGENT_KEYS,
    "llm": LLM_KEYS,
}

#: The agent-side key that carries the run token budget.  #42: the development
#: document calls it ``agent.max_`` / ``max_tokens_total``; both spellings are
#: accepted (the long one wins), never both at once.
TOKEN_BUDGET_KEYS: tuple[str, ...] = ("max_tokens_total", "max_tokens")

#: Environment variables of the key precedence chain (§7.1). Only the keys this
#: process consumes are listed: the storage/rules names belong to the Rust
#: engine, which reads them from the same inherited environment (§9).
ENV_KEYS: dict[str, tuple[str, ...]] = {
    "provider": ("PACKETSAGE_LLM_PROVIDER", "PACKETSAGE_PROVIDER"),
    "model": ("PACKETSAGE_LLM_MODEL",),
    # The provider-specific names come last so the announced variable wins.
    "api_key": ("PACKETSAGE_LLM_API_KEY", "OPENAI_API_KEY", "DEEPSEEK_API_KEY"),
    "base_url": ("PACKETSAGE_LLM_BASE_URL", "OPENAI_BASE_URL"),
}


class ConfigError(RuntimeError):
    """A configuration failure: the CLI maps it to exit 3."""


@dataclass(frozen=True)
class Loaded:
    """Parsed configuration plus its provenance."""

    data: dict[str, Any]
    source: str
    path: Path | None


@dataclass(frozen=True)
class Settings:
    """Effective agent settings after the key precedence chain."""

    provider: str
    model: str | None
    api_key: str | None
    base_url: str | None
    engine_path: str | None
    max_steps: int
    max_llm_calls: int
    max_tool_calls: int
    max_same_tool_calls: int
    max_tokens: int
    max_cost_cents: int
    scenario: str
    max_payload_bytes: int = 65_536
    prompt_version: str | None = None
    temperature: float | None = None


def _segments(name: str) -> list[str]:
    """Splits a key name on ``.``, ``_`` and ``-`` and lowercases it."""
    parts = name.replace("-", "_").replace(".", "_").split("_")
    return [part.lower() for part in parts if part]


def is_secret_key(name: str) -> bool:
    """True when a key *name* denotes a credential.

    Segment-wise matching only; substring matching is explicitly forbidden
    because it would mask legitimate keys such as ``max_tokens_total``.
    """
    segments = _segments(name)
    if not segments:
        return False
    if segments == ["api", "key"]:
        return True
    if len(segments) == 1 and segments[0] in {"secret", "token", "tokens", "password"}:
        return True
    if len(segments) >= 2:
        tail = segments[-2:]
        if tail == ["api", "key"]:
            return True
        if tail[1] in {"secret", "password"}:
            return True
    return False


def redact_value(key: str, value: Any) -> Any:
    """Masks the value of a credential-looking key."""
    return MASK if is_secret_key(key) else value


def redact_mapping(mapping: dict[str, Any], prefix: str = "") -> dict[str, Any]:
    """Recursively masks credentials in a nested mapping."""
    out: dict[str, Any] = {}
    for key, value in mapping.items():
        path = f"{prefix}{key}"
        if isinstance(value, dict):
            out[key] = redact_mapping(value, f"{path}.")
        else:
            out[key] = redact_value(path, value)
    return out


def redact_url(url: str) -> str:
    """Masks the password of a database URL (收口 v0.2 §7.2, §3).

    ``postgres://user:secret@host/db`` → ``postgres://user:***@host/db``. A URL
    without credentials (the common ``sqlite://packetsage.db``) is returned
    unchanged, so error messages stay useful.
    """
    if "://" not in url:
        return url
    scheme, rest = url.split("://", 1)
    if "@" not in rest:
        return url
    credentials, host = rest.rsplit("@", 1)
    if ":" not in credentials:
        return url
    user, _password = credentials.split(":", 1)
    return f"{scheme}://{user}:{MASK}@{host}"


def find_config_file(explicit: str | os.PathLike[str] | None = None) -> tuple[Path | None, str]:
    """Returns ``(path, source-label)`` following the discovery chain.

    Layers 1 and 2 are explicit: a missing file is an error, never a silent
    fallback. Layer 3 is only used when the file exists, and then it must also
    parse (§7.1).
    """
    if explicit is not None:
        return Path(explicit), "--config"
    from_env = os.environ.get("PACKETSAGE_CONFIG")
    if from_env:
        return Path(from_env), "$PACKETSAGE_CONFIG"
    cwd = Path("packetsage.yaml")
    if cwd.exists():
        return cwd, "./packetsage.yaml"
    return None, "built-in defaults"


def load_dotenv(path: Path | None = None) -> dict[str, str]:
    """Loads ``agent/.env`` values that are not already in the environment.

    Stdlib only: a ``KEY=VALUE`` reader with comments and optional quotes. The
    environment always wins, so a shell export cannot be shadowed by the file.

    `$PACKETSAGE_ENV_FILE` moves the file (tests use it to run against a clean
    environment); `agent/.env` stays the default `setup` writes to.
    """
    override = os.environ.get("PACKETSAGE_ENV_FILE")
    candidate = Path(path) if path is not None else Path(override or DEFAULT_ENV_FILE)
    if not candidate.is_file():
        return {}
    try:
        text = candidate.read_text(encoding="utf-8")
    except OSError:
        return {}
    loaded: dict[str, str] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip().strip('"').strip("'")
        if not key:
            continue
        loaded[key] = value
        # An *empty* variable counts as unset: launchers happily export
        # `PACKETSAGE_LLM_PROVIDER=` with nothing in it, and that must not
        # shadow the file the user just wrote with `setup`.
        if not os.environ.get(key):
            os.environ[key] = value
    return loaded


def parse_config(text: str, display: Path | str) -> dict[str, Any]:
    """Strict parse: unknown keys and embedded credentials are errors.

    PyYAML is used when available; a minimal two-level reader covers the
    bare-interpreter case that CI runs the mock loop in.
    """
    data: Any
    try:
        import yaml  # type: ignore
    except ImportError:
        data = _parse_simple_yaml(text, display)
    else:
        try:
            data = yaml.safe_load(text)
        except Exception as error:
            raise ConfigError(f"{display}: cannot parse the configuration ({error})") from error
    if data is None:
        return {}
    if not isinstance(data, dict):
        raise ConfigError(f"{display}: the configuration root must be a mapping of sections")
    _validate_sections(data, display)
    _reject_secrets(data, "", display)
    return data


def _parse_simple_yaml(text: str, display: Path | str) -> dict[str, Any]:
    """Two-level ``section:`` / ``  key: value`` reader used without PyYAML."""
    data: dict[str, Any] = {}
    section: str | None = None
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].rstrip()
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip())
        if ":" not in line:
            raise ConfigError(f"{display}: cannot parse line {raw!r}")
        key, _, value = line.strip().partition(":")
        value = value.strip()
        if indent == 0:
            if not value:
                section = key
                data.setdefault(key, {})
            else:
                section = None
                data[key] = _coerce(value)
        else:
            if section is None:
                raise ConfigError(f"{display}: nested key {key!r} has no parent section")
            target = data.setdefault(section, {})
            if not isinstance(target, dict):
                raise ConfigError(f"{display}: {section} is not a section")
            target[key] = _coerce(value)
    return data


def _coerce(value: str) -> Any:
    lowered = value.lower()
    if lowered in {"true", "false"}:
        return lowered == "true"
    if lowered in {"null", "none", "~", ""}:
        return None
    try:
        return int(value)
    except ValueError:
        pass
    try:
        return float(value)
    except ValueError:
        return value


def _validate_sections(data: dict[str, Any], display: Path | str) -> None:
    """Fail fast on unknown keys *inside* ``agent``/``llm`` only (§9).

    ``engine``/``reassembly``/``emit``/``storage``/``rules`` are consumed by the
    spawned Rust engine through the same file and are deliberately left alone:
    the Python side must not mistake a legal engine key for a typo.
    """
    for section, allowed in (("agent", AGENT_KEYS), ("llm", LLM_KEYS)):
        value = data.get(section)
        if value is None:
            continue
        if not isinstance(value, dict):
            raise ConfigError(f"{display}: {section}: must be a mapping of keys")
        for key, child in value.items():
            if not isinstance(key, str):
                raise ConfigError(f"{display}: {section}: configuration keys must be strings")
            if key not in allowed:
                raise ConfigError(
                    f"{display}: {section}.{key}: unknown configuration key (typo?) — "
                    f"known keys: {', '.join(allowed)}"
                )
            if isinstance(child, dict):
                raise ConfigError(
                    f"{display}: {section}.{key}: nested sections are not supported in "
                    f"{section}"
                )
    agent = data.get("agent")
    if isinstance(agent, dict):
        present = [key for key in TOKEN_BUDGET_KEYS if key in agent]
        if len(present) > 1:
            raise ConfigError(
                f"{display}: agent.{present[0]} and agent.{present[1]} are the same "
                "budget under two names — keep only one"
            )
        version = agent.get("prompt_version")
        if version is not None and version not in available_versions():
            raise ConfigError(
                f"{display}: agent.prompt_version: unknown version {version!r} — "
                f"available: {', '.join(available_versions())}"
            )


def _reject_secrets(value: Any, prefix: str, display: Path | str) -> None:
    if not isinstance(value, dict):
        return
    for key, child in value.items():
        path = f"{prefix}.{key}" if prefix else str(key)
        if is_secret_key(str(key)):
            raise ConfigError(
                f"{display}: {path}: credentials must not live in the configuration file — "
                "move it to $PACKETSAGE_LLM_API_KEY (or agent/.env)"
            )
        _reject_secrets(child, path, display)


def load(explicit: str | os.PathLike[str] | None = None) -> Loaded:
    """Resolves the discovery chain and returns the parsed configuration."""
    path, source = find_config_file(explicit)
    if path is None:
        return Loaded(data={}, source=source, path=None)
    display = path if path.is_absolute() else Path.cwd() / path
    if not path.is_file():
        raise ConfigError(f"{display}: cannot read the configuration file (not found)")
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise ConfigError(f"{display}: cannot read the configuration file ({error})") from error
    return Loaded(data=parse_config(text, display), source=source, path=display)


def _env_first(names: tuple[str, ...]) -> str | None:
    for name in names:
        value = os.environ.get(name)
        if value and value.strip():
            return value
    return None


def _section(loaded: Loaded, name: str) -> dict[str, Any]:
    section = loaded.data.get(name)
    return section if isinstance(section, dict) else {}


def settings(
    loaded: Loaded,
    *,
    provider: str | None = None,
    model: str | None = None,
    scenario: str | None = None,
    max_steps: int | None = None,
    max_llm_calls: int | None = None,
    max_tool_calls: int | None = None,
) -> Settings:
    """Applies ``CLI → environment → file → defaults`` to every agent setting."""
    llm = _section(loaded, "llm")
    agent = _section(loaded, "agent")
    return Settings(
        provider=provider
        or _env_first(ENV_KEYS["provider"])
        or _as_str(llm.get("provider"))
        or PROVIDER_UNSET,
        model=model or _env_first(ENV_KEYS["model"]) or _as_str(llm.get("model")),
        api_key=_env_first(ENV_KEYS["api_key"]),
        base_url=_env_first(ENV_KEYS["base_url"]) or _as_str(llm.get("base_url")),
        engine_path=os.environ.get("PACKETSAGE_ENGINE"),
        max_steps=max_steps or _as_int(agent.get("max_steps")) or 12,
        max_llm_calls=max_llm_calls or _as_int(agent.get("max_llm_calls")) or 24,
        max_tool_calls=max_tool_calls or _as_int(agent.get("max_tool_calls")) or 20,
        max_same_tool_calls=_as_int(agent.get("max_same_tool_calls")) or 5,
        max_tokens=_token_budget(agent),
        max_cost_cents=_as_int(agent.get("max_cost_cents")) or 500,
        scenario=scenario or _as_str(agent.get("scenario")) or "auto",
        max_payload_bytes=_as_int(agent.get("max_payload_bytes")) or 65_536,
        # Effective value, not "unset": the shipped default is v2 (S5).
        prompt_version=_as_str(agent.get("prompt_version")) or DEFAULT_PROMPT_VERSION,
        temperature=_as_float(agent.get("temperature")),
    )


def _token_budget(agent: dict[str, Any]) -> int:
    """The run token budget (#42: ``max_tokens_total`` wins over ``max_tokens``)."""
    for key in TOKEN_BUDGET_KEYS:
        value = _as_int(agent.get(key))
        if value is not None:
            return value
    return 200_000


def _as_str(value: Any) -> str | None:
    return value if isinstance(value, str) and value else None


def _as_int(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        try:
            return int(value)
        except ValueError:
            return None
    return None


def _as_float(value: Any) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        try:
            return float(value)
        except ValueError:
            return None
    return None


def dump(loaded: Loaded, effective: Settings) -> str:
    """Redacted dump of the effective configuration (§7.2)."""
    lines = [
        f"config source: {loaded.source}" + (f" ({loaded.path})" if loaded.path else ""),
        f"llm.provider: {effective.provider or '(unset — run packetsage-agent setup)'}",
        f"llm.model: {effective.model or '-'}",
        f"llm.base_url: {effective.base_url or '-'}",
        f"agent.max_steps: {effective.max_steps}",
        f"agent.max_llm_calls: {effective.max_llm_calls}",
        f"agent.max_tool_calls: {effective.max_tool_calls}",
        f"agent.max_tokens: {effective.max_tokens}",
        f"agent.max_cost_cents: {effective.max_cost_cents}",
        f"agent.max_payload_bytes: {effective.max_payload_bytes}",
        f"agent.scenario: {effective.scenario}",
        f"agent.temperature: {effective.temperature if effective.temperature is not None else '-'}",
        f"agent.prompt_version: {effective.prompt_version or '-'}",
    ]
    for names in ENV_KEYS.values():
        for name in names:
            present = os.environ.get(name)
            if present is None:
                continue
            shown = MASK if is_secret_key(name) else present
            lines.append(f"env.{name}: {shown}")
    return "\n".join(lines)


def provider_unavailable(effective: Settings) -> str | None:
    """Why the configured provider cannot start, or ``None`` when it can.

    An **unset** provider is a blocker, not a licence to fake an analysis: the
    first thing a new user does is configure one (`packetsage-agent setup`).
    ``mock`` stays available, but only when it is named explicitly.
    """
    provider = (effective.provider or PROVIDER_UNSET).strip()
    if not provider:
        return (
            "还没有配置 LLM provider —— 先运行 `packetsage-agent setup`"
            "（或设 $PACKETSAGE_LLM_PROVIDER + $PACKETSAGE_LLM_API_KEY）"
        )
    if provider == "mock":
        # Explicitly requested: deterministic script replay for CI/demos.
        return None
    if provider not in ("openai", "deepseek", "local"):
        return (
            f"unknown provider kind {provider!r} — expected mock, openai, deepseek or "
            "local (set $PACKETSAGE_LLM_PROVIDER or llm.provider)"
        )
    if provider in ("openai", "deepseek") and not effective.api_key:
        env_name = (
            "DEEPSEEK_API_KEY" if provider == "deepseek" else "PACKETSAGE_LLM_API_KEY"
        )
        return (
            f"provider {provider} 没有 API key：运行 `packetsage-agent setup`，"
            f"或设 $PACKETSAGE_LLM_API_KEY（{env_name} 也可，放 agent/.env 亦可）"
        )
    return None
