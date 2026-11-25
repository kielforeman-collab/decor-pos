//! Report errors.

use df_db::DbError;
use df_domain::DomainError;
use thiserror::Error;

/// Errors a report computation can return: database failures, domain
/// math failures (currency mismatch, division by zero) and invalid
/// caller input (e.g. a malformed dispensed-amounts JSON blob).
#[derive(Debug, Error)]
pub enum ReportError {
    #[error("database error: {0}")]
    Db(#[from] DbError),
    #[error("domain error: {0}")]
    Domain(#[from] DomainError),
    #[error("invalid report input: {0}")]
    InvalidInput(String),
}
