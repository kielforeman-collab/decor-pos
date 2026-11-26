//! Portal access control (Phase 5, v0.6; spec §6.1 `/portal/v1`).
//!
//! A contractor reaches the portal only when their customer row
//! has `portal_enabled`. There is no separate portal user table:
//! the portal is scoped by customer id, and every portal call
//! re-checks the flag — disabling portal access locks the
//! contractor out immediately.

use thiserror::Error;

/// Errors the portal surface can return. Mapped to HTTP status
/// codes at the `df-edge` boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PortalError {
    #[error("portal access is not enabled for this customer")]
    NotPortalEnabled,
    #[error("formula not found: {0}")]
    FormulaNotFound(String),
    #[error("formula does not belong to this customer")]
    FormulaNotForCustomer,
    #[error("quantity must be positive: {0}")]
    InvalidQty(String),
    #[error("payment amount must be positive: {0}")]
    InvalidAmount(String),
    #[error("payment amount must use the store currency")]
    WrongCurrency,
    #[error("payment declined by provider: {0}")]
    PaymentDeclined(String),
    #[error("order not found: {0}")]
    OrderNotFound(String),
    #[error("order is not a pickup order")]
    OrderNotPickup,
    #[error("order is already completed")]
    OrderAlreadyCompleted,
    #[error("insufficient on-hand stock for {0}: need {1}, have {2}")]
    InsufficientStock(String, String, String),
    #[error("a payment with this idempotency key already exists")]
    DuplicateIdempotencyKey,
    #[error("product not found: {0}")]
    ProductNotFound(String),
}

/// Require portal access for a customer row. Callers pass the
/// row's `portal_enabled` flag.
pub fn require_portal_access(portal_enabled: bool) -> Result<(), PortalError> {
    if portal_enabled {
        Ok(())
    } else {
        Err(PortalError::NotPortalEnabled)
    }
}
