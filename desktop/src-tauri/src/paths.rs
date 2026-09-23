//! 应用数据目录与本机路径解析（《GUI 工程规格书 v0.2》§7.1）。
//!
//! 打包后没有"仓库根"，所以一切可写状态都落在
//! `%LOCALAPPDATA%\PacketSage\{packetsage.db, reports\, logs\}`；卸载默认保留。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    /// `%LOCALAPPDATA%\PacketSage`
    pub data: PathBuf,
    pub logs: PathBuf,
    pub reports: PathBuf,
    /// 引擎的存储 URL；桌面应用不直连 SQLite，只把它交给 engine sidecar。
    pub db_url: String,
    /// 规则目录（bundled `rules/` 或开发树里的 `rules/`），可不存在。
    pub rules: Option<PathBuf>,
}

impl Paths {
    pub fn resolve(install_dir: Option<&Path>) -> Self {
        let data = local_app_data()
            .unwrap_or_else(std::env::temp_dir)
            .join("PacketSage");
        let logs = data.join("logs");
        let reports = data.join("reports");
        for dir in [&data, &logs, &reports] {
            let _ = std::fs::create_dir_all(dir);
        }
        let db_path = data.join("packetsage.db");
        Self {
            db_url: format!("sqlite://{}", db_path.display()),
            data,
            logs,
            reports,
            rules: resolve_rules(install_dir),
        }
    }

    pub fn engine_log(&self) -> PathBuf {
        self.logs.join("engine.log")
    }

    pub fn agent_log(&self) -> PathBuf {
        self.logs.join("agent.log")
    }
}

fn local_app_data() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("LOCALAPPDATA") {
        if !value.trim().is_empty() {
            return Some(PathBuf::from(value));
        }
    }
    let profile = std::env::var("USERPROFILE").ok()?;
    Some(PathBuf::from(profile).join("AppData").join("Local"))
}

/// 仓库根：开发时 `src-tauri/../../`；打包后不存在（返回 `None`）。
pub fn dev_repo_root() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent()?.parent()?;
    if root.join("Cargo.toml").is_file() && root.join("crates").is_dir() {
        Some(root.to_path_buf())
    } else {
        None
    }
}

/// 规则目录：`<app>/rules/builtin` → `<app>/rules` → 开发树的 `rules/`。
fn resolve_rules(install_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = install_dir {
        for candidate in [dir.join("rules").join("builtin"), dir.join("rules")] {
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    let root = dev_repo_root()?;
    let candidate = root.join("rules");
    candidate.is_dir().then_some(candidate)
}
