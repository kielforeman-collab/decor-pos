//! Transaction repositories (Phase 1, spec §5.5).
//!
//! Write functions are generic over any sqlx executor so the edge
//! daemon can run them inside one local SQLite transaction: pass
//! `&pool` for standalone writes, `&mut *tx` for transactional ones.
//! Read functions take `&AnyPool`.
//!
//! Balances are never stored on transactions; totals are written at
//! checkout time from `df-domain` math and the append-only ledgers
//! (`inventory_ledger`, `ar_ledger`) carry the audit trail.

use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{ArLedgerEntry, Transaction, TransactionLine};

fn transaction_from_row(row: &sqlx::any::AnyRow) -> Result<Transaction, DbError> {
    Ok(Transaction {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        till_session_id: opt_uuid_col(row, "till_session_id")?,
        doc_number: text_col(row, "doc_number")?,
        doc_type: text_col(row, "doc_type")?,
        status: text_col(row, "status")?,
        customer_id: opt_uuid_col(row, "customer_id")?,
        job_id: opt_uuid_col(row, "job_id")?,
        po_number: opt_text_col(row, "po_number")?,
        salesperson_id: opt_uuid_col(row, "salesperson_id")?,
        cashier_id: opt_uuid_col(row, "cashier_id")?,
        currency: text_col(row, "currency")?,
        subtotal: dec_col(row, "subtotal")?,
        tax_total: dec_col(row, "tax_total")?,
        eco_fee_total: dec_col(row, "eco_fee_total")?,
        pos_fee_total: dec_col(row, "pos_fee_total")?,
        grand_total: dec_col(row, "grand_total")?,
        deposit_paid: dec_col(row, "deposit_paid")?,
        fulfillment_mode: text_col(row, "fulfillment_mode")?,
        scheduled_at: opt_dt_col(row, "scheduled_at")?,
        completed_at: opt_dt_col(row, "completed_at")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn transaction_line_from_row(row: &sqlx::any::AnyRow) -> Result<TransactionLine, DbError> {
    Ok(TransactionLine {
        id: uuid_col(row, "id")?,
        transaction_id: uuid_col(row, "transaction_id")?,
        line_no: i32_col(row, "line_no")?,
        product_id: uuid_col(row, "product_id")?,
        description: text_col(row, "description")?,
        qty: dec_col(row, "qty")?,
        uom_code: text_col(row, "uom_code")?,
        unit_price: dec_col(row, "unit_price")?,
        base_unit_cost: dec_col(row, "base_unit_cost")?,
        tint_unit_cost: dec_col(row, "tint_unit_cost")?,
        discount_amount: dec_col(row, "discount_amount")?,
        tax_amount: dec_col(row, "tax_amount")?,
        eco_fee_amount: dec_col(row, "eco_fee_amount")?,
        line_total: dec_col(row, "line_total")?,
        tint_formula_id: opt_uuid_col(row, "tint_formula_id")?,
        prompts_json: json_col(row, "prompts_json")?,
        vertical_spec_json: json_col(row, "vertical_spec_json")?,
        linked_po_line_id: opt_uuid_col(row, "linked_po_line_id")?,
        fulfilled_qty: dec_col(row, "fulfilled_qty")?,
    })
}

pub async fn insert_transaction<'e, E>(executor: E, tx: &Transaction) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO transactions (id, org_id, store_id, till_session_id, doc_number, doc_type, status, customer_id, job_id, po_number, salesperson_id, cashier_id, currency, subtotal, tax_total, eco_fee_total, pos_fee_total, grand_total, deposit_paid, fulfillment_mode, scheduled_at, completed_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(tx.id))
    .bind(uuid_str(tx.org_id))
    .bind(uuid_str(tx.store_id))
    .bind(opt_uuid_str(tx.till_session_id))
    .bind(&tx.doc_number)
    .bind(&tx.doc_type)
    .bind(&tx.status)
    .bind(opt_uuid_str(tx.customer_id))
    .bind(opt_uuid_str(tx.job_id))
    .bind(&tx.po_number)
    .bind(opt_uuid_str(tx.salesperson_id))
    .bind(opt_uuid_str(tx.cashier_id))
    .bind(&tx.currency)
    .bind(dec_str(tx.subtotal))
    .bind(dec_str(tx.tax_total))
    .bind(dec_str(tx.eco_fee_total))
    .bind(dec_str(tx.pos_fee_total))
    .bind(dec_str(tx.grand_total))
    .bind(dec_str(tx.deposit_paid))
    .bind(&tx.fulfillment_mode)
    .bind(opt_dt_str(tx.scheduled_at))
    .bind(opt_dt_str(tx.completed_at))
    .bind(dt_str(tx.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_transaction(pool: &AnyPool, id: Uuid) -> Result<Transaction, DbError> {
    let row = sqlx::query("SELECT * FROM transactions WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("transaction {id}")))?;
    transaction_from_row(&row)
}

pub async fn list_transactions_for_till(
    pool: &AnyPool,
    till_session_id: Uuid,
) -> Result<Vec<Transaction>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM transactions WHERE till_session_id = ? ORDER BY created_at ASC")
            .bind(uuid_str(till_session_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(transaction_from_row).collect()
}

/// Count of transactions for a store (used for sequential doc numbers).
pub async fn count_transactions_for_store(pool: &AnyPool, store_id: Uuid) -> Result<i64, DbError> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM transactions WHERE store_id = ?")
        .bind(uuid_str(store_id))
        .fetch_one(pool)
        .await?;
    i64_col(&row, "n")
}

/// All transactions for a store, oldest first. Callers filter by
/// date / status in Rust (see `list_tint_jobs_for_store` for why the
/// date filter is not SQL).
pub async fn list_transactions_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<Transaction>, DbError> {
    let rows = sqlx::query("SELECT * FROM transactions WHERE store_id = ? ORDER BY created_at ASC")
        .bind(uuid_str(store_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(transaction_from_row).collect()
}

pub async fn set_transaction_status(pool: &AnyPool, id: Uuid, status: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE transactions SET status = ? WHERE id = ?")
        .bind(status)
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_transaction_line<'e, E>(
    executor: E,
    line: &TransactionLine,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO transaction_lines (id, transaction_id, line_no, product_id, description, qty, uom_code, unit_price, base_unit_cost, tint_unit_cost, discount_amount, tax_amount, eco_fee_amount, line_total, tint_formula_id, prompts_json, vertical_spec_json, linked_po_line_id, fulfilled_qty) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(line.id))
    .bind(uuid_str(line.transaction_id))
    .bind(line.line_no)
    .bind(uuid_str(line.product_id))
    .bind(&line.description)
    .bind(dec_str(line.qty))
    .bind(&line.uom_code)
    .bind(dec_str(line.unit_price))
    .bind(dec_str(line.base_unit_cost))
    .bind(dec_str(line.tint_unit_cost))
    .bind(dec_str(line.discount_amount))
    .bind(dec_str(line.tax_amount))
    .bind(dec_str(line.eco_fee_amount))
    .bind(dec_str(line.line_total))
    .bind(opt_uuid_str(line.tint_formula_id))
    .bind(line.prompts_json.to_string())
    .bind(line.vertical_spec_json.to_string())
    .bind(opt_uuid_str(line.linked_po_line_id))
    .bind(dec_str(line.fulfilled_qty))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn list_transaction_lines(
    pool: &AnyPool,
    transaction_id: Uuid,
) -> Result<Vec<TransactionLine>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM transaction_lines WHERE transaction_id = ? ORDER BY line_no ASC",
    )
    .bind(uuid_str(transaction_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(transaction_line_from_row).collect()
}

/// AR entry insert that runs inside a caller's transaction (the
/// pool-based [`crate::insert_ar_entry`] cannot join one).
pub async fn insert_ar_entry_exec<'e, E>(executor: E, entry: &ArLedgerEntry) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO ar_ledger (id, org_id, store_id, customer_id, job_id, entry_type, reference_type, reference_id, amount, due_date, statement_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(entry.id))
    .bind(uuid_str(entry.org_id))
    .bind(uuid_str(entry.store_id))
    .bind(uuid_str(entry.customer_id))
    .bind(opt_uuid_str(entry.job_id))
    .bind(&entry.entry_type)
    .bind(&entry.reference_type)
    .bind(uuid_str(entry.reference_id))
    .bind(dec_str(entry.amount))
    .bind(
        entry
            .due_date
            .map(|d| d.to_string()),
    )
    .bind(opt_uuid_str(entry.statement_id))
    .bind(dt_str(entry.created_at))
    .execute(executor)
    .await?;
    Ok(())
}
