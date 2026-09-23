//! P11：**外壳 ↔ 打包 sidecar** 的连接验收。
//!
//! 其它外壳用例走的是开发态（`python -m packetsage_agent serve`），"真 exe 能不能
//! 被起起来、能不能把协议跑完"没有回归网。这条用例专门补上：
//!
//! * 引擎用发布档 `target/release/packetsage.exe`；
//! * Agent 用打包好的 onedir sidecar
//!   （`desktop/src-tauri/binaries/agent-sidecar/packetsage-agent.exe`，就是
//!   安装包里的那一份）；
//! * 两个产物都走 `resolve_*`，用的正是安装目录旁的布局规则。
//!
//! 产物还没建出来时这条用例**跳过**（打印一行 SKIP），干净检出也能 `cargo test`；
//! 判据是：
//!
//! ```text
//! python scripts/stage_desktop_sidecars.py
//! cd desktop/src-tauri && cargo test
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use packetsage_desktop_lib::jsonl::FrameSink;
use packetsage_desktop_lib::paths::Paths;
use packetsage_desktop_lib::provider::ProviderConfig;
use packetsage_desktop_lib::sidecars::{
    agent_call, engine_call, probe_provider, resolve_agent, resolve_engine, start_with_specs,
};
use serde_json::{json, Value};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root")
        .to_path_buf()
}

fn wait_for(events: &Mutex<Vec<Value>>, name: &str, timeout: Duration) -> Option<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let found = events.lock().ok().and_then(|seen| {
            seen.iter()
                .find(|event| event.get("event").and_then(Value::as_str) == Some(name))
                .cloned()
        });
        if found.is_some() {
            return found;
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn packaged_engine_and_agent_speak_the_frozen_protocol() {
    let root = repo_root();
    let engine_dir = root.join("target").join("release");
    let binaries = root.join("desktop").join("src-tauri").join("binaries");
    let agent_exe = binaries.join("agent-sidecar").join("packetsage-agent.exe");
    let engine_exe = engine_dir.join("packetsage.exe");

    if !agent_exe.is_file() || !engine_exe.is_file() {
        println!(
            "SKIP packaged_engine_and_agent_speak_the_frozen_protocol: missing {agent_exe:?} or \
             {engine_exe:?} (run `python scripts/stage_desktop_sidecars.py` first)"
        );
        return;
    }

    // mock provider：确定性回放，不联网、不用真 key。
    std::env::set_var("PACKETSAGE_LLM_PROVIDER", "mock");
    let scratch = std::env::temp_dir().join("packetsage-packaged-tests");
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let empty_env = scratch.join("empty.env");
    std::fs::write(&empty_env, "").expect("empty env file");
    std::env::set_var("PACKETSAGE_ENV_FILE", &empty_env);

    let engine_spec = resolve_engine(Some(&engine_dir)).expect("resolve the packaged engine");
    let agent_spec = resolve_agent(Some(&binaries)).expect("resolve the packaged agent");
    // 明确断言"这次连的是真 exe"，而不是悄悄退回 `python -m`。
    assert!(
        agent_spec.program.ends_with("packetsage-agent.exe"),
        "agent 解析成了 {:?}（{:?}）",
        agent_spec.program,
        agent_spec.describe
    );
    assert!(
        engine_spec.program.ends_with("packetsage.exe"),
        "engine 解析成了 {:?}（{:?}）",
        engine_spec.program,
        engine_spec.describe
    );

    let data = scratch.join("packaged");
    std::fs::create_dir_all(&data).expect("packaged scratch dir");
    let paths = Paths {
        data: data.clone(),
        logs: data.clone(),
        reports: data.clone(),
        db_url: format!("sqlite://{}", data.join("packaged.db").display()),
        rules: Some(root.join("rules")),
    };

    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let sink: FrameSink = Arc::new(move |frame: Value| {
        if let Ok(mut seen) = sink_events.lock() {
            seen.push(frame);
        }
    });

    // provider 环境留空：这条用例考的是"真 exe 能不能把协议跑完"，
    // 而 provider 走的是 mock（`PACKETSAGE_LLM_PROVIDER` 由测试进程继承）。
    let sidecars = start_with_specs(&paths, engine_spec, agent_spec, &[], Some(sink), None, None)
        .expect("packaged sidecars start");

    // 1) 握手：打包态的 Agent 报的是同一套协议。
    let welcome = agent_call(
        &sidecars.agent,
        "hello",
        json!({"protocol_version": 1, "client": "packetsage-desktop", "client_version": "test"}),
        Duration::from_secs(60),
    )
    .expect("packaged handshake");
    assert_eq!(welcome["protocol_version"], 1);
    assert_eq!(welcome["providers"]["configured"], true);

    // 2) 打包态的引擎：分析 → 拿 task_id。
    let sample = root.join("samples").join("synth-mixed.pcap");
    let analyzed = engine_call(
        &sidecars.engine,
        "analyze_file",
        json!({"path": sample.display().to_string()}),
        Duration::from_secs(120),
    )
    .expect("packaged analyze_file");
    let task_id = analyzed["task_id"].as_str().expect("task id").to_owned();

    // 3) 打包态的 Agent 跑一次完整 run（mock provider → 确定性回放）。
    let ack = agent_call(
        &sidecars.agent,
        "run",
        json!({"task_id": task_id, "goal": "打包态连接自检", "mode": "run"}),
        Duration::from_secs(60),
    )
    .expect("packaged run ack");
    let run_id = ack["run_id"].as_str().expect("run id").to_owned();
    let finished =
        wait_for(&events, "run_finished", Duration::from_secs(180)).expect("packaged run_finished");
    assert_eq!(finished["run_id"], run_id.as_str());
    assert_eq!(finished["data"]["status"], "completed");
    assert!(!finished["data"]["findings"]
        .as_array()
        .expect("findings")
        .is_empty());
    // 3b) U11：token 级流式在**打包态**也要有（旧 sidecar 打进来就不会有）。
    let deltas: Vec<Value> = events
        .lock()
        .expect("events")
        .iter()
        .filter(|event| event["event"] == "llm_delta")
        .cloned()
        .collect();
    assert!(
        !deltas.is_empty(),
        "打包态 agent 没发 llm_delta：sidecar 是旧的（要重打 packetsage-agent.exe）"
    );
    assert!(deltas.iter().all(|event| event["data"]["text"]
        .as_str()
        .is_some_and(|text| !text.is_empty())));

    // 4) 停：`shutdown` 是命令表里的一条（P6），打包态也要回 ack 而不是协议错。
    let stopped = agent_call(
        &sidecars.agent,
        "shutdown",
        json!({}),
        Duration::from_secs(30),
    )
    .expect("packaged shutdown ack");
    assert!(stopped.is_object());

    sidecars.engine.close();
    sidecars.agent.close();
}

/// U7（§7.4 第 2 步）：向导的"校验"走的就是这一条——打包态 Agent 的
/// `setup --verify --print`，**不新增协议、不复制探测逻辑**。
#[test]
fn packaged_agent_verifies_provider_configurations() {
    let root = repo_root();
    let binaries = root.join("desktop").join("src-tauri").join("binaries");
    if !binaries.join("agent-sidecar").join("packetsage-agent.exe").is_file() {
        println!("SKIP packaged_agent_verifies_provider_configurations: sidecar not staged");
        return;
    }
    let scratch = std::env::temp_dir().join("packetsage-packaged-tests");
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let env_file = scratch.join("empty.env");
    std::fs::write(&env_file, "").expect("empty env file");

    let spec = resolve_agent(Some(&binaries)).expect("resolve the packaged agent");

    // 演示模式：不需要 key、不联网，必须判 ok。
    let (ok, detail) = probe_provider(&spec, &ProviderConfig::defaults("mock"), None, &env_file)
        .expect("probe mock");
    assert!(ok, "mock 不该失败：{detail}");
    assert!(detail.contains("mock"), "{detail}");

    // 连不上的本地端点：必须判失败，而且 detail 要说清是哪一步失败。
    let unreachable = ProviderConfig {
        provider: "local".to_owned(),
        model: "local-model".to_owned(),
        base_url: "http://127.0.0.1:1/v1".to_owned(),
        thinking: String::new(),
    };
    let (ok, detail) = probe_provider(&spec, &unreachable, None, &env_file).expect("probe local");
    assert!(!ok, "127.0.0.1:1 不该连得上");
    assert!(
        detail.contains("无法连接") || detail.contains("HTTP"),
        "失败原因要人读得懂：{detail}"
    );

    // 需要 key 的 provider 不给 key：当场拒绝（`setup` 非交互模式本来就这么做）。
    let (ok, detail) =
        probe_provider(&spec, &ProviderConfig::defaults("deepseek"), None, &env_file)
            .expect("probe deepseek");
    assert!(!ok);
    assert!(detail.contains("API key"), "{detail}");

    // 给了 key（哪怕是假的）就该真的去连端点：这里指到一个关着的端口，
    // 判据是"不再是缺 key 的拒绝"，证明 key 确实经环境变量送到了子进程。
    let keyed = ProviderConfig {
        provider: "deepseek".to_owned(),
        model: "deepseek-chat".to_owned(),
        base_url: "http://127.0.0.1:1/v1".to_owned(),
        thinking: String::new(),
    };
    let (ok, detail) = probe_provider(&spec, &keyed, Some("sk-not-a-real-key".to_owned()), &env_file)
        .expect("probe keyed deepseek");
    assert!(!ok);
    assert!(!detail.contains("需要 API key"), "{detail}");
    assert!(!detail.contains("没有 API key"), "{detail}");
}

/// U7 的主干：**没配 provider → 握手说未配置；注入 provider 后重启 Agent → 说配置好了**。
///
/// 这正是向导"保存并重启 Agent"那一步的效果（外壳只是把 `provider.json` +
/// 凭据管理器里的 key 拼成环境变量，见 `sidecars::spawn_agent`）。
#[test]
fn u7_provider_env_flips_the_handshake() {
    let root = repo_root();
    let engine_dir = root.join("target").join("release");
    let binaries = root.join("desktop").join("src-tauri").join("binaries");
    if !binaries.join("agent-sidecar").join("packetsage-agent.exe").is_file()
        || !engine_dir.join("packetsage.exe").is_file()
    {
        println!("SKIP u7_provider_env_flips_the_handshake: sidecars not staged");
        return;
    }

    let scratch = std::env::temp_dir().join("packetsage-packaged-tests").join("u7");
    std::fs::create_dir_all(&scratch).expect("scratch dir");
    let paths = Paths {
        data: scratch.clone(),
        logs: scratch.clone(),
        reports: scratch.clone(),
        db_url: format!("sqlite://{}", scratch.join("u7.db").display()),
        rules: Some(root.join("rules")),
    };
    let engine_spec = resolve_engine(Some(&engine_dir)).expect("engine spec");
    let agent_spec = resolve_agent(Some(&binaries)).expect("agent spec");
    let engine = packetsage_desktop_lib::jsonl::JsonlChild::spawn(
        packetsage_desktop_lib::jsonl::SpawnOptions {
            label: "engine",
            program: &engine_spec.program,
            args: &engine_spec.args,
            envs: &[(
                "PACKETSAGE_STORAGE_URL".to_owned(),
                paths.db_url.clone(),
            )],
            cwd: None,
            log_path: &paths.engine_log(),
            sink: None,
            on_exit: None,
        },
    )
    .expect("engine child");

    let hello = json!({"protocol_version": 1, "client": "packetsage-desktop", "client_version": "test"});

    // 1) 没有 provider：握手必须说未配置（S67 挡住 run 的依据）。
    let unconfigured = packetsage_desktop_lib::sidecars::spawn_agent(
        &paths,
        &agent_spec,
        &engine_spec.program,
        // 显式置空：本进程可能被别的用例设了 PACKETSAGE_LLM_PROVIDER。
        &[("PACKETSAGE_LLM_PROVIDER".to_owned(), String::new())],
        None,
        None,
    )
    .expect("agent without a provider");
    let welcome = agent_call(&unconfigured, "hello", hello.clone(), Duration::from_secs(60))
        .expect("handshake without provider");
    assert_eq!(welcome["providers"]["configured"], false);
    unconfigured.close();

    // 2) 注入 provider（向导保存后重启 Agent 的效果）：必须说配置好了。
    let configured = packetsage_desktop_lib::sidecars::spawn_agent(
        &paths,
        &agent_spec,
        &engine_spec.program,
        &[
            ("PACKETSAGE_LLM_PROVIDER".to_owned(), "mock".to_owned()),
            ("PACKETSAGE_LLM_MODEL".to_owned(), "mock".to_owned()),
        ],
        None,
        None,
    )
    .expect("agent with a provider");
    let welcome = agent_call(&configured, "hello", hello, Duration::from_secs(60))
        .expect("handshake with provider");
    assert_eq!(welcome["providers"]["configured"], true);
    assert_eq!(welcome["providers"]["kind"], "mock");
    configured.close();
    engine.close();
}
