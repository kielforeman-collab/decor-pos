//! Auth errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("invalid PIN: {0}")]
    InvalidPin(String),
    #[error("hashing failed: {0}")]
    Hashing(String),
    #[error("not implemented in Phase 0: {0}")]
    NotImplemented(&'static str),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
}
