//! Retail BOPIS — buy online, pick up in store (Phase 5, v0.6;
//! spec §4 `contractor_portal_ecom`, §7 Phase 5).
//!
//! A BOPIS order is a public (guest) order: no customer account
//! is required, the buyer gives a name and a contact, picks a
//! pickup slot, and pays by card through the configured
//! `PaymentProvider` (mock in dev — no live payment API). Placing
//! the order reserves stock (committed) and records the payment;
//! the counter completes the pickup, which moves committed stock
//! to sold (on-hand down, committed released) and closes the
//! order. Nothing is ever dispensed or tinted here — a BOPIS line
//! may reference a stored formula for its description, but the
//! paint itself is picked from stock.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::PortalError;

/// One line of a BOPIS order: a catalog product and a quantity
/// in the product's base UOM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BopisLine {
    pub product_id: Uuid,
    pub qty: Decimal,
}

/// A BOPIS pickup-order request. `pickup_name` is who collects
/// it; `contact` is an email or phone for the ready notification;
/// `scheduled_at` is the requested pickup slot (must be in the
/// future — the edge enforces it against the order time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BopisOrderRequest {
    pub lines: Vec<BopisLine>,
    pub pickup_name: String,
    pub contact: String,
    pub scheduled_at: DateTime<Utc>,
}

/// Validate a BOPIS order request before any pricing or payment:
/// at least one line, every quantity positive, a pickup name, and
/// a contact.
pub fn validate_bopis_order(req: &BopisOrderRequest) -> Result<(), PortalError> {
    if req.lines.is_empty() {
        return Err(PortalError::InvalidQty(
            "order needs at least one line".to_string(),
        ));
    }
    for line in &req.lines {
        if line.qty <= Decimal::ZERO {
            return Err(PortalError::InvalidQty(line.qty.to_string()));
        }
    }
    if req.pickup_name.trim().is_empty() {
        return Err(PortalError::OrderNotFound(
            "pickup name is required".to_string(),
        ));
    }
    if req.contact.trim().is_empty() {
        return Err(PortalError::OrderNotFound(
            "a contact (email or phone) is required".to_string(),
        ));
    }
    Ok(())
}
