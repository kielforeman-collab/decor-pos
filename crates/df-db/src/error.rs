//! Database errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("invalid data in row: {0}")]
    InvalidData(String),
    #[error("row not found: {0}")]
    NotFound(String),
}
