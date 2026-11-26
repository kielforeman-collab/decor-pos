//! Online bill pay (Phase 5, v0.6; spec §6.1
//! `POST /portal/payments/intent`, §7 Phase 5 exit test).
//!
//! A bill payment is an amount + method + idempotency key. The
//! edge authorizes the amount through the configured
//! `PaymentProvider` (mock in dev — no live payment API, no card
//! data stored) and, only on approval, posts it to AR with FIFO
//! allocation (`df-ar::allocate_payment` via
//! `Edge::post_ar_payment`). The idempotency key makes a retried
//! payment a no-op: the original payment record is returned and
//! nothing is posted twice.

use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::PortalError;

/// A portal bill-payment request. `method` is a label only
/// (`card` or `ach`); `idempotency_key` is client-generated and
/// unique per payment attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BillPayIntent {
    pub customer_id: Uuid,
    pub amount: Money,
    pub method: String,
    pub idempotency_key: String,
}

/// What a completed (or replayed) bill payment returns: the AR
/// payment entry plus the FIFO allocations it made, oldest charge
/// first, and the provider reference for the authorization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BillPayResult {
    pub payment_entry_id: Uuid,
    pub customer_id: Uuid,
    pub amount: Money,
    pub method: String,
    pub provider_reference: String,
    /// (charge entry id, allocated amount), oldest charge first.
    pub allocations: Vec<(Uuid, Decimal)>,
    /// True when this call replayed an earlier payment.
    pub replayed: bool,
}

/// Validate a bill-pay intent before any provider call: positive
/// amount, store currency, a known method label, a non-empty
/// idempotency key.
pub fn validate_bill_pay(
    intent: &BillPayIntent,
    store_currency: Currency,
) -> Result<(), PortalError> {
    if intent.amount.is_zero() || intent.amount.is_negative() {
        return Err(PortalError::InvalidAmount(
            intent.amount.amount().to_string(),
        ));
    }
    if intent.amount.currency() != store_currency {
        return Err(PortalError::WrongCurrency);
    }
    match intent.method.as_str() {
        "card" | "ach" => {}
        other => {
            return Err(PortalError::PaymentDeclined(format!(
                "unsupported bill-pay method: {other}"
            )));
        }
    }
    if intent.idempotency_key.trim().is_empty() {
        return Err(PortalError::PaymentDeclined(
            "idempotency key is required".to_string(),
        ));
    }
    Ok(())
}
