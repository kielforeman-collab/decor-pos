//! `df-portal` — contractor portal, mobile PWA and BOPIS domain
//! (Phase 5, v0.6; spec §4 `contractor_portal_ecom`, §6.1
//! `/portal/v1`, §7 Phase 5).
//!
//! Pure domain logic, no I/O: portal access, bill-pay intent
//! validation, formula-reorder requests, BOPIS pickup orders and
//! the mobile KPIs view. Persistence lives in `df-db` (`portal`
//! module); the edge daemon wires it into HTTP in `df-edge`
//! (`portal.rs`).
//!
//! All money is [`Money`] (`rust_decimal::Decimal`, scale 4) —
//! never floats. No PAN/CVV anywhere: a bill payment carries an
//! amount, a method label (`card` / `ach`) and an idempotency key
//! only — the card/ACH itself is authorized by the configured
//! `PaymentProvider` (mock in dev) and only its reference string
//! is stored.
//!
//! Formula tracking only (Kiel, Nov 26 rescope): a reorder looks
//! up a stored formula and re-creates the sale line for it. The
//! POS never drives a tint machine — there is no dispense,
//! colorant or machine-control concept anywhere in this crate.

pub mod billpay;
pub mod bopis;
pub mod kpis;
pub mod portal;
pub mod reorder;

pub use billpay::{BillPayIntent, BillPayResult, validate_bill_pay};
pub use bopis::{BopisLine, BopisOrderRequest, validate_bopis_order};
pub use kpis::MobileKpis;
pub use portal::{PortalError, require_portal_access};
pub use reorder::{Fulfillment, ReorderRequest, validate_reorder};
