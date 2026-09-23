//! `packetsage db query --readonly` and `packetsage db migrate` (§8).
//!
//! ADR-018 (single writer) is the reason this file exists: the CLI gains a
//! consumption-side guardrail for queries and an explicit, confirmed entry
//! point for the migrations that already ship with the workspace. It never
//! introduces a second writing component.

use std::time::Duration;

use packetsage_storage::{MigrationEntry, MigrationStatus, SqliteRepo};

use crate::cli::{DbArgs, DbKind, DbMigrateArgs, DbQueryArgs};
use crate::color::{stdin_is_tty, Palette};
use crate::config::{self, Settings};
use crate::exit::ExitCode;

/// Maximum rows `db query` will ever return.
const MAX_LIMIT: u64 = 10_000;

/// Runs `db`.
pub fn run(args: &DbArgs, settings: &Settings, palette: Palette) -> ExitCode {
    match &args.kind {
        DbKind::Query(query) => {
            // `db query` never opens a writable handle: when nothing was
            // configured the default URL is rewritten to its read-only form,
            // and any configured URL must already carry `mode=ro` (§8.1).
            let url = match &args.db {
                Some(url) => url.clone(),
                None if settings.db_url == config::DEFAULT_DB_URL => readonly_default(),
                None => settings.db_url.clone(),
            };
            run_query(query, &url, palette)
        }
        DbKind::Migrate(migrate) => run_migrate(migrate, &settings.db_url),
    }
}

/// `db query`'s default: the same file, opened read-only.
#[must_use]
pub fn readonly_default() -> String {
    format!("{}?mode=ro", config::DEFAULT_DB_URL)
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("packetsage: cannot start the async runtime: {error}"))
}

fn run_query(args: &DbQueryArgs, url: &str, palette: Palette) -> ExitCode {
    // Guardrail 1 (§8.1): the read-only contract is acknowledged explicitly.
    if !args.readonly {
        eprintln!(
            "packetsage: refusing to run without --readonly; \
             `packetsage db query --readonly --sql \"SELECT ...\"` is the only supported form"
        );
        return ExitCode::Usage;
    }
    // Guardrail 2 (§8.1): the URL must carry `mode=ro`; SQLite then enforces it.
    if !url.contains("mode=ro") {
        eprintln!(
            "packetsage: {url} does not contain `mode=ro`; refusing to open a writable handle. \
             Use `sqlite://packetsage.db?mode=ro`."
        );
        return ExitCode::Usage;
    }
    // Guardrail 3 (§8.1): UX pre-check on the statement prefix. The real
    // enforcement is the read-only connection above.
    let statement = args.sql.trim_start();
    let keyword = statement
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(keyword.as_str(), "select" | "with" | "explain") {
        eprintln!(
            "packetsage: only SELECT / WITH / EXPLAIN are accepted (got `{keyword}`); \
             this connection is read-only."
        );
        return ExitCode::Usage;
    }
    if args.limit == 0 || args.limit > MAX_LIMIT {
        eprintln!(
            "packetsage: --limit must be between 1 and {MAX_LIMIT} (got {})",
            args.limit
        );
        return ExitCode::Usage;
    }

    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::Internal;
        }
    };
    let timeout = Duration::from_secs(args.timeout.max(1));
    let url = url.to_owned();
    runtime.block_on(async move {
        let repo = match SqliteRepo::connect_readonly(&url, timeout).await {
            Ok(repo) => repo,
            Err(error) => {
                eprintln!("packetsage: {url}: {error}");
                return ExitCode::ConfigError;
            }
        };
        match repo.query_table(&args.sql, Some(args.limit)).await {
            Ok((columns, rows)) => {
                if args.jsonl {
                    for row in &rows {
                        match serde_json::to_string(row) {
                            Ok(line) => println!("{line}"),
                            Err(error) => {
                                eprintln!("packetsage: cannot serialise a row: {error}");
                                return ExitCode::Internal;
                            }
                        }
                    }
                } else {
                    print_table(&columns, &rows, palette);
                }
                ExitCode::Success
            }
            Err(error) => {
                eprintln!("packetsage: {error}");
                ExitCode::ConfigError
            }
        }
    })
}

fn print_table(columns: &[String], rows: &[serde_json::Value], palette: Palette) {
    if columns.is_empty() {
        println!("(0 rows)");
        return;
    }
    let mut text_rows: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for row in rows {
        let mut cells = Vec::with_capacity(columns.len());
        for column in columns {
            let value = row.get(column).cloned().unwrap_or(serde_json::Value::Null);
            cells.push(render_cell(&value));
        }
        text_rows.push(cells);
    }
    let mut widths: Vec<usize> = columns.iter().map(String::len).collect();
    for cells in &text_rows {
        for (index, cell) in cells.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }
    let mut header = String::new();
    for (index, column) in columns.iter().enumerate() {
        let width = widths.get(index).copied().unwrap_or(0);
        header.push_str(&format!("{column:<width$} "));
    }
    println!("{}", palette.paint("1", header.trim_end()));
    for cells in &text_rows {
        let mut line = String::new();
        for (index, cell) in cells.iter().enumerate() {
            let width = widths.get(index).copied().unwrap_or(0);
            line.push_str(&format!("{cell:<width$} "));
        }
        println!("{}", line.trim_end());
    }
    println!("\n{} row(s)", text_rows.len());
}

fn render_cell(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "-".to_owned(),
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn run_migrate(args: &DbMigrateArgs, url: &str) -> ExitCode {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::Internal;
        }
    };
    runtime.block_on(async move { migrate_async(args, url).await })
}

async fn migrate_async(args: &DbMigrateArgs, url: &str) -> ExitCode {
    let repo = match SqliteRepo::connect(url).await {
        Ok(repo) => repo,
        Err(error) => {
            eprintln!(
                "packetsage: {url}: {error}; check the database URL or run `packetsage doctor`"
            );
            return ExitCode::ConfigError;
        }
    };
    let status = match repo.migration_status().await {
        Ok(status) => status,
        Err(error) => {
            eprintln!("packetsage: {url}: {error}");
            return ExitCode::ConfigError;
        }
    };
    print_plan(url, &status);
    if status.pending.is_empty() {
        println!("database {url} is up to date (no pending migration)");
        return ExitCode::Success;
    }
    match confirm(args, status.pending.len()) {
        Decision::Proceed => {}
        Decision::AbortedByUser => {
            eprintln!("migrate aborted by user");
            return ExitCode::Success;
        }
        Decision::NonInteractive => return ExitCode::Usage,
    }
    // Backup hint, never an automatic copy (§8.2: no implicit drift).
    if let Some(path) = sqlite_path(url) {
        eprintln!("packetsage: back up first: `cp {path} {path}.bak` (not done automatically)");
    }
    if let Ok(true) = repo.write_lock_probe().await {
        eprintln!(
            "packetsage: WARN another writer seems to hold the database lock \
             (is a `packetsage serve` running?); the migration may fail"
        );
    }
    match repo.apply_migrations().await {
        Ok(applied) => {
            for entry in &applied {
                eprintln!("applied {} {}", entry.version, entry.description);
            }
            println!(
                "database {url} migrated: {} migration(s) applied",
                applied.len()
            );
            ExitCode::Success
        }
        Err(error) => {
            eprintln!("packetsage: {url}: {error}; the schema is unchanged (migrations are transactional)");
            ExitCode::ConfigError
        }
    }
}

fn print_plan(url: &str, status: &MigrationStatus) {
    let current = status
        .current()
        .map_or_else(|| "none".to_owned(), |v| v.to_string());
    let target = status
        .target()
        .map_or_else(|| "none".to_owned(), |v| v.to_string());
    eprintln!("packetsage: {url}: current={current} target={target}");
    if !status.pending.is_empty() {
        eprintln!("  pending {}", pending_summary(&status.pending));
    }
}

/// Outcome of the `db migrate` confirmation (§8.2 three states).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// `--yes`, or an interactive "y".
    Proceed,
    /// The user declined: not an error, so exit 0.
    AbortedByUser,
    /// Piped stdin without `--yes`: fail closed with exit 1.
    NonInteractive,
}

/// Confirmation policy, kept pure so the three states are unit testable
/// (the interactive branch itself needs a terminal).
#[must_use]
fn decide(yes: bool, stdin_is_tty: bool, answer: Option<&str>) -> Decision {
    if yes {
        return Decision::Proceed;
    }
    if !stdin_is_tty {
        return Decision::NonInteractive;
    }
    match answer.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "y" || value == "yes" => Decision::Proceed,
        _ => Decision::AbortedByUser,
    }
}

/// Asks for confirmation; non-interactive runs fail closed (§8.2).
fn confirm(args: &DbMigrateArgs, pending: usize) -> Decision {
    if args.yes {
        return Decision::Proceed;
    }
    if !stdin_is_tty() {
        eprintln!(
            "packetsage: {pending} migration(s) pending and stdin is not a terminal; \
             re-run with --yes to apply them"
        );
        return Decision::NonInteractive;
    }
    eprint!("apply {pending} migration(s)? [y/N] ");
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return Decision::AbortedByUser;
    }
    decide(false, true, Some(&answer))
}

/// Extracts the file path of a `sqlite://` URL, when there is one.
fn sqlite_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("sqlite://")?;
    let path = rest.split('?').next().unwrap_or(rest);
    if path.is_empty() || path == ":memory:" {
        None
    } else {
        Some(path.to_owned())
    }
}

/// Formats the pending list for the interactive prompt and the tests.
#[must_use]
pub fn pending_summary(entries: &[MigrationEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("{}({})", entry.version, entry.description))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_paths_are_extracted_for_the_backup_hint() {
        assert_eq!(
            sqlite_path("sqlite://packetsage.db?mode=ro").as_deref(),
            Some("packetsage.db")
        );
        assert_eq!(sqlite_path("sqlite::memory:"), None);
    }

    #[test]
    fn pending_summary_lists_versions() {
        let entries = vec![MigrationEntry {
            version: 20260920,
            description: "init".to_owned(),
        }];
        assert!(pending_summary(&entries).contains("20260920(init)"));
    }

    #[test]
    fn the_query_default_is_read_only() {
        // The writable default is used by `analyze`/`query`; `db query`
        // rewrites it so the guardrail in `run_query` passes by default.
        assert!(!config::DEFAULT_DB_URL.contains("mode=ro"));
        assert!(readonly_default().contains("mode=ro"));
    }

    #[test]
    fn migrate_confirmation_has_three_states() {
        // §4: `--yes` proceeds; piped stdin fails closed; an interactive
        // refusal is *not* an error (exit 0 + "migrate aborted by user").
        assert_eq!(decide(true, false, None), Decision::Proceed);
        assert_eq!(decide(false, false, None), Decision::NonInteractive);
        assert_eq!(decide(false, true, Some("y\n")), Decision::Proceed);
        assert_eq!(decide(false, true, Some("YES\n")), Decision::Proceed);
        assert_eq!(decide(false, true, Some("\n")), Decision::AbortedByUser);
        assert_eq!(decide(false, true, Some("n\n")), Decision::AbortedByUser);
        assert_eq!(decide(false, true, None), Decision::AbortedByUser);
    }
}
