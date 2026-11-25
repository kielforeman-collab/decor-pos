//! Sync errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("sync conflict: {0}")]
    Conflict(String),
    #[error("invalid sync payload: {0}")]
    InvalidPayload(String),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
