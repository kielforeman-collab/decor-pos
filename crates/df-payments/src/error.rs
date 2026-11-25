//! Payment errors (spec §1, §6.2).
//!
//! A declined payment is a normal business outcome, not an exception:
//! providers return [`PaymentError::Declined`] and the counter flow asks
//! for another tender. `NotSupported` marks a provider capability that
//! exists as a trait method but is not implemented in this phase.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PaymentError {
    #[error("payment declined: {0}")]
    Declined(String),
    #[error("payment provider unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("invalid amount: {0}")]
    InvalidAmount(String),
    #[error("missing credentials: {0}")]
    MissingCredentials(String),
    #[error("not supported: {0}")]
    NotSupported(String),
}
