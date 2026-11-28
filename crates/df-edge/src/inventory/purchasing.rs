//! Replenishment suggestions and purchase order creation / receiving.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use df_db::models::{PurchaseOrderLineRow, PurchaseOrderRow};
use df_db::{AnyPool, DbError, InventoryBalance};
use df_edi::{LowStockItem, generate_purchase_orders};
use df_sync::Outbox;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::inventory_outbox_entry;
use crate::edge::Edge;
use crate::error::EdgeError;

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

impl Edge {
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
    /// ledger-derived received quantities. `pub(crate)` so the
    /// vendors module can reuse it for a vendor's PO list.
    pub(crate) async fn purchase_order_view(
        &self,
        id: Uuid,
    ) -> Result<PurchaseOrderView, EdgeError> {
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
        // Every PO vendor has a vendor row: the migration backfill
        // covers POs that predate the `vendors` table, this covers
        // POs created after it (a no-op when the row exists).
        df_db::ensure_vendor_for_po(&mut *db_tx, self.config().org_id, &po.vendor_id, now).await?;
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
}
