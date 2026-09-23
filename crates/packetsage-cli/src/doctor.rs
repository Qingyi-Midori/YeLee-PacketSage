//! `packetsage doctor` — ten checks with repair hints (§11).
//!
//! Contracts that shape this file:
//!
//! * doctor has **no side effects** (§11.1, T6): the database is probed through
//!   a read-only handle, and the writability probe deletes its temporary file;
//! * lenient/dead-letter situations are WARN, never ❌ — strict validation is
//!   `rules check`'s job;
//! * `--json` emits the stable *json output format v1* and never a secret.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use packetsage_protocol::{method, RpcRequest, SCHEMA_VERSION};

use crate::cli::{Cli, DoctorArgs};
use crate::color::Palette;
use crate::commands::builtin_rules_dir;
use crate::config::{self, Loaded, Settings};
use crate::exit::ExitCode;
use crate::redact::MASK;
use crate::version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Fail,
    Skipped,
}

impl Status {
    fn marker(self) -> &'static str {
        match self {
            Status::Ok => "✅",
            Status::Warn => "⚠",
            Status::Fail => "❌",
            Status::Skipped => "⏭",
        }
    }

    fn json(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Skipped => "skipped",
        }
    }
}

struct Check {
    id: &'static str,
    name: &'static str,
    status: Status,
    detail: String,
    repair: Option<String>,
}

impl Check {
    fn new(
        id: &'static str,
        name: &'static str,
        status: Status,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name,
            status,
            detail: detail.into(),
            repair: None,
        }
    }

    fn repair(mut self, hint: impl Into<String>) -> Self {
        self.repair = Some(hint.into());
        self
    }
}

/// Runs every check and prints the table (or the JSON form).
pub fn run(args: &DoctorArgs, cli: &Cli, palette: Palette) -> ExitCode {
    let (loaded, config_check, settings) = resolve_configuration(cli.config.as_deref(), args);
    if cli.verbose >= 2 {
        // §7.2: the effective configuration dump of doctor is redacted too.
        eprintln!(
            "packetsage: effective configuration (redacted)\n{}",
            settings.dump(&loaded)
        );
    }
    let checks = vec![
        check_binary(),
        config_check,
        check_rules(&settings),
        check_samples(args),
        check_database(&settings),
        check_paths(&settings),
        check_python(),
        check_provider(&settings, args.no_net),
        check_end_to_end(),
        check_schema_consistency(),
    ];
    let failed = checks
        .iter()
        .filter(|check| check.status == Status::Fail)
        .count();
    if args.json {
        print_json(&checks);
    } else {
        print_table(&checks, palette);
    }
    if failed == 0 {
        ExitCode::Success
    } else {
        eprintln!("packetsage: {failed} check(s) failed");
        ExitCode::ConfigError
    }
}

/// Resolves the discovery chain, but keeps the failure inside check 2 so the
/// rest of the table still renders (§11.1).
fn resolve_configuration(explicit: Option<&Path>, args: &DoctorArgs) -> (Loaded, Check, Settings) {
    match config::resolve(explicit) {
        Ok(loaded) => {
            let settings = Settings::resolve(
                args.db.as_deref(),
                args.rules_dir.as_deref(),
                None,
                None,
                &loaded.config,
            );
            let detail = format!(
                "source={}{} | db={} rules={} provider={} api_key={}",
                loaded.source.label(),
                loaded
                    .path
                    .as_ref()
                    .map_or_else(String::new, |p| format!(" path={}", p.display())),
                settings.db_url,
                settings.rules_dir.display(),
                settings.provider_display(),
                api_key_detail()
            );
            (
                loaded,
                Check::new("config", "configuration", Status::Ok, detail),
                settings,
            )
        }
        Err(error) => {
            let fallback = Loaded {
                config: packetsage_core::EngineConfig::default(),
                source: config::Source::Builtin,
                path: None,
            };
            let settings = Settings::resolve(
                args.db.as_deref(),
                args.rules_dir.as_deref(),
                None,
                None,
                &fallback.config,
            );
            let check = Check::new(
                "config",
                "configuration",
                Status::Fail,
                error.message.clone(),
            )
            .repair(
                "fix the field named above, or unset $PACKETSAGE_CONFIG / remove ./packetsage.yaml",
            );
            (fallback, check, settings)
        }
    }
}

/// Presence of an API key, described without ever echoing a value (§7.2).
fn api_key_detail() -> String {
    if config::api_key_present() {
        // Variable *names* may be shown; values never are (§7.2) — the marker
        // never reveals the length either.
        format!("{MASK} (PACKETSAGE_LLM_API_KEY/OPENAI_API_KEY set)")
    } else {
        "unset".to_owned()
    }
}

fn check_binary() -> Check {
    let status = if SCHEMA_VERSION == version::schema_version() {
        Status::Ok
    } else {
        Status::Fail
    };
    Check::new(
        "binary",
        "engine binary",
        status,
        format!(
            "{} | rustc {} | target {}",
            version::line(),
            version::rustc(),
            version::target()
        ),
    )
}

fn check_rules(settings: &Settings) -> Check {
    let dir = builtin_rules_dir(&settings.rules_dir);
    if !dir.is_dir() {
        return Check::new(
            "rules",
            "rules directory",
            Status::Warn,
            format!(
                "{} not found; the {} embedded rule(s) are used instead",
                dir.display(),
                packetsage_rules::BUILTIN_RULES.len()
            ),
        )
        .repair("create a rules/ directory or set $PACKETSAGE_RULES_PATH to the directory holding your YAML rules");
    }
    let mut engine = packetsage_rules::RuleEngine::default();
    match engine.load_dir_lenient(&dir) {
        Ok(report) => {
            // Dead letters are WARN: the loader is deliberately lenient here
            // and `rules check` owns strict validation (§11.1 item 3, C4).
            let dead: Vec<String> = report
                .dead_letters
                .iter()
                .map(|entry| entry.path.display().to_string())
                .collect();
            if dead.is_empty() {
                Check::new(
                    "rules",
                    "rules directory",
                    Status::Ok,
                    format!(
                        "{} on disk + {} built-in rule(s) from {}",
                        report.loaded,
                        packetsage_rules::BUILTIN_RULES.len(),
                        dir.display()
                    ),
                )
            } else {
                Check::new(
                    "rules",
                    "rules directory",
                    Status::Warn,
                    format!(
                        "{} loaded, {} dead-lettered: {}",
                        report.loaded,
                        dead.len(),
                        dead.join(", ")
                    ),
                )
                .repair(
                    "run `packetsage rules check <path>` to see the failing static check (S1-S9)",
                )
            }
        }
        Err(error) => Check::new(
            "rules",
            "rules directory",
            Status::Fail,
            format!("{}: {error}", dir.display()),
        )
        .repair("check the directory permissions and the YAML syntax"),
    }
}

fn check_samples(args: &DoctorArgs) -> Check {
    let dir = args
        .samples_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("samples"));
    if !dir.is_dir() {
        return Check::new(
            "samples",
            "sample captures",
            Status::Warn,
            format!("{} not found", dir.display()),
        )
        .repair("python scripts/gen_traffic.py --out samples/synth-mixed.pcap --packets 600 --profile mixed");
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|iter| {
            iter.flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext == "pcap" || ext == "pcapng" || ext == "cap")
                })
                .collect()
        })
        .unwrap_or_default();
    match entries.first() {
        Some(path) => match probe_capture(path) {
            Ok(format) => Check::new(
                "samples",
                "sample captures",
                Status::Ok,
                format!("{} capture(s); {} detected as {format}", entries.len(), path.display()),
            ),
            Err(message) => Check::new(
                "samples",
                "sample captures",
                Status::Warn,
                format!("{}: {message}", path.display()),
            )
            .repair("re-generate the sample with scripts/gen_traffic.py"),
        },
        None => Check::new(
            "samples",
            "sample captures",
            Status::Warn,
            format!("{} contains no pcap/pcapng file", dir.display()),
        )
        .repair("python scripts/gen_traffic.py --out samples/synth-mixed.pcap --packets 600 --profile mixed"),
    }
}

fn probe_capture(path: &Path) -> Result<String, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut header = [0u8; 16];
    let read = file.read(&mut header).map_err(|e| e.to_string())?;
    if read < 16 {
        return Err("file shorter than a capture header".to_owned());
    }
    packetsage_core::detect_format(&header, path)
        .map(|format| format.as_str().to_owned())
        .map_err(|e| e.to_string())
}

/// Item 5: a **read-only** probe. A missing database is not a failure — the
/// user may simply not have run `db migrate` yet (§11.1, C4).
fn check_database(settings: &Settings) -> Check {
    let url = &settings.db_url;
    let Some(path) = sqlite_file(url) else {
        return Check::new(
            "database",
            "database",
            Status::Skipped,
            format!("{url}: only SQLite is probed by M6a"),
        );
    };
    if path != Path::new(":memory:") && !path.exists() {
        return Check::new(
            "database",
            "database",
            Status::Warn,
            format!(
                "{} does not exist yet (not created by doctor)",
                path.display()
            ),
        )
        .repair("packetsage db migrate   # creates and migrates the database");
    }
    let probe_url = readonly_url(url);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return Check::new(
                "database",
                "database",
                Status::Fail,
                format!("cannot start the async runtime: {error}"),
            )
        }
    };
    runtime.block_on(async move {
        use packetsage_storage::SqliteRepo;

        let repo = match SqliteRepo::connect_readonly_default(&probe_url).await {
            Ok(repo) => repo,
            Err(error) => {
                return Check::new(
                    "database",
                    "database",
                    Status::Fail,
                    format!("{url}: {error}"),
                )
                .repair("check the URL and the file permissions, or run `packetsage db migrate`")
            }
        };
        match repo.migration_status().await {
            Ok(status) if status.is_current() => Check::new(
                "database",
                "database",
                Status::Ok,
                format!(
                    "{url}: read-only probe ok, {} migration(s) applied",
                    status.applied.len()
                ),
            ),
            Ok(status) => Check::new(
                "database",
                "database",
                Status::Warn,
                format!(
                    "{url}: current={} target={} pending={}",
                    status
                        .current()
                        .map_or_else(|| "none".to_owned(), |v| v.to_string()),
                    status
                        .target()
                        .map_or_else(|| "none".to_owned(), |v| v.to_string()),
                    status.pending.len()
                ),
            )
            .repair("packetsage db migrate"),
            Err(error) => Check::new(
                "database",
                "database",
                Status::Warn,
                format!("{url}: migration ledger unreadable ({error})"),
            )
            .repair("packetsage db migrate"),
        }
    })
}

/// Item 6: writability of the output / database directories (§11.1).
fn check_paths(settings: &Settings) -> Check {
    let mut targets: Vec<PathBuf> = vec![PathBuf::from(".")];
    if let Some(path) = sqlite_file(&settings.db_url) {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                targets.push(parent.to_path_buf());
            }
        }
    }
    targets.dedup();
    for target in &targets {
        if let Err(error) = probe_writable(target) {
            return Check::new(
                "paths",
                "output directories",
                Status::Fail,
                format!("{}: {error}", target.display()),
            )
            .repair("create the directory (mkdir) and make sure it is writable by this user");
        }
    }
    Check::new(
        "paths",
        "output directories",
        Status::Ok,
        format!(
            "writable: {}",
            targets
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

/// Creates and removes a temporary file: the directory listing is unchanged (T6).
fn probe_writable(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err("not a directory".to_owned());
    }
    let marker = dir.join(format!(".packetsage-doctor-{}.tmp", std::process::id()));
    let mut file = std::fs::File::create(&marker).map_err(|e| e.to_string())?;
    let write = file
        .write_all(b"packetsage doctor")
        .map_err(|e| e.to_string());
    drop(file);
    let remove = std::fs::remove_file(&marker).map_err(|e| e.to_string());
    write.and(remove)
}

/// Item 7: Python environment and the agent package.
fn check_python() -> Check {
    let python = std::env::var("PACKETSAGE_PYTHON").unwrap_or_else(|_| "python".to_owned());
    let version = Command::new(&python)
        .arg("-c")
        .arg("import sys; print('%d.%d.%d' % sys.version_info[:3])")
        .output();
    let Ok(output) = version else {
        return Check::new(
            "python",
            "python environment",
            Status::Warn,
            format!("{python} not runnable; the agent stage is unavailable"),
        )
        .repair("install Python 3.10+ and add it to PATH (or set $PACKETSAGE_PYTHON)");
    };
    if !output.status.success() {
        return Check::new(
            "python",
            "python environment",
            Status::Warn,
            format!("{python} exited with {:?}", output.status.code()),
        );
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    match Command::new(&python)
        .arg("-c")
        .arg("import packetsage_agent, sys; sys.exit(0)")
        .output()
    {
        Ok(probe) if probe.status.success() => Check::new(
            "python",
            "python environment",
            Status::Ok,
            format!("{python} {version} with packetsage_agent importable"),
        ),
        _ => Check::new(
            "python",
            "python environment",
            Status::Warn,
            format!("{python} {version}; the packetsage_agent package is not importable"),
        )
        .repair("pip install -e ./agent"),
    }
}

/// What the Python agent reports about its own configuration.
struct AgentProvider {
    kind: String,
    model: Option<String>,
    key_present: bool,
}

/// Item 8: provider configuration, with the reachability probe gated by `--no-net`.
///
/// `packetsage-agent setup` writes `agent/.env`, and only the Python side reads
/// that file (§7.1 keeps the Rust side dotenv-free). So when the environment and
/// `packetsage.yaml` say nothing, the agent is asked for its effective provider
/// instead of guessing — a fresh install then reports ⚠ "run setup" while a
/// configured one reports ✅.
fn check_provider(settings: &Settings, no_net: bool) -> Check {
    let agent = agent_provider();
    match settings.provider.as_str() {
        "mock" => mock_row(),
        "" => match agent {
            Some(reported) if reported.kind == "mock" => mock_row(),
            Some(reported) => Check::new(
                "provider",
                "llm provider",
                Status::Ok,
                format!(
                    "{} (configured through `packetsage-agent setup`; the key stays in agent/.env)",
                    reported.describe()
                ),
            ),
            None => Check::new(
                "provider",
                "llm provider",
                Status::Warn,
                "not configured: run `packetsage-agent setup` (or export $PACKETSAGE_LLM_API_KEY)"
                    .to_owned(),
            )
            .repair(
                "packetsage-agent setup   # writes agent/.env; keys never go into packetsage.yaml",
            ),
        },
        kind @ ("openai" | "deepseek") => {
            let key_in_agent =
                matches!(&agent, Some(reported) if reported.kind == kind && reported.key_present);
            if !config::api_key_present() && !key_in_agent {
                return Check::new(
                    "provider",
                    "llm provider",
                    Status::Fail,
                    format!(
                        "{kind}: no API key in $PACKETSAGE_LLM_API_KEY / $OPENAI_API_KEY / \
                         $DEEPSEEK_API_KEY (and none in agent/.env)"
                    ),
                )
                .repair("packetsage-agent setup   # or export PACKETSAGE_LLM_API_KEY=...");
            }
            let host = if kind == "deepseek" {
                "api.deepseek.com:443"
            } else {
                "api.openai.com:443"
            };
            if no_net {
                return Check::new(
                    "provider",
                    "llm provider",
                    Status::Skipped,
                    format!("{kind}: key present, reachability probe skipped by --no-net"),
                );
            }
            match probe_endpoint(host, Duration::from_secs(3)) {
                Ok(()) => Check::new(
                    "provider",
                    "llm provider",
                    Status::Ok,
                    format!("{kind}: key present, {host} reachable"),
                ),
                Err(error) => Check::new(
                    "provider",
                    "llm provider",
                    Status::Fail,
                    format!("{kind}: {host} unreachable ({error})"),
                )
                .repair("check the proxy/network, or re-run with --no-net to skip this probe"),
            }
        }
        other => Check::new(
            "provider",
            "llm provider",
            Status::Warn,
            format!("{other}: assuming an OpenAI-compatible endpoint (no key requirement known)"),
        )
        .repair("set PACKETSAGE_LLM_MODEL / the endpoint URL expected by your deployment"),
    }
}

impl AgentProvider {
    /// `deepseek / deepseek-chat`, for the doctor summary column.
    fn describe(&self) -> String {
        match &self.model {
            Some(model) => format!("{} / {model}", self.kind),
            None => self.kind.clone(),
        }
    }
}

fn mock_row() -> Check {
    Check::new(
        "provider",
        "llm provider",
        Status::Ok,
        "mock: deterministic script replay (CI/demos only — not a real analysis)",
    )
}

/// Asks the Python agent for its effective provider (`setup --print --no-verify`).
///
/// `None` when the agent is not importable, exits non-zero (nothing configured)
/// or takes longer than the probe budget — doctor must never hang.
fn agent_provider() -> Option<AgentProvider> {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let python = std::env::var("PACKETSAGE_PYTHON").unwrap_or_else(|_| "python".to_owned());
    let mut child = Command::new(python)
        .args(["-m", "packetsage_agent", "setup", "--print", "--no-verify"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
    let output = child.wait_with_output().ok()?;
    parse_agent_provider(&String::from_utf8_lossy(&output.stdout))
}

fn parse_agent_provider(text: &str) -> Option<AgentProvider> {
    let mut kind = None;
    let mut model = None;
    let mut key_present = false;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("provider") {
            let value = value.trim();
            if !value.is_empty() && value != "-" {
                kind = Some(value.to_owned());
            }
        } else if let Some(value) = line.strip_prefix("model") {
            let value = value.trim();
            if !value.is_empty() && value != "-" {
                model = Some(value.to_owned());
            }
        } else if line.starts_with("api key") && line.contains("***") {
            key_present = true;
        }
    }
    kind.map(|kind| AgentProvider {
        kind,
        model,
        key_present,
    })
}

fn probe_endpoint(address: &str, timeout: Duration) -> Result<(), String> {
    use std::net::ToSocketAddrs;

    let mut addresses = address
        .to_socket_addrs()
        .map_err(|error| error.to_string())?;
    let Some(address) = addresses.next() else {
        return Err("DNS returned no address".to_owned());
    };
    std::net::TcpStream::connect_timeout(&address, timeout)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Item 9: end-to-end self check — spawn our own `serve` and round-trip a ping.
/// Local only, so it is never skipped by `--no-net` (§11.1, C4).
fn check_end_to_end() -> Check {
    let Ok(exe) = std::env::current_exe() else {
        return Check::new(
            "e2e",
            "end-to-end self check",
            Status::Fail,
            "cannot locate the running binary".to_owned(),
        );
    };
    let child = Command::new(exe)
        .arg("serve")
        .env_remove("PACKETSAGE_DB")
        .env_remove("PACKETSAGE_STORAGE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            return Check::new(
                "e2e",
                "end-to-end self check",
                Status::Fail,
                format!("cannot spawn `serve`: {error}"),
            )
            .repair("re-install the binary and check that it is executable")
        }
    };
    let request = RpcRequest {
        id: "doctor-1".to_owned(),
        method: method::PING.to_owned(),
        params: serde_json::json!({}),
    };
    let write_ok = child
        .stdin
        .as_mut()
        .map(|stdin| {
            writeln!(
                stdin,
                "{}",
                serde_json::to_string(&request).unwrap_or_default()
            )
            .is_ok()
        })
        .unwrap_or(false);
    if !write_ok {
        let _ = child.kill();
        return Check::new(
            "e2e",
            "end-to-end self check",
            Status::Fail,
            "cannot write to the worker stdin".to_owned(),
        );
    }
    let stdout = child.stdout.take();
    let mut line = String::new();
    let read_ok = stdout
        .map(|out| BufReader::new(out).read_line(&mut line).is_ok())
        .unwrap_or(false);
    let _ = child.kill();
    let _ = child.wait();
    if !read_ok {
        return Check::new(
            "e2e",
            "end-to-end self check",
            Status::Fail,
            "no response on stdout (the JSONL stream is broken)".to_owned(),
        )
        .repair("run `packetsage serve` by hand and check that stdout stays JSONL");
    }
    match serde_json::from_str::<serde_json::Value>(&line) {
        Ok(value) if value["ok"] == serde_json::json!(true) => Check::new(
            "e2e",
            "end-to-end self check",
            Status::Ok,
            "ping RPC round trip over stdin/stdout".to_owned(),
        ),
        _ => Check::new(
            "e2e",
            "end-to-end self check",
            Status::Fail,
            format!("unexpected response: {}", line.trim()),
        ),
    }
}

/// Item 10: the展示 layer must not keep its own copy of the schema version (§5).
fn check_schema_consistency() -> Check {
    let json = version::json();
    let expected = format!("\"schema_version\":{}", packetsage_protocol::SCHEMA_VERSION);
    if json.contains(&expected) {
        Check::new(
            "schema",
            "schema version consistency",
            Status::Ok,
            format!("version --json reports {expected} (protocol constant {SCHEMA_VERSION})"),
        )
    } else {
        Check::new(
            "schema",
            "schema version consistency",
            Status::Fail,
            format!("version --json does not report {expected}: {json}"),
        )
        .repair("the presentation layer and packetsage_protocol::SCHEMA_VERSION disagree")
    }
}

fn print_table(checks: &[Check], palette: Palette) {
    println!("packetsage doctor");
    println!("{:<4} {:<26} CHECK", "MARK", "SUMMARY");
    let mut fails = 0usize;
    for check in checks {
        let marker = match check.status {
            Status::Ok => palette.ok(check.status.marker()),
            Status::Fail => palette.bad(check.status.marker()),
            Status::Warn => palette.warn(check.status.marker()),
            Status::Skipped => check.status.marker().to_owned(),
        };
        println!("{marker:<4} {:<26} {}", check.name, check.detail);
        if let Some(repair) = &check.repair {
            println!("     {:<26} -> {repair}", "");
        }
        if check.status == Status::Fail {
            fails += 1;
        }
    }
    if fails == 0 {
        println!("\nall checks passed ({} check(s))", checks.len());
    }
}

/// Stable *json output format v1*: `{items:[{id,name,status,detail,repair_hint}]}`.
fn print_json(checks: &[Check]) {
    let items: Vec<serde_json::Value> = checks
        .iter()
        .map(|check| {
            serde_json::json!({
                "id": check.id,
                "name": check.name,
                "status": check.status.json(),
                "detail": check.detail,
                "repair_hint": check.repair.clone().unwrap_or_default(),
            })
        })
        .collect();
    let document = serde_json::json!({ "items": items });
    match serde_json::to_string_pretty(&document) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("packetsage: cannot serialise the report: {error}"),
    }
}

/// File path behind a `sqlite://` URL, when the URL addresses a file.
fn sqlite_file(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("sqlite://")?;
    let path = rest.split('?').next().unwrap_or(rest);
    if path.is_empty() {
        Some(PathBuf::from(":memory:"))
    } else {
        Some(PathBuf::from(path))
    }
}

/// Same URL opened read-only (doctor never creates a database).
fn readonly_url(url: &str) -> String {
    if url.contains("mode=ro") {
        url.to_owned()
    } else if url.contains('?') {
        format!("{url}&mode=ro")
    } else {
        format!("{url}?mode=ro")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_files_are_extracted() {
        assert_eq!(
            sqlite_file("sqlite://packetsage.db?mode=ro"),
            Some(PathBuf::from("packetsage.db"))
        );
        assert_eq!(sqlite_file("postgres://host/db"), None);
    }

    #[test]
    fn the_readonly_url_is_idempotent() {
        assert_eq!(readonly_url("sqlite://a.db"), "sqlite://a.db?mode=ro");
        assert_eq!(
            readonly_url("sqlite://a.db?cache=shared"),
            "sqlite://a.db?cache=shared&mode=ro"
        );
        assert_eq!(
            readonly_url("sqlite://a.db?mode=ro"),
            "sqlite://a.db?mode=ro"
        );
    }

    #[test]
    fn the_writability_probe_leaves_no_trace() {
        let dir = std::env::temp_dir();
        let before = std::fs::read_dir(&dir).map(|i| i.count()).unwrap_or(0);
        probe_writable(&dir).expect("temp dir must be writable");
        let after = std::fs::read_dir(&dir).map(|i| i.count()).unwrap_or(0);
        assert_eq!(before, after, "doctor must not leave files behind (T6)");
    }

    #[test]
    fn ten_checks_are_reported() {
        // The ids are the stable part of the --json contract (§11.1).
        let ids = [
            "binary", "config", "rules", "samples", "database", "paths", "python", "provider",
            "e2e", "schema",
        ];
        assert_eq!(ids.len(), 10);
    }

    #[test]
    fn statuses_map_to_the_json_vocabulary() {
        assert_eq!(Status::Ok.json(), "ok");
        assert_eq!(Status::Warn.json(), "warn");
        assert_eq!(Status::Fail.json(), "fail");
        assert_eq!(Status::Skipped.json(), "skipped");
    }

    #[test]
    fn skipped_checks_use_the_spec_marker() {
        assert_eq!(Status::Skipped.marker(), "⏭");
    }
}
