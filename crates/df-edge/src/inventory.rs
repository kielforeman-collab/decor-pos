//! Back-office inventory, purchasing, transfers, counts, shelf
//! tags, bulk pricing and AR reads on the edge (Phase 3, v0.4;
//! spec §6.1/§7).
//!
//! All money is `rust_decimal::Decimal` — never floats. Every
//! mutation that moves stock posts to the append-only
//! `inventory_ledger` **inside the same SQLite transaction** as the
//! document update, plus an outbox entry, mirroring the checkout
//! path in [`crate::edge`].
//!
//! Persistence is the real migration `0005_phase3` schema with the
//! `df-db::ar` repositories (Worker A): `inventory_transfers` /
//! `transfer_lines`, `physical_counts` / `count_lines`,
//! `reorder_points`, and the pre-existing `purchase_orders` /
//! `purchase_order_lines` tables from `0004_phase2_edi` (repos in
//! `df-db::purchase_orders`). Received PO quantities are derived
//! from the `inventory_ledger` (`reason_type = 'po_receive'`), never
//! stored on the PO lines.
//!
//! Two deliberate deviations from a pure repo layer, both for
//! atomicity: the transfer-status and count-reconcile `UPDATE`s run
//! as raw SQL inside the ledger transaction (`set_transfer_status`
//! / `reconcile_physical_count` are pool-only, so they cannot join
//! it — a status flip after the commit would leave shipped stock
//! with a draft transfer). The SQL is identical to the repo
//! statements.
//!
//! # Multi-store note
//!
//! One edge serves one store, but a transfer names both ends
//! explicitly (`from_store_id` / `to_store_id`) and posts ledger
//! rows for each side in the same edge database (the ledger carries
//! `store_id` per row and has no FK to `stores`), so an A→B
//! transfer is verifiable from one edge. A second store row is not
//! required.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use df_db::models::{
    ArLedgerEntry, CountLine as CountLineRow, Customer, InventoryLedgerEntry, InventoryTransfer,
    PhysicalCount as PhysicalCountRow, PricingRuleRow, PurchaseOrderLineRow, PurchaseOrderRow,
    ReorderPoint, TransferLine as TransferLineRow,
};
use df_db::{AnyPool, DbError, InventoryBalance};
use df_domain::ar::{ArEntry, aging_totals};
use df_domain::pricing::{PricingRule, validate_rule};
use df_domain::{Currency, Money};
use df_edi::{LowStockItem, generate_purchase_orders};
use df_sync::{Outbox, OutboxEntry};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

// --- API view types ---------------------------------------------------------

/// One line of an inter-store transfer (API view over
/// `transfer_lines`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransferLineView {
    pub product_id: Uuid,
    pub qty: Decimal,
    pub unit_cost: Decimal,
}

/// An inter-store transfer with its lines. `status` is `draft` |
/// `shipped` | `received` | `cancelled`. Stock moves only on
/// ship/receive (see [`Edge::ship_transfer`] /
/// [`Edge::receive_transfer`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transfer {
    pub id: Uuid,
    pub from_store_id: Uuid,
    pub to_store_id: Uuid,
    pub status: String,
    pub lines: Vec<TransferLineView>,
    pub created_at: DateTime<Utc>,
}

/// One scanned line of a physical count (API view over
/// `count_lines`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CountLineView {
    pub product_id: Uuid,
    pub counted_qty: Decimal,
    pub system_qty: Decimal,
    pub variance_qty: Decimal,
    pub unit_cost: Decimal,
}

/// One variance posted by a count reconciliation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CountAdjustment {
    pub product_id: Uuid,
    pub on_hand_before: Decimal,
    pub counted_qty: Decimal,
    /// `counted_qty - on_hand_before`; posted to the ledger as a
    /// `count_adj` entry (zero adjustments post nothing).
    pub adjustment: Decimal,
}

/// A physical count with its lines. `status` is `open` |
/// `reconciled`. Only scanned lines are reconciled; unscanned
/// products are untouched. `adjustments` is empty while the count
/// is open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysicalCount {
    pub id: Uuid,
    pub store_id: Uuid,
    pub status: String,
    pub lines: Vec<CountLineView>,
    pub adjustments: Vec<CountAdjustment>,
    pub created_at: DateTime<Utc>,
    pub counted_at: Option<DateTime<Utc>>,
}

// --- request / response types ----------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetReorderPointRequest {
    pub product_id: Uuid,
    pub store_id: Option<Uuid>,
    pub min_qty: Decimal,
    pub max_qty: Decimal,
    /// Quantity to order when below minimum (before the shortfall
    /// is considered — see [`Edge::suggest_replenishment`]).
    pub reorder_qty: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplenishmentSuggestRequest {
    pub store_id: Option<Uuid>,
    /// Trailing days of sales history used for velocity (default 30).
    pub days: Option<i64>,
    /// Lead time in days: suggestions cover at least
    /// `velocity * lead_time_days` (default 7).
    pub lead_time_days: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestedLine {
    pub product_id: Uuid,
    pub sku: String,
    pub description: String,
    pub qty: Decimal,
    pub uom: String,
    pub unit_cost: Decimal,
    pub on_hand: Decimal,
    pub on_order: Decimal,
    pub min_qty: Decimal,
    pub max_qty: Decimal,
    /// Units sold per day over the trailing window.
    pub velocity_per_day: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestedPurchaseOrder {
    /// Preview number only — the real PO number is assigned on create.
    pub po_number: String,
    pub vendor_id: String,
    pub lines: Vec<SuggestedLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatePoLineRequest {
    pub product_id: Uuid,
    pub qty: Decimal,
    pub uom: Option<String>,
    pub unit_cost: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatePurchaseOrderRequest {
    pub vendor_id: String,
    pub store_id: Option<Uuid>,
    pub lines: Vec<CreatePoLineRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoLineView {
    pub id: Uuid,
    pub line_no: i32,
    pub product_id: Uuid,
    pub sku: String,
    pub description: String,
    pub qty: Decimal,
    pub received_qty: Decimal,
    pub uom: String,
    pub unit_cost: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurchaseOrderView {
    pub id: Uuid,
    pub po_number: String,
    pub vendor_id: String,
    pub store_id: Uuid,
    pub status: String,
    pub total: Decimal,
    pub lines: Vec<PoLineView>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceivePoLineRequest {
    pub line_id: Option<Uuid>,
    pub product_id: Uuid,
    pub qty: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceivePurchaseOrderRequest {
    pub lines: Vec<ReceivePoLineRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTransferRequest {
    pub from_store_id: Uuid,
    pub to_store_id: Uuid,
    pub lines: Vec<TransferLineView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StartCountRequest {
    pub store_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanCountRequest {
    pub product_id: Uuid,
    pub counted_qty: Decimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CountReconcileResponse {
    pub count_id: Uuid,
    pub store_id: Uuid,
    pub status: String,
    pub adjustments: Vec<CountAdjustment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShelfTagResponse {
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub price: Money,
    /// The barcode encoded on the tag. No barcode repo is wired yet,
    /// so this is the SKU (Code 128 encodes it verbatim).
    pub barcode: String,
    pub zpl: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingRuleUpdate {
    pub id: Uuid,
    pub calc_method: String,
    pub calc_value: Decimal,
    pub min_margin_floor: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BulkPricingRequest {
    pub rules: Vec<PricingRuleUpdate>,
}

/// One customer's AR position (spec §6.1 `GET /ar/customers/{id}/summary`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArCustomerSummary {
    pub customer_id: Uuid,
    pub account_number: String,
    pub company_name: Option<String>,
    pub customer_type: String,
    pub charge_enabled: bool,
    pub credit_limit: Money,
    pub credit_hold: bool,
    pub balance: Money,
    pub entry_count: usize,
    pub oldest_due_date: Option<NaiveDate>,
}

/// Per-customer aging row: buckets are
/// [current, 31-60, 61-90, 91-120, 120+].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgingCustomerRow {
    pub customer_id: Uuid,
    pub account_number: String,
    pub balance: Money,
    pub buckets: [Money; 5],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgingView {
    pub as_of: NaiveDate,
    pub totals: [Money; 5],
    pub customers: Vec<AgingCustomerRow>,
}

/// One collections-workspace row (spec §6.1 `GET /ar/collections`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionsRow {
    pub customer_id: Uuid,
    pub account_number: String,
    pub company_name: Option<String>,
    pub balance: Money,
    pub credit_limit: Money,
    pub credit_hold: bool,
    /// Amount in the 31-60 / 61-90 / 91-120 / 120+ buckets.
    pub overdue: Money,
    pub oldest_due_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionsView {
    pub as_of: NaiveDate,
    pub customers: Vec<CollectionsRow>,
}

// --- helpers ----------------------------------------------------------------

fn zero_money(currency: Currency) -> Money {
    Money::zero(currency)
}

/// Outbox entry for one inventory-ledger row, carrying the full row
/// (all four deltas, unit cost, lot, created_at) so HQ can rebuild
/// it exactly — mirroring checkout. The transfer "close" leg's
/// `delta_in_transit` used to be lost in sync; the full row keeps it.
fn inventory_outbox_entry(store_id: Uuid, entry: &InventoryLedgerEntry) -> OutboxEntry {
    OutboxEntry::new(
        store_id,
        "inventory_ledger",
        entry.id,
        serde_json::json!({
            "product_id": entry.product_id.to_string(),
            "delta_on_hand": entry.delta_on_hand.to_string(),
            "delta_committed": entry.delta_committed.to_string(),
            "delta_on_order": entry.delta_on_order.to_string(),
            "delta_in_transit": entry.delta_in_transit.to_string(),
            "unit_cost": entry.unit_cost.to_string(),
            "lot_number": entry.lot_number,
            "reason_type": entry.reason_type,
            "reference_id": entry.reference_id.to_string(),
            "created_at": entry.created_at.to_rfc3339(),
        }),
    )
}

/// Units sold per day for a product at a store over the trailing
/// `days` days, from `sale` ledger rows. Decimal math throughout.
async fn velocity_per_day(
    pool: &AnyPool,
    store_id: Uuid,
    product_id: Uuid,
    days: i64,
    now: DateTime<Utc>,
) -> Result<Decimal, EdgeError> {
    if days <= 0 {
        return Ok(Decimal::ZERO);
    }
    let since = now - chrono::Duration::days(days);
    let entries = df_db::list_inventory_entries_for_product(pool, store_id, product_id).await?;
    let mut sold = Decimal::ZERO;
    for entry in &entries {
        if entry.reason_type == "sale"
            && entry.created_at >= since
            && entry.delta_on_hand < Decimal::ZERO
        {
            sold -= entry.delta_on_hand;
        }
    }
    Ok(sold / Decimal::from(days))
}

/// The min-margin guardrail for a bulk pricing update: the rule's own
/// computed price at `list_price` / `cost` must not sit below its
/// `min_margin_floor`. Returns the rejection reason, if any.
fn margin_guardrail_rejection(
    rule: &PricingRule,
    list_price: Money,
    cost: Money,
    qty: Decimal,
    on: NaiveDate,
) -> Option<String> {
    let floor = rule.min_margin_floor?;
    let input = df_domain::pricing::PricingInput {
        list_price,
        cost,
        qty,
        customer_id: rule.customer_id,
        job_id: rule.job_id,
        price_level_id: rule.price_level_id,
        product_id: rule.product_id,
        product_line_id: rule.product_line_id,
        class_id: rule.class_id,
        department_id: rule.department_id,
        vendor_id: rule.vendor_id,
        on,
    };
    let Some(price) = rule.compute_price(&input) else {
        return Some(format!(
            "rule {} computes no valid price for its product",
            rule.id
        ));
    };
    let margin = price.margin_pct(cost).ok().flatten();
    let below_floor = match margin {
        Some(margin) => margin < floor,
        None => true,
    };
    if below_floor {
        Some(format!(
            "rule {} would price at {} (margin {:?}%) which is below its min-margin floor of {}%",
            rule.id,
            price.amount(),
            margin,
            floor
        ))
    } else {
        None
    }
}

impl Edge {
    fn store_or_default(&self, store_id: Option<Uuid>) -> Uuid {
        store_id.unwrap_or(self.config().store_id)
    }

    // --- inventory balances -------------------------------------------------

    /// Stock positions for every product at a store, derived from
    /// the append-only ledger.
    pub async fn inventory_balances(
        &self,
        store_id: Uuid,
    ) -> Result<Vec<InventoryBalance>, EdgeError> {
        Ok(df_db::inventory_balances(self.pool(), self.config().org_id, store_id).await?)
    }

    // --- reorder points -----------------------------------------------------

    /// All reorder points at a store (`reorder_points`, 0005).
    pub async fn list_reorder_points(
        &self,
        store_id: Uuid,
    ) -> Result<Vec<ReorderPoint>, EdgeError> {
        Ok(df_db::list_reorder_points_for_store(self.pool(), store_id).await?)
    }

    /// Set (or replace) the min/max reorder point for a product
    /// (upsert on the `UNIQUE (store_id, product_id)` key).
    pub async fn set_reorder_point(
        &self,
        request: SetReorderPointRequest,
    ) -> Result<ReorderPoint, EdgeError> {
        if request.min_qty < Decimal::ZERO {
            return Err(EdgeError::InvalidInput(format!(
                "min_qty must not be negative: {}",
                request.min_qty
            )));
        }
        if request.max_qty < request.min_qty {
            return Err(EdgeError::InvalidInput(format!(
                "max_qty {} is below min_qty {}",
                request.max_qty, request.min_qty
            )));
        }
        if request.reorder_qty <= Decimal::ZERO {
            return Err(EdgeError::InvalidInput(format!(
                "reorder_qty must be positive: {}",
                request.reorder_qty
            )));
        }
        df_db::get_product(self.pool(), request.product_id).await?;
        let point = ReorderPoint {
            id: Uuid::now_v7(),
            store_id: self.store_or_default(request.store_id),
            product_id: request.product_id,
            min_qty: request.min_qty,
            max_qty: request.max_qty,
            reorder_qty: request.reorder_qty,
        };
        df_db::upsert_reorder_point(self.pool(), &point).await?;
        Ok(point)
    }

    // --- replenishment / purchasing ------------------------------------------

    /// Suggest replenishment from min/max + velocity: for every
    /// product with a reorder point whose on-hand + on-order supply
    /// is at or below its minimum, suggest
    /// `max(reorder_qty, max_qty - supply, velocity * lead_time_days)`
    /// units, grouped into one PO per vendor by `df-edi`
    /// (`generate_purchase_orders`). Nothing is persisted — the
    /// returned PO numbers are previews.
    pub async fn suggest_replenishment(
        &self,
        request: ReplenishmentSuggestRequest,
    ) -> Result<Vec<SuggestedPurchaseOrder>, EdgeError> {
        let store_id = self.store_or_default(request.store_id);
        let days = request.days.unwrap_or(30);
        if days <= 0 {
            return Err(EdgeError::InvalidInput(format!(
                "velocity window days must be positive: {days}"
            )));
        }
        let lead_time_days = request.lead_time_days.unwrap_or(7);
        if lead_time_days < 0 {
            return Err(EdgeError::InvalidInput(format!(
                "lead_time_days must not be negative: {lead_time_days}"
            )));
        }
        let points = self.list_reorder_points(store_id).await?;
        if points.is_empty() {
            return Ok(Vec::new());
        }
        let balances = self.inventory_balances(store_id).await?;
        let by_product: HashMap<Uuid, &InventoryBalance> = balances
            .iter()
            .map(|balance| (balance.product_id, balance))
            .collect();
        let now = Utc::now();
        let mut items: Vec<LowStockItem> = Vec::new();
        let mut detail: HashMap<Uuid, SuggestedLine> = HashMap::new();
        for point in &points {
            let product = df_db::get_product(self.pool(), point.product_id).await?;
            let balance = by_product.get(&point.product_id);
            let on_hand = balance.map(|b| b.on_hand).unwrap_or(Decimal::ZERO);
            let on_order = balance.map(|b| b.on_order).unwrap_or(Decimal::ZERO);
            let supply = on_hand + on_order;
            if supply > point.min_qty {
                continue;
            }
            let velocity =
                velocity_per_day(self.pool(), store_id, point.product_id, days, now).await?;
            let lead_demand = velocity * Decimal::from(lead_time_days);
            let to_max = point.max_qty - supply;
            let qty = [point.reorder_qty, to_max, lead_demand].into_iter().fold(
                Decimal::ZERO,
                |acc, value| {
                    if value > acc { value } else { acc }
                },
            );
            if qty <= Decimal::ZERO {
                continue;
            }
            let vendor_id = product
                .primary_vendor_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string());
            items.push(LowStockItem {
                product_id: product.id,
                vendor_id,
                vendor_sku: product.sku.clone(),
                description: product.name.clone(),
                on_hand: supply,
                min_level: point.min_qty,
                reorder_qty: qty,
                unit_cost: product.avg_cost,
                uom: product.base_uom.clone(),
            });
            detail.insert(
                product.id,
                SuggestedLine {
                    product_id: product.id,
                    sku: product.sku,
                    description: product.name,
                    qty,
                    uom: product.base_uom,
                    unit_cost: product.avg_cost,
                    on_hand,
                    on_order,
                    min_qty: point.min_qty,
                    max_qty: point.max_qty,
                    velocity_per_day: velocity,
                },
            );
        }
        let date = now.date_naive();
        let pos = generate_purchase_orders(&items, store_id, date);
        Ok(pos
            .into_iter()
            .map(|po| SuggestedPurchaseOrder {
                po_number: po.po_number,
                vendor_id: po.vendor_id,
                lines: po
                    .lines
                    .iter()
                    .filter_map(|line| detail.get(&line.product_id).cloned())
                    .collect(),
            })
            .collect())
    }

    /// Build the API view of a PO: header + lines with SKUs and
    /// ledger-derived received quantities.
    async fn purchase_order_view(&self, id: Uuid) -> Result<PurchaseOrderView, EdgeError> {
        let po = df_db::get_purchase_order(self.pool(), id).await?;
        let lines = df_db::list_purchase_order_lines(self.pool(), id).await?;
        let received = df_db::po_received_by_product(self.pool(), id).await?;
        let mut views = Vec::with_capacity(lines.len());
        for line in lines {
            let sku = df_db::get_product(self.pool(), line.product_id)
                .await
                .map(|product| product.sku)
                .unwrap_or_default();
            views.push(PoLineView {
                id: line.id,
                line_no: line.line_no,
                product_id: line.product_id,
                sku,
                description: line.description.clone(),
                qty: line.qty,
                received_qty: received
                    .get(&line.product_id)
                    .copied()
                    .unwrap_or(Decimal::ZERO),
                uom: line.uom.clone(),
                unit_cost: line.unit_cost,
            });
        }
        Ok(PurchaseOrderView {
            id: po.id,
            po_number: po.po_number,
            vendor_id: po.vendor_id,
            store_id: po.store_id,
            status: po.status,
            total: po.total,
            lines: views,
            created_at: po.created_at,
        })
    }

    /// Create a draft PO from explicit lines (the back-office posts
    /// suggested lines, or manual ones). PO numbers are
    /// `PO-<YYYYMMDD>-<n>` with `n` the per-store sequence.
    pub async fn create_purchase_order(
        &self,
        request: CreatePurchaseOrderRequest,
    ) -> Result<PurchaseOrderView, EdgeError> {
        if request.vendor_id.trim().is_empty() {
            return Err(EdgeError::InvalidInput("vendor_id is required".to_string()));
        }
        if request.lines.is_empty() {
            return Err(EdgeError::InvalidInput(
                "a purchase order needs at least one line".to_string(),
            ));
        }
        let store_id = self.store_or_default(request.store_id);
        let mut lines: Vec<PurchaseOrderLineRow> = Vec::with_capacity(request.lines.len());
        let mut total = Decimal::ZERO;
        for (index, input) in request.lines.iter().enumerate() {
            if input.qty <= Decimal::ZERO {
                return Err(EdgeError::InvalidInput(format!(
                    "PO line qty must be positive: {}",
                    input.qty
                )));
            }
            let product = df_db::get_product(self.pool(), input.product_id).await?;
            let unit_cost = input.unit_cost.unwrap_or(product.avg_cost);
            if unit_cost < Decimal::ZERO {
                return Err(EdgeError::InvalidInput(format!(
                    "PO line unit_cost must not be negative: {unit_cost}"
                )));
            }
            total += input.qty * unit_cost;
            lines.push(PurchaseOrderLineRow {
                id: Uuid::now_v7(),
                po_id: Uuid::nil(),
                product_id: input.product_id,
                line_no: (index + 1) as i32,
                vendor_sku: product.sku.clone(),
                description: product.name.clone(),
                qty: input.qty,
                uom: input.uom.clone().unwrap_or(product.base_uom),
                unit_cost,
            });
        }
        let po_id = Uuid::now_v7();
        let now = Utc::now();
        let seq = df_db::count_purchase_orders_for_store(self.pool(), store_id).await? + 1;
        let po = PurchaseOrderRow {
            id: po_id,
            po_number: format!("PO-{}-{:04}", now.format("%Y%m%d"), seq),
            vendor_id: request.vendor_id,
            store_id,
            status: "draft".to_string(),
            total,
            created_at: now,
        };
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_purchase_order(&mut *db_tx, &po).await?;
        for line in &lines {
            let mut line = line.clone();
            line.po_id = po_id;
            df_db::insert_purchase_order_line(&mut *db_tx, &line).await?;
        }
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        self.purchase_order_view(po_id).await
    }

    /// POs for a store, newest first.
    pub async fn list_purchase_orders(
        &self,
        store_id: Uuid,
    ) -> Result<Vec<PurchaseOrderView>, EdgeError> {
        let pos = df_db::list_purchase_orders_for_store(self.pool(), store_id).await?;
        let mut views = Vec::with_capacity(pos.len());
        for po in pos {
            views.push(self.purchase_order_view(po.id).await?);
        }
        Ok(views)
    }

    /// Receive against a PO: per-line received quantities post
    /// `po_receive` ledger entries and move the PO to
    /// `partially_received` (or `received` when every line is
    /// fulfilled), all in one transaction. Receiving more than the
    /// ordered quantity (cumulative) is rejected and posts nothing.
    pub async fn receive_purchase_order(
        &self,
        id: Uuid,
        request: ReceivePurchaseOrderRequest,
    ) -> Result<PurchaseOrderView, EdgeError> {
        if request.lines.is_empty() {
            return Err(EdgeError::InvalidInput(
                "receive needs at least one line".to_string(),
            ));
        }
        let po = df_db::get_purchase_order(self.pool(), id).await?;
        if po.status == "received" || po.status == "cancelled" {
            return Err(EdgeError::InvalidInput(format!(
                "purchase order {} is {} and cannot be received",
                po.po_number, po.status
            )));
        }
        let lines = df_db::list_purchase_order_lines(self.pool(), id).await?;
        let already = df_db::po_received_by_product(self.pool(), id).await?;
        // Resolve each requested line and validate before posting.
        let mut resolved: Vec<(&PurchaseOrderLineRow, Decimal)> = Vec::new();
        for input in &request.lines {
            if input.qty <= Decimal::ZERO {
                return Err(EdgeError::InvalidInput(format!(
                    "received qty must be positive: {}",
                    input.qty
                )));
            }
            let line = match input.line_id {
                Some(line_id) => lines.iter().find(|l| l.id == line_id),
                None => lines.iter().find(|l| l.product_id == input.product_id),
            }
            .ok_or_else(|| {
                EdgeError::NotFound(format!(
                    "no PO line for product {} on order {}",
                    input.product_id, po.po_number
                ))
            })?;
            let received_so_far = already
                .get(&line.product_id)
                .copied()
                .unwrap_or(Decimal::ZERO);
            if received_so_far + input.qty > line.qty {
                return Err(EdgeError::InvalidInput(format!(
                    "cannot receive {} of {}: {} already received of {} ordered",
                    input.qty, line.product_id, received_so_far, line.qty
                )));
            }
            resolved.push((line, input.qty));
        }
        let now = Utc::now();
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        for (line, qty) in &resolved {
            let entry = df_db::models::InventoryLedgerEntry {
                id: Uuid::now_v7(),
                org_id: self.config().org_id,
                store_id: po.store_id,
                product_id: line.product_id,
                lot_number: None,
                delta_on_hand: *qty,
                delta_committed: Decimal::ZERO,
                delta_on_order: Decimal::ZERO,
                delta_in_transit: Decimal::ZERO,
                unit_cost: line.unit_cost,
                reason_type: "po_receive".to_string(),
                reference_id: id,
                created_at: now,
            };
            df_db::insert_inventory_delta(&mut *db_tx, &entry).await?;
            let outbox = inventory_outbox_entry(po.store_id, &entry);
            Outbox::enqueue_exec(&mut *db_tx, &outbox).await?;
        }
        // Recompute received totals including this receive.
        let mut received_after = already.clone();
        for (line, qty) in &resolved {
            *received_after
                .entry(line.product_id)
                .or_insert(Decimal::ZERO) += *qty;
        }
        let fully_received = lines.iter().all(|line| {
            received_after
                .get(&line.product_id)
                .copied()
                .unwrap_or(Decimal::ZERO)
                >= line.qty
        });
        let status = if fully_received {
            "received"
        } else {
            "partially_received"
        };
        df_db::update_purchase_order_status(&mut *db_tx, id, status).await?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        self.purchase_order_view(id).await
    }

    // --- inter-store transfers ------------------------------------------------

    /// Build the API view of a transfer from its rows.
    async fn transfer_view(&self, id: Uuid) -> Result<Transfer, EdgeError> {
        let row = df_db::get_inventory_transfer(self.pool(), id).await?;
        let lines = df_db::list_transfer_lines(self.pool(), id).await?;
        Ok(Transfer {
            id: row.id,
            from_store_id: row.from_store_id,
            to_store_id: row.to_store_id,
            status: row.status,
            lines: lines
                .into_iter()
                .map(|line| TransferLineView {
                    product_id: line.product_id,
                    qty: line.qty,
                    unit_cost: line.unit_cost,
                })
                .collect(),
            created_at: row.created_at,
        })
    }

    /// Draft a transfer. Nothing moves until [`Edge::ship_transfer`].
    pub async fn create_transfer(
        &self,
        request: CreateTransferRequest,
    ) -> Result<Transfer, EdgeError> {
        if request.from_store_id == request.to_store_id {
            return Err(EdgeError::InvalidInput(
                "a transfer needs two different stores".to_string(),
            ));
        }
        if request.lines.is_empty() {
            return Err(EdgeError::InvalidInput(
                "a transfer needs at least one line".to_string(),
            ));
        }
        let transfer_id = Uuid::now_v7();
        let now = Utc::now();
        let mut rows: Vec<TransferLineRow> = Vec::with_capacity(request.lines.len());
        for line in &request.lines {
            if line.qty <= Decimal::ZERO {
                return Err(EdgeError::InvalidInput(format!(
                    "transfer qty must be positive: {}",
                    line.qty
                )));
            }
            let product = df_db::get_product(self.pool(), line.product_id).await?;
            rows.push(TransferLineRow {
                id: Uuid::now_v7(),
                transfer_id,
                product_id: line.product_id,
                qty: line.qty,
                unit_cost: product.avg_cost,
            });
        }
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_inventory_transfer(
            &mut *db_tx,
            &InventoryTransfer {
                id: transfer_id,
                org_id: self.config().org_id,
                from_store_id: request.from_store_id,
                to_store_id: request.to_store_id,
                status: "draft".to_string(),
                reference: None,
                created_at: now,
            },
        )
        .await?;
        for row in &rows {
            df_db::insert_transfer_line(&mut *db_tx, row).await?;
        }
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        self.transfer_view(transfer_id).await
    }

    /// Transfers out of or into a store, newest first.
    pub async fn list_transfers(&self, store_id: Uuid) -> Result<Vec<Transfer>, EdgeError> {
        let rows = df_db::list_transfers_for_store(self.pool(), store_id).await?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            views.push(self.transfer_view(row.id).await?);
        }
        Ok(views)
    }

    /// Ship a draft transfer: per line, one ledger entry at the
    /// source store with `delta_on_hand = -qty` and
    /// `delta_in_transit = +qty` (`reason_type = 'transfer'`,
    /// `reference_id` = transfer id), plus the status flip, in one
    /// transaction. The source must hold the full quantity of every
    /// line.
    pub async fn ship_transfer(&self, id: Uuid) -> Result<Transfer, EdgeError> {
        let row = df_db::get_inventory_transfer(self.pool(), id).await?;
        if row.status != "draft" {
            return Err(EdgeError::InvalidInput(format!(
                "transfer {} is {} and cannot ship",
                row.id, row.status
            )));
        }
        let lines = df_db::list_transfer_lines(self.pool(), id).await?;
        for line in &lines {
            let on_hand =
                df_db::inventory_on_hand(self.pool(), row.from_store_id, line.product_id).await?;
            if on_hand < line.qty {
                return Err(EdgeError::InvalidInput(format!(
                    "cannot ship {} of {} from store {}: only {} on hand",
                    line.qty, line.product_id, row.from_store_id, on_hand
                )));
            }
        }
        let now = Utc::now();
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        for line in &lines {
            let entry = df_db::models::InventoryLedgerEntry {
                id: Uuid::now_v7(),
                org_id: self.config().org_id,
                store_id: row.from_store_id,
                product_id: line.product_id,
                lot_number: None,
                delta_on_hand: -line.qty,
                delta_committed: Decimal::ZERO,
                delta_on_order: Decimal::ZERO,
                delta_in_transit: line.qty,
                unit_cost: line.unit_cost,
                reason_type: "transfer".to_string(),
                reference_id: id,
                created_at: now,
            };
            df_db::insert_inventory_delta(&mut *db_tx, &entry).await?;
            let outbox = inventory_outbox_entry(row.from_store_id, &entry);
            Outbox::enqueue_exec(&mut *db_tx, &outbox).await?;
        }
        // Same statement as `df-db::ar::set_transfer_status`, inside
        // this transaction so the flip and the ledger post commit
        // together (see the module docs).
        sqlx::query("UPDATE inventory_transfers SET status = $1 WHERE id = $2")
            .bind("shipped")
            .bind(id.to_string())
            .execute(&mut *db_tx)
            .await
            .map_err(DbError::Sqlx)?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        self.transfer_view(id).await
    }

    /// Receive a shipped transfer: the in-transit leg closes at the
    /// source (`delta_in_transit = -qty`) and the destination gains
    /// the stock (`delta_on_hand = +qty`), both `reason_type =
    /// 'transfer'` with the transfer id, plus the status flip, in
    /// one transaction.
    pub async fn receive_transfer(&self, id: Uuid) -> Result<Transfer, EdgeError> {
        let row = df_db::get_inventory_transfer(self.pool(), id).await?;
        if row.status != "shipped" {
            return Err(EdgeError::InvalidInput(format!(
                "transfer {} is {} and cannot be received (ship it first)",
                row.id, row.status
            )));
        }
        let lines = df_db::list_transfer_lines(self.pool(), id).await?;
        let now = Utc::now();
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        for line in &lines {
            let close_entry = df_db::models::InventoryLedgerEntry {
                id: Uuid::now_v7(),
                org_id: self.config().org_id,
                store_id: row.from_store_id,
                product_id: line.product_id,
                lot_number: None,
                delta_on_hand: Decimal::ZERO,
                delta_committed: Decimal::ZERO,
                delta_on_order: Decimal::ZERO,
                delta_in_transit: -line.qty,
                unit_cost: line.unit_cost,
                reason_type: "transfer".to_string(),
                reference_id: id,
                created_at: now,
            };
            df_db::insert_inventory_delta(&mut *db_tx, &close_entry).await?;
            let close_outbox = inventory_outbox_entry(row.from_store_id, &close_entry);
            Outbox::enqueue_exec(&mut *db_tx, &close_outbox).await?;
            let arrive_entry = df_db::models::InventoryLedgerEntry {
                id: Uuid::now_v7(),
                org_id: self.config().org_id,
                store_id: row.to_store_id,
                product_id: line.product_id,
                lot_number: None,
                delta_on_hand: line.qty,
                delta_committed: Decimal::ZERO,
                delta_on_order: Decimal::ZERO,
                delta_in_transit: Decimal::ZERO,
                unit_cost: line.unit_cost,
                reason_type: "transfer".to_string(),
                reference_id: id,
                created_at: now,
            };
            df_db::insert_inventory_delta(&mut *db_tx, &arrive_entry).await?;
            let arrive_outbox = inventory_outbox_entry(row.to_store_id, &arrive_entry);
            Outbox::enqueue_exec(&mut *db_tx, &arrive_outbox).await?;
        }
        sqlx::query("UPDATE inventory_transfers SET status = $1 WHERE id = $2")
            .bind("received")
            .bind(id.to_string())
            .execute(&mut *db_tx)
            .await
            .map_err(DbError::Sqlx)?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        self.transfer_view(id).await
    }

    // --- physical counts ------------------------------------------------------

    /// Build the API view of a count from its rows. Adjustments are
    /// derived from the stored line variances (empty while open).
    async fn count_view(&self, id: Uuid) -> Result<PhysicalCount, EdgeError> {
        let row = df_db::get_physical_count(self.pool(), id).await?;
        let lines = df_db::list_count_lines(self.pool(), id).await?;
        let mut views = Vec::with_capacity(lines.len());
        let mut adjustments = Vec::with_capacity(lines.len());
        for line in lines {
            views.push(CountLineView {
                product_id: line.product_id,
                counted_qty: line.counted_qty,
                system_qty: line.system_qty,
                variance_qty: line.variance_qty,
                unit_cost: line.unit_cost,
            });
            if row.status == "reconciled" {
                adjustments.push(CountAdjustment {
                    product_id: line.product_id,
                    on_hand_before: line.system_qty,
                    counted_qty: line.counted_qty,
                    adjustment: line.variance_qty,
                });
            }
        }
        Ok(PhysicalCount {
            id: row.id,
            store_id: row.store_id,
            status: row.status,
            lines: views,
            adjustments,
            created_at: row.created_at,
            counted_at: row.counted_at,
        })
    }

    /// Start an open count at a store.
    pub async fn start_count(
        &self,
        request: StartCountRequest,
    ) -> Result<PhysicalCount, EdgeError> {
        let id = Uuid::now_v7();
        df_db::insert_physical_count(
            self.pool(),
            &PhysicalCountRow {
                id,
                store_id: self.store_or_default(request.store_id),
                status: "open".to_string(),
                counted_at: None,
                created_at: Utc::now(),
            },
        )
        .await?;
        self.count_view(id).await
    }

    /// Counts at a store, newest first.
    pub async fn list_counts(&self, store_id: Uuid) -> Result<Vec<PhysicalCount>, EdgeError> {
        let rows = df_db::list_physical_counts_for_store(self.pool(), store_id).await?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            views.push(self.count_view(row.id).await?);
        }
        Ok(views)
    }

    /// Record (or replace) the counted quantity for one product on
    /// an open count. The `count_lines` row stores the system
    /// quantity at scan time and the variance against it; reconcile
    /// recomputes both against the on-hand at reconcile time.
    pub async fn scan_count(
        &self,
        id: Uuid,
        request: ScanCountRequest,
    ) -> Result<PhysicalCount, EdgeError> {
        if request.counted_qty < Decimal::ZERO {
            return Err(EdgeError::InvalidInput(format!(
                "counted qty must not be negative: {}",
                request.counted_qty
            )));
        }
        let count = df_db::get_physical_count(self.pool(), id).await?;
        if count.status != "open" {
            return Err(EdgeError::InvalidInput(format!(
                "physical count {} is {} and cannot be scanned",
                count.id, count.status
            )));
        }
        let product = df_db::get_product(self.pool(), request.product_id).await?;
        let system_qty =
            df_db::inventory_on_hand(self.pool(), count.store_id, request.product_id).await?;
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        sqlx::query("DELETE FROM count_lines WHERE count_id = $1 AND product_id = $2")
            .bind(id.to_string())
            .bind(request.product_id.to_string())
            .execute(&mut *db_tx)
            .await
            .map_err(DbError::Sqlx)?;
        df_db::insert_count_line(
            &mut *db_tx,
            &CountLineRow {
                id: Uuid::now_v7(),
                count_id: id,
                product_id: request.product_id,
                counted_qty: request.counted_qty,
                system_qty,
                variance_qty: request.counted_qty - system_qty,
                unit_cost: product.avg_cost,
            },
        )
        .await?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        self.count_view(id).await
    }

    /// Reconcile an open count: for every scanned line, post a
    /// `count_adj` ledger entry of `counted - on_hand` (zero
    /// adjustments post nothing) and rewrite the line row with the
    /// final system quantity and variance, then mark the count
    /// reconciled — all in one transaction, so reconciled on-hand
    /// equals the counted quantity. A reconciled count cannot be
    /// scanned or reconciled again.
    pub async fn reconcile_count(&self, id: Uuid) -> Result<CountReconcileResponse, EdgeError> {
        let count = df_db::get_physical_count(self.pool(), id).await?;
        if count.status != "open" {
            return Err(EdgeError::InvalidInput(format!(
                "physical count {} is {} and cannot be reconciled",
                count.id, count.status
            )));
        }
        let lines = df_db::list_count_lines(self.pool(), id).await?;
        // Read on-hand before opening the transaction (the pool may
        // hold a single connection for :memory: SQLite; a live
        // concurrent sale between this read and the post is a
        // genuine race, documented here rather than hidden).
        let now = Utc::now();
        let mut adjustments: Vec<CountAdjustment> = Vec::with_capacity(lines.len());
        let mut rewrites: Vec<CountLineRow> = Vec::with_capacity(lines.len());
        for line in &lines {
            let on_hand_before =
                df_db::inventory_on_hand(self.pool(), count.store_id, line.product_id).await?;
            let adjustment = line.counted_qty - on_hand_before;
            adjustments.push(CountAdjustment {
                product_id: line.product_id,
                on_hand_before,
                counted_qty: line.counted_qty,
                adjustment,
            });
            rewrites.push(CountLineRow {
                id: Uuid::now_v7(),
                count_id: id,
                product_id: line.product_id,
                counted_qty: line.counted_qty,
                system_qty: on_hand_before,
                variance_qty: adjustment,
                unit_cost: line.unit_cost,
            });
        }
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        for (line, rewrite) in lines.iter().zip(rewrites.iter()) {
            let adjustment = rewrite.variance_qty;
            if adjustment != Decimal::ZERO {
                let entry = df_db::models::InventoryLedgerEntry {
                    id: Uuid::now_v7(),
                    org_id: self.config().org_id,
                    store_id: count.store_id,
                    product_id: line.product_id,
                    lot_number: None,
                    delta_on_hand: adjustment,
                    delta_committed: Decimal::ZERO,
                    delta_on_order: Decimal::ZERO,
                    delta_in_transit: Decimal::ZERO,
                    unit_cost: line.unit_cost,
                    reason_type: "count_adj".to_string(),
                    reference_id: id,
                    created_at: now,
                };
                df_db::insert_inventory_delta(&mut *db_tx, &entry).await?;
                let outbox = inventory_outbox_entry(count.store_id, &entry);
                Outbox::enqueue_exec(&mut *db_tx, &outbox).await?;
            }
            sqlx::query("DELETE FROM count_lines WHERE id = $1")
                .bind(line.id.to_string())
                .execute(&mut *db_tx)
                .await
                .map_err(DbError::Sqlx)?;
            df_db::insert_count_line(&mut *db_tx, rewrite).await?;
        }
        // Same statement as `df-db::ar::reconcile_physical_count`,
        // inside this transaction (see the module docs).
        sqlx::query(
            "UPDATE physical_counts SET status = 'reconciled', counted_at = $1 WHERE id = $2",
        )
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&mut *db_tx)
        .await
        .map_err(DbError::Sqlx)?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        Ok(CountReconcileResponse {
            count_id: id,
            store_id: count.store_id,
            status: "reconciled".to_string(),
            adjustments,
        })
    }

    // --- shelf tags -----------------------------------------------------------

    /// Printable ZPL shelf tag for one product, via the real
    /// `df-hw` label template (price = list price, store currency).
    pub async fn shelf_tag(&self, product_id: Uuid) -> Result<ShelfTagResponse, EdgeError> {
        let product = df_db::get_product(self.pool(), product_id).await?;
        let price = Money::new(product.list_price, self.config().currency);
        let zpl = df_hw::shelf_label_zpl(&product.sku, &product.name, price, &product.sku);
        Ok(ShelfTagResponse {
            product_id,
            sku: product.sku.clone(),
            name: product.name,
            price,
            barcode: product.sku,
            zpl,
        })
    }

    // --- bulk matrix pricing --------------------------------------------------

    /// All pricing rules for the org, by priority.
    pub async fn list_pricing_rules(&self) -> Result<Vec<PricingRuleRow>, EdgeError> {
        Ok(df_db::list_pricing_rules(self.pool(), self.config().org_id).await?)
    }

    /// Bulk-update pricing-rule calculations. Every update is fully
    /// validated first (static rule checks + the min-margin
    /// guardrail, evaluated against the rule's product when the rule
    /// targets one product); a single rejection fails the whole
    /// batch with no rule changed.
    pub async fn bulk_update_pricing_rules(
        &self,
        request: BulkPricingRequest,
    ) -> Result<Vec<PricingRuleRow>, EdgeError> {
        if request.rules.is_empty() {
            return Err(EdgeError::InvalidInput(
                "bulk pricing update needs at least one rule".to_string(),
            ));
        }
        let today = Utc::now().date_naive();
        let mut updated: Vec<PricingRuleRow> = Vec::with_capacity(request.rules.len());
        for input in &request.rules {
            let mut row = df_db::get_pricing_rule(self.pool(), input.id).await?;
            match input.calc_method.as_str() {
                "fixed" | "pct_off_list" | "markup_cost" | "target_gm" => {}
                other => {
                    return Err(EdgeError::InvalidInput(format!(
                        "unknown calc_method for rule {}: {other:?}",
                        input.id
                    )));
                }
            }
            let value_ok = match input.calc_method.as_str() {
                "fixed" => input.calc_value >= Decimal::ZERO,
                "pct_off_list" => {
                    input.calc_value >= Decimal::ZERO && input.calc_value <= Decimal::from(100)
                }
                "markup_cost" => input.calc_value >= Decimal::ZERO,
                "target_gm" => {
                    input.calc_value >= Decimal::ZERO && input.calc_value < Decimal::from(100)
                }
                _ => false,
            };
            if !value_ok {
                return Err(EdgeError::InvalidInput(format!(
                    "calc_value {} is out of range for calc_method {} (rule {})",
                    input.calc_value, input.calc_method, input.id
                )));
            }
            row.calc_method = input.calc_method.clone();
            row.calc_value = input.calc_value;
            row.min_margin_floor = input.min_margin_floor;
            let domain: PricingRule = row.to_domain()?;
            validate_rule(&domain)?;
            if let Some(product_id) = domain.product_id {
                let product = df_db::get_product(self.pool(), product_id).await?;
                let list_price = Money::new(product.list_price, self.config().currency);
                let cost = Money::new(product.avg_cost, self.config().currency);
                if let Some(reason) =
                    margin_guardrail_rejection(&domain, list_price, cost, domain.min_qty, today)
                {
                    return Err(EdgeError::InvalidInput(format!(
                        "bulk pricing update rejected: {reason}"
                    )));
                }
            }
            updated.push(row);
        }
        for row in &updated {
            df_db::update_pricing_rule(self.pool(), row).await?;
        }
        Ok(updated)
    }

    // --- AR reads (delegate to df-domain::ar math + df-db ledger) -------------

    /// One customer's AR position: ledger balance, credit controls
    /// and entry count.
    pub async fn ar_customer_summary(
        &self,
        customer_id: Uuid,
    ) -> Result<ArCustomerSummary, EdgeError> {
        let customer: Customer = df_db::get_customer(self.pool(), customer_id).await?;
        let entries = df_db::list_ar_entries_for_customer(self.pool(), customer_id).await?;
        let balance = entries
            .iter()
            .map(|entry| entry.amount)
            .fold(Decimal::ZERO, |acc, amount| acc + amount);
        let oldest_due_date = entries.iter().filter_map(|entry| entry.due_date).min();
        Ok(ArCustomerSummary {
            customer_id,
            account_number: customer.account_number,
            company_name: customer.company_name,
            customer_type: customer.customer_type,
            charge_enabled: customer.charge_enabled,
            credit_limit: Money::new(customer.credit_limit, self.config().currency),
            credit_hold: customer.credit_hold,
            balance: Money::new(balance, self.config().currency),
            entry_count: entries.len(),
            oldest_due_date,
        })
    }

    /// Bucket one customer's AR entries (30/60/90/120 aging) with
    /// `df-domain::ar`. Positive entries bucket by due date (no due
    /// date = current); payments and credit memos (negative amounts)
    /// are unallocated, so they reduce the Current bucket — the
    /// bucket sum always equals the customer's balance.
    fn age_customer_entries(
        entries: &[ArLedgerEntry],
        as_of: NaiveDate,
        currency: Currency,
    ) -> [Money; 5] {
        let mut open: Vec<ArEntry> = Vec::new();
        let mut credits = Money::zero(currency);
        for entry in entries {
            if entry.amount >= Decimal::ZERO {
                open.push(ArEntry {
                    id: entry.id,
                    customer_id: entry.customer_id,
                    job_id: entry.job_id,
                    balance: Money::new(entry.amount, currency),
                    due_date: entry.due_date.unwrap_or(entry.created_at.date_naive()),
                });
            } else {
                credits = credits
                    .checked_add(Money::new(entry.amount, currency))
                    .unwrap_or(credits);
            }
        }
        let mut buckets = aging_totals(&open, as_of, currency);
        if !credits.is_zero() {
            buckets[0] = buckets[0].checked_add(credits).unwrap_or(buckets[0]);
        }
        buckets
    }

    /// 30/60/90/120 aging for every customer with AR entries at a
    /// store (spec §6.1 `GET /ar/aging`).
    pub async fn ar_aging(&self, store_id: Uuid, as_of: NaiveDate) -> Result<AgingView, EdgeError> {
        let currency = self.config().currency;
        let entries = df_db::list_ar_entries_for_store(self.pool(), store_id).await?;
        let mut by_customer: HashMap<Uuid, Vec<&ArLedgerEntry>> = HashMap::new();
        for entry in &entries {
            by_customer
                .entry(entry.customer_id)
                .or_default()
                .push(entry);
        }
        let mut customers: Vec<AgingCustomerRow> = Vec::new();
        let mut totals = [
            zero_money(currency),
            zero_money(currency),
            zero_money(currency),
            zero_money(currency),
            zero_money(currency),
        ];
        for (customer_id, customer_entries) in &by_customer {
            let customer = df_db::get_customer(self.pool(), *customer_id).await?;
            let owned: Vec<ArLedgerEntry> = customer_entries
                .iter()
                .map(|entry| (*entry).clone())
                .collect();
            let buckets = Self::age_customer_entries(&owned, as_of, currency);
            let balance = owned
                .iter()
                .map(|entry| entry.amount)
                .fold(Decimal::ZERO, |acc, amount| acc + amount);
            for (index, bucket) in buckets.iter().enumerate() {
                totals[index] = totals[index].checked_add(*bucket).unwrap_or(totals[index]);
            }
            customers.push(AgingCustomerRow {
                customer_id: *customer_id,
                account_number: customer.account_number,
                balance: Money::new(balance, currency),
                buckets,
            });
        }
        customers.sort_by(|a, b| a.account_number.cmp(&b.account_number));
        Ok(AgingView {
            as_of,
            totals,
            customers,
        })
    }

    /// Collections workspace: customers who owe money and are
    /// overdue, on credit hold, or over their credit limit — most
    /// overdue first (spec §6.1 `GET /ar/collections`).
    pub async fn ar_collections(
        &self,
        store_id: Uuid,
        as_of: NaiveDate,
    ) -> Result<CollectionsView, EdgeError> {
        let currency = self.config().currency;
        let zero = zero_money(currency);
        let aging = self.ar_aging(store_id, as_of).await?;
        let mut rows: Vec<CollectionsRow> = Vec::new();
        for customer_row in &aging.customers {
            let customer = df_db::get_customer(self.pool(), customer_row.customer_id).await?;
            if customer_row.balance.amount() <= Decimal::ZERO {
                continue;
            }
            let overdue = customer_row.buckets[1..]
                .iter()
                .fold(zero, |acc, bucket| acc.checked_add(*bucket).unwrap_or(acc));
            let over_limit = customer_row.balance.amount() > customer.credit_limit;
            if overdue.amount() <= Decimal::ZERO && !customer.credit_hold && !over_limit {
                continue;
            }
            let entries =
                df_db::list_ar_entries_for_customer(self.pool(), customer_row.customer_id).await?;
            let oldest_due_date = entries
                .iter()
                .filter(|entry| entry.amount > Decimal::ZERO)
                .filter_map(|entry| entry.due_date)
                .min();
            rows.push(CollectionsRow {
                customer_id: customer_row.customer_id,
                account_number: customer_row.account_number.clone(),
                company_name: customer.company_name,
                balance: customer_row.balance,
                credit_limit: Money::new(customer.credit_limit, currency),
                credit_hold: customer.credit_hold,
                overdue,
                oldest_due_date,
            });
        }
        rows.sort_by(|a, b| {
            b.overdue
                .amount()
                .cmp(&a.overdue.amount())
                .then(b.balance.amount().cmp(&a.balance.amount()))
        });
        Ok(CollectionsView {
            as_of,
            customers: rows,
        })
    }
}
