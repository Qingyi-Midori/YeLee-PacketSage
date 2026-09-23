//! `packetsage rules list|check`.

use packetsage_rules::{load_dir, CompiledRule};

use crate::cli::{RulesArgs, RulesKind};
use crate::commands::builtin_rules_dir;
use crate::config::Settings;
use crate::exit::ExitCode;

/// Runs `rules`.
pub fn run(args: &RulesArgs, settings: &Settings) -> ExitCode {
    match &args.kind {
        RulesKind::List { dir } => {
            let dir = dir.clone().unwrap_or_else(|| settings.rules_dir.clone());
            list(&dir)
        }
        RulesKind::Check { path } => check(path),
    }
}

fn list(dir: &std::path::Path) -> ExitCode {
    let dir = builtin_rules_dir(dir);
    // The runtime always starts from the embedded rules and lets a same-id file
    // on disk override them, so `rules list` reports the same set.
    let mut engine =
        packetsage_rules::RuleEngine::with_builtin_rules(packetsage_rules::WindowBudget::default());
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
            Err(error) => {
                eprintln!("packetsage: cannot read {}: {error}", dir.display());
                return ExitCode::ConfigError;
            }
        }
    } else {
        eprintln!(
            "packetsage: {} does not contain rule files; listing the {} embedded rule(s)",
            dir.display(),
            packetsage_rules::BUILTIN_RULES.len()
        );
    }
    for rule in engine.rules() {
        println!(
            "{} v{} {:?} {} scope={:?} metric={:?} window={} cooldown={} hash={} source={}",
            rule.file.id,
            rule.file.version,
            rule.file.severity,
            rule.file.name,
            rule.file.scope,
            rule.file.threshold.metric,
            rule.file.threshold.window,
            rule.file.dedupe.per_group_cooldown,
            rule.short_hash(),
            rule.source.display()
        );
    }
    ExitCode::Success
}

fn print_rules(rules: &[CompiledRule]) {
    for rule in rules {
        println!(
            "{} v{} {:?} {} scope={:?} metric={:?} window={} cooldown={} hash={}",
            rule.file.id,
            rule.file.version,
            rule.file.severity,
            rule.file.name,
            rule.file.scope,
            rule.file.threshold.metric,
            rule.file.threshold.window,
            rule.file.dedupe.per_group_cooldown,
            rule.short_hash()
        );
    }
}

fn check(path: &std::path::Path) -> ExitCode {
    if path.is_dir() {
        return match load_dir(path) {
            Ok(rules) => {
                println!("{} rule(s) valid", rules.len());
                print_rules(&rules);
                ExitCode::Success
            }
            Err(error) => {
                eprintln!("packetsage: invalid rules: {error}");
                ExitCode::ConfigError
            }
        };
    }
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("packetsage: cannot read {}: {error}", path.display());
            return ExitCode::CaptureError;
        }
    };
    match CompiledRule::from_text(&text, path.to_path_buf()) {
        Ok(rule) => {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let issues = rule
                .file
                .validate(&stem, &std::collections::BTreeSet::new());
            let mut errors = 0usize;
            for issue in &issues {
                let label = if issue.is_error() { "ERROR" } else { "WARN " };
                println!("{label} {}", issue.message());
                if issue.is_error() {
                    errors += 1;
                }
            }
            if errors == 0 {
                println!("{}: valid (hash {})", rule.file.id, rule.short_hash());
                ExitCode::Success
            } else {
                eprintln!("packetsage: {errors} error(s) in {}", path.display());
                ExitCode::ConfigError
            }
        }
        Err(error) => {
            eprintln!("packetsage: {}: {error}", path.display());
            ExitCode::ConfigError
        }
    }
}
