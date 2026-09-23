//! `packetsage query`: structured (never raw SQL) database queries.

use std::io::Write;

use packetsage_storage::{AlertFilter, AlertRow, FindingRow, Repository, SqliteRepo};
use serde::Serialize;

use crate::cli::{QueryArgs, QueryKind};
use crate::config::Settings;
use crate::exit::ExitCode;

/// Runs `query`.
pub fn run(args: &QueryArgs, settings: &Settings) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("packetsage: cannot start the async runtime: {error}");
            return ExitCode::Internal;
        }
    };
    runtime.block_on(async move { run_async(args, &settings.db_url).await })
}

async fn run_async(args: &QueryArgs, url: &str) -> ExitCode {
    let repo = match SqliteRepo::connect(url).await {
        Ok(repo) => repo,
        Err(error) => {
            eprintln!("packetsage: {url}: {error}; check the URL or run `packetsage doctor`");
            return ExitCode::ConfigError;
        }
    };
    if let Err(error) = repo.migrate().await {
        eprintln!("packetsage: {url}: migration failed ({error}); run `packetsage db migrate`");
        return ExitCode::ConfigError;
    }

    let task_id = match resolve_task(&repo, args.task_id.as_deref()).await {
        Ok(Some(task_id)) => task_id,
        Ok(None) => {
            eprintln!("packetsage: no analysed task found in {url}");
            return ExitCode::ConfigError;
        }
        Err(error) => {
            eprintln!("packetsage: {error}");
            return ExitCode::ConfigError;
        }
    };

    match &args.kind {
        QueryKind::Sessions { sort_by, limit } => {
            let mut sessions = match repo.list_sessions(&task_id, *limit).await {
                Ok(rows) => rows,
                Err(error) => {
                    eprintln!("packetsage: {error}");
                    return ExitCode::ConfigError;
                }
            };
            if sort_by == "packets" {
                sessions.sort_by_key(|row| std::cmp::Reverse(row.packets));
            } else if sort_by == "duration" {
                sessions.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
            }
            if args.jsonl || args.json.is_some() {
                let rows: Vec<serde_json::Value> =
                    sessions.iter().map(session_machine_row).collect();
                if let Some(code) = emit_machine(args, &rows) {
                    return code;
                }
            }
            println!("task {task_id}");
            println!(
                "{:<10} {:<5} {:<39} {:<6} {:<39} {:<6} {:>8} {:>10} {:<16} app",
                "session", "proto", "src", "sport", "dst", "dport", "packets", "bytes", "state"
            );
            for row in sessions {
                println!(
                    "{:<10} {:<5} {:<39} {:<6} {:<39} {:<6} {:>8} {:>10} {:<16} {}",
                    row.id,
                    row.protocol,
                    row.src_ip,
                    row.src_port,
                    row.dst_ip,
                    row.dst_port,
                    row.packets,
                    row.bytes,
                    row.state,
                    row.app_protocol.unwrap_or_else(|| "-".to_owned())
                );
            }
            ExitCode::Success
        }
        QueryKind::Stats { layer } => {
            let sessions = repo
                .list_sessions(&task_id, 100_000)
                .await
                .unwrap_or_default();
            let mut by_protocol: std::collections::BTreeMap<String, (u64, u64)> =
                std::collections::BTreeMap::new();
            for row in &sessions {
                let entry = by_protocol.entry(row.protocol.clone()).or_insert((0, 0));
                entry.0 += 1;
                entry.1 += row.bytes.max(0) as u64;
            }
            let rows: Vec<serde_json::Value> = by_protocol
                .iter()
                .map(|(protocol, (count, bytes))| {
                    serde_json::json!({
                        "task_id": task_id,
                        "layer": layer,
                        "protocol": protocol,
                        "sessions": count,
                        "bytes": bytes,
                    })
                })
                .collect();
            if let Some(code) = emit_machine(args, &rows) {
                return code;
            }
            println!("task {task_id}");
            println!(
                "layer {layer} (session level aggregates; per-packet counters live in `serve`)"
            );
            println!("{:<10} {:>10} {:>14}", "protocol", "sessions", "bytes");
            for (name, (count, bytes)) in by_protocol {
                println!("{name:<10} {count:>10} {bytes:>14}");
            }
            ExitCode::Success
        }
        QueryKind::Alerts {
            severity,
            rule_id,
            limit,
        } => {
            let alerts = match repo
                .list_alerts(&AlertFilter {
                    task_id: Some(task_id.clone()),
                    severity: severity.clone(),
                    rule_id: rule_id.clone(),
                    session_id: None,
                    limit: Some(*limit),
                })
                .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    eprintln!("packetsage: {error}");
                    return ExitCode::ConfigError;
                }
            };
            if args.jsonl || args.json.is_some() {
                let rows: Vec<serde_json::Value> = alerts.iter().map(alert_machine_row).collect();
                if let Some(code) = emit_machine(args, &rows) {
                    return code;
                }
            }
            println!("task {task_id}  ({} alert(s))", alerts.len());
            for alert in alerts {
                println!(
                    "{:<28} {:<7} {:<28} packets {}-{} ts {}..{} hash {}",
                    alert.id,
                    alert.severity,
                    alert.rule_id,
                    alert.first_packet,
                    alert.last_packet,
                    alert.first_ts,
                    alert.last_ts,
                    alert.rule_content_hash
                );
            }
            ExitCode::Success
        }
        QueryKind::Findings { limit } => {
            let findings = match repo.list_findings(&task_id, *limit).await {
                Ok(rows) => rows,
                Err(error) => {
                    eprintln!("packetsage: {error}");
                    return ExitCode::ConfigError;
                }
            };
            if args.jsonl || args.json.is_some() {
                let rows: Vec<serde_json::Value> =
                    findings.iter().map(finding_machine_row).collect();
                if let Some(code) = emit_machine(args, &rows) {
                    return code;
                }
            }
            println!("task {task_id}  ({} finding(s))", findings.len());
            for finding in findings {
                println!(
                    "{:<8} {:<7} {:<24} {:<12} {}",
                    finding.id,
                    finding.severity,
                    finding.basis,
                    finding.validator_status,
                    finding.title
                );
            }
            ExitCode::Success
        }
    }
}

/// Serialises one typed row without ever changing the field names.
fn value_of<T: Serialize>(row: &T) -> serde_json::Value {
    serde_json::to_value(row).unwrap_or(serde_json::Value::Null)
}

/// Parses a `*_json` TEXT column; a malformed value becomes `null` in the
/// convenience field instead of failing the whole query.
fn parse_json_or_null(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or(serde_json::Value::Null)
}

/// `alerts` machine row: the `AlertRow` fields plus the two JSON columns
/// decoded once, so the GUI does not have to string-split them.
fn alert_machine_row(alert: &AlertRow) -> serde_json::Value {
    let mut value = value_of(alert);
    if let Some(object) = value.as_object_mut() {
        object.insert("group".to_owned(), parse_json_or_null(&alert.group_json));
        object.insert(
            "evidence".to_owned(),
            parse_json_or_null(&alert.evidence_json),
        );
    }
    value
}

/// `findings` machine row: the `FindingRow` fields plus `evidence` decoded
/// from `evidence_json`.
fn finding_machine_row(finding: &FindingRow) -> serde_json::Value {
    let mut value = value_of(finding);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "evidence".to_owned(),
            parse_json_or_null(&finding.evidence_json),
        );
        // The DB stores the Rust `Debug` spelling (`rulematch`); the frozen
        // wire vocabulary is snake_case (`rule_match`). The machine row keeps
        // both: `basis` is the wire value the GUI switches on, `basis_stored`
        // is the untouched column.
        object.insert(
            "basis".to_owned(),
            serde_json::json!(normalize_basis(&finding.basis)),
        );
        object.insert("basis_stored".to_owned(), serde_json::json!(finding.basis));
    }
    value
}

/// `sessions` machine row: `SessionRow` plus the wire spelling of `state`.
fn session_machine_row(session: &packetsage_storage::SessionRow) -> serde_json::Value {
    let mut value = value_of(session);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "state".to_owned(),
            serde_json::json!(normalize_state(&session.state)),
        );
        object.insert("state_stored".to_owned(), serde_json::json!(session.state));
    }
    value
}

/// Maps the stored `Debug`-lowercased basis to the frozen wire vocabulary.
fn normalize_basis(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "rulematch" | "rule_match" => "rule_match".to_owned(),
        "directobservation" | "direct_observation" => "direct_observation".to_owned(),
        "correlatedobservation" | "correlated_observation" => "correlated_observation".to_owned(),
        other => other.to_owned(),
    }
}

/// Maps the stored `Debug`-lowercased state to the frozen wire vocabulary.
fn normalize_state(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "halfclosed" | "half_closed" => "half_closed".to_owned(),
        "bufferoverflow" | "buffer_overflow" => "buffer_overflow".to_owned(),
        other => other.to_owned(),
    }
}

/// Writes the machine stream when `--jsonl` / `--json` asked for it.
///
/// `--json <PATH>` writes the rows to a file (like `analyze --json`);
/// `--jsonl` prints the same rows on stdout. Both are additive to the human
/// table: when neither flag is given this returns `None`.
fn emit_machine<T: Serialize>(args: &QueryArgs, rows: &[T]) -> Option<ExitCode> {
    if !args.jsonl && args.json.is_none() {
        return None;
    }
    let mut lines: Vec<String> = Vec::with_capacity(rows.len());
    for row in rows {
        match serde_json::to_string(row) {
            Ok(line) => lines.push(line),
            Err(error) => {
                eprintln!("packetsage: cannot serialise a row: {error}");
                return Some(ExitCode::Internal);
            }
        }
    }
    if let Some(path) = &args.json {
        let mut body = lines.join("\n");
        if !body.is_empty() {
            body.push('\n');
        }
        if let Err(error) =
            std::fs::File::create(path).and_then(|mut file| file.write_all(body.as_bytes()))
        {
            eprintln!("packetsage: cannot write {}: {error}", path.display());
            return Some(ExitCode::CaptureError);
        }
        if !args.jsonl {
            eprintln!(
                "packetsage: wrote {} rows to {}",
                lines.len(),
                path.display()
            );
        }
    }
    if args.jsonl {
        for line in lines {
            println!("{line}");
        }
    }
    Some(ExitCode::Success)
}

async fn resolve_task(
    repo: &SqliteRepo,
    requested: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(task_id) = requested {
        let exists = repo.task_exists(task_id).await.map_err(|e| e.to_string())?;
        return Ok(if exists {
            Some(task_id.to_owned())
        } else {
            None
        });
    }
    let tasks = repo.list_tasks(1).await.map_err(|e| e.to_string())?;
    Ok(tasks.first().map(|task| task.id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basis_normalisation_covers_both_spellings() {
        assert_eq!(normalize_basis("rulematch"), "rule_match");
        assert_eq!(normalize_basis("rule_match"), "rule_match");
        assert_eq!(
            normalize_basis("correlatedobservation"),
            "correlated_observation"
        );
        assert_eq!(normalize_basis("hypothesis"), "hypothesis");
        assert_eq!(normalize_basis("DirectObservation"), "direct_observation");
    }

    #[test]
    fn state_normalisation_covers_the_frozen_six() {
        assert_eq!(normalize_state("halfclosed"), "half_closed");
        assert_eq!(normalize_state("bufferoverflow"), "buffer_overflow");
        assert_eq!(normalize_state("new"), "new");
        assert_eq!(normalize_state("closed"), "closed");
    }

    #[test]
    fn json_columns_fall_back_to_null_instead_of_failing() {
        assert_eq!(parse_json_or_null("{not json"), serde_json::Value::Null);
        assert_eq!(
            parse_json_or_null("[[\"src_ip\",\"10.0.0.1\"]]"),
            serde_json::json!([["src_ip", "10.0.0.1"]])
        );
    }
}
