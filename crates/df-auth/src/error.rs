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
    /// The account exists but is deactivated. Login services return
    /// this (never a successful session); HTTP handlers map it to a
    /// generic 401 alongside [`AuthError::InvalidCredentials`] so an
    /// attacker cannot probe which emails are active.
    #[error("account is inactive")]
    AccountInactive,
    /// A session token that is unknown, expired, or revoked. One
    /// variant on purpose: callers must not distinguish the three
    /// cases to the client.
    #[error("invalid or expired session")]
    InvalidSession,
    /// A portal login for a `PortalContractor` user with no linked
    /// customer (`users.portal_customer_id` is NULL). A config error,
    /// not a credential error — fix the link, don't retry the login.
    #[error("portal user has no linked customer")]
    PortalNotLinked,
    /// A database failure surfaced through a login/session service.
    /// The message carries the `DbError` text, never a credential.
    #[error("database error: {0}")]
    Database(String),
}
