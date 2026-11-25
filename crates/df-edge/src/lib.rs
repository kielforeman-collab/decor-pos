//! `df-edge` — local store daemon (Phase 1, v0.2; spec §7).
//!
//! The edge daemon is the offline counter brain: it prices, checks
//! out, voids and returns sales against a local SQLite database,
//! opens/closes tills, parks holds, records quotes, prints receipts,
//! kicks the cash drawer, and drains the sync outbox to HQ when a
//! sink is available — with **no network use at all** while selling.
//!
//! Start here: [`EdgeConfig`] + [`Edge::open`]. The local HTTP/WS
//! API the POS talks to is [`Edge::router`] (routes documented in
//! [`http`]); [`Edge::serve`] runs it.
//!
//! Payments and hardware come from the real sibling crates:
//! [`payments`] re-exports `df_payments` (`PaymentProvider`,
//! `MockTerminal`, surcharge rules) and [`hw`] re-exports `df_hw`
//! (`Printer`, `MockPrinter`, ESC/POS `ReceiptBuilder`). Receipt
//! payments are mapped from `PaymentResult` to `df_hw`'s
//! `PaymentSummary` at the boundary in [`edge`].
//!
//! Money is `rust_decimal::Decimal` throughout — never floats. Card
//! surcharge default is 3% (see
//! [`EdgeConfig::default_surcharge_rules`]).

pub mod edge;
pub mod error;
pub mod http;
pub mod hw;
pub mod payments;
pub mod pricing;
pub mod tinting;
pub mod types;

pub use df_db::models::TillSession;
pub use edge::Edge;
pub use error::EdgeError;
pub use hw::{
    MockPrinter, PaymentSummary, Printer, ReceiptBuilder, ReceiptData, ReceiptLine,
    drawer_kick_bytes,
};
pub use payments::{
    MockTerminal, PaymentError, PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus,
    SurchargeRule, TenderKind, compute_surcharge, surcharge_rule_for,
};
pub use tinting::{
    ColorantLevelView, ColorantsResponse, DispenseTintRequest, DispenseTintResponse,
    tint_job_to_row, virtual_tinter_from_levels,
};
pub use types::{
    CompletedSale, EdgeConfig, PricePreview, PricePreviewLine, ReturnLineInput, SaleInput,
    SaleLineInput, TenderInput, ZReport,
};
