//! Tinter errors (Phase 2, spec §6.2).
//!
//! All failures are deterministic: a short colorant fails the dispense
//! before anything is decremented, and the failed job is still recorded
//! so the sale line can be traced to it. No floating-point anywhere;
//! volumes are `rust_decimal::Decimal` fluid ounces.

use df_domain::DomainError;
use rust_decimal::Decimal;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum TinterError {
    #[error("unknown colorant code: {0}")]
    UnknownColorant(String),
    #[error("insufficient colorant {code}: need {needed_fl_oz} fl oz, have {on_hand_fl_oz} fl oz")]
    InsufficientColorant {
        code: String,
        needed_fl_oz: Decimal,
        on_hand_fl_oz: Decimal,
    },
    #[error("unknown tint job: {0}")]
    UnknownJob(Uuid),
    #[error("invalid dispense request: {0}")]
    InvalidRequest(String),
    #[error("invalid tint job transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },
    #[error("tint formula error: {0}")]
    Domain(#[from] DomainError),
}
