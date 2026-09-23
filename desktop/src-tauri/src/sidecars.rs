//! 两个 sidecar 的解析与启动（《GUI 工程规格书 v0.2》§2.2 / U3）。
//!
//! 解析顺序对两者一致，且**打包后不需要 PATH**：
//!
//! 1. 环境变量（`PACKETSAGE_ENGINE` / `PACKETSAGE_AGENT`）——开发与排障用；
//! 2. 程序旁边的 sidecar（Tauri `externalBin` 的落点，安装包里就是这一条）；
//! 3. PATH；
//! 4. 开发树回退（`target/release/packetsage.exe`、`python -m packetsage_agent`）。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::Value;

use crate::jsonl::{hide_console_window, ExitSink, FrameSink, JsonlChild, SpawnOptions};
use crate::paths::{dev_repo_root, Paths};
use crate::provider;
use crate::secrets;

#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// 人读的"这个进程是什么"，用于错误提示。
    pub describe: String,
}

pub struct Sidecars {
    pub engine: Arc<JsonlChild>,
    pub agent: Arc<JsonlChild>,
    pub engine_spec: LaunchSpec,
    pub agent_spec: LaunchSpec,
}

/// 引擎 sidecar：`packetsage serve`。
pub fn resolve_engine(install_dir: Option<&Path>) -> Result<LaunchSpec, String> {
    if let Ok(path) = std::env::var("PACKETSAGE_ENGINE") {
        if !path.trim().is_empty() {
            return Ok(spec(path, vec!["serve".into()], None, "engine (from $PACKETSAGE_ENGINE)"));
        }
    }
    let exe_name = if cfg!(windows) { "packetsage.exe" } else { "packetsage" };
    if let Some(dir) = install_dir {
        let beside = dir.join(exe_name);
        if beside.is_file() {
            return Ok(spec(
                beside.to_string_lossy().into(),
                vec!["serve".into()],
                None,
                "engine (bundled sidecar)",
            ));
        }
    }
    if let Some(found) = which(exe_name) {
        return Ok(spec(found, vec!["serve".into()], None, "engine (from PATH)"));
    }
    if let Some(root) = dev_repo_root() {
        for candidate in [
            root.join("target").join("release").join(exe_name),
            root.join("target").join("debug").join(exe_name),
        ] {
            if candidate.is_file() {
                return Ok(spec(
                    candidate.to_string_lossy().into(),
                    vec!["serve".into()],
                    None,
                    "engine (development build)",
                ));
            }
        }
    }
    Err(format!(
        "engine sidecar not found: install PacketSage properly, or set $PACKETSAGE_ENGINE \
         (expected {exe_name} next to the app or on PATH)"
    ))
}

/// Agent sidecar：打包后是 `packetsage-agent.exe serve`，开发时退回 `python -m`。
pub fn resolve_agent(install_dir: Option<&Path>) -> Result<LaunchSpec, String> {
    if let Ok(raw) = std::env::var("PACKETSAGE_AGENT") {
        if !raw.trim().is_empty() {
            let parts = split_command(&raw);
            if let Some((program, rest)) = parts.split_first() {
                return Ok(spec(
                    program.clone(),
                    [rest.to_vec(), vec!["serve".to_owned()]].concat(),
                    None,
                    "agent (from $PACKETSAGE_AGENT)",
                ));
            }
        }
    }
    let exe_name = if cfg!(windows) {
        "packetsage-agent.exe"
    } else {
        "packetsage-agent"
    };
    if let Some(dir) = install_dir {
        let beside = dir.join(exe_name);
        if beside.is_file() {
            return Ok(spec(
                beside.to_string_lossy().into(),
                vec!["serve".into()],
                None,
                "agent (bundled sidecar)",
            ));
        }
        // `--onedir` 布局：`agent-sidecar/packetsage-agent.exe`
        let nested = dir.join("agent-sidecar").join(exe_name);
        if nested.is_file() {
            return Ok(spec(
                nested.to_string_lossy().into(),
                vec!["serve".into()],
                None,
                "agent (bundled onedir sidecar)",
            ));
        }
    }
    if let Some(found) = which(exe_name) {
        return Ok(spec(found, vec!["serve".into()], None, "agent (from PATH)"));
    }
    if let Some(root) = dev_repo_root() {
        let agent_dir = root.join("agent");
        let module = vec!["-m".to_owned(), "packetsage_agent".to_owned(), "serve".to_owned()];
        for python in ["python", "python3"] {
            if which(python).is_some() {
                return Ok(spec(
                    python.to_owned(),
                    module,
                    Some(agent_dir),
                    "agent (development `python -m packetsage_agent serve`)",
                ));
            }
        }
    }
    Err(format!(
        "agent sidecar not found: install PacketSage properly, or set $PACKETSAGE_AGENT \
         (expected {exe_name} next to the app or on PATH)"
    ))
}

fn spec(program: String, args: Vec<String>, cwd: Option<PathBuf>, describe: &str) -> LaunchSpec {
    LaunchSpec {
        program,
        args,
        cwd,
        describe: describe.to_owned(),
    }
}

/// 启动两个 sidecar；Agent 通过环境拿到引擎路径与存储 URL（U7 的注入点）。
pub fn start(
    paths: &Paths,
    install_dir: Option<&Path>,
    agent_sink: Option<FrameSink>,
    agent_exit: Option<ExitSink>,
    engine_exit: Option<ExitSink>,
) -> Result<Sidecars, String> {
    let engine_spec = resolve_engine(install_dir)?;
    let agent_spec = resolve_agent(install_dir)?;
    let provider_env = provider_env(paths);
    start_with_specs(
        paths,
        engine_spec,
        agent_spec,
        &provider_env,
        agent_sink,
        agent_exit,
        engine_exit,
    )
}

/// provider 的注入环境（U7）：`provider.json` + 凭据管理器里的 key。
///
/// 拿不到凭据（没配过、或凭据管理器读失败）时**不注入 key**：侧车会报
/// `providers.configured=false`，界面据此进向导 / 拦住 run（S67），不静默 mock。
pub fn provider_env(paths: &Paths) -> Vec<(String, String)> {
    let config = provider::load(&paths.data);
    let key = match secrets::read() {
        Ok(key) => key,
        Err(error) => {
            eprintln!("packetsage-desktop: credential manager read failed: {error}");
            None
        }
    };
    provider::agent_env(config.as_ref(), key)
}

/// `start` 的"已经解析好"版本：打包态连接用例（P11）直接喂真 exe 的路径，
/// 从而覆盖"外壳 ↔ 打包 sidecar"这条没有 PATH、没有 `python -m` 的路径。
pub fn start_with_specs(
    paths: &Paths,
    engine_spec: LaunchSpec,
    agent_spec: LaunchSpec,
    agent_env: &[(String, String)],
    agent_sink: Option<FrameSink>,
    agent_exit: Option<ExitSink>,
    engine_exit: Option<ExitSink>,
) -> Result<Sidecars, String> {
    let engine = JsonlChild::spawn(SpawnOptions {
        label: "engine",
        program: &engine_spec.program,
        args: &engine_spec.args,
        envs: &[
            ("PACKETSAGE_STORAGE_URL".to_owned(), paths.db_url.clone()),
            ("PACKETSAGE_DB".to_owned(), paths.db_url.clone()),
        ],
        cwd: engine_spec.cwd.as_deref(),
        log_path: &paths.engine_log(),
        sink: None,
        on_exit: engine_exit,
    })?;

    let agent = spawn_agent(
        paths,
        &agent_spec,
        &engine_spec.program,
        agent_env,
        agent_sink,
        agent_exit,
    )?;

    Ok(Sidecars {
        engine,
        agent,
        engine_spec,
        agent_spec,
    })
}

/// 起一个 Agent sidecar（U7 保存配置后要重启它，让新的环境变量生效）。
pub fn spawn_agent(
    paths: &Paths,
    agent_spec: &LaunchSpec,
    engine_program: &str,
    provider_env: &[(String, String)],
    agent_sink: Option<FrameSink>,
    agent_exit: Option<ExitSink>,
) -> Result<Arc<JsonlChild>, String> {
    let mut envs = vec![
        ("PACKETSAGE_ENGINE".to_owned(), engine_program.to_owned()),
        ("PACKETSAGE_STORAGE_URL".to_owned(), paths.db_url.clone()),
        ("PACKETSAGE_DB".to_owned(), paths.db_url.clone()),
        ("PYTHONUTF8".to_owned(), "1".to_owned()),
        ("PYTHONIOENCODING".to_owned(), "utf-8".to_owned()),
        // 外壳自己注入 provider，绝不读开发树里的 agent/.env（U7 的"不落明文"）。
        (
            "PACKETSAGE_ENV_FILE".to_owned(),
            paths.data.join("agent.env").display().to_string(),
        ),
    ];
    if let Some(rules) = paths.rules.as_ref() {
        envs.push((
            "PACKETSAGE_RULES".to_owned(),
            rules.to_string_lossy().into_owned(),
        ));
    }
    envs.extend(provider_env.iter().cloned());
    JsonlChild::spawn(SpawnOptions {
        label: "agent",
        program: &agent_spec.program,
        args: &agent_spec.args,
        envs: &envs,
        cwd: agent_spec.cwd.as_deref(),
        log_path: &paths.agent_log(),
        sink: agent_sink,
        on_exit: agent_exit,
    })
}

/// 引擎 RPC 信封：`{"id","method","params"}` → 返回 `content`（§5.4）。
pub fn engine_call(
    engine: &JsonlChild,
    method: &str,
    params: Value,
    timeout: std::time::Duration,
) -> Result<Value, String> {
    let id = new_id();
    let frame = serde_json::json!({"id": id, "method": method, "params": params});
    let response = engine.request(frame, &id, timeout)?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        let result = response.get("result").cloned().unwrap_or(Value::Null);
        if result.get("trusted_as_instruction").is_some() {
            return Ok(result.get("content").cloned().unwrap_or(Value::Null));
        }
        return Ok(result);
    }
    let error = response.get("error").cloned().unwrap_or(Value::Null);
    let code = error.get("code").and_then(Value::as_str).unwrap_or("UNKNOWN");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("engine refused the call");
    Err(format!("{code}: {message}"))
}

/// Agent 命令：`{"id","type":"cmd","command","params"}` → 返回 `ack.result`。
pub fn agent_call(
    agent: &JsonlChild,
    command: &str,
    params: Value,
    timeout: std::time::Duration,
) -> Result<Value, String> {
    let id = new_id();
    let frame = serde_json::json!({
        "id": id,
        "type": "cmd",
        "command": command,
        "params": params,
    });
    let response = agent.request(frame, &id, timeout)?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(response.get("result").cloned().unwrap_or(Value::Null));
    }
    let error = response.get("error").cloned().unwrap_or(Value::Null);
    let code = error.get("code").and_then(Value::as_str).unwrap_or("INTERNAL");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("agent refused the command");
    let retryable = error.get("retryable").and_then(Value::as_bool).unwrap_or(false);
    Err(format!("{code}|{retryable}|{message}"))
}

/// 引擎 CLI 的只读出口（`doctor --json` / `db query --readonly --jsonl`，§5.1/U8）。
pub fn engine_cli(spec: &LaunchSpec, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = Command::new(&spec.program);
    command
        .args(args)
        .current_dir(spec.cwd.clone().unwrap_or_else(|| PathBuf::from(".")));
    // 一次性 CLI 也会弹黑框（P4）；桌面应用一律不开控制台。
    hide_console_window(&mut command);
    command
        .output()
        .map_err(|error| format!("cannot run {} {:?}: {error}", spec.program, args))
}

/// 校验一个 provider 配置（U7 / §7.4 第 2 步的 `GET /models`）。
///
/// 走**Agent 自己的** `setup --verify --print`：探测逻辑只有一份（`probe_provider`），
/// key 只经环境变量交给一个短命子进程，`--print` 保证它不写任何文件。
/// 返回 `(ok, 人读的 detail)`。
pub fn probe_provider(
    spec: &LaunchSpec,
    config: &crate::provider::ProviderConfig,
    api_key: Option<String>,
    env_file: &Path,
) -> Result<(bool, String), String> {
    let env_pairs = crate::provider::agent_env(Some(config), api_key);
    let mut command = Command::new(&spec.program);
    command
        // 注意：**不能带 `spec.args`**——Agent 的 spec 是 `["serve"]`，
        // 而这里要跑的是一次性 `setup`（同一个 exe，不同的子命令）。
        .args(["setup", "--provider", &config.provider, "--print"])
        // 隔离：不读也不写开发树的 agent/.env（U7 的"不落明文"）。
        .env("PACKETSAGE_ENV_FILE", env_file)
        .current_dir(spec.cwd.clone().unwrap_or_else(|| PathBuf::from(".")));
    if !config.model.is_empty() {
        command.args(["--model", &config.model]);
    }
    if !config.base_url.is_empty() {
        command.args(["--base-url", &config.base_url]);
    }
    for (key, value) in &env_pairs {
        command.env(key, value);
    }
    hide_console_window(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("cannot run {}: {error}", spec.program))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok(parse_probe(&stdout, &stderr))
}

/// `GET /models` 的模型名列表（桌面端"模型"下拉，见 DeepSeek list-models 指南）。
///
/// 与 [`probe_provider`] 同一条路：跑 Agent 自己的 `setup --list-models --print`，
/// 拿不到就返回空列表（界面照实说"拉不到"，不猜候选）。
pub fn list_provider_models(
    spec: &LaunchSpec,
    config: &crate::provider::ProviderConfig,
    api_key: Option<String>,
    env_file: &Path,
) -> Result<Vec<String>, String> {
    let env_pairs = crate::provider::agent_env(Some(config), api_key);
    let mut command = Command::new(&spec.program);
    command
        .args(["setup", "--provider", &config.provider, "--list-models", "--print"])
        .env("PACKETSAGE_ENV_FILE", env_file)
        .current_dir(spec.cwd.clone().unwrap_or_else(|| PathBuf::from(".")));
    if !config.base_url.is_empty() {
        command.args(["--base-url", &config.base_url]);
    }
    for (key, value) in &env_pairs {
        command.env(key, value);
    }
    hide_console_window(&mut command);
    let output = command
        .output()
        .map_err(|error| format!("cannot run {}: {error}", spec.program))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_models(&stdout))
}

/// `{"models": [...]}` → 名字列表；解析不了就是空（不把日志当模型名）。
fn parse_models(stdout: &str) -> Vec<String> {
    for line in stdout.lines().rev() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(rows) = value.get("models").and_then(|rows| rows.as_array()) {
                let mut names: Vec<String> = rows
                    .iter()
                    .filter_map(|row| row.as_str().map(str::to_owned))
                    .collect();
                names.sort();
                names.dedup();
                return names;
            }
        }
    }
    Vec::new()
}

/// `setup --print` 的输出里只有 `verify` 那一行是我们关心的。
///
/// 注意：`--print` 不论校验成败都 exit 0（它不写文件，就不需要按失败退出），
/// 所以判据是行里的 ✅/❌，不是退出码。
fn parse_probe(stdout: &str, stderr: &str) -> (bool, String) {
    let raw = stdout
        .lines()
        .find_map(|line| line.strip_prefix("verify").map(|rest| rest.trim().to_owned()));
    match raw {
        Some(raw) => {
            let detail = raw.trim_start_matches(['✅', '⚠', '❌']).trim().to_owned();
            (!raw.contains('❌') && raw.contains('✅'), detail)
        }
        None => {
            let tail: Vec<&str> = stderr
                .lines()
                .chain(stdout.lines())
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            let tail = tail
                .iter()
                .rev()
                .take(3)
                .rev()
                .copied()
                .collect::<Vec<_>>()
                .join(" | ");
            (
                false,
                if tail.is_empty() {
                    "校验没有输出：Agent sidecar 可能没起来".to_owned()
                } else {
                    tail
                },
            )
        }
    }
}

pub fn new_id() -> String {
    // 只用于 pending-map 配对，不需要密码学随机性；纳秒 + 计数器足够唯一。
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{nanos:x}-{n:x}")
}

/// `db query --readonly` 的 URL 必须含 `mode=ro`（§8.1 的护栏在 Rust 侧）。
pub fn readonly_url(db_url: &str) -> String {
    if db_url.contains("mode=ro") {
        return db_url.to_owned();
    }
    let joiner = if db_url.contains('?') { '&' } else { '?' };
    format!("{db_url}{joiner}mode=ro")
}

fn which(name: &str) -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    // Windows binaries carry an extension and PATH entries name the file without
    // it ("python" lives in `...\bin\python.exe`), so PATHEXT is honoured.
    let mut candidates = vec![name.to_owned()];
    if !name.contains('.') {
        let exts = if cfg!(windows) {
            std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
        } else {
            String::new()
        };
        for ext in exts.split(';').filter(|ext| !ext.trim().is_empty()) {
            candidates.push(format!("{name}{}", ext.to_lowercase()));
        }
    }
    for dir in std::env::split_paths(&paths) {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if path.is_file() {
                return Some(path.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// `--engine "C:\path with space\packetsage.exe serve"` 的拆分（Windows 感知）。
fn split_command(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in raw.chars() {
        match ch {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_style_commands_keep_spaces_in_paths() {
        let parts = split_command(r#""C:\Program Files\ps\packetsage.exe" serve"#);
        assert_eq!(parts, vec!["C:\\Program Files\\ps\\packetsage.exe", "serve"]);
        assert_eq!(split_command("python -m packetsage_agent"), vec!["python", "-m", "packetsage_agent"]);
    }

    #[test]
    fn ids_are_unique() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
    }
}
