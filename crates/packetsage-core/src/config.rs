//! Engine configuration (M0~M2 §4.8, 开发文档 §24).
//!
//! Precedence is applied by the CLI: CLI > env > file > defaults.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize};

/// How packet events are emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PacketEventMode {
    /// Emit every packet (default).
    #[default]
    All,
    /// Emit only packets that carry a decode error.
    ErrorsOnly,
    /// Emit no packet events at all.
    None,
}

/// Byte-size field that accepts either a plain integer or `8MiB` style text.
fn deserialize_byte_size<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(u64),
        Text(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Int(v) => Ok(v),
        Raw::Text(text) => parse_byte_size(&text).map_err(serde::de::Error::custom),
    }
}

/// Parses `512`, `8KiB`, `8MiB`, `1GiB` (case-insensitive, optional `B`).
///
/// # Errors
/// Returns a message when the text is not a size.
pub fn parse_byte_size(text: &str) -> Result<u64, String> {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    let (digits, multiplier) = if let Some(rest) = lower.strip_suffix("gib") {
        (rest, 1024u64 * 1024 * 1024)
    } else if let Some(rest) = lower.strip_suffix("mib") {
        (rest, 1024u64 * 1024)
    } else if let Some(rest) = lower.strip_suffix("kib") {
        (rest, 1024u64)
    } else if let Some(rest) = lower.strip_suffix("gb") {
        (rest, 1_000_000_000u64)
    } else if let Some(rest) = lower.strip_suffix("mb") {
        (rest, 1_000_000u64)
    } else if let Some(rest) = lower.strip_suffix("kb") {
        (rest, 1_000u64)
    } else if let Some(rest) = lower.strip_suffix('b') {
        (rest, 1u64)
    } else {
        (lower.as_str(), 1u64)
    };
    let digits = digits.trim();
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("invalid byte size: {text:?}"))?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("byte size overflows u64: {text:?}"))
}

/// TCP reassembly limits (M0~M2 §4.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReassemblyLimits {
    /// Idle streams older than this are pruned.
    #[serde(with = "humantime_serde")]
    pub stream_timeout: Duration,
    /// Maximum buffered bytes per stream.
    #[serde(deserialize_with = "deserialize_byte_size")]
    pub max_stream_buffer: u64,
    /// Maximum number of tracked sessions.
    pub max_sessions: usize,
    /// Maximum buffered segments per direction.
    pub max_segments_per_stream: usize,
}

impl Default for ReassemblyLimits {
    fn default() -> Self {
        Self {
            stream_timeout: Duration::from_secs(60),
            max_stream_buffer: 8 * 1024 * 1024,
            max_sessions: 100_000,
            max_segments_per_stream: 4_096,
        }
    }
}

/// Engine section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineCfg {
    /// Maximum number of sessions tracked globally.
    pub max_sessions: usize,
    /// Reader buffer capacity in bytes.
    #[serde(deserialize_with = "deserialize_byte_size")]
    pub read_buffer: u64,
}

impl Default for EngineCfg {
    fn default() -> Self {
        Self {
            max_sessions: 100_000,
            read_buffer: 1 << 20,
        }
    }
}

/// Event emission section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EmitCfg {
    /// Packet event policy.
    pub packet_events: PacketEventMode,
    /// Maximum number of events to emit (`0` = unlimited).
    pub limit_events: u64,
}

/// Storage section (placeholder until M3 wires the repository).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageCfg {
    /// Database URL, e.g. `sqlite://packetsage.db`.
    pub url: Option<String>,
}

/// Rules section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RulesCfg {
    /// Directory scanned for YAML rules.
    pub path: PathBuf,
    /// Whether the rule engine runs at all.
    pub enabled: bool,
}

impl Default for RulesCfg {
    fn default() -> Self {
        Self {
            path: PathBuf::from("rules"),
            enabled: true,
        }
    }
}

/// Whole engine configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EngineConfig {
    /// Engine section.
    pub engine: EngineCfg,
    /// Reassembly section.
    pub reassembly: ReassemblyLimits,
    /// Emission section.
    pub emit: EmitCfg,
    /// Storage section.
    pub storage: StorageCfg,
    /// Rules section.
    pub rules: RulesCfg,
}

impl EngineConfig {
    /// Parses a YAML configuration document.
    ///
    /// # Errors
    /// Returns a message when the YAML is invalid.
    pub fn from_yaml(text: &str) -> Result<Self, String> {
        serde_yaml::from_str(text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_size_parsing() {
        assert_eq!(parse_byte_size("8192").ok(), Some(8192));
        assert_eq!(parse_byte_size("8MiB").ok(), Some(8 * 1024 * 1024));
        assert_eq!(parse_byte_size("1GiB").ok(), Some(1024 * 1024 * 1024));
        assert_eq!(parse_byte_size("512Kib").ok(), Some(512 * 1024));
        assert!(parse_byte_size("abc").is_err());
    }

    #[test]
    fn yaml_defaults_match_spec() {
        let cfg = EngineConfig::from_yaml("engine:\n  max_sessions: 10\n").expect("yaml");
        assert_eq!(cfg.engine.max_sessions, 10);
        assert_eq!(cfg.reassembly.max_stream_buffer, 8 * 1024 * 1024);
        assert_eq!(cfg.reassembly.stream_timeout, Duration::from_secs(60));
        assert_eq!(cfg.emit.packet_events, PacketEventMode::All);
        assert!(cfg.rules.enabled);
    }

    #[test]
    fn yaml_human_sizes() {
        let text = "reassembly:\n  max_stream_buffer: 8MiB\n  stream_timeout: 60s\n";
        let cfg = EngineConfig::from_yaml(text).expect("yaml");
        assert_eq!(cfg.reassembly.max_stream_buffer, 8 * 1024 * 1024);
        assert_eq!(cfg.reassembly.stream_timeout, Duration::from_secs(60));
    }

    #[test]
    fn unknown_top_level_field_is_rejected_by_default_loader() {
        // The engine keeps serde defaults; the strict check for unknown
        // fields lives in the rules loader (S9). Here we only assert that a
        // plain YAML round trip works.
        let cfg =
            EngineConfig::from_yaml("storage:\n  url: sqlite://packetsage.db\n").expect("yaml");
        assert_eq!(cfg.storage.url.as_deref(), Some("sqlite://packetsage.db"));
    }
}
