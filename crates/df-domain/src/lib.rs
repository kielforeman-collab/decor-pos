//! `df-domain` — pure domain models for decor-pos (Phase 0, v0.1).
//!
//! No I/O, no database, no network. Everything here is deterministic
//! and covered by the financial & pricing test suite:
//!
//! * [`money`] — `Money` newtype over `rust_decimal::Decimal` (scale 4).
//!   Never floating-point.
//! * [`uom`] — unit-of-measure conversions, flooring waste + box rounding.
//! * [`pricing`] — matrix pricing with wildcards, priority and margin floor.
//! * [`tax`] — US/CA tax and PaintCare / eco-fee rule engine (sample presets).
//! * [`ar`] — contractor AR aging buckets and finance charges.
//! * [`tint`] — tint formula colorant costing.
//! * [`ids`] — UUIDv7 identifiers.
//! * [`features`] — per-organization module feature flags.

pub mod ar;
pub mod error;
pub mod features;
pub mod ids;
pub mod money;
pub mod pricing;
pub mod tax;
pub mod tint;
pub mod uom;

pub use error::DomainError;
pub use ids::new_id;
pub use money::{Currency, Money};
