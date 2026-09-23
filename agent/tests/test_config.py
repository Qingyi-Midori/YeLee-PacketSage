"""Configuration discovery chain, strictness and redaction (§7, T1/T5)."""

from __future__ import annotations

import os
from pathlib import Path

import pytest
from packetsage_agent import config


@pytest.fixture(autouse=True)
def _clean_env(monkeypatch):
    """Every test starts without the PACKETSAGE_* / OPENAI_* variables."""
    for name in (
        "PACKETSAGE_CONFIG",
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
    ):
        monkeypatch.delenv(name, raising=False)
    return monkeypatch


def test_explicit_file_wins_over_the_environment(_clean_env, tmp_path, monkeypatch):
    explicit = tmp_path / "explicit.yaml"
    explicit.write_text("llm:\n  provider: local\n", encoding="utf-8")
    env_file = tmp_path / "env.yaml"
    env_file.write_text("llm:\n  provider: openai\n", encoding="utf-8")
    monkeypatch.setenv("PACKETSAGE_CONFIG", str(env_file))

    loaded = config.load(explicit)
    assert loaded.source == "--config"
    assert loaded.data["llm"]["provider"] == "local"


def test_environment_layer_wins_over_the_file(_clean_env, tmp_path, monkeypatch):
    path = tmp_path / "packetsage.yaml"
    path.write_text("llm:\n  provider: openai\n  model: from-file\n", encoding="utf-8")
    monkeypatch.setenv("PACKETSAGE_LLM_PROVIDER", "local")

    loaded = config.load(path)
    effective = config.settings(loaded)
    assert effective.provider == "local"
    # …while an untouched key still comes from the file.
    assert effective.model == "from-file"


def test_cli_flag_wins_over_the_environment(_clean_env, tmp_path, monkeypatch):
    path = tmp_path / "packetsage.yaml"
    path.write_text("llm:\n  provider: local\n", encoding="utf-8")
    monkeypatch.setenv("PACKETSAGE_LLM_PROVIDER", "openai")

    effective = config.settings(config.load(path), provider="mock")
    assert effective.provider == "mock"


def test_missing_explicit_file_is_an_error(_clean_env, tmp_path):
    with pytest.raises(config.ConfigError):
        config.load(tmp_path / "absent.yaml")


def test_missing_working_directory_file_falls_back_to_defaults(_clean_env, tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    loaded = config.load()
    assert loaded.source == "built-in defaults"
    assert loaded.data == {}


def test_unknown_key_is_rejected_with_its_path(_clean_env, tmp_path):
    path = tmp_path / "packetsage.yaml"
    text = "agent:\n  max_stesp: 3\n"
    with pytest.raises(config.ConfigError) as error:
        config.parse_config(text, path)
    assert "agent.max_stesp" in str(error.value)


def test_rust_sections_are_accepted_by_the_python_side(_clean_env, tmp_path):
    path = tmp_path / "packetsage.yaml"
    data = config.parse_config(
        "storage:\n  url: sqlite://packetsage.db\nagent:\n  max_steps: 4\n", path
    )
    assert data["agent"]["max_steps"] == 4


def test_credentials_in_the_file_are_refused(_clean_env, tmp_path):
    path = tmp_path / "packetsage.yaml"
    with pytest.raises(config.ConfigError) as error:
        config.parse_config("llm:\n  api_key: sk-123\n", path)
    assert "api_key" in str(error.value)


# --------------------------------------------------------------------------
# Redaction: T1 positive/negative cases (§7.2)
# --------------------------------------------------------------------------
@pytest.mark.parametrize(
    "key",
    [
        "api_key",
        "llm.api_key",
        "llm_api_key",
        "secret",
        "client_secret",
        "token",
        "tokens",
        "password",
        "db.password",
        "OPENAI_API_KEY",
    ],
)
def test_credential_names_are_masked(key):
    assert config.is_secret_key(key)
    assert config.redact_value(key, "sk-abcdef") == config.MASK


@pytest.mark.parametrize(
    "key",
    [
        # The substring matcher used to mask these legitimate keys (§14.1 T1).
        "max_tokens_total",
        "token_budget_max",
        "tokens_per_second",
        "api_version",
        "key_rotation_seconds",
        "max_sessions",
    ],
)
def test_legitimate_names_survive(key):
    assert not config.is_secret_key(key)
    assert config.redact_value(key, 42) == 42


def test_the_mask_never_reveals_the_length():
    masked = config.redact_value("llm.api_key", "sk-abcdefghijklmnop")
    assert masked == config.MASK
    assert str(len("sk-abcdefghijklmnop")) not in masked


def test_nested_dumps_are_redacted(_clean_env, tmp_path, monkeypatch):
    path = tmp_path / "packetsage.yaml"
    path.write_text("llm:\n  provider: openai\n", encoding="utf-8")
    monkeypatch.setenv("PACKETSAGE_LLM_API_KEY", "sk-super-secret")

    dumped = config.dump(config.load(path), config.settings(config.load(path)))
    assert "sk-super-secret" not in dumped
    assert "PACKETSAGE_LLM_API_KEY: ***" in dumped


def test_agent_budget_comes_from_the_file(_clean_env, tmp_path):
    path = tmp_path / "packetsage.yaml"
    path.write_text("agent:\n  max_steps: 5\n  max_tool_calls: 7\n", encoding="utf-8")
    effective = config.settings(config.load(path))
    assert (effective.max_steps, effective.max_tool_calls) == (5, 7)
    # Untouched keys keep the documented defaults.
    assert effective.max_tokens == 200_000


def test_dotenv_is_loaded_without_overriding_the_environment(_clean_env, tmp_path, monkeypatch):
    env_file = tmp_path / ".env"
    env_file.write_text('PACKETSAGE_LLM_MODEL="from-dotenv"\n', encoding="utf-8")
    monkeypatch.setenv("PACKETSAGE_LLM_MODEL", "from-shell")

    config.load_dotenv(env_file)
    assert os.environ["PACKETSAGE_LLM_MODEL"] == "from-shell"

    env_file.write_text("PACKETSAGE_LLM_BASE_URL=http://localhost:8000/v1\n", encoding="utf-8")
    config.load_dotenv(env_file)
    assert os.environ["PACKETSAGE_LLM_BASE_URL"] == "http://localhost:8000/v1"


def test_openai_without_a_key_is_unavailable(_clean_env, tmp_path):
    path = tmp_path / "packetsage.yaml"
    path.write_text("llm:\n  provider: openai\n", encoding="utf-8")
    effective = config.settings(config.load(path))
    assert config.provider_unavailable(effective) is not None


def test_the_help_text_is_identical_for_both_entry_points():
    """T8: the console script and `python -m` share one parser and one prog name."""
    from packetsage_agent import cli as cli_module

    parser = cli_module.build_parser()
    assert parser.prog == "packetsage-agent"
    assert Path(cli_module.__file__).name == "cli.py"
