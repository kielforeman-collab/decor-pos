//! Portal payment records (Phase 5, spec §6.1
//! `POST /portal/payments/intent`).
//!
//! One row per bill payment, keyed by the client-generated
//! idempotency key: a retried payment with the same key returns
//! the original AR payment entry and posts nothing twice. The
//! row stores the provider reference string only — no card data,
//! no PAN/CVV, ever.

use sqlx::AnyPool;
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{BopisOrderRow, PortalPaymentRow};

fn portal_payment_from_row(row: &sqlx::any::AnyRow) -> Result<PortalPaymentRow, DbError> {
    Ok(PortalPaymentRow {
        idempotency_key: text_col(row, "idempotency_key")?,
        customer_id: uuid_col(row, "customer_id")?,
        payment_entry_id: uuid_col(row, "payment_entry_id")?,
        amount: dec_col(row, "amount")?,
        method: text_col(row, "method")?,
        provider_reference: text_col(row, "provider_reference")?,
        created_at: dt_col(row, "created_at")?,
    })
}

pub async fn insert_portal_payment<'e, E>(
    executor: E,
    row: &PortalPaymentRow,
) -> Result<(), DbError>
where
    E: sqlx::Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO portal_payments (idempotency_key, customer_id, payment_entry_id, amount, method, provider_reference, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.idempotency_key)
    .bind(uuid_str(row.customer_id))
    .bind(uuid_str(row.payment_entry_id))
    .bind(dec_str(row.amount))
    .bind(&row.method)
    .bind(&row.provider_reference)
    .bind(dt_str(row.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_portal_payment(
    pool: &AnyPool,
    idempotency_key: &str,
) -> Result<Option<PortalPaymentRow>, DbError> {
    let row = sqlx::query("SELECT * FROM portal_payments WHERE idempotency_key = ?")
        .bind(idempotency_key)
        .fetch_optional(pool)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(portal_payment_from_row(&row)?)),
    }
}

pub async fn list_portal_payments_for_customer(
    pool: &AnyPool,
    customer_id: Uuid,
) -> Result<Vec<PortalPaymentRow>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM portal_payments WHERE customer_id = ? ORDER BY created_at DESC")
            .bind(uuid_str(customer_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(portal_payment_from_row).collect()
}

fn bopis_order_from_row(row: &sqlx::any::AnyRow) -> Result<BopisOrderRow, DbError> {
    Ok(BopisOrderRow {
        order_id: uuid_col(row, "order_id")?,
        pickup_name: text_col(row, "pickup_name")?,
        contact: text_col(row, "contact")?,
        created_at: dt_col(row, "created_at")?,
    })
}

pub async fn insert_bopis_order<'e, E>(executor: E, row: &BopisOrderRow) -> Result<(), DbError>
where
    E: sqlx::Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO bopis_orders (order_id, pickup_name, contact, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(uuid_str(row.order_id))
    .bind(&row.pickup_name)
    .bind(&row.contact)
    .bind(dt_str(row.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_bopis_order(pool: &AnyPool, order_id: Uuid) -> Result<BopisOrderRow, DbError> {
    let row = sqlx::query("SELECT * FROM bopis_orders WHERE order_id = ?")
        .bind(uuid_str(order_id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("bopis order {order_id}")))?;
    bopis_order_from_row(&row)
}
