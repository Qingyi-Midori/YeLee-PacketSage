//! provider 配置的非密钥部分（U7 / §7.4）。
//!
//! 分工是刻意的：
//!
//! * **provider / model / base_url** → `%LOCALAPPDATA%\PacketSage\provider.json`
//!   （可读、可诊断，删掉就等于"没配过"）；
//! * **api key** → Windows 凭据管理器（[`crate::secrets`]），**不落明文**；
//! * 启动 sidecar 时两者合并成 `PACKETSAGE_LLM_*` 环境变量注入（§7.4 第 3 步）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 与 agent 侧 `provider.PROVIDER_KINDS` 对齐（`mock` 不需要 key）。
pub const KINDS: [&str; 4] = ["mock", "openai", "deepseek", "local"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    /// 思考模式（DeepSeek `{"thinking": {"type": …}}`）：`""` = 自动（跟随厂商默认）。
    /// 空串必须在旧 `provider.json` 上也能反序列化，所以给了 `serde(default)`。
    #[serde(default)]
    pub thinking: String,
    /// 推理强度（DeepSeek `reasoning_effort`）：`""` = 自动（不显式传，跟随厂商默认）。
    /// 思考模式只有开关，强度才有档位——界面只暴露这一项（2026-09-23）。
    #[serde(default)]
    pub effort: String,
}

impl ProviderConfig {
    /// 文档里的默认端点与模型（与 agent 侧 `PROVIDER_DEFAULTS` 一致）。
    pub fn defaults(provider: &str) -> Self {
        let (model, base_url) = match provider {
            "openai" => ("gpt-4o-mini", "https://api.openai.com/v1"),
            // 2026-09-22：`deepseek-chat` / `deepseek-reasoner` 已下线，
            // 现役是 `deepseek-flash` 与 `deepseek-v4-pro`（都支持思考模式）。
            "deepseek" => ("deepseek-flash", "https://api.deepseek.com/v1"),
            "local" => ("local-model", "http://localhost:8000/v1"),
            _ => ("", ""),
        };
        Self {
            provider: provider.to_owned(),
            model: model.to_owned(),
            base_url: base_url.to_owned(),
            thinking: String::new(),
            effort: String::new(),
        }
    }

    /// 去掉空白；空掉的字段回落到该 provider 的默认值。
    pub fn normalised(self) -> Self {
        let provider = self.provider.trim().to_lowercase();
        let defaults = Self::defaults(&provider);
        let model = trimmed(&self.model);
        let base_url = trimmed(&self.base_url);
        Self {
            provider,
            model: if model.is_empty() { defaults.model } else { model },
            base_url: if base_url.is_empty() {
                defaults.base_url
            } else {
                base_url
            },
            thinking: normalise_thinking(&self.thinking),
            effort: normalise_effort(&self.effort),
        }
    }

    /// 思考模式是否要显式注入（`auto` 与空串都不注入）。
    pub fn thinking_env(&self) -> Option<&'static str> {
        match self.thinking.as_str() {
            "enabled" => Some("enabled"),
            "disabled" => Some("disabled"),
            _ => None,
        }
    }

    /// 推理强度是否要显式注入（`auto` 与空串都不注入）。
    pub fn effort_env(&self) -> Option<&'static str> {
        match self.effort.as_str() {
            "off" => Some("off"),
            "low" => Some("low"),
            "high" => Some("high"),
            "max" => Some("max"),
            _ => None,
        }
    }

    /// `openai` / `deepseek` 要 key；`mock` / `local` 不要（与 agent 侧一致）。
    pub fn needs_key(&self) -> bool {
        matches!(self.provider.as_str(), "openai" | "deepseek")
    }

    pub fn is_known_kind(&self) -> bool {
        KINDS.contains(&self.provider.as_str())
    }
}

fn trimmed(value: &str) -> String {
    value.trim().to_owned()
}

/// 只认 `auto` / `enabled` / `disabled`（其余一律当自动）。
fn normalise_thinking(value: &str) -> String {
    match value.trim().to_lowercase().as_str() {
        "enabled" | "on" | "1" | "true" => "enabled".to_owned(),
        "disabled" | "off" | "0" | "false" => "disabled".to_owned(),
        _ => String::new(),
    }
}

/// 只认 `auto` / `low` / `high` / `max`（其余一律当自动）。
///
/// 厂商的别名按 DeepSeek 文档的映射表收敛：`minimal` → low、`medium` → high、
/// `xhigh` / `ultra` → max；模型不认的档位由 provider 侧再兜一次。
fn normalise_effort(value: &str) -> String {
    match value.trim().to_lowercase().as_str() {
        "off" | "disabled" | "none" => "off".to_owned(),
        "low" | "minimal" => "low".to_owned(),
        "high" | "medium" => "high".to_owned(),
        "max" | "xhigh" | "ultra" => "max".to_owned(),
        // 空串 = 没选过（旧 provider.json / 向导没提这一项）：界面按"高"显示，
        // 侧车那边就是厂商默认，两者一致。
        _ => String::new(),
    }
}

/// `provider.json` 的位置。
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("provider.json")
}

/// 读回保存过的配置；文件不存在或读不动都算"没配过"。
pub fn load(data_dir: &Path) -> Option<ProviderConfig> {
    let text = std::fs::read_to_string(path(data_dir)).ok()?;
    let config: ProviderConfig = serde_json::from_str(&text).ok()?;
    let config = config.normalised();
    config.is_known_kind().then_some(config)
}

/// 写配置（只有非密钥部分）。
pub fn save(data_dir: &Path, config: &ProviderConfig) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    std::fs::write(path(data_dir), text).map_err(|error| {
        format!("cannot write {}: {error}", path(data_dir).display())
    })
}

/// 删掉配置；本来就没有时也算成功。
pub fn clear(data_dir: &Path) -> Result<(), String> {
    match std::fs::remove_file(path(data_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove provider.json: {error}")),
    }
}

/// 注入 sidecar 的环境变量：`PACKETSAGE_LLM_*`（§7.4 第 3 步）。
///
/// key 缺失时**不设** `PACKETSAGE_LLM_API_KEY`——侧车随后会照常报
/// `providers.configured=false`，界面据此把 run/chat 拦住（S67），不静默 mock。
pub fn agent_env(
    config: Option<&ProviderConfig>,
    api_key: Option<String>,
) -> Vec<(String, String)> {
    let Some(config) = config else {
        return Vec::new();
    };
    let mut pairs = vec![
        ("PACKETSAGE_LLM_PROVIDER".to_owned(), config.provider.clone()),
        ("PACKETSAGE_LLM_MODEL".to_owned(), config.model.clone()),
    ];
    if !config.base_url.is_empty() {
        pairs.push(("PACKETSAGE_LLM_BASE_URL".to_owned(), config.base_url.clone()));
    }
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        pairs.push(("PACKETSAGE_LLM_API_KEY".to_owned(), key));
    }
        if let Some(thinking) = config.thinking_env() {
            pairs.push(("PACKETSAGE_LLM_THINKING".to_owned(), thinking.to_owned()));
        }
        if let Some(effort) = config.effort_env() {
            pairs.push(("PACKETSAGE_LLM_EFFORT".to_owned(), effort.to_owned()));
        }
    // `local` 端点常常在局域网/本机，明确不要代理，免得 curl 式的代理环境把
    // 请求带走（有 key 的两种 provider 不受影响）。
    if config.provider == "local" {
        pairs.push(("NO_PROXY".to_owned(), "localhost,127.0.0.1".to_owned()));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("packetsage-provider-tests").join(name);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn defaults_match_the_agent_table() {
        assert_eq!(ProviderConfig::defaults("deepseek").model, "deepseek-flash");
        assert_eq!(
            ProviderConfig::defaults("openai").base_url,
            "https://api.openai.com/v1"
        );
        assert!(ProviderConfig::defaults("mock").model.is_empty());
    }

    #[test]
    fn save_load_and_clear_round_trip() {
        let dir = scratch("round-trip");
        clear(&dir).expect("clean");
        assert_eq!(load(&dir), None);
        let config = ProviderConfig::defaults("deepseek");
        save(&dir, &config).expect("save");
        assert_eq!(load(&dir), Some(config));
        clear(&dir).expect("clear");
        assert_eq!(load(&dir), None);
    }

    #[test]
    fn normalising_fills_the_documented_defaults() {
        let config = ProviderConfig {
            provider: " DeepSeek ".to_owned(),
            model: "  ".to_owned(),
            base_url: String::new(),
            thinking: " OFF ".to_owned(),
            effort: " HIGH ".to_owned(),
        }
        .normalised();
        assert_eq!(config.provider, "deepseek");
        assert_eq!(config.model, "deepseek-flash");
        assert_eq!(config.base_url, "https://api.deepseek.com/v1");
        assert!(config.needs_key());
        // 思考模式只认三种值，"OFF" 归一成 disabled（侧车据此发 thinking 字段）。
        assert_eq!(config.thinking, "disabled");
        assert_eq!(config.thinking_env(), Some("disabled"));
        // 强度同样只认四档：别名按文档收敛，界面只给 auto/low/high/max。
        assert_eq!(config.effort, "high");
        assert_eq!(config.effort_env(), Some("high"));
        assert!(!ProviderConfig::defaults("mock").needs_key());
    }

    #[test]
    fn agent_env_carries_the_key_only_when_there_is_one() {
        let config = ProviderConfig::defaults("deepseek");
        let with = agent_env(Some(&config), Some("sk-test".to_owned()));
        assert!(with.contains(&("PACKETSAGE_LLM_API_KEY".to_owned(), "sk-test".to_owned())));
        let without = agent_env(Some(&config), None);
        assert!(!without.iter().any(|(key, _)| key == "PACKETSAGE_LLM_API_KEY"));
        assert!(agent_env(None, Some("sk-test".to_owned())).is_empty());
    }

    #[test]
    fn thinking_env_is_injected_only_when_explicit() {
        let auto = ProviderConfig::defaults("deepseek");
        assert!(!agent_env(Some(&auto), None)
            .iter()
            .any(|(key, _)| key == "PACKETSAGE_LLM_THINKING"));

        let off = ProviderConfig {
            thinking: "disabled".to_owned(),
            ..ProviderConfig::defaults("deepseek")
        };
        assert!(agent_env(Some(&off), None)
            .contains(&("PACKETSAGE_LLM_THINKING".to_owned(), "disabled".to_owned())));
    }
}
