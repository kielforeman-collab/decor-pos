//! Public input/output types for the edge daemon.
//!
//! All money is [`Money`] (`rust_decimal::Decimal`, scale 4) — never
//! floats. All ids are UUIDv7. These types are the JSON wire format
//! for the local HTTP API as well as the Rust API (`SaleInput` is the
//! `POST /pos/transactions` body verbatim).

use chrono::{DateTime, Utc};
use df_domain::{Currency, Money};
use df_payments::SurchargeRule;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::payments::{PaymentResult, TenderKind};

/// Static configuration for one store edge node.
///
/// Remote HQ (Phase 8): the push target is NOT a field here — the
/// `df-server` binary constructs `EdgeConfig` with struct literals,
/// so a new required field would break its construction sites.
/// Instead [`Edge::open`](crate::edge::Edge::open) reads `DF_HQ_URL`
/// and `DF_HQ_TOKEN` from the environment (both must be non-empty),
/// or call [`Edge::set_remote_hq`](crate::edge::Edge::set_remote_hq).
///
/// `DF_HQ_TOKEN` is a staff session token with the `SyncIngest`
/// permission (Manager/Admin only, 8h TTL). It is held in memory
/// only and never logged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeConfig {
    pub store_id: Uuid,
    pub org_id: Uuid,
    /// sqlx URL, e.g. `sqlite:edge.db` or `sqlite::memory:`.
    pub db_url: String,
    pub store_name: String,
    pub currency: Currency,
    /// Per-tender surcharge rules, applied by
    /// [`df_payments::compute_surcharge`] at checkout and price
    /// preview. Defaults (see [`EdgeConfig::default_surcharge_rules`])
    /// to a single 3%-on-card rule, preserving the Phase 1 behaviour.
    pub surcharge_rules: Vec<SurchargeRule>,
}

impl EdgeConfig {
    /// The default surcharge rules for a store: one 3% card rule, no
    /// flat fee, no cap. Cash, charge-account and gift-card tenders
    /// carry no surcharge unless a rule is added for them.
    pub fn default_surcharge_rules(currency: Currency) -> Vec<SurchargeRule> {
        vec![SurchargeRule {
            tender: TenderKind::Card,
            pct: Decimal::from(3),
            flat: Money::zero(currency),
            max: None,
        }]
    }
}

/// One sale line as entered at the counter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaleLineInput {
    pub product_id: Uuid,
    pub sku: String,
    pub description: String,
    pub qty: Decimal,
    pub uom: String,
    /// List / fallback unit price — matrix pricing may replace it.
    pub unit_price: Money,
    pub unit_cost: Money,
}

/// One tender as entered at the counter. `amount = None` means "the
/// remaining balance" (the usual counter flow: the last tender covers
/// whatever is left). Cash may exceed the remaining balance (change
/// due); card / charge / gift-card tenders may not. `reference` is
/// the reference recorded with the payment — for E-Transfer the
/// reference the customer supplied (e.g. their confirmation number);
/// other tenders ignore it and record the provider reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenderInput {
    pub kind: TenderKind,
    pub amount: Option<Money>,
    #[serde(default)]
    pub reference: Option<String>,
}

/// A sale (or quote / hold) as entered at the counter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaleInput {
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    /// The price level to price this sale at. When `None`, the
    /// customer's stored level is used (when the sale has a
    /// customer); otherwise list pricing applies.
    #[serde(default)]
    pub price_level_id: Option<Uuid>,
    /// The job name printed on the invoice (free text; distinct
    /// from `job_id`, which points at a `customer_jobs` row).
    #[serde(default)]
    pub job_name: Option<String>,
    pub lines: Vec<SaleLineInput>,
    pub tenders: Vec<TenderInput>,
    pub po_number: Option<String>,
}

/// One returned line: a quantity of a product from the original sale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReturnLineInput {
    pub product_id: Uuid,
    pub qty: Decimal,
}

/// Outcome of a completed sale or return.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletedSale {
    pub transaction_id: Uuid,
    pub doc_number: String,
    pub subtotal: Money,
    pub tax_total: Money,
    pub eco_fee_total: Money,
    pub pos_fee_total: Money,
    pub grand_total: Money,
    pub change_due: Money,
    pub payments: Vec<PaymentResult>,
    /// False when no printer was configured (the sale still succeeds)
    /// or the printer failed; true when the receipt printed.
    pub receipt_printed: bool,
}

/// One priced line in a [`PricePreview`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricePreviewLine {
    pub product_id: Uuid,
    pub description: String,
    pub qty: Decimal,
    pub unit_price: Money,
    pub line_total: Money,
    pub tax_amount: Money,
    pub eco_fee_amount: Money,
}

/// Totals for a cart, computed without persisting anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricePreview {
    pub lines: Vec<PricePreviewLine>,
    pub subtotal: Money,
    pub tax_total: Money,
    pub eco_fee_total: Money,
    pub pos_fee_total: Money,
    pub grand_total: Money,
}

/// Till close report. `gross_sales` / `tax_total` cover completed
/// `sale` documents only; `return_total` is the (negative) sum of
/// completed `return` documents. `expected_cash` is the opening float
/// plus cash payments on sales; `over_short` is
/// `counted_cash - expected_cash`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZReport {
    pub till_id: Uuid,
    pub opened_at: DateTime<Utc>,
    pub closed_at: DateTime<Utc>,
    pub expected_cash: Money,
    pub counted_cash: Money,
    pub over_short: Money,
    pub sale_count: u32,
    pub gross_sales: Money,
    pub tax_total: Money,
    pub cash_sales: Money,
    pub card_sales: Money,
    pub charge_sales: Money,
    pub return_count: u32,
    pub return_total: Money,
}
