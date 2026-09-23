//! SQLite implementation of [`Repository`] (ADR-004).

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Column, Row, SqlitePool, ValueRef};
use std::str::FromStr;
use std::time::Duration;

use crate::models::{
    AgentRunRow, AlertFilter, AlertRow, ArtifactRow, CaptureRow, FindingRow, SessionRow, TaskRow,
    ToolCallRow,
};
use crate::repository::Repository;
use crate::{Result, StorageError};

/// SQLite repository.
#[derive(Debug, Clone)]
pub struct SqliteRepo {
    pool: SqlitePool,
}

impl SqliteRepo {
    /// Connects to `sqlite://path` (or `sqlite::memory:`).
    ///
    /// # Errors
    /// Returns [`StorageError`] when the database cannot be opened.
    pub async fn connect(url: &str) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(url)
            .map_err(|e| StorageError::InvalidUrl(format!("{url}: {e}")))?
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    /// In-memory database (tests).
    ///
    /// # Errors
    /// Returns [`StorageError`] when the pool cannot be created.
    pub async fn in_memory() -> Result<Self> {
        Self::connect("sqlite::memory:").await
    }

    /// Underlying pool.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// One entry of the migration plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationEntry {
    /// sqlx migration version (timestamp of the file).
    pub version: i64,
    /// Human readable description.
    pub description: String,
}

/// Applied vs pending migrations (`packetsage db migrate`, CLI spec §8.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationStatus {
    /// Migrations recorded in `_sqlx_migrations`.
    pub applied: Vec<MigrationEntry>,
    /// Migrations shipped by the binary but not applied yet.
    pub pending: Vec<MigrationEntry>,
}

impl MigrationStatus {
    /// Highest applied version, when any.
    #[must_use]
    pub fn current(&self) -> Option<i64> {
        self.applied.iter().map(|entry| entry.version).max()
    }

    /// Highest version shipped by the binary, when any.
    #[must_use]
    pub fn target(&self) -> Option<i64> {
        self.applied
            .iter()
            .chain(self.pending.iter())
            .map(|entry| entry.version)
            .max()
    }

    /// True when the schema is up to date.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.pending.is_empty()
    }
}

/// The migrations shipped with this build.
fn migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!("../../migrations")
}

impl SqliteRepo {
    /// Opens a **read-only** connection, used by the `db query` guardrail and by
    /// doctor's non-destructive probe (§8.1, §11.1 item 5).
    ///
    /// A missing file is an error here: read-only mode never creates one.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the URL is malformed or the file cannot be
    /// opened read-only.
    pub async fn connect_readonly(url: &str, busy_timeout: Duration) -> Result<Self> {
        Self::connect_readonly_inner(url, Some(busy_timeout)).await
    }

    /// Variant used by doctor, which must not create a missing database.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the database cannot be opened read-only.
    pub async fn connect_readonly_default(url: &str) -> Result<Self> {
        Self::connect_readonly_inner(url, None).await
    }

    async fn connect_readonly_inner(url: &str, busy_timeout: Option<Duration>) -> Result<Self> {
        let mut options = SqliteConnectOptions::from_str(url)
            .map_err(|e| StorageError::InvalidUrl(format!("{url}: {e}")))?
            .create_if_missing(false)
            .read_only(true);
        if let Some(timeout) = busy_timeout {
            options = options.busy_timeout(timeout);
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    /// Reads the migration plan: what is applied, what is still pending.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the ledger cannot be read. A database that
    /// was never migrated simply reports every migration as pending.
    pub async fn migration_status(&self) -> Result<MigrationStatus> {
        let applied_versions: Vec<i64> = match sqlx::query_scalar::<_, i64>(
            "SELECT version FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&self.pool)
        .await
        {
            Ok(versions) => versions,
            Err(error) if is_missing_table(&error) => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        let mut status = MigrationStatus::default();
        for migration in migrator().iter() {
            let entry = MigrationEntry {
                version: migration.version,
                description: migration.description.to_string(),
            };
            if applied_versions.contains(&entry.version) {
                status.applied.push(entry);
            } else {
                status.pending.push(entry);
            }
        }
        Ok(status)
    }

    /// Applies the pending migrations and returns the ones that ran.
    ///
    /// # Errors
    /// Returns [`StorageError`] when a migration fails.
    pub async fn apply_migrations(&self) -> Result<Vec<MigrationEntry>> {
        let pending = self.migration_status().await?.pending;
        migrator().run(&self.pool).await?;
        Ok(pending)
    }

    /// Best-effort probe for another writer (ADR-018 single writer).
    ///
    /// `BEGIN IMMEDIATE` with a short `busy_timeout` fails when a `serve`
    /// process holds the write lock. SQLite locks are transactional, so this
    /// can only ever be a probabilistic warning (#C5): a TOCTOU window remains.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the probe cannot even connect.
    pub async fn write_lock_probe(&self) -> Result<bool> {
        let mut connection = self.pool.acquire().await?;
        sqlx::query("PRAGMA busy_timeout = 500")
            .execute(&mut *connection)
            .await?;
        match sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
        {
            Ok(_) => {
                sqlx::query("COMMIT").execute(&mut *connection).await?;
                Ok(false)
            }
            Err(_) => Ok(true),
        }
    }

    /// Runs a read-only statement and returns one JSON object per row.
    ///
    /// The trailing semicolon is stripped and, unless this is an `EXPLAIN`,
    /// the statement is wrapped as `SELECT * FROM (<sql>) LIMIT <limit>` —
    /// which is also the only long-query protection sqlx/SQLite offers (#36).
    ///
    /// # Errors
    /// Returns [`StorageError`] when the statement fails.
    pub async fn query_json_rows(
        &self,
        sql: &str,
        limit: Option<u64>,
    ) -> Result<Vec<serde_json::Value>> {
        let statement = sql.trim().trim_end_matches(';').trim();
        let bounded = match limit {
            Some(limit) if !statement.to_ascii_lowercase().starts_with("explain") => {
                format!("SELECT * FROM ({statement}) LIMIT {limit}")
            }
            _ => statement.to_owned(),
        };
        let rows = sqlx::query(&bounded).fetch_all(&self.pool).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut object = serde_json::Map::new();
            for (index, column) in row.columns().iter().enumerate() {
                object.insert(column.name().to_owned(), cell(&row, index));
            }
            out.push(serde_json::Value::Object(object));
        }
        Ok(out)
    }

    /// Column names of a query, for the human readable table header.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the statement fails.
    pub async fn query_table(
        &self,
        sql: &str,
        limit: Option<u64>,
    ) -> Result<(Vec<String>, Vec<serde_json::Value>)> {
        let rows = self.query_json_rows(sql, limit).await?;
        let columns = rows
            .first()
            .and_then(|row| row.as_object())
            .map(|object| object.keys().cloned().collect())
            .unwrap_or_default();
        Ok((columns, rows))
    }
}

/// True when the error says the table does not exist yet.
fn is_missing_table(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|db| db.message().contains("no such table"))
}

/// Converts one SQLite cell into a JSON value.
fn cell(row: &sqlx::sqlite::SqliteRow, index: usize) -> serde_json::Value {
    use sqlx::Row;

    if let Ok(raw) = row.try_get_raw(index) {
        if raw.is_null() {
            return serde_json::Value::Null;
        }
    }
    if let Ok(value) = row.try_get::<i64, _>(index) {
        return serde_json::Value::from(value);
    }
    if let Ok(value) = row.try_get::<f64, _>(index) {
        return serde_json::Value::from(value);
    }
    if let Ok(value) = row.try_get::<String, _>(index) {
        return serde_json::Value::from(value);
    }
    if let Ok(value) = row.try_get::<Vec<u8>, _>(index) {
        return serde_json::Value::from(to_hex(&value));
    }
    serde_json::Value::from("<unreadable>")
}

/// Lowercase hex for BLOB columns (kept local: storage adds no dependency).
fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[async_trait]
impl Repository for SqliteRepo {
    async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("../../migrations").run(&self.pool).await?;
        Ok(())
    }

    async fn upsert_task(&self, row: &TaskRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO analysis_tasks (id, status, source_path, source_sha256, started_at, \
             finished_at, packet_count, byte_count, error_code, index_mode, rules_hash) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, \
             finished_at = excluded.finished_at, packet_count = excluded.packet_count, \
             byte_count = excluded.byte_count, error_code = excluded.error_code, \
             index_mode = excluded.index_mode, rules_hash = excluded.rules_hash",
        )
        .bind(&row.id)
        .bind(&row.status)
        .bind(&row.source_path)
        .bind(&row.source_sha256)
        .bind(&row.started_at)
        .bind(&row.finished_at)
        .bind(row.packet_count)
        .bind(row.byte_count)
        .bind(&row.error_code)
        .bind(&row.index_mode)
        .bind(&row.rules_hash)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn upsert_capture(&self, row: &CaptureRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO captures (task_id, format, first_ts, last_ts, interfaces, \
             linktypes_json) VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(task_id) DO UPDATE SET format = excluded.format, \
             first_ts = excluded.first_ts, last_ts = excluded.last_ts, \
             interfaces = excluded.interfaces, linktypes_json = excluded.linktypes_json",
        )
        .bind(&row.task_id)
        .bind(&row.format)
        .bind(&row.first_ts)
        .bind(&row.last_ts)
        .bind(row.interfaces)
        .bind(&row.linktypes_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn upsert_sessions(&self, rows: &[SessionRow]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for row in rows {
            sqlx::query(
                "INSERT INTO sessions (id, task_id, protocol, src_ip, src_port, dst_ip, \
                 dst_port, first_ts, last_ts, packets, bytes, state, app_protocol, interface_id, \
                 vlan_tag, direction_basis) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(id) DO UPDATE SET packets = excluded.packets, \
                 bytes = excluded.bytes, last_ts = excluded.last_ts, state = excluded.state, \
                 app_protocol = excluded.app_protocol",
            )
            .bind(&row.id)
            .bind(&row.task_id)
            .bind(&row.protocol)
            .bind(&row.src_ip)
            .bind(row.src_port)
            .bind(&row.dst_ip)
            .bind(row.dst_port)
            .bind(&row.first_ts)
            .bind(&row.last_ts)
            .bind(row.packets)
            .bind(row.bytes)
            .bind(&row.state)
            .bind(&row.app_protocol)
            .bind(row.interface_id)
            .bind(row.vlan_tag)
            .bind(&row.direction_basis)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn upsert_alerts(&self, rows: &[AlertRow]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for row in rows {
            sqlx::query(
                "INSERT INTO alerts (id, task_id, rule_id, rule_version, rule_content_hash, \
                 severity, first_packet, last_packet, first_ts, last_ts, src_ip, dst_ip, \
                 session_id, group_json, evidence_json) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(id) DO NOTHING",
            )
            .bind(&row.id)
            .bind(&row.task_id)
            .bind(&row.rule_id)
            .bind(row.rule_version)
            .bind(&row.rule_content_hash)
            .bind(&row.severity)
            .bind(row.first_packet)
            .bind(row.last_packet)
            .bind(&row.first_ts)
            .bind(&row.last_ts)
            .bind(&row.src_ip)
            .bind(&row.dst_ip)
            .bind(&row.session_id)
            .bind(&row.group_json)
            .bind(&row.evidence_json)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn upsert_findings(&self, rows: &[FindingRow]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for row in rows {
            sqlx::query(
                "INSERT INTO findings (id, task_id, title, severity, basis, summary, \
                 evidence_json, validator_status) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(id) DO UPDATE SET severity = excluded.severity, \
                 basis = excluded.basis, summary = excluded.summary, \
                 evidence_json = excluded.evidence_json, \
                 validator_status = excluded.validator_status",
            )
            .bind(&row.id)
            .bind(&row.task_id)
            .bind(&row.title)
            .bind(&row.severity)
            .bind(&row.basis)
            .bind(&row.summary)
            .bind(&row.evidence_json)
            .bind(&row.validator_status)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn upsert_agent_run(&self, row: &AgentRunRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO agent_runs (id, task_id, model, status, started_at, finished_at, \
             report_path, prompt_version, temperature, tokens_in, tokens_out, cost_cents) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, \
             finished_at = excluded.finished_at, report_path = excluded.report_path, \
             tokens_in = excluded.tokens_in, tokens_out = excluded.tokens_out, \
             cost_cents = excluded.cost_cents",
        )
        .bind(&row.id)
        .bind(&row.task_id)
        .bind(&row.model)
        .bind(&row.status)
        .bind(&row.started_at)
        .bind(&row.finished_at)
        .bind(&row.report_path)
        .bind(&row.prompt_version)
        .bind(row.temperature)
        .bind(row.tokens_in)
        .bind(row.tokens_out)
        .bind(row.cost_cents)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_tool_call(&self, row: &ToolCallRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO tool_calls (id, agent_run_id, step, tool_name, args_json, \
             result_summary, status, duration_ms, created_at, envelope_hash, redactions_json, \
             numbers_json, ref_ids_json, tokens_json) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
        )
        .bind(&row.id)
        .bind(&row.agent_run_id)
        .bind(row.step)
        .bind(&row.tool_name)
        .bind(&row.args_json)
        .bind(&row.result_summary)
        .bind(&row.status)
        .bind(row.duration_ms)
        .bind(&row.created_at)
        .bind(&row.envelope_hash)
        .bind(&row.redactions_json)
        .bind(&row.numbers_json)
        .bind(&row.ref_ids_json)
        .bind(&row.tokens_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn upsert_artifact(&self, row: &ArtifactRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO artifacts (id, task_id, kind, path, sha256, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET path = excluded.path, \
             sha256 = excluded.sha256, created_at = excluded.created_at",
        )
        .bind(&row.id)
        .bind(&row.task_id)
        .bind(&row.kind)
        .bind(&row.path)
        .bind(&row.sha256)
        .bind(&row.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_tasks(&self, limit: usize) -> Result<Vec<TaskRow>> {
        let rows = sqlx::query(
            "SELECT id, status, source_path, source_sha256, started_at, finished_at, \
             packet_count, byte_count, error_code, index_mode, rules_hash \
             FROM analysis_tasks ORDER BY started_at DESC, id DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(map_task).collect())
    }

    async fn get_task(&self, task_id: &str) -> Result<Option<TaskRow>> {
        let row = sqlx::query(
            "SELECT id, status, source_path, source_sha256, started_at, finished_at, \
             packet_count, byte_count, error_code, index_mode, rules_hash \
             FROM analysis_tasks WHERE id = ?",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.as_ref().map(map_task))
    }

    async fn get_capture(&self, task_id: &str) -> Result<Option<CaptureRow>> {
        let row = sqlx::query(
            "SELECT task_id, format, first_ts, last_ts, interfaces, linktypes_json \
             FROM captures WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| CaptureRow {
            task_id: row.get("task_id"),
            format: row.get("format"),
            first_ts: row.get("first_ts"),
            last_ts: row.get("last_ts"),
            interfaces: row.get::<Option<i64>, _>("interfaces").unwrap_or(0),
            linktypes_json: row.get("linktypes_json"),
        }))
    }

    async fn list_sessions(&self, task_id: &str, limit: usize) -> Result<Vec<SessionRow>> {
        let rows = sqlx::query(
            "SELECT id, task_id, protocol, src_ip, src_port, dst_ip, dst_port, first_ts, \
             last_ts, packets, bytes, state, app_protocol, interface_id, vlan_tag, \
             direction_basis FROM sessions WHERE task_id = ? ORDER BY bytes DESC, id LIMIT ?",
        )
        .bind(task_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| SessionRow {
                id: row.get("id"),
                task_id: row.get("task_id"),
                protocol: row.get("protocol"),
                src_ip: row.get("src_ip"),
                src_port: row.get::<Option<i64>, _>("src_port").unwrap_or(0),
                dst_ip: row.get("dst_ip"),
                dst_port: row.get::<Option<i64>, _>("dst_port").unwrap_or(0),
                first_ts: row.get("first_ts"),
                last_ts: row.get("last_ts"),
                packets: row.get("packets"),
                bytes: row.get("bytes"),
                state: row.get::<Option<String>, _>("state").unwrap_or_default(),
                app_protocol: row.get("app_protocol"),
                interface_id: row.get("interface_id"),
                vlan_tag: row.get("vlan_tag"),
                direction_basis: row
                    .get::<Option<String>, _>("direction_basis")
                    .unwrap_or_default(),
            })
            .collect())
    }

    async fn list_alerts(&self, filter: &AlertFilter) -> Result<Vec<AlertRow>> {
        let mut sql = String::from(
            "SELECT id, task_id, rule_id, rule_version, rule_content_hash, severity, \
             first_packet, last_packet, first_ts, last_ts, src_ip, dst_ip, session_id, \
             group_json, evidence_json FROM alerts WHERE 1 = 1",
        );
        if filter.task_id.is_some() {
            sql.push_str(" AND task_id = ?");
        }
        if filter.severity.is_some() {
            sql.push_str(" AND severity = ?");
        }
        if filter.rule_id.is_some() {
            sql.push_str(" AND rule_id = ?");
        }
        if filter.session_id.is_some() {
            sql.push_str(" AND session_id = ?");
        }
        sql.push_str(" ORDER BY first_ts ASC, id LIMIT ?");

        let mut query = sqlx::query(&sql);
        if let Some(task_id) = &filter.task_id {
            query = query.bind(task_id);
        }
        if let Some(severity) = &filter.severity {
            query = query.bind(severity);
        }
        if let Some(rule_id) = &filter.rule_id {
            query = query.bind(rule_id);
        }
        if let Some(session_id) = &filter.session_id {
            query = query.bind(session_id);
        }
        query = query.bind(filter.limit.unwrap_or(50) as i64);

        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows
            .iter()
            .map(|row| AlertRow {
                id: row.get("id"),
                task_id: row.get("task_id"),
                rule_id: row.get("rule_id"),
                rule_version: row.get("rule_version"),
                rule_content_hash: row.get("rule_content_hash"),
                severity: row.get("severity"),
                first_packet: row.get("first_packet"),
                last_packet: row.get("last_packet"),
                first_ts: row.get::<Option<String>, _>("first_ts").unwrap_or_default(),
                last_ts: row.get::<Option<String>, _>("last_ts").unwrap_or_default(),
                src_ip: row.get("src_ip"),
                dst_ip: row.get("dst_ip"),
                session_id: row.get("session_id"),
                group_json: row.get("group_json"),
                evidence_json: row.get("evidence_json"),
            })
            .collect())
    }

    async fn list_findings(&self, task_id: &str, limit: usize) -> Result<Vec<FindingRow>> {
        let rows = sqlx::query(
            "SELECT id, task_id, title, severity, basis, summary, evidence_json, \
             validator_status FROM findings WHERE task_id = ? ORDER BY id LIMIT ?",
        )
        .bind(task_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| FindingRow {
                id: row.get("id"),
                task_id: row.get("task_id"),
                title: row.get("title"),
                severity: row.get("severity"),
                basis: row.get("basis"),
                summary: row.get("summary"),
                evidence_json: row.get("evidence_json"),
                validator_status: row.get("validator_status"),
            })
            .collect())
    }

    async fn list_tool_calls(&self, agent_run_id: &str) -> Result<Vec<ToolCallRow>> {
        let rows = sqlx::query(
            "SELECT id, agent_run_id, step, tool_name, args_json, result_summary, status, \
             duration_ms, created_at, envelope_hash, redactions_json, numbers_json, \
             ref_ids_json, tokens_json FROM tool_calls WHERE agent_run_id = ? ORDER BY step, id",
        )
        .bind(agent_run_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| ToolCallRow {
                id: row.get("id"),
                agent_run_id: row.get("agent_run_id"),
                step: row.get("step"),
                tool_name: row.get("tool_name"),
                args_json: row.get("args_json"),
                result_summary: row.get("result_summary"),
                status: row.get("status"),
                duration_ms: row.get("duration_ms"),
                created_at: row.get("created_at"),
                envelope_hash: row.get("envelope_hash"),
                redactions_json: row.get("redactions_json"),
                numbers_json: row.get("numbers_json"),
                ref_ids_json: row.get("ref_ids_json"),
                tokens_json: row.get("tokens_json"),
            })
            .collect())
    }

    /// Every ledger row of one task, across the agent runs that touched it.
    ///
    /// Cold recovery (ADR-019) uses this to rebuild the evidence facts a report
    /// needs, without knowing which run produced them.
    async fn list_tool_calls_for_task(
        &self,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<ToolCallRow>> {
        let rows = sqlx::query(
            "SELECT tc.id, tc.agent_run_id, tc.step, tc.tool_name, tc.args_json, \
             tc.result_summary, tc.status, tc.duration_ms, tc.created_at, tc.envelope_hash, \
             tc.redactions_json, tc.numbers_json, tc.ref_ids_json, tc.tokens_json \
             FROM tool_calls tc \
             JOIN agent_runs ar ON ar.id = tc.agent_run_id \
             WHERE ar.task_id = ? ORDER BY tc.created_at, tc.step, tc.id LIMIT ?",
        )
        .bind(task_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| ToolCallRow {
                id: row.get("id"),
                agent_run_id: row.get("agent_run_id"),
                step: row.get("step"),
                tool_name: row.get("tool_name"),
                args_json: row.get("args_json"),
                result_summary: row.get("result_summary"),
                status: row.get("status"),
                duration_ms: row.get("duration_ms"),
                created_at: row.get("created_at"),
                envelope_hash: row.get("envelope_hash"),
                redactions_json: row.get("redactions_json"),
                numbers_json: row.get("numbers_json"),
                ref_ids_json: row.get("ref_ids_json"),
                tokens_json: row.get("tokens_json"),
            })
            .collect())
    }

    async fn task_exists(&self, task_id: &str) -> Result<bool> {
        let row = sqlx::query("SELECT 1 AS one FROM analysis_tasks WHERE id = ?")
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.is_some())
    }
}

fn map_task(row: &sqlx::sqlite::SqliteRow) -> TaskRow {
    TaskRow {
        id: row.get("id"),
        status: row.get("status"),
        source_path: row.get("source_path"),
        source_sha256: row.get("source_sha256"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        packet_count: row.get::<Option<i64>, _>("packet_count").unwrap_or(0),
        byte_count: row.get::<Option<i64>, _>("byte_count").unwrap_or(0),
        error_code: row.get("error_code"),
        index_mode: row
            .get::<Option<String>, _>("index_mode")
            .unwrap_or_else(|| "FULL".to_owned()),
        rules_hash: row
            .get::<Option<String>, _>("rules_hash")
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> TaskRow {
        TaskRow {
            id: id.to_owned(),
            status: "ok".to_owned(),
            source_path: "x.pcap".to_owned(),
            source_sha256: "abc".to_owned(),
            started_at: "2026-01-01T00:00:00Z".to_owned(),
            finished_at: Some("2026-01-01T00:00:01Z".to_owned()),
            packet_count: 7,
            byte_count: 500,
            error_code: None,
            index_mode: "FULL".to_owned(),
            rules_hash: "deadbeef".to_owned(),
        }
    }

    fn session_row(task_id: &str) -> SessionRow {
        SessionRow {
            id: "S-000001".to_owned(),
            task_id: task_id.to_owned(),
            protocol: "tcp".to_owned(),
            src_ip: "10.0.0.1".to_owned(),
            src_port: 40000,
            dst_ip: "10.0.0.2".to_owned(),
            dst_port: 80,
            first_ts: "1".to_owned(),
            last_ts: "2".to_owned(),
            packets: 10,
            bytes: 1000,
            state: "closed".to_owned(),
            app_protocol: Some("http".to_owned()),
            interface_id: Some(0),
            vlan_tag: None,
            direction_basis: "syn_first".to_owned(),
        }
    }

    fn alert_row(task_id: &str) -> AlertRow {
        AlertRow {
            id: "alert_1".to_owned(),
            task_id: task_id.to_owned(),
            rule_id: "NET-TCP-SYN-BURST-001".to_owned(),
            rule_version: 1,
            rule_content_hash: "a1b2c3d4".to_owned(),
            severity: "high".to_owned(),
            first_packet: 0,
            last_packet: 10,
            first_ts: "1".to_owned(),
            last_ts: "2".to_owned(),
            src_ip: Some("10.0.0.1".to_owned()),
            dst_ip: None,
            session_id: None,
            group_json: "[[\"src_ip\",\"10.0.0.1\"]]".to_owned(),
            evidence_json: "{}".to_owned(),
        }
    }

    #[tokio::test]
    async fn migrate_round_trip() {
        let repo = SqliteRepo::in_memory().await.expect("pool");
        repo.migrate().await.expect("migrate");
        repo.upsert_task(&task("task_TEST0000000000000000000000"))
            .await
            .expect("task");
        assert!(repo
            .task_exists("task_TEST0000000000000000000000")
            .await
            .expect("exists"));
        let rows = repo.list_tasks(10).await.expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].packet_count, 7);
        assert_eq!(rows[0].index_mode, "FULL");
    }

    #[tokio::test]
    async fn sessions_and_alerts_round_trip() {
        let repo = SqliteRepo::in_memory().await.expect("pool");
        repo.migrate().await.expect("migrate");
        repo.upsert_task(&task("t1")).await.expect("task");
        repo.upsert_capture(&CaptureRow {
            task_id: "t1".to_owned(),
            format: "pcap".to_owned(),
            first_ts: Some("1".to_owned()),
            last_ts: Some("2".to_owned()),
            interfaces: 1,
            linktypes_json: "[1]".to_owned(),
        })
        .await
        .expect("capture");
        repo.upsert_sessions(&[session_row("t1")])
            .await
            .expect("sessions");
        repo.upsert_alerts(&[alert_row("t1")])
            .await
            .expect("alerts");

        let sessions = repo.list_sessions("t1", 10).await.expect("list sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "S-000001");
        assert_eq!(sessions[0].bytes, 1000);

        let capture = repo.get_capture("t1").await.expect("capture").expect("row");
        assert_eq!(capture.format, "pcap");

        let alerts = repo
            .list_alerts(&AlertFilter {
                task_id: Some("t1".to_owned()),
                severity: Some("high".to_owned()),
                ..AlertFilter::default()
            })
            .await
            .expect("list alerts");
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "NET-TCP-SYN-BURST-001");

        let none = repo
            .list_alerts(&AlertFilter {
                task_id: Some("t1".to_owned()),
                severity: Some("low".to_owned()),
                ..AlertFilter::default()
            })
            .await
            .expect("list alerts");
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn invalid_url_is_reported() {
        assert!(SqliteRepo::connect("postgres://nope").await.is_err());
    }
}
