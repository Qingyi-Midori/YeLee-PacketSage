//! Repository abstraction (开发文档 §20): SQLite and PostgreSQL share it.

use async_trait::async_trait;

use crate::models::{
    AgentRunRow, AlertFilter, AlertRow, ArtifactRow, CaptureRow, FindingRow, SessionRow, TaskRow,
    ToolCallRow,
};
use crate::Result;

/// Everything the engine needs to persist and query.
#[async_trait]
pub trait Repository: Send + Sync {
    /// Applies the migrations (idempotent).
    async fn migrate(&self) -> Result<()>;

    /// Inserts or replaces a task row.
    async fn upsert_task(&self, row: &TaskRow) -> Result<()>;

    /// Inserts or replaces a capture row.
    async fn upsert_capture(&self, row: &CaptureRow) -> Result<()>;

    /// Inserts or replaces session rows.
    async fn upsert_sessions(&self, rows: &[SessionRow]) -> Result<()>;

    /// Inserts or replaces alert rows.
    async fn upsert_alerts(&self, rows: &[AlertRow]) -> Result<()>;

    /// Inserts finding rows.
    async fn upsert_findings(&self, rows: &[FindingRow]) -> Result<()>;

    /// Records an agent run.
    async fn upsert_agent_run(&self, row: &AgentRunRow) -> Result<()>;

    /// Records a tool call (the ledger backing V2).
    async fn insert_tool_call(&self, row: &ToolCallRow) -> Result<()>;

    /// Records an artifact (report / event stream).
    async fn upsert_artifact(&self, row: &ArtifactRow) -> Result<()>;

    /// Lists tasks, newest first.
    async fn list_tasks(&self, limit: usize) -> Result<Vec<TaskRow>>;

    /// Loads one task.
    async fn get_task(&self, task_id: &str) -> Result<Option<TaskRow>>;

    /// Loads one capture row.
    async fn get_capture(&self, task_id: &str) -> Result<Option<CaptureRow>>;

    /// Lists sessions of one task ordered by bytes.
    async fn list_sessions(&self, task_id: &str, limit: usize) -> Result<Vec<SessionRow>>;

    /// Lists alerts matching a structured filter.
    async fn list_alerts(&self, filter: &AlertFilter) -> Result<Vec<AlertRow>>;

    /// Lists findings of one task.
    async fn list_findings(&self, task_id: &str, limit: usize) -> Result<Vec<FindingRow>>;

    /// Lists tool calls of one agent run.
    async fn list_tool_calls(&self, agent_run_id: &str) -> Result<Vec<ToolCallRow>>;
    /// Ledger rows of one task, across every agent run that touched it.
    async fn list_tool_calls_for_task(
        &self,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<ToolCallRow>>;

    /// True when the given task id was already stored.
    async fn task_exists(&self, task_id: &str) -> Result<bool>;
}
