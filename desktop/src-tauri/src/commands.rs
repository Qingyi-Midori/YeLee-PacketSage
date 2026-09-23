//! 前端能调的命令（《GUI 工程规格书 v0.2》§2.2 / §4.1 通道 A、B、C）。
//!
//! 三条纪律：
//!
//! * **证据路径不经过 Python**：摘要/会话/告警/finding/台账全部走引擎（通道 A）；
//!   Python 只负责 run 与报告（通道 B）；
//! * 长任务不占主线程：命令声明成 `async`，阻塞工作在 `spawn_blocking` 里做；
//! * 错误一律是"可读字符串"，前端照原样显示（§6.4 规矩 1）。

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::State;

use crate::jsonl::{EventQueue, ExitSink, FrameSink, JsonlChild};
use crate::paths::Paths;
use crate::provider::ProviderConfig;
use crate::sidecars::{
    agent_call, engine_call, engine_cli, probe_provider, provider_env, spawn_agent, LaunchSpec,
    Sidecars,
};
use crate::{provider, secrets};

/// 引擎的 RPC 超时沿用收口 §5.12 的矩阵。
const TIMEOUT_QUERY: Duration = Duration::from_secs(10);
const TIMEOUT_ANALYZE: Duration = Duration::from_secs(60);
const TIMEOUT_AGENT_RUN: Duration = Duration::from_secs(120);
const TIMEOUT_AGENT_REPORT: Duration = Duration::from_secs(180);

pub struct AppState {
    pub paths: Paths,
    /// 两个 sidecar；**起不来时是 `None`**（P5：外壳照样起，界面显示"安装损坏"）。
    /// 用 `Mutex` 是因为 U7 保存配置后要**只重启 Agent**（引擎里还有已分析的抓包）。
    sidecars: std::sync::Mutex<Option<Sidecars>>,
    /// `sidecars == None` 时的原因（原样给用户看，§6.4 规矩 1）。
    pub start_error: Option<String>,
    /// 事件队列（P8）；诊断里能看见深度与累计丢弃数。
    pub event_queue: Arc<EventQueue>,
    /// 当前 run 的 id（`cancel` 与并发保护都用它）。
    /// `run_finished` / sidecar 退出时清空（P9）——防止 `cancel` 拿旧 id 发空操作。
    pub run: Arc<std::sync::Mutex<Option<String>>>,
    /// 事件通道：每次 `run_agent` 覆盖，事件按 §4.5 原样转发给前端。
    pub events: Arc<std::sync::Mutex<Option<Channel<Value>>>>,
    /// 重启 Agent 时复用的落点（P8 的队列 + 通道 C）。
    pub agent_sink: Option<FrameSink>,
    /// Agent / 引擎退出时的通知（P2）。
    pub exit_sink: ExitSink,
}

impl AppState {
    /// 外壳组装状态（字段本身不公开：Agent 可以被 U7 换掉，必须走这里的办法）。
    #[allow(clippy::too_many_arguments)] // 一次性组装，参数就是全部状态
    pub fn new(
        paths: Paths,
        sidecars: Option<Sidecars>,
        start_error: Option<String>,
        event_queue: Arc<EventQueue>,
        run: Arc<std::sync::Mutex<Option<String>>>,
        events: Arc<std::sync::Mutex<Option<Channel<Value>>>>,
        agent_sink: FrameSink,
        exit_sink: ExitSink,
    ) -> Self {
        Self {
            paths,
            sidecars: std::sync::Mutex::new(sidecars),
            start_error,
            event_queue,
            run,
            events,
            agent_sink: Some(agent_sink),
            exit_sink,
        }
    }

    fn with<T>(&self, pick: impl FnOnce(&Sidecars) -> T) -> Result<T, String> {
        self.sidecars
            .lock()
            .map_err(|_| "sidecars lock poisoned".to_owned())?
            .as_ref()
            .map(pick)
            .ok_or_else(|| self.not_running())
    }

    pub fn engine(&self) -> Result<Arc<JsonlChild>, String> {
        self.with(|sidecars| Arc::clone(&sidecars.engine))
    }

    pub fn agent(&self) -> Result<Arc<JsonlChild>, String> {
        self.with(|sidecars| Arc::clone(&sidecars.agent))
    }

    pub fn engine_spec(&self) -> Result<LaunchSpec, String> {
        self.with(|sidecars| sidecars.engine_spec.clone())
    }

    pub fn agent_spec(&self) -> Result<LaunchSpec, String> {
        self.with(|sidecars| sidecars.agent_spec.clone())
    }

    /// 换掉 Agent（U7：把新的 provider 环境变量交给一个新进程）。
    fn replace_agent(&self, agent: Arc<JsonlChild>) -> Result<(), String> {
        let mut guard = self
            .sidecars
            .lock()
            .map_err(|_| "sidecars lock poisoned".to_owned())?;
        let sidecars = guard.as_mut().ok_or_else(|| self.not_running())?;
        let old = std::mem::replace(&mut sidecars.agent, agent);
        drop(guard);
        old.close();
        Ok(())
    }

    /// 安装损坏 / sidecar 起不来时的统一说法（§6.1 的"安装损坏"提示）。
    pub fn not_running(&self) -> String {
        self.start_error.clone().unwrap_or_else(|| {
            "两个 sidecar 都没有起来：请重新安装 PacketSage（或设 $PACKETSAGE_ENGINE / \
             $PACKETSAGE_AGENT 指到可执行文件）"
                .to_owned()
        })
    }
}

#[derive(Serialize)]
pub struct AppInfo {
    pub engine: String,
    pub agent: String,
    pub engine_program: String,
    pub agent_program: String,
    pub db_url: String,
    pub data_dir: String,
    pub engine_alive: bool,
    pub agent_alive: bool,
    /// `Some(...)` = sidecar 没起来；界面据此显示"安装损坏"（P5）。
    pub start_error: Option<String>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let (engine, agent) = match (state.engine_spec(), state.agent_spec()) {
        (Ok(engine), Ok(agent)) => (engine.describe, agent.describe),
        _ => (
            "engine (not started)".to_owned(),
            "agent (not started)".to_owned(),
        ),
    };
    let (engine_program, agent_program) = match (state.engine_spec(), state.agent_spec()) {
        (Ok(engine), Ok(agent)) => (engine.program, agent.program),
        _ => (String::new(), String::new()),
    };
    AppInfo {
        engine,
        agent,
        engine_program,
        agent_program,
        db_url: state.paths.db_url.clone(),
        data_dir: state.paths.data.display().to_string(),
        engine_alive: state.engine().is_ok_and(|engine| engine.alive()),
        agent_alive: state.agent().is_ok_and(|agent| agent.alive()),
        start_error: state.start_error.clone(),
    }
}

/// 握手：`hello` → `welcome`（§4.3）。版本不匹配由 sidecar 回 `PROTOCOL_MISMATCH`。
#[tauri::command(async)]
pub async fn handshake(state: State<'_, AppState>) -> Result<Value, String> {
    let agent = state.agent()?;
    let payload = tauri::async_runtime::spawn_blocking(move || {
        agent_call(
            &agent,
            "hello",
            json!({
                "protocol_version": 1,
                "client": "packetsage-desktop",
                "client_version": env!("CARGO_PKG_VERSION"),
            }),
            TIMEOUT_QUERY,
        )
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(payload)
}

/// `packetsage doctor --json --no-net`（§5.8 的十项自检）。
#[tauri::command(async)]
pub async fn doctor(state: State<'_, AppState>) -> Result<Value, String> {
    let spec = state.engine_spec()?;
    tauri::async_runtime::spawn_blocking(move || {
        let output = engine_cli(&spec, &["doctor", "--json", "--no-net"])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str::<Value>(stdout.trim())
            .map_err(|error| format!("doctor did not return JSON: {error}; {stdout}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 历史 task：`db query --readonly --jsonl`（U8 允许的唯一 SQL 出口）。
#[tauri::command(async)]
pub async fn list_tasks(state: State<'_, AppState>) -> Result<Value, String> {
    let spec = state.engine_spec()?;
    let url = crate::sidecars::readonly_url(&state.paths.db_url);
    tauri::async_runtime::spawn_blocking(move || {
        let sql = "SELECT id, status, source_path, packet_count, byte_count, started_at \
                   FROM analysis_tasks ORDER BY started_at DESC";
        let output = engine_cli(
            &spec,
            &[
                "db", "query", "--readonly", "--db", &url, "--jsonl", "--limit", "50", "--sql",
                sql,
            ],
        )?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let rows: Vec<Value> = stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
            .collect();
        Ok(json!(rows))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// `analyze_file`：落库 + 留在引擎内存（通道 A，不经过 Python）。
#[tauri::command(async)]
pub async fn analyze(state: State<'_, AppState>, path: String) -> Result<Value, String> {
    let engine = state.engine()?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = engine_call(&engine, "analyze_file", json!({"path": path}), TIMEOUT_ANALYZE)?;
        let task_id = result
            .get("task_id")
            .and_then(Value::as_str)
            .ok_or("analyze_file did not return a task_id")?
            .to_owned();
        let summary = engine_call(
            &engine,
            "get_capture_summary",
            json!({"task_id": task_id}),
            TIMEOUT_QUERY,
        )
        .unwrap_or(Value::Null);
        Ok(json!({"task_id": task_id, "summary": summary}))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 任务面板的五份数据（证据链与告警都是一等公民，§5.2）。
#[tauri::command(async)]
pub async fn task_overview(state: State<'_, AppState>, task_id: String) -> Result<Value, String> {
    let engine = state.engine()?;
    tauri::async_runtime::spawn_blocking(move || {
        let summary = engine_call(
            &engine,
            "get_capture_summary",
            json!({"task_id": task_id}),
            TIMEOUT_QUERY,
        )?;
        let alerts = engine_call(
            &engine,
            "check_alerts",
            json!({"task_id": task_id}),
            TIMEOUT_QUERY,
        )
        .unwrap_or_else(|error| json!({"alerts": [], "error": error}));
        let findings = engine_call(
            &engine,
            "query_history",
            json!({"task_id": task_id, "kind": "findings", "limit": 50}),
            TIMEOUT_QUERY,
        )
        .unwrap_or_else(|error| json!({"findings": [], "error": error}));
        let trace = engine_call(
            &engine,
            "query_history",
            json!({"task_id": task_id, "kind": "trace", "limit": 200}),
            TIMEOUT_QUERY,
        )
        .unwrap_or_else(|error| json!({"trace": [], "error": error}));
        let artifacts = engine_call(
            &engine,
            "get_task_artifacts",
            json!({"task_id": task_id}),
            TIMEOUT_QUERY,
        )
        .unwrap_or(Value::Null);
        Ok(json!({
            "summary": summary,
            "alerts": alerts.get("alerts").cloned().unwrap_or(json!([])),
            "findings": findings.get("findings").cloned().unwrap_or(json!([])),
            "trace": trace.get("trace").cloned().unwrap_or(json!([])),
            "artifacts": artifacts,
        }))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 一次 run：ack 立即返回 `run_id`，事件经通道 C 推给前端（U4）。
#[tauri::command(async)]
pub async fn run_agent(
    state: State<'_, AppState>,
    task_id: String,
    goal: String,
    mode: String,
    channel: Channel<Value>,
) -> Result<Value, String> {
    *state.events.lock().unwrap() = Some(channel.clone());
    let agent = state.agent()?;
    let params = json!({"task_id": task_id, "goal": goal, "mode": mode});
    let payload = tauri::async_runtime::spawn_blocking(move || {
        agent_call(&agent, "run", params, TIMEOUT_AGENT_RUN)
    })
    .await
    .map_err(|error| error.to_string())??;
    if let Some(run_id) = payload.get("run_id").and_then(Value::as_str) {
        *state.run.lock().map_err(|_| "run lock poisoned".to_owned())? = Some(run_id.to_owned());
    }
    Ok(payload)
}

/// 取消：只表示"已受理"，界面在 `run_finished` 之前显示"正在收尾…"（§4.7）。
#[tauri::command(async)]
pub async fn cancel(state: State<'_, AppState>) -> Result<Value, String> {
    // P9：`run_finished` / sidecar 退出都会清空 run id，所以"没有 run"现在会被
    // 明确拒绝，而不是拿空 id 发一条没有任何作用的 `cancel`。
    let run_id = state
        .run
        .lock()
        .map_err(|_| "run lock poisoned".to_owned())?
        .clone()
        .ok_or_else(|| "没有正在进行的 run，无需取消".to_owned())?;
    let agent = state.agent()?;
    tauri::async_runtime::spawn_blocking(move || {
        agent_call(&agent, "cancel", json!({"run_id": run_id}), TIMEOUT_QUERY)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(async)]
pub async fn agent_status(state: State<'_, AppState>) -> Result<Value, String> {
    let agent = state.agent()?;
    tauri::async_runtime::spawn_blocking(move || {
        agent_call(&agent, "status", json!({}), Duration::from_secs(10))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 九节报告（通道 B 的 `report` 命令）；默认写到 app data 的 `reports/`。
#[tauri::command(async)]
pub async fn report(state: State<'_, AppState>, task_id: String) -> Result<Value, String> {
    let agent = state.agent()?;
    let out = state
        .paths
        .reports
        .join(format!("PacketSage-{task_id}.md"))
        .display()
        .to_string();
    tauri::async_runtime::spawn_blocking(move || {
        agent_call(
            &agent,
            "report",
            json!({"task_id": task_id, "out_path": out}),
            TIMEOUT_AGENT_REPORT,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|error| format!("cannot read {path}: {error}"))
}

/// sidecar 的 stderr 尾巴（界面只在崩溃时显示，§4.9）。
#[tauri::command]
pub fn sidecar_diagnostics(state: State<'_, AppState>) -> Value {
    let (queued, dropped) = state.event_queue.stats();
    json!({
        "start_error": state.start_error.clone(),
        "engine": {
            "alive": state.engine().is_ok_and(|engine| engine.alive()),
            "pid": state.engine().ok().and_then(|engine| engine.pid()),
            "stderr_tail": state.engine().map(|engine| engine.stderr_tail(3)).unwrap_or_default(),
        },
        "agent": {
            "alive": state.agent().is_ok_and(|agent| agent.alive()),
            "pid": state.agent().ok().and_then(|agent| agent.pid()),
            "stderr_tail": state.agent().map(|agent| agent.stderr_tail(3)).unwrap_or_default(),
        },
        // P8 的可观测面：队列深度与累计丢弃数。
        "events": {"queued": queued, "dropped": dropped, "capacity": crate::jsonl::EVENT_QUEUE_CAPACITY},
    })
}

/// 退出：先把 run 收尾，再让两个 sidecar 优雅退出（M14 不留孤儿进程）。
pub fn shutdown(state: &AppState) {
    let sidecars = match state.sidecars.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(sidecars) = sidecars.as_ref() else {
        return;
    };
    let _ = agent_call(&sidecars.agent, "shutdown", json!({}), Duration::from_secs(10));
    sidecars.engine.close();
    sidecars.agent.close();
}

// ---------------------------------------------------------------- U7 首次运行

#[derive(Serialize)]
pub struct ProviderStatus {
    /// 配置里有没有一条能开跑的 provider（`mock` 也算），**不含**"key 一定对"。
    pub configured: bool,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    /// key 是否已经在 Windows 凭据管理器里。
    pub has_key: bool,
    /// 思考模式：`""` = 自动（跟随厂商默认）。向导据此回填。
    pub thinking: String,
    /// `provider.json` 的位置，出错时让人知道去哪儿看。
    pub config_path: String,
}

#[derive(Serialize)]
pub struct VerifyResult {
    pub ok: bool,
    pub detail: String,
    pub provider: String,
    pub model: String,
    pub base_url: String,
}

/// 向导要的状态：配过什么、key 在不在（U7）。
#[tauri::command]
pub fn provider_status(state: State<'_, AppState>) -> Result<ProviderStatus, String> {
    let config = provider::load(&state.paths.data);
    let has_key = secrets::read()?.is_some();
    Ok(ProviderStatus {
        configured: config.is_some(),
        provider: config.as_ref().map(|c| c.provider.clone()).unwrap_or_default(),
        model: config.as_ref().map(|c| c.model.clone()).unwrap_or_default(),
        base_url: config
            .as_ref()
            .map(|c| c.base_url.clone())
            .unwrap_or_default(),
        has_key,
        thinking: config
            .as_ref()
            .map(|c| c.thinking.clone())
            .unwrap_or_default(),
        config_path: provider::path(&state.paths.data).display().to_string(),
    })
}

/// 模型下拉的数据源：`GET /models`（DeepSeek list-models 指南）。
///
/// 与 `provider_verify` 同一条路——跑 Agent 自己的 `setup --list-models --print`；
/// 拉不到就返回空数组，界面照实说"拉不到"，**不猜候选**。
#[tauri::command(async)]
pub async fn provider_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    // 没配过 provider 就没得拉（不是错误，界面显示"先配置"）。
    let Some(config) = provider::load(&state.paths.data) else {
        return Ok(Vec::new());
    };
    let spec = state.agent_spec()?;
    let env_file = state.paths.data.join("agent.env");
    let key = secrets::read().ok().flatten();
    tauri::async_runtime::spawn_blocking(move || {
        crate::sidecars::list_provider_models(&spec, &config, key, &env_file)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 校验：`GET /models`（§7.4 第 2 步）。
///
/// 走**打包好的 Agent** 自己的 `setup --verify`，不另写一份探测逻辑：
/// key 只通过环境变量交给一个短命子进程，`--print` 保证它不写任何文件。
#[tauri::command(async)]
pub async fn provider_verify(
    state: State<'_, AppState>,
    provider: String,
    model: String,
    base_url: String,
    api_key: Option<String>,
    thinking: Option<String>,
) -> Result<VerifyResult, String> {
    let spec = state.agent_spec()?;
    let config = ProviderConfig {
        provider,
        model,
        base_url,
        thinking: thinking.unwrap_or_default(),
    }
    .normalised();
    if !config.is_known_kind() {
        return Err(format!(
            "unknown provider {:?}; expected {}",
            config.provider,
            provider::KINDS.join(", ")
        ));
    }
    let env_file = state.paths.data.join("agent.env");
    // 留空 = 沿用已保存的 key（向导第 1 步的提示就是这么说的）。
    let key_for_probe = api_key
        .filter(|key| !key.trim().is_empty())
        .or_else(|| secrets::read().ok().flatten());

    tauri::async_runtime::spawn_blocking(move || {
        let (ok, detail) = probe_provider(&spec, &config, key_for_probe, &env_file)?;
        Ok::<_, String>(VerifyResult {
            ok,
            detail,
            provider: config.provider.clone(),
            model: config.model.clone(),
            base_url: config.base_url.clone(),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

/// 保存（§7.4 第 3 步）：key 进凭据管理器、其余进 `provider.json`，然后**只重启 Agent**。
#[tauri::command(async)]
pub async fn provider_save(
    state: State<'_, AppState>,
    provider: String,
    model: String,
    base_url: String,
    api_key: Option<String>,
    thinking: Option<String>,
) -> Result<Value, String> {
    let config = ProviderConfig {
        provider,
        model,
        base_url,
        thinking: thinking.unwrap_or_default(),
    }
    .normalised();
    if !config.is_known_kind() {
        return Err(format!(
            "unknown provider {:?}; expected {}",
            config.provider,
            provider::KINDS.join(", ")
        ));
    }
    let key = api_key.map(|key| key.trim().to_owned()).filter(|k| !k.is_empty());
    if let Some(key) = key.as_ref() {
        secrets::write(&config.provider, key)?;
    } else if config.needs_key() {
        return Err(format!(
            "{} 需要 API key：把 key 填进向导，或改用 mock / local",
            config.provider
        ));
    } else {
        // mock / local 不需要 key：把上一次可能留下的 key 删掉，
        // 免得下次启动又被注入进 sidecar。
        let _ = secrets::delete();
    }
    provider::save(&state.paths.data, &config)?;
    let welcome = match restart_agent_inner(&state).await {
        Ok(welcome) => welcome,
        Err(error) => {
            return Err(format!(
                "配置已保存（key 在 Windows 凭据管理器里），但重启 Agent 失败：{error}"
            ))
        }
    };
    Ok(json!({
        "provider": config.provider,
        "model": config.model,
        "base_url": config.base_url,
        "key_stored": key.is_some(),
        "welcome": welcome,
    }))
}

/// 清空配置（向导里的"重新配置"）：删凭据、删 `provider.json`，再重启 Agent。
#[tauri::command(async)]
pub async fn provider_clear(state: State<'_, AppState>) -> Result<Value, String> {
    let removed_key = secrets::delete()?;
    provider::clear(&state.paths.data)?;
    let welcome = restart_agent_inner(&state).await?;
    Ok(json!({"removed_key": removed_key, "welcome": welcome}))
}

/// 一键重建 Agent（U6）：退出后按当前 provider 配置起一个新的，并重新握手。
#[tauri::command(async)]
pub async fn restart_agent(state: State<'_, AppState>) -> Result<Value, String> {
    restart_agent_inner(&state).await
}

async fn restart_agent_inner(state: &AppState) -> Result<Value, String> {
    let spec = state.agent_spec()?;
    let engine_program = state.engine_spec()?.program;
    let env = provider_env(&state.paths);
    let sink = state.agent_sink.clone();
    let exit = Arc::clone(&state.exit_sink);
    let paths = state.paths.clone();
    let agent = tauri::async_runtime::spawn_blocking(move || {
        spawn_agent(&paths, &spec, &engine_program, &env, sink, Some(exit))
    })
    .await
    .map_err(|error| error.to_string())??;
    state.replace_agent(agent)?;

    // 重新握手：把新的 `providers.configured` 直接交给向导，省一次往返。
    let agent = state.agent()?;
    tauri::async_runtime::spawn_blocking(move || {
        agent_call(
            &agent,
            "hello",
            json!({
                "protocol_version": 1,
                "client": "packetsage-desktop",
                "client_version": env!("CARGO_PKG_VERSION"),
            }),
            TIMEOUT_QUERY,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}
