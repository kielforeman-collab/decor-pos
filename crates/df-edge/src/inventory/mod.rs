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

mod ar_reads;
mod counts;
mod pricing;
mod purchasing;
mod transfers;

pub use ar_reads::{
    AgingCustomerRow, AgingView, ArCustomerSummary, CollectionsRow, CollectionsView,
};
pub use counts::{
    CountAdjustment, CountLineView, CountReconcileResponse, PhysicalCount, ScanCountRequest,
    StartCountRequest,
};
pub use pricing::{BulkPricingRequest, PricingRuleUpdate};
pub use purchasing::{
    CreatePoLineRequest, CreatePurchaseOrderRequest, PoLineView, PurchaseOrderView,
    ReceivePoLineRequest, ReceivePurchaseOrderRequest, ReplenishmentSuggestRequest, SuggestedLine,
    SuggestedPurchaseOrder,
};
pub use transfers::{CreateTransferRequest, Transfer, TransferLineView};

use df_db::InventoryBalance;
use df_db::models::{InventoryLedgerEntry, ReorderPoint};
use df_domain::Money;
use df_sync::OutboxEntry;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// GUI product import (`POST /pos/inventory/import`): the back
/// office parses the spreadsheet in the browser and posts the rows
/// as JSON — the file itself is never uploaded. `store_id` selects
/// the store context (defaults to the edge's configured store);
/// `rows` carry their 1-based spreadsheet row numbers so failures
/// point at the exact line in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportProductsRequest {
    pub store_id: Option<Uuid>,
    pub rows: Vec<df_db::ImportProductRow>,
}

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
pub struct ShelfTagResponse {
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub price: Money,
    /// The barcode encoded on the tag: the product's UPC when
    /// present, falling back to its SKU (Code 128 encodes it verbatim).
    pub barcode: String,
    pub zpl: String,
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

    // --- product import (GUI) -------------------------------------------------

    /// Upsert a vendor price list into `products` (GUI product
    /// import, `POST /pos/inventory/import`). Rows are validated and
    /// upserted by [`df_db::import_products`]: new SKUs are created,
    /// existing SKUs are repriced, bad rows are reported without
    /// failing the batch. The store must exist; products themselves
    /// are org-scoped. A 10,000-row cap keeps one request from
    /// importing an unbounded file.
    pub async fn import_products(
        &self,
        request: ImportProductsRequest,
    ) -> Result<df_db::ProductImportResult, EdgeError> {
        let store_id = self.store_or_default(request.store_id);
        df_db::get_store(self.pool(), store_id).await?;
        if request.rows.is_empty() {
            return Err(EdgeError::InvalidInput(
                "rows must not be empty".to_string(),
            ));
        }
        if request.rows.len() > 10_000 {
            return Err(EdgeError::InvalidInput(format!(
                "rows exceeds the 10,000-row import limit: {}",
                request.rows.len()
            )));
        }
        Ok(df_db::import_products(
            self.pool(),
            self.config().org_id,
            Some(store_id),
            &request.rows,
        )
        .await?)
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

    // --- shelf tags -----------------------------------------------------------

    /// Printable ZPL shelf tag for one product, via the real
    /// `df-hw` label template (price = list price, store currency).
    /// Encodes `product.upc` when present, falling back to `sku`.
    pub async fn shelf_tag(&self, product_id: Uuid) -> Result<ShelfTagResponse, EdgeError> {
        let product = df_db::get_product(self.pool(), product_id).await?;
        let price = Money::new(product.list_price, self.config().currency);
        let barcode = product
            .upc
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| product.sku.clone());
        let zpl = df_hw::shelf_label_zpl(&product.sku, &product.name, price, &barcode);
        Ok(ShelfTagResponse {
            product_id,
            sku: product.sku,
            name: product.name,
            price,
            barcode,
            zpl,
        })
    }
}
