//! Inter-store inventory transfers (draft → ship → receive).

use chrono::{DateTime, Utc};
use df_db::DbError;
use df_db::models::{InventoryTransfer, TransferLine as TransferLineRow};
use df_sync::Outbox;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::inventory_outbox_entry;
use crate::edge::Edge;
use crate::error::EdgeError;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTransferRequest {
    pub from_store_id: Uuid,
    pub to_store_id: Uuid,
    pub lines: Vec<TransferLineView>,
}

impl Edge {
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
}
