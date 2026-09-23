//! Configuration discovery chain and effective settings (§7, dev doc §24).
//!
//! Two chains live here, and they are different things:
//!
//! * the **file discovery chain** answers "which file is in use":
//!   `--config` → `$PACKETSAGE_CONFIG` → `./packetsage.yaml` → built-in
//!   defaults. Layers 1 and 2 fail fast when the file is missing or invalid,
//!   and so does layer 3 when the file exists (§7.1);
//! * the **key precedence chain** answers "where does one value come from":
//!   CLI flag → environment → config file → defaults.

use std::path::{Path, PathBuf};

use packetsage_core::EngineConfig;

use crate::redact;

/// Where the engine configuration came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `--config <path>`.
    Flag,
    /// `$PACKETSAGE_CONFIG`.
    Environment,
    /// `./packetsage.yaml`.
    WorkingDirectory,
    /// No file: built-in defaults.
    Builtin,
}

impl Source {
    /// Human readable label used by doctor and `-vv`.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Source::Flag => "--config",
            Source::Environment => "$PACKETSAGE_CONFIG",
            Source::WorkingDirectory => "./packetsage.yaml",
            Source::Builtin => "built-in defaults",
        }
    }
}

/// A configuration failure: always exit 3 (§4).
#[derive(Debug, Clone)]
pub struct ConfigError {
    /// Message shown on stderr, with the absolute path and the reason.
    pub message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// A loaded configuration plus its provenance.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// Parsed engine configuration.
    pub config: EngineConfig,
    /// Which layer provided the file.
    pub source: Source,
    /// Absolute path, when a file was used.
    pub path: Option<PathBuf>,
}

/// Resolves the file discovery chain and parses the result strictly.
///
/// # Errors
/// Returns [`ConfigError`] when an explicit layer points at a missing or
/// unreadable file, when layer 3 exists but cannot be parsed, when an unknown
/// key is present, or when a secret-looking key is found (§7.1).
pub fn resolve(explicit: Option<&Path>) -> Result<Loaded, ConfigError> {
    if let Some(path) = explicit {
        return load_strict(path, Source::Flag);
    }
    if let Some(path) = std::env::var_os("PACKETSAGE_CONFIG") {
        let path = PathBuf::from(path);
        if path.as_os_str().is_empty() {
            return Err(ConfigError::new(
                "$PACKETSAGE_CONFIG is set but empty; unset it or point it at a configuration file",
            ));
        }
        return load_strict(&path, Source::Environment);
    }
    let cwd = PathBuf::from("packetsage.yaml");
    if cwd.exists() {
        return load_strict(&cwd, Source::WorkingDirectory);
    }
    Ok(Loaded {
        config: EngineConfig::default(),
        source: Source::Builtin,
        path: None,
    })
}

fn load_strict(path: &Path, source: Source) -> Result<Loaded, ConfigError> {
    let display = absolute(path);
    let text = std::fs::read_to_string(path).map_err(|error| {
        ConfigError::new(format!(
            "{}: cannot read the configuration file ({error})",
            display.display()
        ))
    })?;
    let config = parse_strict(&text, &display)?;
    Ok(Loaded {
        config,
        source,
        path: Some(display),
    })
}

/// Parses YAML with unknown keys and embedded secrets treated as errors.
///
/// # Errors
/// Returns [`ConfigError`] naming the offending field path.
pub fn parse_strict(text: &str, display: &Path) -> Result<EngineConfig, ConfigError> {
    let value: serde_yaml::Value = serde_yaml::from_str(text).map_err(|error| {
        ConfigError::new(format!(
            "{}: cannot parse the configuration ({error})",
            display.display()
        ))
    })?;
    validate_keys(&value, "")?;
    reject_secrets(&value, "")?;
    serde_yaml::from_value(value).map_err(|error| {
        ConfigError::new(format!(
            "{}: cannot parse the configuration ({error})",
            display.display()
        ))
    })
}

/// Field names accepted per section.
///
/// The table is shared with `agent/packetsage_agent/config.py`: one
/// `packetsage.yaml` serves both sides, so both sides must accept exactly the
/// same keys. Rust reads `engine`/`reassembly`/`emit`/`storage`/`rules`;
/// Python reads `agent`/`llm`; each side validates the whole table so a typo is
/// caught whichever entry point the user runs (T5).
const SCHEMA: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "engine",
            "reassembly",
            "emit",
            "storage",
            "rules",
            "agent",
            "llm",
        ],
    ),
    ("engine", &["max_sessions", "read_buffer"]),
    (
        "reassembly",
        &[
            "stream_timeout",
            "max_stream_buffer",
            "max_sessions",
            "max_segments_per_stream",
        ],
    ),
    ("emit", &["packet_events", "limit_events"]),
    ("storage", &["url"]),
    ("rules", &["path", "enabled"]),
    (
        "agent",
        &[
            "max_steps",
            "max_llm_calls",
            "max_tool_calls",
            "max_same_tool_calls",
            "max_tokens",
            "max_cost_cents",
            "scenario",
            "temperature",
        ],
    ),
    (
        "llm",
        &["provider", "model", "base_url", "temperature", "timeout_s"],
    ),
];

fn allowed(prefix: &str) -> &'static [&'static str] {
    SCHEMA
        .iter()
        .find(|(name, _)| *name == prefix)
        .map_or(&[], |(_, keys)| *keys)
}

fn validate_keys(value: &serde_yaml::Value, prefix: &str) -> Result<(), ConfigError> {
    let Some(mapping) = value.as_mapping() else {
        if prefix.is_empty() {
            return Err(ConfigError::new(
                "the configuration root must be a mapping of sections",
            ));
        }
        return Ok(());
    };
    for (key, child) in mapping {
        let Some(name) = key.as_str() else {
            return Err(ConfigError::new(format!(
                "{}: configuration keys must be strings",
                join(prefix, "<non-string>")
            )));
        };
        let path = join(prefix, name);
        let keys = allowed(prefix);
        if !keys.contains(&name) {
            let hint = if keys.is_empty() {
                "this section takes no nested keys".to_owned()
            } else {
                format!("known keys: {}", keys.join(", "))
            };
            return Err(ConfigError::new(format!(
                "{path}: unknown configuration key (typo?) — {hint}"
            )));
        }
        if child.is_mapping() {
            validate_keys(child, &path)?;
        }
    }
    Ok(())
}

fn reject_secrets(value: &serde_yaml::Value, prefix: &str) -> Result<(), ConfigError> {
    let Some(mapping) = value.as_mapping() else {
        return Ok(());
    };
    for (key, child) in mapping {
        let Some(name) = key.as_str() else {
            continue;
        };
        let path = join(prefix, name);
        if redact::is_secret_key(name) {
            return Err(ConfigError::new(format!(
                "{path}: credentials must not live in the configuration file — \
                 move it to $PACKETSAGE_LLM_API_KEY (or agent/.env)"
            )));
        }
        reject_secrets(child, &path)?;
    }
    Ok(())
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}.{name}")
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| {
        let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        base.join(path)
    })
}

/// Effective values after the key precedence chain (CLI → env → file → default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Database URL.
    pub db_url: String,
    /// Rules directory.
    pub rules_dir: PathBuf,
    /// Agent provider.
    pub provider: String,
    /// Agent model.
    pub model: Option<String>,
}

/// Default database used when nothing else is configured.
pub const DEFAULT_DB_URL: &str = "sqlite://packetsage.db";

/// The provider stays **unset** until the user configures one: the CLI must not
/// silently replay the deterministic `mock` script and pass it off as analysis.
/// `mock` remains available, but only when it is named explicitly (CI, demos).
pub const PROVIDER_UNSET: &str = "";

impl Settings {
    /// Applies the key precedence chain to the given CLI overrides.
    #[must_use]
    pub fn resolve(
        cli_db: Option<&str>,
        cli_rules: Option<&Path>,
        cli_provider: Option<&str>,
        cli_model: Option<&str>,
        config: &EngineConfig,
    ) -> Self {
        Self {
            db_url: cli_db
                .map(str::to_owned)
                .or_else(|| env_first(&["PACKETSAGE_STORAGE_URL", "PACKETSAGE_DB"]))
                .or_else(|| config.storage.url.clone())
                .unwrap_or_else(|| DEFAULT_DB_URL.to_owned()),
            rules_dir: cli_rules
                .map(Path::to_path_buf)
                .or_else(|| {
                    env_first(&["PACKETSAGE_RULES_PATH", "PACKETSAGE_RULES"]).map(PathBuf::from)
                })
                .unwrap_or_else(|| config.rules.path.clone()),
            provider: cli_provider
                .map(str::to_owned)
                .or_else(|| env_first(&["PACKETSAGE_LLM_PROVIDER", "PACKETSAGE_PROVIDER"]))
                .unwrap_or_else(|| PROVIDER_UNSET.to_owned()),
            model: cli_model
                .map(str::to_owned)
                .or_else(|| env_first(&["PACKETSAGE_LLM_MODEL"])),
        }
    }

    /// Redacted dump used by `-vv` and by doctor's effective-configuration line.
    #[must_use]
    pub fn dump(&self, loaded: &Loaded) -> String {
        let mut lines = vec![
            format!(
                "config source: {}",
                match &loaded.path {
                    Some(path) => format!("{} ({})", loaded.source.label(), path.display()),
                    None => loaded.source.label().to_owned(),
                }
            ),
            format!("storage.url: {}", self.db_url),
            format!("rules.path: {}", self.rules_dir.display()),
            format!("rules.enabled: {}", loaded.config.rules.enabled),
            format!("emit.packet_events: {:?}", loaded.config.emit.packet_events),
            format!("engine.max_sessions: {}", loaded.config.engine.max_sessions),
            format!(
                "reassembly.max_stream_buffer: {}",
                loaded.config.reassembly.max_stream_buffer
            ),
            format!("llm.provider: {}", self.provider_display()),
            format!(
                "llm.model: {}",
                self.model.clone().unwrap_or_else(|| "-".to_owned())
            ),
        ];
        for name in [
            "PACKETSAGE_STORAGE_URL",
            "PACKETSAGE_DB",
            "PACKETSAGE_RULES_PATH",
            "PACKETSAGE_RULES",
            "PACKETSAGE_LLM_PROVIDER",
            "PACKETSAGE_LLM_MODEL",
            "PACKETSAGE_LLM_API_KEY",
            "OPENAI_API_KEY",
        ] {
            let value = match std::env::var(name) {
                Ok(value) if !value.is_empty() => redact::redact_value(name, &value),
                _ => "-".to_owned(),
            };
            lines.push(format!("env.{name}: {value}"));
        }
        lines.join("\n")
    }

    /// `(unset)` reads better than an empty value in dumps and doctor rows.
    #[must_use]
    pub fn provider_display(&self) -> String {
        if self.provider.is_empty() {
            "(unset)".to_owned()
        } else {
            self.provider.clone()
        }
    }
}

/// First environment variable that is set and non-empty.
#[must_use]
pub fn env_first(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    })
}

/// True when an API key is available for the OpenAI-compatible providers.
#[must_use]
pub fn api_key_present() -> bool {
    env_first(&["PACKETSAGE_LLM_API_KEY", "OPENAI_API_KEY"]).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> PathBuf {
        PathBuf::from("/tmp/packetsage.yaml")
    }

    #[test]
    fn unknown_key_is_rejected_with_its_path() {
        let error = parse_strict("engine:\n  max_stesp: 3\n", &display()).expect_err("must fail");
        assert!(
            error.message.contains("engine.max_stesp"),
            "{}",
            error.message
        );
    }

    #[test]
    fn known_keys_parse() {
        let config = parse_strict(
            "engine:\n  max_sessions: 10\nrules:\n  path: rules\n",
            &display(),
        )
        .expect("valid");
        assert_eq!(config.engine.max_sessions, 10);
    }

    #[test]
    fn nested_unknown_section_is_rejected() {
        let error = parse_strict("emit:\n  packet_event: all\n", &display()).expect_err("fail");
        assert!(
            error.message.contains("emit.packet_event"),
            "{}",
            error.message
        );
    }

    #[test]
    fn credentials_in_the_file_are_refused() {
        let error = parse_strict("storage:\n  api_key: abc\n", &display()).expect_err("fail");
        assert!(error.message.contains("api_key"), "{}", error.message);
    }

    #[test]
    fn settings_fall_back_to_defaults() {
        let config = EngineConfig::default();
        let settings = Settings::resolve(None, None, None, None, &config);
        assert_eq!(settings.db_url, DEFAULT_DB_URL);
        assert_eq!(settings.rules_dir, PathBuf::from("rules"));
        // Unconfigured is *unset*, never a silent `mock` (the agent refuses to
        // run until `packetsage-agent setup` has run).
        assert_eq!(settings.provider, PROVIDER_UNSET);
    }

    #[test]
    fn cli_wins_over_the_file() {
        let mut config = EngineConfig::default();
        config.storage.url = Some("sqlite://from-file.db".to_owned());
        let settings = Settings::resolve(Some("sqlite://from-cli.db"), None, None, None, &config);
        assert_eq!(settings.db_url, "sqlite://from-cli.db");
    }
}
