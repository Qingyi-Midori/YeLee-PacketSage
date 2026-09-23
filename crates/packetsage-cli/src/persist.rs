//! Maps an in-memory [`AnalysisStore`] onto the storage rows (ADR-018: only the
//! Rust engine writes; Python submits through RPC).

use packetsage_core::AnalysisStore;
use packetsage_storage::{
    AgentRunRow, AlertRow, CaptureRow, FindingRow, Repository, SessionRow, SqliteRepo,
    StorageError, TaskRow, ToolCallRow,
};

/// Writes the whole task (task, capture, sessions, alerts, findings).
///
/// # Errors
/// Returns [`StorageError`] when the database rejects a statement.
pub async fn persist(repo: &SqliteRepo, store: &AnalysisStore) -> Result<(), StorageError> {
    repo.migrate().await?;

    let summary = &store.summary;
    repo.upsert_task(&TaskRow {
        id: store.task_id.clone(),
        status: store.status.clone(),
        source_path: summary.source_path.clone(),
        source_sha256: store.source_sha256.clone(),
        started_at: store.started_at.clone(),
        finished_at: Some(packetsage_core::pipeline::now_rfc3339()),
        packet_count: i64::try_from(summary.packets).unwrap_or(i64::MAX),
        byte_count: i64::try_from(summary.bytes).unwrap_or(i64::MAX),
        error_code: store.error_code.clone(),
        index_mode: "FULL".to_owned(),
        rules_hash: store
            .alerts
            .first()
            .map_or_else(String::new, |alert| alert.rule_content_hash.clone()),
    })
    .await?;

    repo.upsert_capture(&CaptureRow {
        task_id: store.task_id.clone(),
        format: summary.format.clone(),
        first_ts: summary.first_ts_ns.clone(),
        last_ts: summary.last_ts_ns.clone(),
        interfaces: i64::try_from(summary.interfaces.len()).unwrap_or(0),
        linktypes_json: serde_json::to_string(
            &summary
                .interfaces
                .iter()
                .map(|interface| interface.linktype)
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| "[]".to_owned()),
    })
    .await?;

    let sessions: Vec<SessionRow> = store
        .sessions
        .sessions()
        .iter()
        .map(|entry| SessionRow {
            id: entry.session_id.clone(),
            task_id: store.task_id.clone(),
            protocol: entry.key.proto.as_str().to_owned(),
            src_ip: entry.key.a.ip.to_string(),
            src_port: i64::from(entry.key.a.port),
            dst_ip: entry.key.b.ip.to_string(),
            dst_port: i64::from(entry.key.b.port),
            first_ts: entry.stats.first_ts_ns.to_string(),
            last_ts: entry.stats.last_ts_ns.to_string(),
            packets: i64::try_from(entry.stats.packets).unwrap_or(i64::MAX),
            bytes: i64::try_from(entry.stats.bytes).unwrap_or(i64::MAX),
            state: format!("{:?}", entry.stats.state).to_lowercase(),
            app_protocol: entry
                .stats
                .app_protocol
                .map(|a| format!("{a:?}").to_lowercase()),
            interface_id: entry.interface_id.map(i64::from),
            vlan_tag: entry.vlan_tag.map(i64::from),
            direction_basis: match entry.direction_basis {
                packetsage_protocol::DirectionBasis::SynFirst => "syn_first",
                packetsage_protocol::DirectionBasis::PortHeuristic => "port_heuristic",
                packetsage_protocol::DirectionBasis::FirstSeen => "first_seen",
                packetsage_protocol::DirectionBasis::ConfigOverride => "config_override",
            }
            .to_owned(),
        })
        .collect();
    repo.upsert_sessions(&sessions).await?;

    let alerts: Vec<AlertRow> = store
        .alerts
        .iter()
        .map(|alert| AlertRow {
            id: alert.alert_id.clone(),
            task_id: store.task_id.clone(),
            rule_id: alert.rule_id.clone(),
            rule_version: i64::from(alert.rule_version),
            rule_content_hash: alert.rule_content_hash.clone(),
            severity: alert.severity.clone(),
            first_packet: i64::try_from(alert.first_packet).unwrap_or(i64::MAX),
            last_packet: i64::try_from(alert.last_packet).unwrap_or(i64::MAX),
            first_ts: alert.first_ts_ns.clone(),
            last_ts: alert.last_ts_ns.clone(),
            src_ip: group_value(&alert.group_key, "src_ip"),
            dst_ip: group_value(&alert.group_key, "dst_ip"),
            session_id: alert.session_id.clone(),
            group_json: serde_json::to_string(&alert.group_key).unwrap_or_else(|_| "[]".to_owned()),
            evidence_json: serde_json::to_string(&alert.evidence)
                .unwrap_or_else(|_| "{}".to_owned()),
        })
        .collect();
    repo.upsert_alerts(&alerts).await?;

    let findings: Vec<FindingRow> = store
        .findings
        .iter()
        .map(|finding| FindingRow {
            id: finding.finding_id.clone(),
            task_id: store.task_id.clone(),
            title: finding.title.clone(),
            severity: finding.severity.as_str().to_owned(),
            basis: format!("{:?}", finding.basis).to_lowercase(),
            summary: finding.summary.clone(),
            evidence_json: serde_json::to_string(&finding.evidence)
                .unwrap_or_else(|_| "[]".to_owned()),
            validator_status: format!("{:?}", finding.validator_status).to_lowercase(),
        })
        .collect();
    if !findings.is_empty() {
        repo.upsert_findings(&findings).await?;
    }

    // Agent run + tool-call ledger (three-fixed metadata, M3~M6 §3.7/§5.1).
    if let Some(meta) = &store.report_meta {
        repo.upsert_agent_run(&AgentRunRow {
            id: meta.agent_run_id.clone(),
            task_id: store.task_id.clone(),
            model: meta.model.clone(),
            status: meta.status.clone(),
            started_at: store.started_at.clone(),
            finished_at: Some(packetsage_core::pipeline::now_rfc3339()),
            report_path: Some(meta.report_path.clone()),
            prompt_version: meta.prompt_version.clone(),
            temperature: meta.temperature,
            tokens_in: i64::try_from(meta.tokens_in).unwrap_or(i64::MAX),
            tokens_out: i64::try_from(meta.tokens_out).unwrap_or(i64::MAX),
            cost_cents: i64::try_from(meta.cost_cents).unwrap_or(i64::MAX),
        })
        .await?;
        for (step, (id, entry)) in store.ledger.entries().into_iter().enumerate() {
            repo.insert_tool_call(&ToolCallRow {
                id,
                agent_run_id: meta.agent_run_id.clone(),
                step: i64::try_from(step).unwrap_or(i64::MAX),
                tool_name: entry.method,
                args_json: entry.args_json,
                result_summary: None,
                status: entry.status,
                duration_ms: i64::try_from(entry.duration_ms).unwrap_or(i64::MAX),
                created_at: packetsage_core::pipeline::now_rfc3339(),
                envelope_hash: None,
                redactions_json: None,
                numbers_json: serde_json::to_string(&entry.numbers)
                    .unwrap_or_else(|_| "{}".to_owned()),
                ref_ids_json: serde_json::to_string(&entry.ref_ids)
                    .unwrap_or_else(|_| "[]".to_owned()),
                tokens_json: serde_json::to_string(&entry.tokens)
                    .unwrap_or_else(|_| "[]".to_owned()),
            })
            .await?;
        }
    }
    Ok(())
}

fn group_value(group: &[(String, String)], field: &str) -> Option<String> {
    group
        .iter()
        .find(|(key, _)| key == field)
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
}
