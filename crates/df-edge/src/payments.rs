//! Payment provider shapes (Phase 1, v0.2).
//!
//! Re-exports of the real [`df_payments`] crate: the placeholder
//! copies that used to live here were deleted in the Phase 1
//! integration and `df-edge` now depends on `df-payments` directly.
//! [`PaymentProvider`] uses the boxed-future form (a literal
//! `async fn` in a trait is not dyn-compatible in Rust 1.99), so
//! `&dyn PaymentProvider` works everywhere; callers still write
//! `provider.authorize(&req).await`.
//!
//! No card data is stored anywhere: a [`PaymentRequest`] carries only
//! an amount, an idempotency key, a tender kind and a surcharge. The
//! provider returns a reference and an authorization token, never a
//! PAN/CVV.

pub use df_payments::{
    MockTerminal, PaymentError, PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus,
    SurchargeRule, TenderKind, compute_surcharge, surcharge_rule_for,
};
