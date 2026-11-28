//! Row models for the spec §5 tables.
//!
//! Money is stored as `rust_decimal::Decimal` (NUMERIC(14,4) in
//! PostgreSQL; NUMERIC/text in SQLite) — never floats. Primary keys are
//! UUIDv7, minted with [`df_domain::new_id`]. JSON columns are
//! `serde_json::Value` in the models; the repository layer serialises
//! them to text for SQLite and JSONB for PostgreSQL.

use chrono::{DateTime, NaiveDate, Utc};
use df_domain::pricing::{CalcMethod, PricingRule};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DbError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub default_currency: String,
    pub enabled_modules: serde_json::Value,
    pub settings_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub id: Uuid,
    pub org_id: Uuid,
    pub code: String,
    pub name: String,
    pub country_code: String,
    pub region_code: String,
    pub postal_code: String,
    pub currency: String,
    pub tax_zone_id: Uuid,
    pub timezone: String,
    pub is_warehouse: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub password_hash: Option<String>,
    pub oidc_subject: Option<String>,
    pub counter_pin_hash: Option<String>,
    pub badge_token_hash: Option<String>,
    pub role_id: Uuid,
    /// Contractor customer this user may access in the portal
    /// (`users.portal_customer_id`, migration `0012_phase7`).
    /// `Some` only for `PortalContractor` users; `None` for staff.
    pub portal_customer_id: Option<Uuid>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Product {
    pub id: Uuid,
    pub org_id: Uuid,
    pub sku: String,
    pub manufacturer_code: Option<String>,
    pub name: String,
    pub department_id: Uuid,
    pub class_id: Option<Uuid>,
    pub product_line_id: Option<Uuid>,
    pub primary_vendor_id: Option<Uuid>,
    pub product_type: String,
    pub base_uom: String,
    pub list_price: Decimal,
    pub standard_cost: Decimal,
    pub avg_cost: Decimal,
    /// The price list's MSRP, when the file supplied one
    /// (`products.msrp`, migration `0014_counter_ui`). `None` when
    /// the file left it blank (most BM rows) or the product predates
    /// the import.
    pub msrp: Option<Decimal>,
    /// The price list's MAP, when the file supplied one
    /// (`products.map`, migration `0014_counter_ui`).
    pub map: Option<Decimal>,
    pub min_margin_pct: Option<Decimal>,
    pub sheen: Option<String>,
    pub paint_base_code: Option<String>,
    pub container_volume_ml: Option<i32>,
    pub eco_fee_category: Option<String>,
    pub tax_class_id: Uuid,
    pub prompt_template_id: Option<Uuid>,
    /// Product UPC / barcode from the price list (`products.upc`, migration `0018`).
    #[serde(default)]
    pub upc: Option<String>,
    /// Price-list category (e.g. `INTERIOR`, `COLORANT`, migration `0018`).
    #[serde(default)]
    pub category: Option<String>,
    /// Price-list line code (e.g. `00229`, `N547`, migration `0018`).
    #[serde(default)]
    pub line_code: Option<String>,
    /// Price-list color / base label (migration `0018`).
    #[serde(default)]
    pub color_name: Option<String>,
    /// Raw size code from the price list (e.g. `001`, `004`, `1 gal`, migration `0018`).
    #[serde(default)]
    pub size_code: Option<String>,
    /// Lossless JSON map of every source spreadsheet column (`products.source_metadata_json`, migration `0018`).
    #[serde(default)]
    pub source_metadata: Option<serde_json::Value>,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorCatalogEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub manufacturer: String,
    pub color_code: String,
    pub color_name: String,
    pub collection: Option<String>,
    pub rgb_hex: Option<String>,
    pub recommended_base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Colorant {
    pub id: Uuid,
    pub org_id: Uuid,
    pub system_name: String,
    pub code: String,
    pub name: String,
    pub cost_per_fl_oz: Decimal,
    pub shots_per_fl_oz: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TintFormula {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub color_id: Option<Uuid>,
    pub color_code_snapshot: String,
    pub color_name_snapshot: String,
    pub base_product_id: Uuid,
    pub container_volume_ml: i32,
    pub sheen: Option<String>,
    pub room_or_usage: Option<String>,
    pub is_custom_match: bool,
    pub colorants_json: serde_json::Value,
    pub total_tint_cost: Decimal,
    pub tint_surcharge: Decimal,
    pub dispenser_name: Option<String>,
    pub external_formula_ref: Option<String>,
    pub dispensed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Customer {
    pub id: Uuid,
    pub org_id: Uuid,
    pub account_number: String,
    pub customer_type: String,
    pub company_name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub price_level_id: Option<Uuid>,
    pub salesperson_id: Option<Uuid>,
    pub tax_exempt_cert: Option<String>,
    pub tax_exempt_expires: Option<NaiveDate>,
    pub charge_enabled: bool,
    pub credit_limit: Decimal,
    pub payment_terms: String,
    /// Explicit terms in days (`customers.terms_days`, Phase 3).
    /// `None` means "parse `payment_terms`" (`NET30` → 30, …).
    pub terms_days: Option<i32>,
    /// Monthly finance-charge rate in percent, e.g. `1.5` for
    /// 1.5%/month (`customers.finance_rate_pct_monthly`, Phase 3).
    /// `None` means "use the rate the assessment run was given".
    pub finance_rate_pct_monthly: Option<Decimal>,
    pub credit_hold: bool,
    pub require_po_number: bool,
    pub require_job_name: bool,
    pub portal_enabled: bool,
    /// Billing / mailing address line 1 (`customers.address_line1`, migration `0019`).
    pub address_line1: Option<String>,
    /// Address line 2 (suite, unit; migration `0019`).
    pub address_line2: Option<String>,
    /// City (migration `0019`).
    pub city: Option<String>,
    /// Province / state (migration `0019`).
    pub province: Option<String>,
    /// Postal / ZIP code (migration `0019`).
    pub postal_code: Option<String>,
    /// Free-form account notes shown on the customer screen (migration `0019`).
    pub notes: Option<String>,
}

/// One row of `customer_jobs` (spec §5.4): a job sub-ledger under
/// one charge account. AR entries can point at a job, so a
/// contractor's balance splits per job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomerJob {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub job_code: String,
    pub job_name: String,
    pub site_address_json: serde_json::Value,
    pub tax_zone_override: Option<Uuid>,
    pub is_active: bool,
}

/// One row of `ar_allocations` (spec §5.4): part of one payment
/// entry applied to one charge entry (FIFO — see `df-ar`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArAllocation {
    pub id: Uuid,
    pub payment_entry_id: Uuid,
    pub charge_entry_id: Uuid,
    pub allocated_amount: Decimal,
    pub created_at: DateTime<Utc>,
}

/// One row of `statements` (Phase 3): a frozen month-end statement
/// for one charge account. Money is `Decimal`; `payments_total`
/// counts payments **and credit memos** in the period (positive
/// magnitude). `aging_json` is
/// `{"current": "…", "31-60": "…", "61-90": "…", "91-120": "…",
/// "120+": "…"}` with exact decimal strings. `pdf_sha256` is set
/// by the PDF worker when it renders this statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementRow {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub opening_balance: Decimal,
    pub charges_total: Decimal,
    pub payments_total: Decimal,
    pub finance_charge_total: Decimal,
    pub closing_balance: Decimal,
    pub aging_json: serde_json::Value,
    pub pdf_sha256: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One row of `inventory_transfers` (Phase 3): stock moving between
/// two stores. `status` is `draft` | `shipped` | `received` |
/// `cancelled`. The inventory movement itself is recorded in
/// `inventory_ledger` by the worker that ships/receives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryTransfer {
    pub id: Uuid,
    pub org_id: Uuid,
    pub from_store_id: Uuid,
    pub to_store_id: Uuid,
    pub status: String,
    pub reference: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One row of `transfer_lines` (Phase 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferLine {
    pub id: Uuid,
    pub transfer_id: Uuid,
    pub product_id: Uuid,
    pub qty: Decimal,
    pub unit_cost: Decimal,
}

/// One row of `physical_counts` (Phase 3): a stock count at one
/// store. `status` is `open` | `reconciled`; `counted_at` is set
/// when the count is reconciled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysicalCount {
    pub id: Uuid,
    pub store_id: Uuid,
    pub status: String,
    pub counted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// One row of `count_lines` (Phase 3): `variance_qty` is
/// `counted_qty - system_qty`, computed by the caller at insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CountLine {
    pub id: Uuid,
    pub count_id: Uuid,
    pub product_id: Uuid,
    pub counted_qty: Decimal,
    pub system_qty: Decimal,
    pub variance_qty: Decimal,
    pub unit_cost: Decimal,
}

/// One row of `reorder_points` (Phase 3): min/max stock levels for
/// one product at one store. Unique per (store, product).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReorderPoint {
    pub id: Uuid,
    pub store_id: Uuid,
    pub product_id: Uuid,
    pub min_qty: Decimal,
    pub max_qty: Decimal,
    pub reorder_qty: Decimal,
}

/// A row of `pricing_rules` (spec §5.4). `calc_method` is stored as text
/// (`fixed`, `pct_off_list`, `markup_cost`, `target_gm`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingRuleRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub priority: i32,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub price_level_id: Option<Uuid>,
    pub product_id: Option<Uuid>,
    pub product_line_id: Option<Uuid>,
    pub class_id: Option<Uuid>,
    pub department_id: Option<Uuid>,
    pub vendor_id: Option<Uuid>,
    pub min_qty: Decimal,
    pub calc_method: String,
    pub calc_value: Decimal,
    pub min_margin_floor: Option<Decimal>,
    pub effective_from: Option<DateTime<Utc>>,
    pub effective_to: Option<DateTime<Utc>>,
}

impl PricingRuleRow {
    /// Convert to the domain pricing rule (spec §5.4 `effective_from/to`
    /// are timestamps in the table; the domain engine compares dates).
    pub fn to_domain(&self) -> Result<PricingRule, DbError> {
        let calc_method = match self.calc_method.as_str() {
            "fixed" => CalcMethod::Fixed,
            "pct_off_list" => CalcMethod::PctOffList,
            "markup_cost" => CalcMethod::MarkupCost,
            "target_gm" => CalcMethod::TargetGm,
            other => {
                return Err(DbError::InvalidData(format!(
                    "unknown calc_method: {other}"
                )));
            }
        };
        Ok(PricingRule {
            id: self.id,
            priority: self.priority,
            customer_id: self.customer_id,
            job_id: self.job_id,
            price_level_id: self.price_level_id,
            product_id: self.product_id,
            product_line_id: self.product_line_id,
            class_id: self.class_id,
            department_id: self.department_id,
            vendor_id: self.vendor_id,
            min_qty: self.min_qty,
            calc_method,
            calc_value: self.calc_value,
            min_margin_floor: self.min_margin_floor,
            effective_from: self.effective_from.map(|d| d.date_naive()),
            effective_to: self.effective_to.map(|d| d.date_naive()),
        })
    }
}

/// One append-only row of `ar_ledger` (spec §5.4). `amount` is positive
/// when it increases the AR balance, negative when it reduces it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArLedgerEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub customer_id: Uuid,
    pub job_id: Option<Uuid>,
    pub entry_type: String,
    pub reference_type: String,
    pub reference_id: Uuid,
    pub amount: Decimal,
    pub due_date: Option<NaiveDate>,
    pub statement_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

// --- Phase 1 models (spec §5.5) --------------------------------------------

/// One row of `portal_payments` (Phase 5, spec §6.1
/// `POST /portal/payments/intent`). Keyed by the client-generated
/// idempotency key: a retried bill payment with the same key
/// returns the original AR payment entry and posts nothing twice.
/// `provider_reference` is the provider's reference string only —
/// no card data, no PAN/CVV, ever.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalPaymentRow {
    pub idempotency_key: String,
    pub customer_id: Uuid,
    pub payment_entry_id: Uuid,
    pub amount: Decimal,
    pub method: String,
    pub provider_reference: String,
    pub created_at: DateTime<Utc>,
}

/// One row of `bopis_orders` (Phase 5): who picks a BOPIS order
/// up and how the store reaches them. The order itself is a
/// `transactions` row (`doc_type = 'order'`, fulfillment
/// `'pickup'`); this table holds only the guest contact details,
/// since a BOPIS buyer has no customer account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BopisOrderRow {
    pub order_id: Uuid,
    pub pickup_name: String,
    pub contact: String,
    pub created_at: DateTime<Utc>,
}

/// One row of `price_levels` (migration `0014_counter_ui`): a
/// contractor price level. `discount_pct` is a fraction, e.g.
/// `0.20` for Gold's 20% off list; the matching matrix rule stores
/// the same discount as percent points (`20`) because
/// `df-domain::pricing::CalcMethod::PctOffList` takes percent — see
/// [`crate::ensure_price_levels`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceLevel {
    pub id: Uuid,
    pub org_id: Uuid,
    pub code: String,
    pub name: String,
    pub discount_pct: Decimal,
}

/// One row of `vendors` (migration `0015_vendors`): back-office
/// master data for the vendor-id string used on purchase orders
/// (e.g. `"Benjamin Moore"`). `name` is the PO key — it is never
/// edited after creation — while `display_name` is the human label
/// shown in the UI. Contact fields are `None` until entered by
/// hand; the migration backfill leaves them `NULL`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vendor {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub contact_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub edi_enabled: bool,
    pub created_at: DateTime<Utc>,
}

/// One row of `transactions` (spec §5.5). Money is `Decimal` at scale 4;
/// `doc_type` is `sale` | `return` | `quote` | `order` | `layaway`,
/// `status` is `open` | `parked` | `completed` | `voided`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub till_session_id: Option<Uuid>,
    pub doc_number: String,
    pub doc_type: String,
    pub status: String,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub po_number: Option<String>,
    /// The counter-entered job name (`transactions.job_name`,
    /// migration `0014_counter_ui`): free text shown on the invoice.
    /// Distinct from `job_id`, which points at a `customer_jobs` row.
    pub job_name: Option<String>,
    pub salesperson_id: Option<Uuid>,
    pub cashier_id: Option<Uuid>,
    pub currency: String,
    pub subtotal: Decimal,
    pub tax_total: Decimal,
    pub eco_fee_total: Decimal,
    pub pos_fee_total: Decimal,
    pub grand_total: Decimal,
    pub deposit_paid: Decimal,
    pub fulfillment_mode: String,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// One row of `transaction_lines` (spec §5.5). `qty` is negative on
/// return lines; `line_total` is the signed extended total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionLine {
    pub id: Uuid,
    pub transaction_id: Uuid,
    pub line_no: i32,
    pub product_id: Uuid,
    pub description: String,
    pub qty: Decimal,
    pub uom_code: String,
    pub unit_price: Decimal,
    pub base_unit_cost: Decimal,
    pub tint_unit_cost: Decimal,
    pub discount_amount: Decimal,
    pub tax_amount: Decimal,
    pub eco_fee_amount: Decimal,
    pub line_total: Decimal,
    pub tint_formula_id: Option<Uuid>,
    pub prompts_json: serde_json::Value,
    pub vertical_spec_json: serde_json::Value,
    pub linked_po_line_id: Option<Uuid>,
    pub fulfilled_qty: Decimal,
}

/// One append-only row of `inventory_ledger` (spec §5.5). Balances are
/// derived by summing `delta_on_hand` per product — never stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryLedgerEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub product_id: Uuid,
    pub lot_number: Option<String>,
    pub delta_on_hand: Decimal,
    pub delta_committed: Decimal,
    pub delta_on_order: Decimal,
    pub delta_in_transit: Decimal,
    pub unit_cost: Decimal,
    pub reason_type: String,
    pub reference_id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// One row of `till_sessions`. `status` is `open` | `closed`;
/// `closed_at`, `counted_cash` and `expected_cash` are set on close.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TillSession {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub cashier_id: Uuid,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub opening_float: Decimal,
    pub counted_cash: Option<Decimal>,
    pub expected_cash: Option<Decimal>,
    pub status: String,
}

/// One row of `payments`. No card number / PAN / CVV columns exist
/// anywhere in this schema (spec §8 guardrail 3): only the tender, the
/// amount actually collected, a reference and the authorization token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentRow {
    pub id: Uuid,
    pub transaction_id: Uuid,
    pub tender: String,
    pub amount: Decimal,
    pub reference: String,
    pub authorization: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One row of `sync_state`: the last batch HQ acknowledged for a store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncStateRow {
    pub store_id: Uuid,
    pub last_batch_id: Option<Uuid>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub pending_count: i64,
}

// --- Phase 2 (EDI) / Phase 3 (purchasing) models ---------------------------

/// One row of `purchase_orders` (migration `0004_phase2_edi`).
/// `status` is `draft` | `sent` | `acknowledged` |
/// `partially_received` | `received` | `cancelled`. `total` is the
/// exact ordered total (sum of `qty * unit_cost` over the lines).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurchaseOrderRow {
    pub id: Uuid,
    pub po_number: String,
    pub vendor_id: String,
    pub store_id: Uuid,
    pub status: String,
    pub total: Decimal,
    pub created_at: DateTime<Utc>,
}

/// One row of `purchase_order_lines` (migration `0004_phase2_edi`).
/// There is no `received_qty` column by design: received quantities
/// are derived from the append-only `inventory_ledger`
/// (`reason_type = 'po_receive'`, `reference_id` = PO id) — see
/// [`crate::purchase_orders::po_received_by_product`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurchaseOrderLineRow {
    pub id: Uuid,
    pub po_id: Uuid,
    pub product_id: Uuid,
    pub line_no: i32,
    pub vendor_sku: String,
    pub description: String,
    pub qty: Decimal,
    pub uom: String,
    pub unit_cost: Decimal,
}

// --- Phase 4 (CRM + delivery) models ---------------------------------------

/// One row of `crm_activities` (migration `0006_phase4`): a logged
/// touch with a customer — call, email, visit, quote, note or
/// follow-up. `activity_type` is `call` | `email` | `visit` |
/// `quote` | `note` | `follow_up`. A reminder is an activity with a
/// `due_at`; it is due when `completed_at` is null, `due_at` is at
/// or before the check time, and it is not snoozed past that time
/// (`snoozed_until` null or at/before the check time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrmActivity {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub customer_id: Option<Uuid>,
    pub activity_type: String,
    pub subject: String,
    pub notes: Option<String>,
    pub created_by: Option<Uuid>,
    pub due_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub snoozed_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// One row of `appointments` (migration `0006_phase4`): a booked
/// slot with a customer. `status` is `scheduled` | `completed` |
/// `cancelled` | `no_show`. The table CHECK enforces
/// `ends_at > starts_at`; same-staff overlap is rejected by the
/// edge (`df-edge::crm`), not by the schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Appointment {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub customer_id: Option<Uuid>,
    pub staff_id: Option<Uuid>,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: String,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One row of `crm_saved_views` (migration `0006_phase4`): a named,
/// stored activity filter. `filter_json` is an object with any of
/// `activity_type` (string), `customer_id` (UUID string),
/// `completed` (bool) and `due_before` (RFC 3339); unknown keys are
/// ignored when the view runs. Unique per (store, name).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrmSavedView {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub name: String,
    pub filter_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// One row of `deliveries` (migration `0006_phase4`): a delivery
/// job for a sale (or a standalone drop when `transaction_id` is
/// null). `status` is `draft` | `assigned` | `out_for_delivery` |
/// `delivered` | `cancelled`. `delivered` is only reachable through
/// a POD submission (`df-edge::delivery::submit_pod`) — the status
/// endpoint refuses a direct jump to `delivered`. `address_json`
/// is a free-form address object (`street`, `city`, `postal_code`,
/// …); `scheduled_date` is the planned delivery day.
/// `priority` (migration `0007_phase4_crm_delivery`) is the
/// dispatch urgency — higher number = more urgent — and `vehicle`
/// is the assigned vehicle label, e.g. `"Van 2"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub transaction_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub driver_id: Option<Uuid>,
    pub status: String,
    pub address_json: serde_json::Value,
    pub scheduled_date: Option<NaiveDate>,
    pub priority: i32,
    pub vehicle: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One row of `delivery_pods` (migration `0006_phase4`): proof of
/// delivery for one delivery. The signature is stored as base64
/// text plus its SHA-256 hex digest (`signature_sha256`) so a
/// close-out can be verified without re-reading the bytes; the
/// photo is a reference string (file path or URL) plus an optional
/// SHA-256 — the photo bytes themselves are never stored here and
/// no camera hardware is involved. `photo_at` (migration
/// `0007_phase4_crm_delivery`) is when the photo was taken; when
/// NULL the signature time (`signed_at`) stands in for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryPod {
    pub id: Uuid,
    pub delivery_id: Uuid,
    pub signature_sha256: String,
    pub signature_b64: String,
    pub photo_ref: Option<String>,
    pub photo_sha256: Option<String>,
    pub received_by: String,
    pub signed_at: DateTime<Utc>,
    pub photo_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// One row of `delivery_events` (migration
/// `0007_phase4_crm_delivery`): the audit trail for a delivery's
/// status changes — who moved it, from what to what, and when.
/// Written by `df_db::transition_delivery` /
/// `df_db::close_out_delivery` in the same transaction as the
/// status change, so the trail can never disagree with the row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryEvent {
    pub id: Uuid,
    pub delivery_id: Uuid,
    pub from_status: String,
    pub to_status: String,
    pub actor_user_id: Uuid,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One row of `packing_slips` (migration
/// `0007_phase4_crm_delivery`): a rendered packing slip for one
/// delivery, stored so the exact slip that was printed can be
/// re-printed later. `rendered_text` is the 42-column plain-text
/// slip; `rendered_zpl` is the ZPL label rendering (see
/// `df-delivery::PackingSlip`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackingSlipRow {
    pub id: Uuid,
    pub delivery_id: Uuid,
    pub rendered_text: String,
    pub rendered_zpl: String,
    pub created_at: DateTime<Utc>,
}

/// One row of `shift_templates` (migration `0011_phase6`): a
/// recurring weekly shift pattern for one store. Times are
/// wall-clock `"HH:MM"` strings; `weekday` is 0=Sunday..6=Saturday.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShiftTemplate {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub name: String,
    pub weekday: i32,
    pub starts_at: String,
    pub ends_at: String,
    pub role: Option<String>,
    pub staff_id: Option<Uuid>,
    pub required_count: i32,
    pub is_active: bool,
}

/// One row of `staff_shifts` (migration `0011_phase6`): one
/// concrete shift for one staff member on one date. `status` is
/// `scheduled` | `confirmed` | `cancelled`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaffShift {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub template_id: Option<Uuid>,
    pub staff_id: Uuid,
    pub shift_date: NaiveDate,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: String,
    pub notes: Option<String>,
}

/// One row of `timeclock_entries` (migration `0011_phase6`): a
/// clock-in / clock-out pair. `clock_out` is `None` while the
/// entry is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeclockEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub staff_id: Uuid,
    pub clock_in: DateTime<Utc>,
    pub clock_out: Option<DateTime<Utc>>,
    pub source: String,
}
