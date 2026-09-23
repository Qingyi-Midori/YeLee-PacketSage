//! PacketSage storage (L1', ADR-014): the only place in the workspace where
//! `sqlx` may appear.

#![forbid(unsafe_code)]
// Tests assert with `expect()`; production code uses `?` (workspace lints).
#![cfg_attr(test, allow(clippy::expect_used))]

pub mod models;
pub mod repository;
pub mod sqlite;

pub use models::{
    AgentRunRow, AlertFilter, AlertRow, ArtifactRow, CaptureRow, FindingRow, SessionRow, TaskRow,
    ToolCallRow,
};
pub use repository::Repository;
pub use sqlite::{MigrationEntry, MigrationStatus, SqliteRepo};

/// Storage errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The database rejected the statement.
    #[error("database error: {0}")]
    Database(String),
    /// The URL could not be understood.
    #[error("invalid database url: {0}")]
    InvalidUrl(String),
    /// The requested entity does not exist.
    #[error("not found: {0}")]
    NotFound(String),
}

impl From<sqlx::Error> for StorageError {
    fn from(value: sqlx::Error) -> Self {
        StorageError::Database(value.to_string())
    }
}

impl From<sqlx::migrate::MigrateError> for StorageError {
    fn from(value: sqlx::migrate::MigrateError) -> Self {
        StorageError::Database(value.to_string())
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, StorageError>;
