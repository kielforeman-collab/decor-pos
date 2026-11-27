//! Purchase-order repositories (Phase 2 `0004_phase2_edi` tables,
//! used by the Phase 3 back-office purchasing endpoints).
//!
//! Write functions are generic over any sqlx executor so the edge
//! can run them inside one local SQLite transaction (`&mut *tx`).
//!
//! Received quantities are **not** stored on the PO lines: they are
//! derived from the append-only `inventory_ledger`
//! ([`po_received_by_product`]), so a receive can never disagree
//! with the stock it posted.

use std::collections::HashMap;

use rust_decimal::Decimal;
use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{PurchaseOrderLineRow, PurchaseOrderRow};

pub(crate) fn po_from_row(row: &sqlx::any::AnyRow) -> Result<PurchaseOrderRow, DbError> {
    Ok(PurchaseOrderRow {
        id: uuid_col(row, "id")?,
        po_number: text_col(row, "po_number")?,
        vendor_id: text_col(row, "vendor_id")?,
        store_id: uuid_col(row, "store_id")?,
        status: text_col(row, "status")?,
        total: dec_col(row, "total")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn po_line_from_row(row: &sqlx::any::AnyRow) -> Result<PurchaseOrderLineRow, DbError> {
    Ok(PurchaseOrderLineRow {
        id: uuid_col(row, "id")?,
        po_id: uuid_col(row, "po_id")?,
        product_id: uuid_col(row, "product_id")?,
        line_no: i32_col(row, "line_no")?,
        vendor_sku: text_col(row, "vendor_sku")?,
        description: text_col(row, "description")?,
        qty: dec_col(row, "qty")?,
        uom: text_col(row, "uom")?,
        unit_cost: dec_col(row, "unit_cost")?,
    })
}

pub async fn insert_purchase_order<'e, E>(executor: E, po: &PurchaseOrderRow) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO purchase_orders (id, po_number, vendor_id, store_id, status, total, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid_str(po.id))
    .bind(&po.po_number)
    .bind(&po.vendor_id)
    .bind(uuid_str(po.store_id))
    .bind(&po.status)
    .bind(dec_str(po.total))
    .bind(dt_str(po.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn insert_purchase_order_line<'e, E>(
    executor: E,
    line: &PurchaseOrderLineRow,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO purchase_order_lines (id, po_id, product_id, line_no, vendor_sku, description, qty, uom, unit_cost) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(uuid_str(line.id))
    .bind(uuid_str(line.po_id))
    .bind(uuid_str(line.product_id))
    .bind(line.line_no)
    .bind(&line.vendor_sku)
    .bind(&line.description)
    .bind(dec_str(line.qty))
    .bind(&line.uom)
    .bind(dec_str(line.unit_cost))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_purchase_order(pool: &AnyPool, id: Uuid) -> Result<PurchaseOrderRow, DbError> {
    let row = sqlx::query("SELECT * FROM purchase_orders WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("purchase order {id}")))?;
    po_from_row(&row)
}

/// POs for a store, newest first.
pub async fn list_purchase_orders_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<PurchaseOrderRow>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM purchase_orders WHERE store_id = $1 ORDER BY created_at DESC")
            .bind(uuid_str(store_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(po_from_row).collect()
}

pub async fn list_purchase_order_lines(
    pool: &AnyPool,
    po_id: Uuid,
) -> Result<Vec<PurchaseOrderLineRow>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM purchase_order_lines WHERE po_id = $1 ORDER BY line_no ASC")
            .bind(uuid_str(po_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(po_line_from_row).collect()
}

/// Number of POs already stored for a store — the PO-number sequence
/// (`PO-<YYYYMMDD>-<n>`) is `count + 1` at creation time.
pub async fn count_purchase_orders_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<i64, DbError> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM purchase_orders WHERE store_id = $1")
        .bind(uuid_str(store_id))
        .fetch_one(pool)
        .await?;
    i64_col(&row, "n")
}

pub async fn update_purchase_order_status<'e, E>(
    executor: E,
    id: Uuid,
    status: &str,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query("UPDATE purchase_orders SET status = $1 WHERE id = $2")
        .bind(status)
        .bind(uuid_str(id))
        .execute(executor)
        .await?;
    Ok(())
}

/// Quantities already received against a PO, by product: the sum of
/// `delta_on_hand` over `inventory_ledger` rows with
/// `reason_type = 'po_receive'` and `reference_id` = the PO id.
/// Derived from the ledger (never stored), so it always agrees with
/// the stock the receives posted.
pub async fn po_received_by_product(
    pool: &AnyPool,
    po_id: Uuid,
) -> Result<HashMap<Uuid, Decimal>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM inventory_ledger WHERE reference_id = $1 AND reason_type = 'po_receive' ORDER BY created_at ASC",
    )
    .bind(uuid_str(po_id))
    .fetch_all(pool)
    .await?;
    let mut received: HashMap<Uuid, Decimal> = HashMap::new();
    for row in &rows {
        let product_id = uuid_col(row, "product_id")?;
        let delta = dec_col(row, "delta_on_hand")?;
        *received.entry(product_id).or_insert(Decimal::ZERO) += delta;
    }
    Ok(received)
}
