//! PacketSage CLI: argument parsing and assembly only (M0~M2 §2.3).
//!
//! The process boundary lives here: parse → configure the three streams →
//! dispatch. Everything else is a module with one job.

#![forbid(unsafe_code)]
// Tests assert with `expect()`; the CLI itself propagates errors instead.
#![cfg_attr(test, allow(clippy::expect_used))]

mod agent_launcher;
mod banner;
mod cli;
mod color;
mod commands;
mod config;
mod doctor;
mod errors;
mod exit;
mod persist;
mod progress;
mod redact;
mod serve;
mod version;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::{Loaded, Settings};
use crate::exit::ExitCode;

fn main() -> std::process::ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            // `--help` / `--version` are not failures; anything else is a usage
            // error and exits 1 (§4), where clap would default to 2.
            let code = match error.kind() {
                clap::error::ErrorKind::DisplayHelp
                | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                | clap::error::ErrorKind::DisplayVersion => ExitCode::Success,
                _ => ExitCode::Usage,
            };
            let _ = error.print();
            return std::process::ExitCode::from(code.code());
        }
    };
    init_tracing(cli.verbose, cli.quiet);
    install_panic_hook();
    let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&cli)))
        .unwrap_or(ExitCode::Internal);
    std::process::ExitCode::from(code.code())
}

/// Log level: `-q` and the default both stay at WARN, `-v` opens INFO and
/// `-vv` opens DEBUG (C10: the default is *not* info).
fn init_tracing(verbose: u8, quiet: bool) {
    let default_filter = if quiet {
        "warn"
    } else {
        match verbose {
            0 => "warn",
            1 => "info",
            _ => "debug",
        }
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter)),
        )
        .with_writer(std::io::stderr)
        .try_init();
}

/// Panics are a bug, not a user error: report once and let `run` return exit 4.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("{}", errors::panic_message(info.payload()));
    }));
}

fn run(cli: &Cli) -> ExitCode {
    // Test hook for the last row of the §4 matrix (panic -> exit 4 with a panic
    // summary). It exists so the guard is *scriptable*, never set in production.
    #[allow(clippy::panic)] // deliberate: this statement *is* the injected panic
    if std::env::var_os("PACKETSAGE_PANIC_FOR_TEST").is_some() {
        panic!("injected panic (PACKETSAGE_PANIC_FOR_TEST)");
    }
    if cli.version {
        println!("{}", version::line());
        return ExitCode::Success;
    }
    let Some(command) = &cli.command else {
        return ExitCode::Usage;
    };
    let machine = machine_stream(command);
    let palette = color::Palette::stdout(cli.no_color, machine);
    match command {
        Command::Version(args) => commands::version::run(args),
        Command::Schema => {
            commands::schema::print_schema();
            ExitCode::Success
        }
        // doctor resolves the configuration itself so that a broken file shows
        // up as check 2 instead of aborting the whole table (§11.1).
        Command::Doctor(args) => doctor::run(args, cli, palette),
        Command::Analyze(args) => with_config(cli, |loaded| {
            let settings = Settings::resolve(
                args.db.as_deref(),
                args.rules_dir.as_deref(),
                args.provider.as_deref(),
                args.model.as_deref(),
                &loaded.config,
            );
            dump_if_verbose(cli, loaded, &settings);
            commands::analyze::run(args, loaded, &settings, cli)
        }),
        Command::Serve => with_config(cli, |loaded| {
            let settings = Settings::resolve(None, None, None, None, &loaded.config);
            dump_if_verbose(cli, loaded, &settings);
            serve::run(loaded)
        }),
        Command::Query(args) => with_config(cli, |loaded| {
            let settings = Settings::resolve(args.db.as_deref(), None, None, None, &loaded.config);
            dump_if_verbose(cli, loaded, &settings);
            commands::query::run(args, &settings)
        }),
        Command::Db(args) => with_config(cli, |loaded| {
            let settings = Settings::resolve(args.db.as_deref(), None, None, None, &loaded.config);
            dump_if_verbose(cli, loaded, &settings);
            commands::db::run(args, &settings, palette)
        }),
        Command::Rules(args) => with_config(cli, |loaded| {
            let settings = Settings::resolve(None, None, None, None, &loaded.config);
            dump_if_verbose(cli, loaded, &settings);
            commands::rules::run(args, &settings)
        }),
    }
}

/// Resolves the file discovery chain, mapping a failure to exit 3 (§7.1).
fn with_config(cli: &Cli, run: impl FnOnce(&Loaded) -> ExitCode) -> ExitCode {
    match config::resolve(cli.config.as_deref()) {
        Ok(loaded) => run(&loaded),
        Err(error) => {
            eprintln!("packetsage: {error}");
            ExitCode::ConfigError
        }
    }
}

/// `-vv` dumps the effective configuration, redacted (§3.1, §7.2).
fn dump_if_verbose(cli: &Cli, loaded: &Loaded, settings: &Settings) {
    if cli.verbose >= 2 {
        eprintln!(
            "packetsage: effective configuration (redacted)\n{}",
            settings.dump(loaded)
        );
    }
}

/// True when the command's stdout carries a pure machine stream (§3.2).
fn machine_stream(command: &Command) -> bool {
    match command {
        Command::Analyze(args) => args.jsonl,
        Command::Db(args) => matches!(&args.kind, cli::DbKind::Query(query) if query.jsonl),
        Command::Doctor(args) => args.json,
        _ => false,
    }
}
