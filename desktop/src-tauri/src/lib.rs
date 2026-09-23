//! PacketSage 桌面外壳（《GUI 工程规格书 v0.2》§2.2）。
//!
//! 外壳只做四件事：**持有两个 sidecar、转发事件、暴露命令、收拾进程**。
//! 界面逻辑全在前端；证据路径（通道 A）直连引擎，不经过 Python。

mod commands;
// `jsonl` / `paths` / `sidecars` are public so the sidecar-level acceptance tests
// (`tests/sidecar_smoke.rs`) can drive the real protocol without a window.
pub mod jsonl;
pub mod paths;
// U7：provider 的非密钥部分 + Windows 凭据管理器。
pub mod provider;
pub mod secrets;
pub mod sidecars;

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use commands::AppState;
use paths::Paths;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let install_dir = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf));
            let paths = Paths::resolve(install_dir.as_deref());

            // 事件最后落在这里：agent 的每条事件原样转给前端（通道 B → 通道 C）。
            // P8：中间隔一层**有界队列**——前端卡住时丢最旧的帧并合成一条
            // `events_dropped`，反压不会顺着 Python 的 stdout 写拖住 run。
            let events: Arc<Mutex<Option<tauri::ipc::Channel<Value>>>> = Arc::new(Mutex::new(None));
            let sink_events = Arc::clone(&events);
            let event_queue = jsonl::EventQueue::start(
                jsonl::EVENT_QUEUE_CAPACITY,
                Arc::new(move |frame: Value| {
                    if let Ok(guard) = sink_events.lock() {
                        if let Some(channel) = guard.as_ref() {
                            let _ = channel.send(frame);
                        }
                    }
                }),
            );
            // P9：run 的状态只有 `run_agent` 与 `run_finished` 两处会写。
            let run_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let sink_run = Arc::clone(&run_slot);
            let sink_queue = Arc::clone(&event_queue);
            let sink: jsonl::FrameSink = Arc::new(move |frame: Value| {
                // 一次 run 结束（成功、降级或失败）就清掉 run id：`cancel`
                // 之后再也不会拿一个过期 id 发空操作。
                if frame.get("event").and_then(Value::as_str) == Some("run_finished") {
                    if let Ok(mut slot) = sink_run.lock() {
                        *slot = None;
                    }
                }
                sink_queue.push(frame);
            });
            let exit_handle = handle.clone();
            let exit_run = Arc::clone(&run_slot);
            let exit_sink: jsonl::ExitSink = Arc::new(move |code: Option<i32>, label: String| {
                // sidecar 没了，run 也就不可能在跑；同样把 run id 清掉。
                if let Ok(mut slot) = exit_run.lock() {
                    *slot = None;
                }
                let _ = exit_handle.emit("sidecar-exited", (label, code));
            });

            // U7：provider 的环境变量在启动时注入（key 来自凭据管理器）。
            let provider_env = sidecars::provider_env(&paths);
            let keep_sink = Arc::clone(&sink);
            let keep_exit = Arc::clone(&exit_sink);

            // P5：sidecar 起不来时**不再让 `setup` 上抛**（旧行为会让整条
            // `build().expect(...)` panic，用户看到的是"应用起不来"而不是
            //"安装损坏"）。外壳照常起，界面拿到 start_error 后显示可读提示。
            let started = sidecars::resolve_engine(install_dir.as_deref()).and_then(|engine| {
                sidecars::resolve_agent(install_dir.as_deref()).and_then(|agent| {
                    sidecars::start_with_specs(
                        &paths,
                        engine,
                        agent,
                        &provider_env,
                        Some(sink),
                        Some(Arc::clone(&exit_sink)),
                        Some(exit_sink),
                    )
                })
            });
            let (sidecars, start_error) = match started {
                Ok(sidecars) => (Some(sidecars), None),
                Err(error) => {
                    eprintln!("packetsage-desktop: sidecars are not running: {error}");
                    (None, Some(error))
                }
            };

            app.manage(AppState::new(
                paths,
                sidecars,
                start_error,
                event_queue,
                run_slot,
                events,
                keep_sink,
                keep_exit,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::handshake,
            commands::doctor,
            commands::list_tasks,
            commands::analyze,
            commands::task_overview,
            commands::run_agent,
            commands::cancel,
            commands::agent_status,
            commands::restart_agent,
            commands::report,
            commands::read_text_file,
            commands::sidecar_diagnostics,
            commands::provider_status,
            commands::provider_verify,
            commands::provider_models,
            commands::provider_save,
            commands::provider_clear,
        ])
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::CloseRequested { .. }) {
                if let Some(state) = window.try_state::<AppState>() {
                    commands::shutdown(&state);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building the PacketSage shell")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    commands::shutdown(&state);
                }
            }
        });
}
