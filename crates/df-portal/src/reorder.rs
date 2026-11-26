//! Formula reorders (Phase 5, v0.6; spec §6.1
//! `POST /portal/reorders`, §7 Phase 5 exit test).
//!
//! Formula tracking only (Kiel, Nov 26 rescope): a reorder looks
//! up a stored formula and re-creates the sale line for it —
//! same base product, same colorant recipe reference, priced
//! through the normal matrix-pricing path. The POS never drives a
//! tint machine; there is no dispense step here and none is ever
//! added.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::PortalError;

/// Where the reordered paint goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fulfillment {
    /// Pick up at the counter.
    Pickup,
    /// Deliver (joins the Phase 4 dispatch board).
    Delivery,
}

impl Fulfillment {
    pub fn as_str(self) -> &'static str {
        match self {
            Fulfillment::Pickup => "pickup",
            Fulfillment::Delivery => "delivery",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pickup" => Some(Fulfillment::Pickup),
            "delivery" => Some(Fulfillment::Delivery),
            _ => None,
        }
    }
}

/// A portal formula-reorder request: re-create the sale line for
/// a stored formula. `qty` is in gallons of the formula's base
/// product; `job_id` optionally files it under a customer job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReorderRequest {
    pub customer_id: Uuid,
    pub formula_id: Uuid,
    pub qty: Decimal,
    pub job_id: Option<Uuid>,
    pub fulfillment: Fulfillment,
}

/// Validate a reorder request before any database write: positive
/// quantity, and a fulfillment mode the edge understands.
pub fn validate_reorder(req: &ReorderRequest) -> Result<(), PortalError> {
    if req.qty <= Decimal::ZERO {
        return Err(PortalError::InvalidQty(req.qty.to_string()));
    }
    Ok(())
}
