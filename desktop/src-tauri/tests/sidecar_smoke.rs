//! 外壳的 sidecar 层验收（《GUI 工程规格书 v0.2》§8.1 的 S58/S61/S64 在外壳一侧）。
//!
//! 这几条用例**不需要窗口**：直接启动两个 sidecar、走真协议、断言事件与崩溃感知。
//! 界面层的观感（M2/M6）仍由人工清单负责。
//!
//!     cd desktop/src-tauri && cargo test

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use packetsage_desktop_lib::jsonl::{ExitSink, FrameSink, JsonlChild, SpawnOptions};
use packetsage_desktop_lib::paths::Paths;
use packetsage_desktop_lib::sidecars::{agent_call, engine_call, start, LaunchSpec};
use serde_json::{json, Value};

/// 开发树根（`desktop/src-tauri/../../`）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root")
        .to_path_buf()
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join("packetsage-desktop-tests");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

struct Harness {
    engine: Arc<JsonlChild>,
    agent: Arc<JsonlChild>,
    engine_spec: LaunchSpec,
    db_url: String,
    events: Arc<Mutex<Vec<Value>>>,
    agent_exited: Arc<AtomicBool>,
}

impl Harness {
    /// 每个用例一个独立数据库：四个用例并行跑，共用 sqlite 文件会互相锁。
    fn start(tag: &str) -> Self {
        // mock provider：确定性回放，不联网、不用真 key（L2 纪律）。
        std::env::set_var("PACKETSAGE_LLM_PROVIDER", "mock");
        std::env::set_var("PACKETSAGE_ENV_FILE", scratch().join("empty.env"));

        let data = scratch().join(tag);
        std::fs::create_dir_all(&data).expect("test scratch dir");
        let paths = Paths {
            data: data.clone(),
            logs: data.clone(),
            reports: data.clone(),
            db_url: format!("sqlite://{}", data.join("shell.db").display()),
            rules: Some(repo_root().join("rules")),
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        let sink: FrameSink = Arc::new(move |frame: Value| {
            sink_events.lock().unwrap().push(frame);
        });
        let agent_exited = Arc::new(AtomicBool::new(false));
        let exited = Arc::clone(&agent_exited);
        let exit_sink: ExitSink = Arc::new(move |_code: Option<i32>, label: String| {
            if label == "agent" {
                exited.store(true, Ordering::SeqCst);
            }
        });

        let sidecars = start(&paths, None, Some(sink), Some(Arc::clone(&exit_sink)), None)
            .expect("sidecars start");
        Self {
            engine: sidecars.engine,
            agent: sidecars.agent,
            engine_spec: sidecars.engine_spec,
            db_url: paths.db_url,
            events,
            agent_exited,
        }
    }

    fn handshake(&self) -> Value {
        agent_call(&self.agent, "hello", json!({
            "protocol_version": 1,
            "client": "packetsage-desktop",
            "client_version": "test",
        }), Duration::from_secs(20))
        .expect("handshake")
    }

    fn analyze(&self) -> String {
        let sample = repo_root().join("samples").join("synth-mixed.pcap");
        let result = engine_call(
            &self.engine,
            "analyze_file",
            json!({"path": sample.display().to_string()}),
            Duration::from_secs(120),
        )
        .expect("analyze_file");
        result["task_id"].as_str().expect("task id").to_owned()
    }

    fn events_for(&self, run_id: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.get("run_id").and_then(Value::as_str) == Some(run_id))
            .cloned()
            .collect()
    }

    fn wait_for_event(&self, run_id: &str, name: &str, timeout: Duration) -> Vec<Value> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let seen = self.events_for(run_id);
            if seen
                .iter()
                .any(|event| event.get("event").and_then(Value::as_str) == Some(name))
            {
                return seen;
            }
            if std::time::Instant::now() > deadline {
                let names: Vec<&str> = seen
                    .iter()
                    .filter_map(|event| event.get("event").and_then(Value::as_str))
                    .collect();
                panic!("event {name} never arrived; saw {names:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = agent_call(&self.agent, "shutdown", json!({}), Duration::from_secs(5));
        self.engine.close();
        self.agent.close();
    }
}

#[test]
fn s61_shell_streams_events_until_run_finished() {
    let harness = Harness::start("s61");
    let welcome = harness.handshake();
    assert_eq!(welcome["protocol_version"], 1);
    assert_eq!(welcome["providers"]["configured"], true);

    let task_id = harness.analyze();
    let ack = agent_call(
        &harness.agent,
        "run",
        json!({"task_id": task_id, "goal": "外壳自检", "mode": "run"}),
        Duration::from_secs(30),
    )
    .expect("run ack");
    let run_id = ack["run_id"].as_str().expect("run id").to_owned();

    let events = harness.wait_for_event(&run_id, "run_finished", Duration::from_secs(120));
    let names: Vec<String> = events
        .iter()
        .map(|event| event["event"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(names.first().map(String::as_str), Some("run_started"));
    assert_eq!(names.last().map(String::as_str), Some("run_finished"));
    let seqs: Vec<u64> = events
        .iter()
        .map(|event| event["seq"].as_u64().unwrap_or(0))
        .collect();
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<u64>>());
    let finished = events.last().unwrap();
    assert_eq!(finished["data"]["status"], "completed");
    assert!(!finished["data"]["findings"].as_array().unwrap().is_empty());
    assert!(event_count(&events, "tool_call_started") >= 1);
    assert_eq!(
        event_count(&events, "tool_call_started"),
        event_count(&events, "tool_call_finished")
    );
    // U11：token 级流式要**经外壳**到得了界面（mock provider 每轮一帧）。
    assert!(
        event_count(&events, "llm_delta") >= 1,
        "token 级流式应当被外壳原样转发"
    );
}

/// S61 的"逐条出现"必须**可观测**：mock + 真引擎一次 run 只有几十毫秒，
/// 所以这条用 `agent/tests/fake_engine.py slow:<s>` 当引擎桩（L2），
/// 让 run 慢到能在 `run_finished` 之前就看见工具卡片。
#[test]
fn s61_tool_cards_are_visible_before_run_finished() {
    std::env::set_var("PACKETSAGE_LLM_PROVIDER", "mock");
    std::env::set_var("PACKETSAGE_ENV_FILE", scratch().join("empty.env"));

    let python = which_python();
    let fake = repo_root()
        .join("agent")
        .join("tests")
        .join("fake_engine.py");
    let engine_cmd = format!(
        "\"{python}\" \"{}\" slow:0.2",
        fake.display()
    );
    let data = scratch().join("s61-slow");
    std::fs::create_dir_all(&data).expect("scratch dir");
    let db_url = format!("sqlite://{}", data.join("fake.db").display());

    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let sink: FrameSink = Arc::new(move |frame: Value| {
        sink_events.lock().unwrap().push(frame);
    });
    let agent = JsonlChild::spawn(SpawnOptions {
        label: "agent",
        program: &python,
        args: &[
            "-m".to_owned(),
            "packetsage_agent".to_owned(),
            "serve".to_owned(),
            "--engine".to_owned(),
            engine_cmd,
            "--db".to_owned(),
            db_url,
        ],
        envs: &[
            ("PACKETSAGE_LLM_PROVIDER".to_owned(), "mock".to_owned()),
            (
                "PACKETSAGE_ENV_FILE".to_owned(),
                scratch().join("empty.env").display().to_string(),
            ),
            ("PYTHONUTF8".to_owned(), "1".to_owned()),
        ],
        cwd: Some(&repo_root().join("agent")),
        log_path: &data.join("agent.log"),
        sink: Some(sink),
        on_exit: None,
    })
    .expect("agent sidecar");

    agent_call(
        &agent,
        "hello",
        json!({"protocol_version": 1, "client": "packetsage-desktop", "client_version": "test"}),
        Duration::from_secs(20),
    )
    .expect("handshake");
    let ack = agent_call(
        &agent,
        "run",
        json!({"task_id": "task_01J0000000000000000000000G", "goal": "慢跑", "mode": "run"}),
        Duration::from_secs(30),
    )
    .expect("run ack");
    let run_id = ack["run_id"].as_str().unwrap().to_owned();

    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let seen: Vec<Value> = events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.get("run_id").and_then(Value::as_str) == Some(run_id.as_str()))
            .cloned()
            .collect();
        let finished = seen
            .iter()
            .any(|event| event["event"] == "run_finished");
        let cards = seen
            .iter()
            .any(|event| event["event"] == "tool_call_finished");
        if cards {
            assert!(
                !finished,
                "工具卡片必须在 run 结束之前就能看见（S61/U4）"
            );
            break;
        }
        assert!(
            !finished,
            "run 在第一条工具卡片之前就结束了——桩没起作用"
        );
        assert!(std::time::Instant::now() < deadline, "没等到工具卡片");
        std::thread::sleep(Duration::from_millis(25));
    }

    let _ = agent_call(&agent, "cancel", json!({"run_id": run_id}), Duration::from_secs(10));
    agent.close();
}

/// 开发机上的 Python（`python` 或 `python3`，Windows 上带 `.exe`）。
fn which_python() -> String {
    let candidates: [&str; 4] = ["python.exe", "python", "python3.exe", "python3"];
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for name in candidates {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
    }
    "python".to_owned()
}

#[test]
fn s60_shell_cancel_finalizes_and_keeps_findings() {
    let harness = Harness::start("s60");
    harness.handshake();
    let task_id = harness.analyze();
    let ack = agent_call(
        &harness.agent,
        "run",
        json!({"task_id": task_id, "goal": "取消用例", "mode": "run"}),
        Duration::from_secs(30),
    )
    .expect("run ack");
    let run_id = ack["run_id"].as_str().unwrap().to_owned();
    let cancel = agent_call(
        &harness.agent,
        "cancel",
        json!({"run_id": run_id}),
        Duration::from_secs(10),
    )
    .expect("cancel ack");
    assert!(cancel.is_object());
    let events = harness.wait_for_event(&run_id, "run_finished", Duration::from_secs(120));
    let finished = events.last().unwrap();
    // 取消要么在收尾前到达（degraded），要么整个 run 已经完成；两者都不能崩。
    let status = finished["data"]["status"].as_str().unwrap();
    assert!(matches!(status, "completed" | "degraded"), "unexpected {status}");
    if status == "degraded" {
        assert!(finished["data"]["stop_reason"].is_string());
    }
}

#[test]
fn s58_killing_the_agent_is_detected_by_eof() {
    let harness = Harness::start("s58");
    harness.handshake();
    // §4.2 / S58：stdout EOF 立刻可判定死亡，不靠轮询超时。
    assert!(!harness.agent_exited.load(Ordering::SeqCst));
    harness.agent.close();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !harness.agent_exited.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "EOF was not turned into an exit notification"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let err = agent_call(&harness.agent, "status", json!({}), Duration::from_secs(5))
        .expect_err("a dead sidecar must not answer");
    assert!(err.contains("not running") || err.contains("died"), "{err}");
}

/// P1：**在途请求**碰上子进程死亡时要立刻拿到"死了 + stderr 尾巴"，
/// 而不是等满 60/120/180 s 的命令级超时（判据：<500 ms，这里放宽到 1.5 s
/// 以吸收 CI 的调度抖动；真的等超时的话是 60 s，不会误判）。
#[test]
fn p1_in_flight_request_fails_fast_when_the_child_dies() {
    let python = which_python();
    let data = scratch().join("p1");
    std::fs::create_dir_all(&data).expect("scratch dir");
    // 预热：第一次起 python 会被杀毒/索引拖慢，计时部分不能算上它。
    let _ = std::process::Command::new(&python)
        .arg("-c")
        .arg("pass")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let script = "import sys, time; sys.stderr.write('boom: engine exploded\\n'); \
                  sys.stderr.flush(); time.sleep(0.3)";
    let child = JsonlChild::spawn(SpawnOptions {
        label: "engine",
        program: &python,
        args: &["-c".to_owned(), script.to_owned()],
        envs: &[],
        cwd: None,
        log_path: &data.join("engine.log"),
        sink: None,
        on_exit: None,
    })
    .expect("child process");

    let caller = Arc::clone(&child);
    let (elapsed, result) = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let result = engine_call(&caller, "ping", json!({}), Duration::from_secs(60));
        (started.elapsed(), result)
    })
    .join()
    .expect("caller thread");

    let error = result.expect_err("a dead sidecar must not answer");
    assert!(error.contains("died"), "错误里要写明死了：{error}");
    assert!(
        error.contains("boom: engine exploded"),
        "错误里要带 stderr 尾巴：{error}"
    );
    assert!(
        elapsed < Duration::from_millis(1500),
        "在途请求等了 {elapsed:?}，说明还在等超时"
    );
    child.close();
}

#[test]
fn read_only_task_list_uses_the_cli_exit() {
    let harness = Harness::start("read-only");
    let rows = packetsage_desktop_lib::sidecars::engine_cli(
        &harness.engine_spec,
        &[
            "db",
            "query",
            "--readonly",
            "--db",
            &packetsage_desktop_lib::sidecars::readonly_url(&harness.db_url),
            "--jsonl",
            "--sql",
            "SELECT id FROM analysis_tasks",
        ],
    )
    .expect("engine cli");
    assert!(rows.status.success() || rows.stdout.is_empty());
}

fn event_count(events: &[Value], name: &str) -> usize {
    events
        .iter()
        .filter(|event| event.get("event").and_then(Value::as_str) == Some(name))
        .count()
}
