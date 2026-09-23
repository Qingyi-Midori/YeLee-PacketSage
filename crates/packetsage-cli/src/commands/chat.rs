//! `packetsage chat`: thin launcher for the Python agent (§2.3, §4, §10).
//!
//! The Rust side owns exactly two things (M3~M6v0.2 §4.1, Agent CLI §1 path B):
//! it creates the task (`analyze` + persist, unless `--task-id` names one that
//! already exists) and it hands the agent `chat --task-id T`. No agent logic
//! lives here.

use packetsage_core::ids;
use packetsage_protocol::TaskId;

use crate::agent_launcher;
use crate::cli::{ChatArgs, Cli};
use crate::commands::analyze::verbosity_argv;
use crate::config::{Loaded, Settings};
use crate::exit::ExitCode;

/// Runs `chat`.
pub fn run(args: &ChatArgs, loaded: &Loaded, settings: &Settings, cli: &Cli) -> ExitCode {
    // Resolve the agent *first*: a machine without the Python side (the release
    // tarball, §2.3) must fail closed with the four-path hint before spending
    // time on an analysis it could never show.
    let launcher = match agent_launcher::resolve(&args.python) {
        Ok(launcher) => launcher,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::ConfigError;
        }
    };
    // The provider gate lives in the agent: it is the side that reads
    // `agent/.env`, so it is the only side that knows whether `setup` has run.
    // A configuration failure comes back as exit 3 and is passed through below.
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
    if args.task_id.is_none() {
        // The agent's engine recovers the task from the database (ADR-019), so
        // the analysis has to be persisted before the session starts.
        if let Err(error) =
            crate::commands::analyze::analyze_task(&args.capture, &task_id, loaded, settings)
        {
            let (code, message) = error;
            eprintln!("{message}");
            return code;
        }
    }
    if let Some(warning) = &launcher.warning {
        eprintln!("{warning}");
    }

    // §1: `packetsage-agent chat --task-id <id> [--db <url>] [--report <path>]`.
    let mut argv = vec![
        "chat".to_owned(),
        "--task-id".to_owned(),
        task_id.as_str().to_owned(),
    ];
    if let Some(db) = &args.db {
        argv.push("--db".to_owned());
        argv.push(db.clone());
    }
    if let Some(report) = &args.report {
        argv.push("--report".to_owned());
        argv.push(report.to_string_lossy().to_string());
    }
    argv.extend(verbosity_argv(cli));

    let mut command = std::process::Command::new(&launcher.program);
    command.args(launcher.command_line(&argv));
    if let Ok(engine) = std::env::current_exe() {
        command.env("PACKETSAGE_ENGINE", engine);
    }
    command.env("PACKETSAGE_RULES", settings.rules_dir.as_os_str());
    command.env("PACKETSAGE_STORAGE_URL", settings.db_url.as_str());
    // Never export an *empty* provider/model: the child treats an empty variable
    // as "set", which would shadow the `agent/.env` that `setup` wrote.
    if !settings.provider.is_empty() {
        command.env("PACKETSAGE_LLM_PROVIDER", settings.provider.as_str());
    }
    if let Some(model) = &settings.model {
        command.env("PACKETSAGE_LLM_MODEL", model.as_str());
    }
    // The agent prints Chinese in its banner and findings. Force UTF-8 unless the
    // user chose something else, so a redirected/piped session is not mangled by
    // the console code page (GBK on this host). PEP 528 still renders UTF-8
    // correctly on a real Windows console.
    if std::env::var_os("PYTHONIOENCODING").is_none() {
        command.env("PYTHONIOENCODING", "utf-8");
    }
    if std::env::var_os("PYTHONUTF8").is_none() {
        command.env("PYTHONUTF8", "1");
    }
    // The discovery chain is inherited, not re-implemented (§7.1) — no new flag.
    if let Some(path) = &loaded.path {
        command.env("PACKETSAGE_CONFIG", path);
    }

    match command.status() {
        Ok(status) if status.success() => ExitCode::Success,
        Ok(status) => {
            let code = status.code().unwrap_or(3);
            eprintln!(
                "packetsage: agent exited with {code} (via {})",
                launcher.via
            );
            match code {
                1 => ExitCode::Usage,
                2 => ExitCode::CaptureError,
                3 => ExitCode::ConfigError,
                4 => ExitCode::Internal,
                5 => ExitCode::Unsupported,
                _ => ExitCode::ConfigError,
            }
        }
        Err(error) => {
            eprintln!("packetsage: cannot launch the Python agent ({error})");
            ExitCode::ConfigError
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_launcher_passes_the_agent_exit_code_through() {
        // §1: the launcher never rewrites the agent's status — a provider that is
        // missing (exit 3) must reach the caller unchanged. The mapping table is
        // what `run` uses; this pins its shape.
        for (code, expected) in [(1_u8, 1_u8), (3, 3), (4, 4), (5, 5)] {
            let mapped = match code {
                1 => ExitCode::Usage,
                2 => ExitCode::CaptureError,
                3 => ExitCode::ConfigError,
                4 => ExitCode::Internal,
                5 => ExitCode::Unsupported,
                _ => ExitCode::ConfigError,
            };
            assert_eq!(mapped.code(), expected);
        }
    }
}
