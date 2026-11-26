//! Accounting errors: database failures, I/O failures, invalid
//! caller input and malformed import files.
use df_db::DbError;
use thiserror::Error;

/// Errors an accounting export or import can return.
#[derive(Debug, Error)]
pub enum AccountingError {
    #[error("database error: {0}")]
    Db(#[from] DbError),
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid accounting input: {0}")]
    InvalidInput(String),
    #[error("malformed import file: {0}")]
    Parse(String),
}
