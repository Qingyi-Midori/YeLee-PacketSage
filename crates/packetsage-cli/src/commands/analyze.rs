//! `packetsage analyze` (§3.2 three streams, §3.3 progress, §4 exit codes).

use std::sync::Arc;

use packetsage_core::pipeline::{now_rfc3339, AnalyzePipeline, CountingSink, EventSink, JsonlSink};
use packetsage_core::{ids, EngineConfig, PacketEventMode, ProgressCounters};
use packetsage_protocol::TaskId;
use packetsage_rules::{RuleEngine, WindowBudget};

use crate::agent_launcher;
use crate::cli::{AnalyzeArgs, Cli};
use crate::commands::{builtin_rules_dir, human_bytes, human_seconds};
use crate::config::{Loaded, Settings};
use crate::errors::{self, Context};
use crate::exit::ExitCode;
use crate::progress::Progress;

/// Runs `analyze`.
pub fn run(args: &AnalyzeArgs, loaded: &Loaded, settings: &Settings, cli: &Cli) -> ExitCode {
    let context = Context::default()
        .with_path(&args.capture)
        .with_url(&settings.db_url)
        .with_llm(Some(&settings.provider), settings.model.as_deref());

    let config = effective_config(args, loaded, settings);

    let task_id = match &args.task_id {
        Some(raw) => match TaskId::parse(raw) {
            Ok(task) => task,
            Err(error) => {
                eprintln!("packetsage: {error}; expected the form task_<26 Crockford characters>");
                return ExitCode::Usage;
            }
        },
        None => match ids::new_task_id() {
            Ok(task) => task,
            Err(message) => {
                eprintln!("packetsage: internal error: {message}");
                return ExitCode::Internal;
            }
        },
    };
    let context = context.with_task(task_id.as_str());
    let started_at = now_rfc3339();

    let sink: Box<dyn EventSink> = if let Some(path) = &args.json {
        match std::fs::File::create(path) {
            Ok(file) => Box::new(JsonlSink::new(
                std::io::BufWriter::new(file),
                args.limit_events,
            )),
            Err(error) => {
                eprintln!("packetsage: cannot write {}: {error}", path.display());
                return ExitCode::CaptureError;
            }
        }
    } else if args.jsonl {
        Box::new(JsonlSink::new(std::io::stdout(), args.limit_events))
    } else {
        Box::new(CountingSink::default())
    };

    // Progress reads atomics only: the JSONL stream cannot change because of
    // it (§3.3 determinism guard, §14.1 T7).
    let counters = Arc::new(ProgressCounters::new());
    let progress = Progress::start(Arc::clone(&counters), "analyze", cli.quiet);

    let rules = build_rules(&config, args);
    let mut pipeline = AnalyzePipeline::new(config, sink).with_progress(counters);
    pipeline.rules = rules;

    let result = match pipeline.run(&args.capture, &task_id, &started_at) {
        Ok(result) => result,
        Err(error) => {
            progress.finish(0);
            eprintln!("{}", errors::render(&error, &context, cli.verbose > 0));
            return ExitCode::from(error);
        }
    };
    progress.finish(result.summary.packets);

    if args.jsonl || args.json.is_some() {
        // Machine readable mode: the event stream is the product.
        if !args.jsonl {
            eprintln!(
                "packetsage: wrote {} events to {}",
                result.summary.packets,
                args.json
                    .as_ref()
                    .map_or_else(String::new, |p| p.display().to_string())
            );
        }
        return ExitCode::Success;
    }

    print_human_summary(&result, &task_id);

    // `--full` needs the task in the database: the agent stage drives a fresh
    // `serve` worker, and ADR-019 cold recovery can only rebuild a task whose
    // row (and source file) still exist.
    let persist_url = args
        .db
        .clone()
        .or_else(|| (args.full && args.report.is_some()).then(|| settings.db_url.clone()));
    if let Some(url) = &persist_url {
        match persist(&result, url) {
            Ok(()) => println!("database        {url} (task stored)"),
            Err(message) => {
                eprintln!(
                    "packetsage: {url}: {message}; if a migration is missing run `packetsage db migrate`"
                );
                return ExitCode::ConfigError;
            }
        }
    }

    if args.full {
        return run_full(args, loaded, settings, &task_id, cli);
    }
    if args.report.is_some() {
        eprintln!(
            "packetsage: --report requires --full; report generation belongs to the agent stage"
        );
        return ExitCode::Unsupported;
    }
    ExitCode::Success
}

/// Applies the §24 precedence to the engine configuration: CLI > env > file > defaults.
fn effective_config(args: &AnalyzeArgs, loaded: &Loaded, settings: &Settings) -> EngineConfig {
    let mut config = base_config(loaded, settings);
    config.emit.limit_events = args.limit_events;
    if args.errors_only {
        config.emit.packet_events = PacketEventMode::ErrorsOnly;
    }
    if args.no_rules {
        config.rules.enabled = false;
    }
    config
}

/// Configuration of the analysis itself, without the CLI-only emit switches.
fn base_config(loaded: &Loaded, settings: &Settings) -> EngineConfig {
    let mut config = loaded.config.clone();
    config.rules.path = settings.rules_dir.clone();
    config
}

/// Analyses a capture into `task_id` quietly and persists it.
///
/// Used by the `chat` launcher: the agent's `serve` worker runs in another
/// process, so the task has to exist in the database before the session starts
/// (ADR-019 cold recovery). Returns the exit code and the rendered message of
/// the first failure instead of printing, so the caller owns the UX.
pub(crate) fn analyze_task(
    capture: &std::path::Path,
    task_id: &TaskId,
    loaded: &Loaded,
    settings: &Settings,
) -> Result<(), (ExitCode, String)> {
    let config = base_config(loaded, settings);
    let rules = build_rules_for(&config, false);
    let sink: Box<dyn EventSink> = Box::new(CountingSink::default());
    let mut pipeline = AnalyzePipeline::new(config, sink);
    pipeline.rules = rules;
    let started_at = now_rfc3339();
    let context = Context::default()
        .with_path(capture)
        .with_url(&settings.db_url)
        .with_task(task_id.as_str());
    let result = pipeline
        .run(capture, task_id, &started_at)
        .map_err(|error| {
            let rendered = errors::render(&error, &context, false);
            (ExitCode::from(error), rendered)
        })?;
    persist(&result, &settings.db_url).map_err(|message| {
        (
            ExitCode::ConfigError,
            format!(
                "packetsage: {}: {message}; if a migration is missing run `packetsage db migrate`",
                settings.db_url
            ),
        )
    })
}

fn persist(result: &packetsage_core::AnalysisResult, url: &str) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let repo = packetsage_storage::SqliteRepo::connect(url)
            .await
            .map_err(|e| e.to_string())?;
        crate::persist::persist(&repo, &result.store)
            .await
            .map_err(|e| e.to_string())
    })
}

fn build_rules(
    config: &EngineConfig,
    args: &AnalyzeArgs,
) -> Option<Box<dyn packetsage_core::pipeline::RuleHook>> {
    build_rules_for(config, args.no_rules)
}

/// Rule hook for one configuration; `no_rules` is the CLI `--no-rules` switch.
fn build_rules_for(
    config: &EngineConfig,
    no_rules: bool,
) -> Option<Box<dyn packetsage_core::pipeline::RuleHook>> {
    if no_rules || !config.rules.enabled {
        return None;
    }
    // The four built-in rules ship inside the binary; a rules directory on disk
    // refines them and may override a same-id file (M3~M6 §3.5).
    let mut engine = RuleEngine::with_builtin_rules(WindowBudget::default());
    let dir = builtin_rules_dir(&config.rules.path);
    if dir.is_dir() {
        match engine.load_dir_lenient(&dir) {
            Ok(report) => {
                for dead in &report.dead_letters {
                    eprintln!(
                        "packetsage: rule {} dead-lettered: {}",
                        dead.path.display(),
                        dead.reason
                    );
                }
            }
            Err(error) => eprintln!("packetsage: cannot read {}: {error}", dir.display()),
        }
    }
    Some(Box::new(engine))
}

fn print_human_summary(result: &packetsage_core::AnalysisResult, task_id: &TaskId) {
    let summary = &result.store.summary;
    println!("task            {task_id}");
    println!("file            {}", summary.source_path);
    println!("sha256          {}", summary.source_sha256);
    println!("format          {}", summary.format);
    println!(
        "packets         {} ({} captured)",
        summary.packets,
        human_bytes(summary.bytes)
    );
    println!(
        "time range      {} .. {} s",
        summary
            .first_ts_ns
            .as_deref()
            .map_or_else(|| "-".to_owned(), |v| human_seconds(v.parse().unwrap_or(0))),
        summary
            .last_ts_ns
            .as_deref()
            .map_or_else(|| "-".to_owned(), |v| human_seconds(v.parse().unwrap_or(0)))
    );
    println!("duration        {:.6} s", summary.duration_s);
    println!("sessions        {}", summary.sessions);
    println!("alerts          {}", summary.alerts);
    println!("decode errors   {}", summary.decode_errors);
    println!("truncated       {}", summary.truncated_packets);
    println!("incomplete TCP  {}", summary.incomplete_sessions);
    println!("dropped sessions {}", summary.dropped_sessions);
    if !summary.protocols.is_empty() {
        let protocols: Vec<String> = summary
            .protocols
            .iter()
            .map(|(name, count)| format!("{name}={count}"))
            .collect();
        println!("protocols       {}", protocols.join(" "));
    }

    let conversations = result.store.sessions.sessions();
    if !conversations.is_empty() {
        println!();
        println!("top conversations (by bytes)");
        let mut rows: Vec<_> = conversations.iter().collect();
        rows.sort_by_key(|entry| std::cmp::Reverse(entry.stats.bytes));
        for entry in rows.iter().take(10) {
            println!(
                "  {} {} {}:{} -> {}:{} packets={} bytes={} syn={} retrans={} state={:?}",
                entry.session_id,
                entry.key.proto.as_str(),
                entry.key.a.ip,
                entry.key.a.port,
                entry.key.b.ip,
                entry.key.b.port,
                entry.stats.packets,
                entry.stats.bytes,
                entry.stats.syn_count,
                entry.stats.retransmission_count,
                entry.stats.state
            );
        }
    }

    if !result.store.alerts.is_empty() {
        println!();
        println!("alerts");
        for alert in result.store.alerts.iter().take(20) {
            println!(
                "  {} {} {} packets {}-{} value={} threshold={}",
                alert.alert_id,
                alert.severity,
                alert.rule_id,
                alert.first_packet,
                alert.last_packet,
                alert.evidence.value,
                alert.evidence.threshold
            );
        }
    }
}

/// The agent stage of `--full`.
///
/// Agent CLI 工程规格书 §1 path A: `run --task-id T` then `report --task-id T`.
/// The launcher only assembles argv; the Python side owns the agent loop.
///
/// §4: a failing agent is a *degraded* path — the analysis already succeeded,
/// so the exit code stays 0 and a WARN explains what was skipped. The single
/// exception is the anti-hallucination hard failure (exit 4), which the agent
/// reports on stderr and which must never be downgraded to a warning.
fn run_full(
    args: &AnalyzeArgs,
    loaded: &Loaded,
    settings: &Settings,
    task_id: &TaskId,
    cli: &Cli,
) -> ExitCode {
    let Some(report) = &args.report else {
        eprintln!("packetsage: --full requires --report <path>");
        return ExitCode::Usage;
    };
    let launcher = match agent_launcher::resolve(&args.python) {
        Ok(launcher) => launcher,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("packetsage: WARN agent stage skipped (report not generated)");
            return ExitCode::Success;
        }
    };
    if let Some(warning) = &launcher.warning {
        eprintln!("{warning}");
    }

    // ① investigation run.
    let mut run_argv = vec![
        "run".to_owned(),
        "--task-id".to_owned(),
        task_id.as_str().to_owned(),
    ];
    run_argv.extend(verbosity_argv(cli));
    if let Some(db) = &args.db {
        run_argv.push("--db".to_owned());
        run_argv.push(db.clone());
    }
    let run_output = match agent_command(&launcher, run_argv, loaded, settings).output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("packetsage: cannot launch the Python agent ({error})");
            eprintln!(
                "packetsage: WARN agent stage skipped; install it with `pip install -e ./agent`"
            );
            return ExitCode::Success;
        }
    };
    print!("{}", String::from_utf8_lossy(&run_output.stdout));
    let run_stderr = String::from_utf8_lossy(&run_output.stderr).into_owned();
    eprint!("{run_stderr}");
    if !run_output.status.success() {
        // Exit 3 is the agent's configuration gate (no provider, no key, bad
        // config). `--full` is *the* agent chain, so it must not be reported as
        // "degraded but fine": the user asked for the investigation.
        if run_output.status.code() == Some(i32::from(ExitCode::ConfigError.code())) {
            eprintln!(
                "packetsage: the agent stage cannot run with the current configuration; \
                 the analysis itself is complete (see the message above)"
            );
            return ExitCode::ConfigError;
        }
        if is_anti_hallucination(&run_stderr) {
            eprintln!("packetsage: anti-hallucination hard failure — the report was not written");
            return ExitCode::Internal;
        }
        eprintln!(
            "packetsage: WARN agent stage failed with {:?}; the analysis itself is complete",
            run_output.status.code()
        );
        return ExitCode::Success;
    }

    // ② report rendering (M5 §5.1 entry point).
    let mut report_argv = vec![
        "report".to_owned(),
        "--task-id".to_owned(),
        task_id.as_str().to_owned(),
        "--report".to_owned(),
        report.to_string_lossy().to_string(),
    ];
    report_argv.extend(verbosity_argv(cli));
    if let Some(db) = &args.db {
        report_argv.push("--db".to_owned());
        report_argv.push(db.clone());
    }
    match agent_command(&launcher, report_argv, loaded, settings).output() {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            eprint!("{stderr}");
            if output.status.success() {
                print!("{}", String::from_utf8_lossy(&output.stdout));
                println!("report          {}", report.display());
                ExitCode::Success
            } else if is_anti_hallucination(&stderr) {
                eprintln!(
                    "packetsage: anti-hallucination hard failure — the report was not written"
                );
                ExitCode::Internal
            } else {
                eprintln!(
                    "packetsage: WARN report stage failed with {:?}; the analysis itself is complete",
                    output.status.code()
                );
                ExitCode::Success
            }
        }
        Err(error) => {
            eprintln!("packetsage: cannot launch the Python agent ({error})");
            eprintln!("packetsage: WARN report stage skipped");
            ExitCode::Success
        }
    }
}

/// Builds the agent command for one stage: argv carries the sub-command and its
/// flags, the shared configuration travels through the inherited environment
/// (Agent CLI §1, 收口 v0.2 §7.1).
fn agent_command(
    launcher: &agent_launcher::Launcher,
    argv: Vec<String>,
    loaded: &Loaded,
    settings: &Settings,
) -> std::process::Command {
    let mut command = std::process::Command::new(&launcher.program);
    command.args(launcher.command_line(&argv));
    if let Ok(engine) = std::env::current_exe() {
        command.env("PACKETSAGE_ENGINE", engine);
    }
    command.env("PACKETSAGE_RULES", settings.rules_dir.as_os_str());
    // The agent's engine recovers the task from this database (ADR-019).
    command.env("PACKETSAGE_STORAGE_URL", settings.db_url.as_str());
    // Empty values would shadow the agent's own `agent/.env` (split by `setup`).
    if !settings.provider.is_empty() {
        command.env("PACKETSAGE_LLM_PROVIDER", settings.provider.as_str());
    }
    if let Some(model) = &settings.model {
        command.env("PACKETSAGE_LLM_MODEL", model.as_str());
    }
    // The discovery chain is inherited, not re-implemented (§7.1).
    if let Some(path) = &loaded.path {
        command.env("PACKETSAGE_CONFIG", path);
    }
    // Keep the agent stage readable when its output is redirected (see chat.rs).
    if std::env::var_os("PYTHONIOENCODING").is_none() {
        command.env("PYTHONIOENCODING", "utf-8");
    }
    if std::env::var_os("PYTHONUTF8").is_none() {
        command.env("PYTHONUTF8", "1");
    }
    command
}

/// `-v`/`-q` of the launcher travel to the agent's own global flags (§7).
pub(crate) fn verbosity_argv(cli: &Cli) -> Vec<String> {
    let mut argv = Vec::new();
    if cli.quiet {
        argv.push("-q".to_owned());
    }
    for _ in 0..cli.verbose {
        argv.push("-v".to_owned());
    }
    argv
}

/// True when the agent reported the M5 anti-hallucination hard failure (§4, C9).
#[must_use]
fn is_anti_hallucination(stderr: &str) -> bool {
    stderr.contains("anti-hallucination")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn args() -> AnalyzeArgs {
        AnalyzeArgs {
            capture: PathBuf::from("samples/x.pcap"),
            task_id: None,
            jsonl: false,
            json: None,
            limit_events: 7,
            rules_dir: None,
            no_rules: true,
            full: false,
            db: None,
            report: None,
            python: "python".to_owned(),
            provider: None,
            model: None,
            errors_only: true,
        }
    }

    #[test]
    fn cli_overrides_win_over_the_file() {
        let loaded = Loaded {
            config: EngineConfig::default(),
            source: crate::config::Source::Builtin,
            path: None,
        };
        let settings = Settings::resolve(None, None, None, None, &loaded.config);
        let config = effective_config(&args(), &loaded, &settings);
        assert_eq!(config.emit.limit_events, 7);
        assert_eq!(config.emit.packet_events, PacketEventMode::ErrorsOnly);
        assert!(!config.rules.enabled);
    }

    #[test]
    fn anti_hallucination_is_detected_only_by_its_keyword() {
        assert!(is_anti_hallucination("anti-hallucination: cited 42"));
        assert!(!is_anti_hallucination("Traceback: KeyError"));
    }

    #[test]
    fn verbose_agents_receive_the_json_flag() {
        // Guards the argv assembly used by `run_full`.
        let mut argv = vec!["run".to_owned()];
        argv.push("--json".to_owned());
        assert!(argv.contains(&"--json".to_owned()));
    }
}
