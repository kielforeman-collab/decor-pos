//! `df-payments` — **Phase 0 stub**.
//!
//! PaymentProvider trait, Stripe Terminal, standalone mode and surcharge rules. Implemented in Phase 1 (spec §7). No card data is stored anywhere in this workspace.
//!
//! This crate intentionally contains no implementation yet. It exists so
//! the workspace layout from spec §3 resolves in Phase 0; do not treat
//! anything here as a working feature.

/// Marker proving the crate links. Removed when the real implementation lands.
pub fn phase0_stub() -> &'static str {
    "df-payments: Phase 0 stub — not implemented"
}
