//! Inventory ledger, till session, payment and sync-state repositories
//! (Phase 1, spec §5.5).
//!
//! Write functions are generic over any sqlx executor so the edge
//! daemon can run them inside one local SQLite transaction: pass
//! `&pool` for standalone writes, `&mut *tx` for transactional ones.
//! Read functions take `&AnyPool`.
//!
//! Inventory and AR balances are always derived by summing the
//! append-only ledger rows in Rust — never from a stored mutable
//! balance (spec §5).

use rust_decimal::Decimal;
use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{InventoryLedgerEntry, PaymentRow, SyncStateRow, TillSession};

fn inventory_entry_from_row(row: &sqlx::any::AnyRow) -> Result<InventoryLedgerEntry, DbError> {
    Ok(InventoryLedgerEntry {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        product_id: uuid_col(row, "product_id")?,
        lot_number: opt_text_col(row, "lot_number")?,
        delta_on_hand: dec_col(row, "delta_on_hand")?,
        delta_committed: dec_col(row, "delta_committed")?,
        delta_on_order: dec_col(row, "delta_on_order")?,
        delta_in_transit: dec_col(row, "delta_in_transit")?,
        unit_cost: dec_col(row, "unit_cost")?,
        reason_type: text_col(row, "reason_type")?,
        reference_id: uuid_col(row, "reference_id")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn till_session_from_row(row: &sqlx::any::AnyRow) -> Result<TillSession, DbError> {
    Ok(TillSession {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        cashier_id: uuid_col(row, "cashier_id")?,
        opened_at: dt_col(row, "opened_at")?,
        closed_at: opt_dt_col(row, "closed_at")?,
        opening_float: dec_col(row, "opening_float")?,
        counted_cash: opt_dec_col(row, "counted_cash")?,
        expected_cash: opt_dec_col(row, "expected_cash")?,
        status: text_col(row, "status")?,
    })
}

fn payment_row_from_row(row: &sqlx::any::AnyRow) -> Result<PaymentRow, DbError> {
    Ok(PaymentRow {
        id: uuid_col(row, "id")?,
        transaction_id: uuid_col(row, "transaction_id")?,
        tender: text_col(row, "tender")?,
        amount: dec_col(row, "amount")?,
        reference: text_col(row, "reference")?,
        authorization: opt_text_col(row, "authorization")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn sync_state_from_row(row: &sqlx::any::AnyRow) -> Result<SyncStateRow, DbError> {
    Ok(SyncStateRow {
        store_id: uuid_col(row, "store_id")?,
        last_batch_id: opt_uuid_col(row, "last_batch_id")?,
        last_synced_at: opt_dt_col(row, "last_synced_at")?,
        pending_count: i64_col(row, "pending_count")?,
    })
}

// --- inventory ledger -------------------------------------------------------

pub async fn insert_inventory_delta<'e, E>(
    executor: E,
    entry: &InventoryLedgerEntry,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO inventory_ledger (id, org_id, store_id, product_id, lot_number, delta_on_hand, delta_committed, delta_on_order, delta_in_transit, unit_cost, reason_type, reference_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(entry.id))
    .bind(uuid_str(entry.org_id))
    .bind(uuid_str(entry.store_id))
    .bind(uuid_str(entry.product_id))
    .bind(&entry.lot_number)
    .bind(dec_str(entry.delta_on_hand))
    .bind(dec_str(entry.delta_committed))
    .bind(dec_str(entry.delta_on_order))
    .bind(dec_str(entry.delta_in_transit))
    .bind(dec_str(entry.unit_cost))
    .bind(&entry.reason_type)
    .bind(uuid_str(entry.reference_id))
    .bind(dt_str(entry.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn list_inventory_entries_for_product(
    pool: &AnyPool,
    store_id: Uuid,
    product_id: Uuid,
) -> Result<Vec<InventoryLedgerEntry>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM inventory_ledger WHERE store_id = ? AND product_id = ? ORDER BY created_at ASC",
    )
    .bind(uuid_str(store_id))
    .bind(uuid_str(product_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(inventory_entry_from_row).collect()
}

/// On-hand balance for a product at a store, derived by summing the
/// append-only ledger in Rust (decimals are TEXT in SQLite, so SQL
/// `SUM` would be lossy/inexact).
pub async fn inventory_on_hand(
    pool: &AnyPool,
    store_id: Uuid,
    product_id: Uuid,
) -> Result<Decimal, DbError> {
    let entries = list_inventory_entries_for_product(pool, store_id, product_id).await?;
    Ok(entries
        .iter()
        .map(|e| e.delta_on_hand)
        .fold(Decimal::ZERO, |acc, delta| acc + delta))
}

/// One product's stock position at a store, derived from the
/// append-only `inventory_ledger` (spec §5: balances are derived,
/// never stored). Products with no ledger rows yet report zeroes.
/// Sorted by SKU.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InventoryBalance {
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub base_uom: String,
    pub list_price: Decimal,
    pub standard_cost: Decimal,
    pub on_hand: Decimal,
    pub committed: Decimal,
    pub on_order: Decimal,
    pub in_transit: Decimal,
}

/// Stock positions for every product of an org at one store.
pub async fn inventory_balances(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
) -> Result<Vec<InventoryBalance>, DbError> {
    let products = crate::list_products_for_org(pool, org_id).await?;
    let entries = list_inventory_entries_for_store(pool, store_id).await?;
    let mut balances: Vec<InventoryBalance> = products
        .into_iter()
        .map(|product| InventoryBalance {
            product_id: product.id,
            sku: product.sku,
            name: product.name,
            base_uom: product.base_uom,
            list_price: product.list_price,
            standard_cost: product.standard_cost,
            on_hand: Decimal::ZERO,
            committed: Decimal::ZERO,
            on_order: Decimal::ZERO,
            in_transit: Decimal::ZERO,
        })
        .collect();
    for entry in &entries {
        if let Some(balance) = balances
            .iter_mut()
            .find(|b| b.product_id == entry.product_id)
        {
            balance.on_hand += entry.delta_on_hand;
            balance.committed += entry.delta_committed;
            balance.on_order += entry.delta_on_order;
            balance.in_transit += entry.delta_in_transit;
        }
    }
    balances.sort_by(|a, b| a.sku.cmp(&b.sku));
    Ok(balances)
}

/// All inventory ledger entries for a store, oldest first — the raw
/// material for a low-stock report, which derives on-hand balances
/// per product by summing `delta_on_hand` in Rust (see
/// [`inventory_on_hand`]).
pub async fn list_inventory_entries_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<InventoryLedgerEntry>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM inventory_ledger WHERE store_id = ? ORDER BY created_at ASC")
            .bind(uuid_str(store_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(inventory_entry_from_row).collect()
}

// --- till sessions ----------------------------------------------------------

pub async fn insert_till_session<'e, E>(executor: E, till: &TillSession) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO till_sessions (id, org_id, store_id, cashier_id, opened_at, closed_at, opening_float, counted_cash, expected_cash, status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(till.id))
    .bind(uuid_str(till.org_id))
    .bind(uuid_str(till.store_id))
    .bind(uuid_str(till.cashier_id))
    .bind(dt_str(till.opened_at))
    .bind(opt_dt_str(till.closed_at))
    .bind(dec_str(till.opening_float))
    .bind(opt_dec_str(till.counted_cash))
    .bind(opt_dec_str(till.expected_cash))
    .bind(&till.status)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_till_session(pool: &AnyPool, id: Uuid) -> Result<TillSession, DbError> {
    let row = sqlx::query("SELECT * FROM till_sessions WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("till session {id}")))?;
    till_session_from_row(&row)
}

/// The currently open till for a store, if any (most recently opened).
pub async fn open_till_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Option<TillSession>, DbError> {
    let row = sqlx::query(
        "SELECT * FROM till_sessions WHERE store_id = ? AND status = 'open' ORDER BY opened_at DESC LIMIT 1",
    )
    .bind(uuid_str(store_id))
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(till_session_from_row(&row)?)),
    }
}

/// Close a till: stamp `closed_at`, `counted_cash`, `expected_cash`
/// and set `status = 'closed'`.
pub async fn close_till_session(
    pool: &AnyPool,
    id: Uuid,
    closed_at: chrono::DateTime<chrono::Utc>,
    counted_cash: Decimal,
    expected_cash: Decimal,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE till_sessions SET closed_at = ?, counted_cash = ?, expected_cash = ?, status = 'closed' WHERE id = ?",
    )
    .bind(dt_str(closed_at))
    .bind(dec_str(counted_cash))
    .bind(dec_str(expected_cash))
    .bind(uuid_str(id))
    .execute(pool)
    .await?;
    Ok(())
}

// --- payments ---------------------------------------------------------------

pub async fn insert_payment<'e, E>(executor: E, payment: &PaymentRow) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO payments (id, transaction_id, tender, amount, reference, authorization, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(payment.id))
    .bind(uuid_str(payment.transaction_id))
    .bind(&payment.tender)
    .bind(dec_str(payment.amount))
    .bind(&payment.reference)
    .bind(&payment.authorization)
    .bind(dt_str(payment.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn list_payments_for_transaction(
    pool: &AnyPool,
    transaction_id: Uuid,
) -> Result<Vec<PaymentRow>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM payments WHERE transaction_id = ? ORDER BY created_at ASC")
            .bind(uuid_str(transaction_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(payment_row_from_row).collect()
}

// --- sync state -------------------------------------------------------------

pub async fn get_sync_state(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Option<SyncStateRow>, DbError> {
    let row = sqlx::query("SELECT * FROM sync_state WHERE store_id = ?")
        .bind(uuid_str(store_id))
        .fetch_optional(pool)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(sync_state_from_row(&row)?)),
    }
}

/// Insert or replace the sync-state row for a store.
pub async fn upsert_sync_state(pool: &AnyPool, state: &SyncStateRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO sync_state (store_id, last_batch_id, last_synced_at, pending_count) VALUES (?, ?, ?, ?) ON CONFLICT(store_id) DO UPDATE SET last_batch_id = excluded.last_batch_id, last_synced_at = excluded.last_synced_at, pending_count = excluded.pending_count",
    )
    .bind(uuid_str(state.store_id))
    .bind(opt_uuid_str(state.last_batch_id))
    .bind(opt_dt_str(state.last_synced_at))
    .bind(state.pending_count)
    .execute(pool)
    .await?;
    Ok(())
}
