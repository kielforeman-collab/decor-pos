//! Contractor AR, statement, transfer, count and reorder-point
//! repositories (Phase 3, spec §5.4/§7).
//!
//! Write functions are generic over any sqlx executor so the edge
//! daemon can run them inside one local SQLite transaction: pass
//! `&pool` for standalone writes, `&mut *tx` for transactional
//! ones. Read functions take `&AnyPool`, except
//! [`find_ar_entry_by_reference`], which is executor-generic so
//! the idempotency check can run inside the statement-batch
//! transaction too.
//!
//! Balances are never stored; they are derived in Rust from the
//! append-only `ar_ledger` plus `ar_allocations` (see `df-ar`).

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{
    ArAllocation, ArLedgerEntry, CountLine, Customer, CustomerJob, InventoryTransfer,
    PhysicalCount, ReorderPoint, StatementRow, TransferLine,
};
use df_ar::{Allocation, LedgerEntry};

// --- row decoding -----------------------------------------------------------

fn customer_job_from_row(row: &sqlx::any::AnyRow) -> Result<CustomerJob, DbError> {
    Ok(CustomerJob {
        id: uuid_col(row, "id")?,
        customer_id: uuid_col(row, "customer_id")?,
        job_code: text_col(row, "job_code")?,
        job_name: text_col(row, "job_name")?,
        site_address_json: json_col(row, "site_address_json")?,
        tax_zone_override: opt_uuid_col(row, "tax_zone_override")?,
        is_active: bool_col(row, "is_active")?,
    })
}

fn ar_allocation_from_row(row: &sqlx::any::AnyRow) -> Result<ArAllocation, DbError> {
    Ok(ArAllocation {
        id: uuid_col(row, "id")?,
        payment_entry_id: uuid_col(row, "payment_entry_id")?,
        charge_entry_id: uuid_col(row, "charge_entry_id")?,
        allocated_amount: dec_col(row, "allocated_amount")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn statement_from_row(row: &sqlx::any::AnyRow) -> Result<StatementRow, DbError> {
    Ok(StatementRow {
        id: uuid_col(row, "id")?,
        customer_id: uuid_col(row, "customer_id")?,
        period_start: date_col(row, "period_start")?,
        period_end: date_col(row, "period_end")?,
        opening_balance: dec_col(row, "opening_balance")?,
        charges_total: dec_col(row, "charges_total")?,
        payments_total: dec_col(row, "payments_total")?,
        finance_charge_total: dec_col(row, "finance_charge_total")?,
        closing_balance: dec_col(row, "closing_balance")?,
        aging_json: json_col(row, "aging_json")?,
        pdf_sha256: opt_text_col(row, "pdf_sha256")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn transfer_from_row(row: &sqlx::any::AnyRow) -> Result<InventoryTransfer, DbError> {
    Ok(InventoryTransfer {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        from_store_id: uuid_col(row, "from_store_id")?,
        to_store_id: uuid_col(row, "to_store_id")?,
        status: text_col(row, "status")?,
        reference: opt_text_col(row, "reference")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn transfer_line_from_row(row: &sqlx::any::AnyRow) -> Result<TransferLine, DbError> {
    Ok(TransferLine {
        id: uuid_col(row, "id")?,
        transfer_id: uuid_col(row, "transfer_id")?,
        product_id: uuid_col(row, "product_id")?,
        qty: dec_col(row, "qty")?,
        unit_cost: dec_col(row, "unit_cost")?,
    })
}

fn physical_count_from_row(row: &sqlx::any::AnyRow) -> Result<PhysicalCount, DbError> {
    Ok(PhysicalCount {
        id: uuid_col(row, "id")?,
        store_id: uuid_col(row, "store_id")?,
        status: text_col(row, "status")?,
        counted_at: opt_dt_col(row, "counted_at")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn count_line_from_row(row: &sqlx::any::AnyRow) -> Result<CountLine, DbError> {
    Ok(CountLine {
        id: uuid_col(row, "id")?,
        count_id: uuid_col(row, "count_id")?,
        product_id: uuid_col(row, "product_id")?,
        counted_qty: dec_col(row, "counted_qty")?,
        system_qty: dec_col(row, "system_qty")?,
        variance_qty: dec_col(row, "variance_qty")?,
        unit_cost: dec_col(row, "unit_cost")?,
    })
}

fn reorder_point_from_row(row: &sqlx::any::AnyRow) -> Result<ReorderPoint, DbError> {
    Ok(ReorderPoint {
        id: uuid_col(row, "id")?,
        store_id: uuid_col(row, "store_id")?,
        product_id: uuid_col(row, "product_id")?,
        min_qty: dec_col(row, "min_qty")?,
        max_qty: dec_col(row, "max_qty")?,
        reorder_qty: dec_col(row, "reorder_qty")?,
    })
}

// --- customers: charge accounts ---------------------------------------------

/// One customer by id. Needed by the edge credit check; the
/// Phase 0 repo only had lookup by account number.
pub async fn get_customer(pool: &AnyPool, id: Uuid) -> Result<Customer, DbError> {
    let row = sqlx::query("SELECT * FROM customers WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("customer {id}")))?;
    crate::repos::customer_from_row(&row)
}

/// All charge-enabled customers for an org, by account number.
/// The statement batch and the finance-charge run iterate this.
pub async fn list_charge_customers(pool: &AnyPool, org_id: Uuid) -> Result<Vec<Customer>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM customers WHERE org_id = $1 AND charge_enabled = TRUE ORDER BY account_number ASC",
    )
    .bind(uuid_str(org_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(crate::repos::customer_from_row).collect()
}

/// Replace the credit-control fields of a customer (limit, terms
/// override, finance rate, hold flag). Pass the full new values;
/// `terms_days = None` clears the override (terms then come from
/// `payment_terms` again).
pub async fn update_customer_credit(
    pool: &AnyPool,
    id: Uuid,
    credit_limit: Decimal,
    terms_days: Option<i32>,
    finance_rate_pct_monthly: Option<Decimal>,
    credit_hold: bool,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE customers SET credit_limit = $1, terms_days = $2, finance_rate_pct_monthly = $3, credit_hold = $4 WHERE id = $5",
    )
    .bind(dec_str(credit_limit))
    .bind(terms_days)
    .bind(opt_dec_str(finance_rate_pct_monthly))
    .bind(credit_hold)
    .bind(uuid_str(id))
    .execute(pool)
    .await?;
    Ok(())
}

// --- customer jobs ----------------------------------------------------------

pub async fn insert_customer_job<'e, E>(executor: E, job: &CustomerJob) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO customer_jobs (id, customer_id, job_code, job_name, site_address_json, tax_zone_override, is_active) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid_str(job.id))
    .bind(uuid_str(job.customer_id))
    .bind(&job.job_code)
    .bind(&job.job_name)
    .bind(job.site_address_json.to_string())
    .bind(opt_uuid_str(job.tax_zone_override))
    .bind(job.is_active)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_customer_job(pool: &AnyPool, id: Uuid) -> Result<CustomerJob, DbError> {
    let row = sqlx::query("SELECT * FROM customer_jobs WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("customer job {id}")))?;
    customer_job_from_row(&row)
}

pub async fn list_jobs_for_customer(
    pool: &AnyPool,
    customer_id: Uuid,
) -> Result<Vec<CustomerJob>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM customer_jobs WHERE customer_id = $1 ORDER BY job_code ASC")
            .bind(uuid_str(customer_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(customer_job_from_row).collect()
}

// --- AR ledger: domain views + allocations ----------------------------------

/// Map a stored ledger row to the `df-ar` view type.
pub fn ledger_entry_to_domain(entry: &ArLedgerEntry) -> LedgerEntry {
    LedgerEntry {
        id: entry.id,
        customer_id: entry.customer_id,
        job_id: entry.job_id,
        entry_type: entry.entry_type.clone(),
        amount: entry.amount,
        due_date: entry.due_date,
        created_at: entry.created_at,
    }
}

/// Map a stored allocation row to the `df-ar` view type.
pub fn allocation_to_domain(allocation: &ArAllocation) -> Allocation {
    Allocation {
        payment_entry_id: allocation.payment_entry_id,
        charge_entry_id: allocation.charge_entry_id,
        allocated_amount: allocation.allocated_amount,
    }
}

/// Find one ledger entry by its (customer, type, reference) triple.
/// Used for the finance-charge idempotency check: the assessment
/// stores `reference_type = 'statement_run'` and a deterministic
/// `reference_id` per (customer, period), so a second run finds the
/// entry it already posted. Executor-generic so the check can run
/// inside the statement-batch transaction.
pub async fn find_ar_entry_by_reference<'e, E>(
    executor: E,
    customer_id: Uuid,
    entry_type: &str,
    reference_type: &str,
    reference_id: Uuid,
) -> Result<Option<ArLedgerEntry>, DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    let row = sqlx::query(
        "SELECT * FROM ar_ledger WHERE customer_id = $1 AND entry_type = $2 AND reference_type = $3 AND reference_id = $4",
    )
    .bind(uuid_str(customer_id))
    .bind(entry_type)
    .bind(reference_type)
    .bind(uuid_str(reference_id))
    .fetch_optional(executor)
    .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(crate::repos::ar_entry_from_row(&row)?)),
    }
}

/// One AR ledger entry by id, executor-generic (sync sink replay check:
/// the ingest endpoint skips a ledger entry whose id already exists).
pub async fn get_ar_entry_exec<'e, E>(
    executor: E,
    id: Uuid,
) -> Result<Option<ArLedgerEntry>, DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    let row = sqlx::query("SELECT * FROM ar_ledger WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_optional(executor)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(crate::repos::ar_entry_from_row(&row)?)),
    }
}

pub async fn insert_ar_allocation<'e, E>(
    executor: E,
    allocation: &ArAllocation,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO ar_allocations (id, payment_entry_id, charge_entry_id, allocated_amount, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid_str(allocation.id))
    .bind(uuid_str(allocation.payment_entry_id))
    .bind(uuid_str(allocation.charge_entry_id))
    .bind(dec_str(allocation.allocated_amount))
    .bind(dt_str(allocation.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

/// All allocations whose charge belongs to this customer (one JOIN,
/// not one query per charge). This is the allocation set
/// `df-ar::open_charges` needs alongside the ledger entries.
pub async fn list_allocations_for_customer(
    pool: &AnyPool,
    customer_id: Uuid,
) -> Result<Vec<ArAllocation>, DbError> {
    let rows = sqlx::query(
        "SELECT a.* FROM ar_allocations a JOIN ar_ledger c ON c.id = a.charge_entry_id WHERE c.customer_id = $1 ORDER BY a.created_at ASC",
    )
    .bind(uuid_str(customer_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(ar_allocation_from_row).collect()
}

pub async fn list_allocations_for_payment(
    pool: &AnyPool,
    payment_entry_id: Uuid,
) -> Result<Vec<ArAllocation>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM ar_allocations WHERE payment_entry_id = $1 ORDER BY created_at ASC",
    )
    .bind(uuid_str(payment_entry_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(ar_allocation_from_row).collect()
}

// --- statements -------------------------------------------------------------

pub async fn insert_statement<'e, E>(executor: E, row: &StatementRow) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO statements (id, customer_id, period_start, period_end, opening_balance, charges_total, payments_total, finance_charge_total, closing_balance, aging_json, pdf_sha256, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(uuid_str(row.id))
    .bind(uuid_str(row.customer_id))
    .bind(date_str(row.period_start))
    .bind(date_str(row.period_end))
    .bind(dec_str(row.opening_balance))
    .bind(dec_str(row.charges_total))
    .bind(dec_str(row.payments_total))
    .bind(dec_str(row.finance_charge_total))
    .bind(dec_str(row.closing_balance))
    .bind(row.aging_json.to_string())
    .bind(&row.pdf_sha256)
    .bind(dt_str(row.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

/// The frozen statement for one account and period, if the batch
/// already ran. `None` means the period has not been generated yet.
pub async fn get_statement(
    pool: &AnyPool,
    customer_id: Uuid,
    period_start: NaiveDate,
    period_end: NaiveDate,
) -> Result<Option<StatementRow>, DbError> {
    let row = sqlx::query(
        "SELECT * FROM statements WHERE customer_id = $1 AND period_start = $2 AND period_end = $3",
    )
    .bind(uuid_str(customer_id))
    .bind(date_str(period_start))
    .bind(date_str(period_end))
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(statement_from_row(&row)?)),
    }
}

pub async fn list_statements_for_customer(
    pool: &AnyPool,
    customer_id: Uuid,
) -> Result<Vec<StatementRow>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM statements WHERE customer_id = $1 ORDER BY period_start DESC")
            .bind(uuid_str(customer_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(statement_from_row).collect()
}

/// Record the PDF hash once the PDF worker has rendered a
/// statement. Does not touch any amount.
pub async fn set_statement_pdf_sha256(
    pool: &AnyPool,
    id: Uuid,
    pdf_sha256: &str,
) -> Result<(), DbError> {
    sqlx::query("UPDATE statements SET pdf_sha256 = $1 WHERE id = $2")
        .bind(pdf_sha256)
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- inventory transfers ----------------------------------------------------

pub async fn insert_inventory_transfer<'e, E>(
    executor: E,
    transfer: &InventoryTransfer,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO inventory_transfers (id, org_id, from_store_id, to_store_id, status, reference, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid_str(transfer.id))
    .bind(uuid_str(transfer.org_id))
    .bind(uuid_str(transfer.from_store_id))
    .bind(uuid_str(transfer.to_store_id))
    .bind(&transfer.status)
    .bind(&transfer.reference)
    .bind(dt_str(transfer.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_inventory_transfer(
    pool: &AnyPool,
    id: Uuid,
) -> Result<InventoryTransfer, DbError> {
    let row = sqlx::query("SELECT * FROM inventory_transfers WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("inventory transfer {id}")))?;
    transfer_from_row(&row)
}

/// Transfers out of or into a store, newest first. Used by the
/// back-office transfer list (Worker C's endpoints).
pub async fn list_transfers_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<InventoryTransfer>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM inventory_transfers WHERE from_store_id = $1 OR to_store_id = $2 ORDER BY created_at DESC",
    )
    .bind(uuid_str(store_id))
    .bind(uuid_str(store_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(transfer_from_row).collect()
}

/// Move a transfer through its lifecycle (`draft` → `shipped` →
/// `received`, or `cancelled`). No transition validation here —
/// the caller owns the state machine.
pub async fn set_transfer_status(pool: &AnyPool, id: Uuid, status: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE inventory_transfers SET status = $1 WHERE id = $2")
        .bind(status)
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_transfer_line<'e, E>(executor: E, line: &TransferLine) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO transfer_lines (id, transfer_id, product_id, qty, unit_cost) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid_str(line.id))
    .bind(uuid_str(line.transfer_id))
    .bind(uuid_str(line.product_id))
    .bind(dec_str(line.qty))
    .bind(dec_str(line.unit_cost))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn list_transfer_lines(
    pool: &AnyPool,
    transfer_id: Uuid,
) -> Result<Vec<TransferLine>, DbError> {
    let rows = sqlx::query("SELECT * FROM transfer_lines WHERE transfer_id = $1")
        .bind(uuid_str(transfer_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(transfer_line_from_row).collect()
}

// --- physical counts --------------------------------------------------------

pub async fn insert_physical_count<'e, E>(executor: E, count: &PhysicalCount) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO physical_counts (id, store_id, status, counted_at, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid_str(count.id))
    .bind(uuid_str(count.store_id))
    .bind(&count.status)
    .bind(count.counted_at.map(dt_str))
    .bind(dt_str(count.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_physical_count(pool: &AnyPool, id: Uuid) -> Result<PhysicalCount, DbError> {
    let row = sqlx::query("SELECT * FROM physical_counts WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("physical count {id}")))?;
    physical_count_from_row(&row)
}

/// Counts at a store, newest first. Used by the back-office count
/// list (Worker C's endpoints).
pub async fn list_physical_counts_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<PhysicalCount>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM physical_counts WHERE store_id = $1 ORDER BY created_at DESC")
            .bind(uuid_str(store_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(physical_count_from_row).collect()
}

/// Mark a count reconciled (`status = 'reconciled'`) and stamp
/// `counted_at`. The inventory adjustments themselves are written
/// to `inventory_ledger` by the reconciling worker.
pub async fn reconcile_physical_count(
    pool: &AnyPool,
    id: Uuid,
    counted_at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE physical_counts SET status = 'reconciled', counted_at = $1 WHERE id = $2")
        .bind(dt_str(counted_at))
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn insert_count_line<'e, E>(executor: E, line: &CountLine) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO count_lines (id, count_id, product_id, counted_qty, system_qty, variance_qty, unit_cost) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid_str(line.id))
    .bind(uuid_str(line.count_id))
    .bind(uuid_str(line.product_id))
    .bind(dec_str(line.counted_qty))
    .bind(dec_str(line.system_qty))
    .bind(dec_str(line.variance_qty))
    .bind(dec_str(line.unit_cost))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn list_count_lines(pool: &AnyPool, count_id: Uuid) -> Result<Vec<CountLine>, DbError> {
    let rows = sqlx::query("SELECT * FROM count_lines WHERE count_id = $1")
        .bind(uuid_str(count_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(count_line_from_row).collect()
}

// --- reorder points ---------------------------------------------------------

/// Insert or replace the min/max levels for one product at one
/// store (upsert on the `UNIQUE (store_id, product_id)` key).
/// Implemented as delete + insert: portable across both dialects
/// without `ON CONFLICT` syntax differences.
pub async fn upsert_reorder_point(pool: &AnyPool, point: &ReorderPoint) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM reorder_points WHERE store_id = $1 AND product_id = $2")
        .bind(uuid_str(point.store_id))
        .bind(uuid_str(point.product_id))
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO reorder_points (id, store_id, product_id, min_qty, max_qty, reorder_qty) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(uuid_str(point.id))
    .bind(uuid_str(point.store_id))
    .bind(uuid_str(point.product_id))
    .bind(dec_str(point.min_qty))
    .bind(dec_str(point.max_qty))
    .bind(dec_str(point.reorder_qty))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn get_reorder_point(
    pool: &AnyPool,
    store_id: Uuid,
    product_id: Uuid,
) -> Result<Option<ReorderPoint>, DbError> {
    let row = sqlx::query("SELECT * FROM reorder_points WHERE store_id = $1 AND product_id = $2")
        .bind(uuid_str(store_id))
        .bind(uuid_str(product_id))
        .fetch_optional(pool)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(reorder_point_from_row(&row)?)),
    }
}

pub async fn list_reorder_points_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<ReorderPoint>, DbError> {
    let rows = sqlx::query("SELECT * FROM reorder_points WHERE store_id = $1")
        .bind(uuid_str(store_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(reorder_point_from_row).collect()
}
