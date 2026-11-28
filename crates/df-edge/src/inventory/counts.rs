//! Physical inventory counts: start, scan, and reconcile variances.

use chrono::{DateTime, Utc};
use df_db::DbError;
use df_db::models::{CountLine as CountLineRow, PhysicalCount as PhysicalCountRow};
use df_sync::Outbox;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::inventory_outbox_entry;
use crate::edge::Edge;
use crate::error::EdgeError;

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

impl Edge {
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
}
