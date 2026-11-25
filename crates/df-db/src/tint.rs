//! Tint repositories (Phase 2, spec §5.3/§6.1).
//!
//! Covers the tables migration `0003_phase2` adds (`tint_jobs`,
//! `colorant_levels`) plus the lookups over the Phase 0 tint tables
//! (`color_catalog`, `colorants`, `tint_formulas`) that Phase 2 needs:
//! color search by name/code, colorant CRUD, formula lookup by id /
//! color / customer history, tint-job persistence, and the atomic
//! colorant-level decrement a dispense runs.
//!
//! Same conventions as [`crate::repos`] / [`crate::transactions`]:
//! one implementation serves both dialects over [`sqlx::AnyPool`]
//! with `?` placeholders; ids, money/volumes and timestamps cross
//! the driver as text (UUID hyphenated, `Decimal` exact strings,
//! RFC 3339); write functions are generic over any sqlx executor so
//! a caller can run them inside one transaction (`&mut *tx`) or
//! standalone (`&pool`). Reads take `&AnyPool`.
//!
//! Two dialect notes:
//!
//! * Case-insensitive substring search is written as
//!   `LOWER(col) LIKE ? ESCAPE '\'` — plain `LIKE` is
//!   case-sensitive in PostgreSQL and `ILIKE` does not exist in
//!   SQLite, so the `LOWER()` form is the portable one.
//! * Shortage filtering (`on_hand <= threshold`) is done in Rust,
//!   not SQL: volumes are TEXT in SQLite, so a SQL comparison would
//!   be lexicographic and wrong.

use chrono::Utc;
use rust_decimal::Decimal;
use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{ColorCatalogEntry, Colorant, ColorantLevelRow, TintFormula, TintJobRow};

fn opt_i32_col(row: &sqlx::any::AnyRow, name: &str) -> Result<Option<i32>, DbError> {
    use sqlx::Row;
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

fn tint_job_from_row(row: &sqlx::any::AnyRow) -> Result<TintJobRow, DbError> {
    Ok(TintJobRow {
        id: uuid_col(row, "id")?,
        formula_id: uuid_col(row, "formula_id")?,
        transaction_id: opt_uuid_col(row, "transaction_id")?,
        line_no: opt_i32_col(row, "line_no")?,
        status: text_col(row, "status")?,
        dispensed_json: json_col(row, "dispensed_json")?,
        error: opt_text_col(row, "error")?,
        created_at: dt_col(row, "created_at")?,
        completed_at: opt_dt_col(row, "completed_at")?,
    })
}

fn colorant_level_from_row(row: &sqlx::any::AnyRow) -> Result<ColorantLevelRow, DbError> {
    Ok(ColorantLevelRow {
        colorant_id: uuid_col(row, "colorant_id")?,
        store_id: uuid_col(row, "store_id")?,
        on_hand_fl_oz: dec_col(row, "on_hand_fl_oz")?,
        low_threshold_fl_oz: dec_col(row, "low_threshold_fl_oz")?,
        updated_at: dt_col(row, "updated_at")?,
    })
}

/// Escape `%`, `_` and `\` in a user query, then wrap it in `%...%`
/// for a `LIKE ... ESCAPE '\'` substring match. The caller lowercases
/// the query first (the SQL side applies `LOWER()`).
fn like_pattern(query: &str) -> String {
    let escaped: String = query
        .chars()
        .flat_map(|c| match c {
            '%' | '_' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!("%{escaped}%")
}

// --- color catalog ----------------------------------------------------------

/// One color by id.
pub async fn get_color(pool: &AnyPool, id: Uuid) -> Result<ColorCatalogEntry, DbError> {
    let row = sqlx::query("SELECT * FROM color_catalog WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("color {id}")))?;
    color_from_row(&row)
}

/// Search colors by name or code, scoped to one manufacturer
/// (`GET /colors/search`). Both the manufacturer match and the query
/// match are case-insensitive; the query is a substring of the color
/// code or the color name. An empty query returns every color for
/// that manufacturer, ordered by code.
pub async fn search_colors(
    pool: &AnyPool,
    org_id: Uuid,
    manufacturer: &str,
    query: &str,
) -> Result<Vec<ColorCatalogEntry>, DbError> {
    let pattern = like_pattern(&query.to_lowercase());
    let rows = sqlx::query(
        "SELECT * FROM color_catalog WHERE org_id = ? AND LOWER(manufacturer) = LOWER(?) AND (LOWER(color_code) LIKE ? ESCAPE '\\' OR LOWER(color_name) LIKE ? ESCAPE '\\') ORDER BY color_code ASC",
    )
    .bind(uuid_str(org_id))
    .bind(manufacturer)
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(pool)
    .await?;
    rows.iter().map(color_from_row).collect()
}

// --- colorants (CRUD) ---------------------------------------------------------

pub async fn get_colorant(pool: &AnyPool, id: Uuid) -> Result<Colorant, DbError> {
    let row = sqlx::query("SELECT * FROM colorants WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("colorant {id}")))?;
    colorant_from_row(&row)
}

pub async fn get_colorant_by_code(
    pool: &AnyPool,
    org_id: Uuid,
    system_name: &str,
    code: &str,
) -> Result<Colorant, DbError> {
    let row =
        sqlx::query("SELECT * FROM colorants WHERE org_id = ? AND system_name = ? AND code = ?")
            .bind(uuid_str(org_id))
            .bind(system_name)
            .bind(code)
            .fetch_one(pool)
            .await
            .map_err(not_found(&format!("colorant {system_name} {code}")))?;
    colorant_from_row(&row)
}

pub async fn update_colorant<'e, E>(executor: E, colorant: &Colorant) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "UPDATE colorants SET system_name = ?, code = ?, name = ?, cost_per_fl_oz = ?, shots_per_fl_oz = ? WHERE id = ?",
    )
    .bind(&colorant.system_name)
    .bind(&colorant.code)
    .bind(&colorant.name)
    .bind(dec_str(colorant.cost_per_fl_oz))
    .bind(colorant.shots_per_fl_oz)
    .bind(uuid_str(colorant.id))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn delete_colorant<'e, E>(executor: E, id: Uuid) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query("DELETE FROM colorants WHERE id = ?")
        .bind(uuid_str(id))
        .execute(executor)
        .await?;
    Ok(())
}

// --- tint formulas (lookup) ---------------------------------------------------

pub async fn get_tint_formula(pool: &AnyPool, id: Uuid) -> Result<TintFormula, DbError> {
    let row = sqlx::query("SELECT * FROM tint_formulas WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("tint formula {id}")))?;
    tint_formula_from_row(&row)
}

/// Search formulas by color code or name snapshot
/// (case-insensitive substring), newest first.
pub async fn search_tint_formulas_by_color(
    pool: &AnyPool,
    org_id: Uuid,
    query: &str,
) -> Result<Vec<TintFormula>, DbError> {
    let pattern = like_pattern(&query.to_lowercase());
    let rows = sqlx::query(
        "SELECT * FROM tint_formulas WHERE org_id = ? AND (LOWER(color_code_snapshot) LIKE ? ESCAPE '\\' OR LOWER(color_name_snapshot) LIKE ? ESCAPE '\\') ORDER BY dispensed_at DESC",
    )
    .bind(uuid_str(org_id))
    .bind(&pattern)
    .bind(&pattern)
    .fetch_all(pool)
    .await?;
    rows.iter().map(tint_formula_from_row).collect()
}

/// A customer's colour history (`GET /customers/{id}/formulas`):
/// their past formulas, newest first. With `job_id` given, only
/// formulas for that job are returned — so a repeat tint pulls the
/// same formula.
pub async fn list_tint_formulas_for_customer_job(
    pool: &AnyPool,
    customer_id: Uuid,
    job_id: Option<Uuid>,
) -> Result<Vec<TintFormula>, DbError> {
    let rows = match job_id {
        None => sqlx::query(
            "SELECT * FROM tint_formulas WHERE customer_id = ? ORDER BY dispensed_at DESC",
        )
        .bind(uuid_str(customer_id))
        .fetch_all(pool)
        .await?,
        Some(job) => sqlx::query(
            "SELECT * FROM tint_formulas WHERE customer_id = ? AND job_id = ? ORDER BY dispensed_at DESC",
        )
        .bind(uuid_str(customer_id))
        .bind(uuid_str(job))
        .fetch_all(pool)
        .await?,
    };
    rows.iter().map(tint_formula_from_row).collect()
}

// --- tint jobs ----------------------------------------------------------------

pub async fn insert_tint_job<'e, E>(executor: E, job: &TintJobRow) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO tint_jobs (id, formula_id, transaction_id, line_no, status, dispensed_json, error, created_at, completed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(job.id))
    .bind(uuid_str(job.formula_id))
    .bind(opt_uuid_str(job.transaction_id))
    .bind(job.line_no)
    .bind(&job.status)
    .bind(job.dispensed_json.to_string())
    .bind(&job.error)
    .bind(dt_str(job.created_at))
    .bind(opt_dt_str(job.completed_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_tint_job(pool: &AnyPool, id: Uuid) -> Result<TintJobRow, DbError> {
    let row = sqlx::query("SELECT * FROM tint_jobs WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("tint job {id}")))?;
    tint_job_from_row(&row)
}

/// All jobs for one formula, oldest first.
pub async fn list_tint_jobs_for_formula(
    pool: &AnyPool,
    formula_id: Uuid,
) -> Result<Vec<TintJobRow>, DbError> {
    let rows = sqlx::query("SELECT * FROM tint_jobs WHERE formula_id = ? ORDER BY created_at ASC")
        .bind(uuid_str(formula_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(tint_job_from_row).collect()
}

/// All jobs for one sale, oldest first — the trace from a tinted sale
/// line to its dispense records.
pub async fn list_tint_jobs_for_transaction(
    pool: &AnyPool,
    transaction_id: Uuid,
) -> Result<Vec<TintJobRow>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM tint_jobs WHERE transaction_id = ? ORDER BY created_at ASC")
            .bind(uuid_str(transaction_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(tint_job_from_row).collect()
}

/// All jobs for one store, oldest first (joined through the formula's
/// `store_id` — `tint_jobs` itself carries no store). Callers filter
/// by date in Rust: `created_at` is RFC 3339 text in SQLite and
/// `TIMESTAMPTZ` in PostgreSQL, so a portable SQL date comparison
/// would need dialect-specific casts.
pub async fn list_tint_jobs_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<TintJobRow>, DbError> {
    let rows = sqlx::query(
        "SELECT tj.* FROM tint_jobs tj JOIN tint_formulas f ON f.id = tj.formula_id WHERE f.store_id = ? ORDER BY tj.created_at ASC",
    )
    .bind(uuid_str(store_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(tint_job_from_row).collect()
}

/// Full-row update of a tint job (status, dispensed amounts, error,
/// completion time) — the persistence side of the `TintJob`
/// lifecycle in `df-tinter`.
pub async fn update_tint_job<'e, E>(executor: E, job: &TintJobRow) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "UPDATE tint_jobs SET formula_id = ?, transaction_id = ?, line_no = ?, status = ?, dispensed_json = ?, error = ?, created_at = ?, completed_at = ? WHERE id = ?",
    )
    .bind(uuid_str(job.formula_id))
    .bind(opt_uuid_str(job.transaction_id))
    .bind(job.line_no)
    .bind(&job.status)
    .bind(job.dispensed_json.to_string())
    .bind(&job.error)
    .bind(dt_str(job.created_at))
    .bind(opt_dt_str(job.completed_at))
    .bind(uuid_str(job.id))
    .execute(executor)
    .await?;
    Ok(())
}

// --- colorant levels ------------------------------------------------------------

/// Insert or replace the level row for a colorant at a store.
pub async fn upsert_colorant_level<'e, E>(
    executor: E,
    level: &ColorantLevelRow,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO colorant_levels (colorant_id, store_id, on_hand_fl_oz, low_threshold_fl_oz, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(colorant_id, store_id) DO UPDATE SET on_hand_fl_oz = excluded.on_hand_fl_oz, low_threshold_fl_oz = excluded.low_threshold_fl_oz, updated_at = excluded.updated_at",
    )
    .bind(uuid_str(level.colorant_id))
    .bind(uuid_str(level.store_id))
    .bind(dec_str(level.on_hand_fl_oz))
    .bind(dec_str(level.low_threshold_fl_oz))
    .bind(dt_str(level.updated_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_colorant_level(
    pool: &AnyPool,
    colorant_id: Uuid,
    store_id: Uuid,
) -> Result<Option<ColorantLevelRow>, DbError> {
    let row = sqlx::query("SELECT * FROM colorant_levels WHERE colorant_id = ? AND store_id = ?")
        .bind(uuid_str(colorant_id))
        .bind(uuid_str(store_id))
        .fetch_optional(pool)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(colorant_level_from_row(&row)?)),
    }
}

/// All level rows for a store, ordered by colorant id.
pub async fn list_colorant_levels_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<ColorantLevelRow>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM colorant_levels WHERE store_id = ? ORDER BY colorant_id ASC")
            .bind(uuid_str(store_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(colorant_level_from_row).collect()
}

/// Colorants at or below their low-level threshold at a store
/// (shortage alerts). Filtered in Rust: volumes are TEXT in SQLite,
/// so a SQL comparison would be lexicographic and wrong.
pub async fn list_low_colorant_levels(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<ColorantLevelRow>, DbError> {
    let levels = list_colorant_levels_for_store(pool, store_id).await?;
    Ok(levels
        .into_iter()
        .filter(|level| level.on_hand_fl_oz <= level.low_threshold_fl_oz)
        .collect())
}

/// Atomically decrement several colorants' on-hand levels by the
/// given amounts (fl oz), inside the caller's transaction — the
/// persistence side of one dispense, which touches several
/// colorants at once. Every level is read and validated first, then
/// every update runs, so a short colorant fails the whole dispense
/// with [`DbError::InsufficientColorant`] and the caller's
/// transaction rolls back with nothing written. A missing level row
/// is [`DbError::NotFound`]; a non-positive amount is
/// [`DbError::InvalidData`]. Callers must commit (or drop, to roll
/// back) the transaction themselves — unlike
/// [`decrement_colorant_level`], this function opens no transaction
/// of its own, so a tint-job insert can join the same transaction.
pub async fn decrement_colorant_levels_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    store_id: Uuid,
    amounts: &[(Uuid, Decimal)],
) -> Result<Vec<ColorantLevelRow>, DbError> {
    for (colorant_id, amount) in amounts {
        if *amount <= Decimal::ZERO {
            return Err(DbError::InvalidData(format!(
                "decrement amount must be positive, got {amount} for colorant {colorant_id}"
            )));
        }
    }
    let mut levels = Vec::with_capacity(amounts.len());
    for (colorant_id, amount) in amounts {
        let row =
            sqlx::query("SELECT * FROM colorant_levels WHERE colorant_id = ? AND store_id = ?")
                .bind(uuid_str(*colorant_id))
                .bind(uuid_str(store_id))
                .fetch_one(&mut **tx)
                .await
                .map_err(not_found(&format!(
                    "colorant level {colorant_id} at store {store_id}"
                )))?;
        let level = colorant_level_from_row(&row)?;
        if level.on_hand_fl_oz < *amount {
            return Err(DbError::InsufficientColorant {
                colorant_id: *colorant_id,
                store_id,
                needed: *amount,
                on_hand: level.on_hand_fl_oz,
            });
        }
        levels.push((level, *amount));
    }
    let now = Utc::now();
    let mut updated = Vec::with_capacity(levels.len());
    for (level, amount) in levels {
        let next = ColorantLevelRow {
            colorant_id: level.colorant_id,
            store_id,
            on_hand_fl_oz: level.on_hand_fl_oz - amount,
            low_threshold_fl_oz: level.low_threshold_fl_oz,
            updated_at: now,
        };
        sqlx::query(
            "UPDATE colorant_levels SET on_hand_fl_oz = ?, updated_at = ? WHERE colorant_id = ? AND store_id = ?",
        )
        .bind(dec_str(next.on_hand_fl_oz))
        .bind(dt_str(next.updated_at))
        .bind(uuid_str(level.colorant_id))
        .bind(uuid_str(store_id))
        .execute(&mut **tx)
        .await?;
        updated.push(next);
    }
    Ok(updated)
}

/// Atomically decrement a colorant's on-hand level by `amount` fl oz
/// (the persistence side of a dispense). Runs its own transaction:
/// the level is read and written inside one transaction, so two
/// concurrent dispenses cannot both spend the same fluid ounce.
/// Fails with [`DbError::NotFound`] when no level row exists and
/// [`DbError::InsufficientColorant`] when the level is short — in
/// both cases nothing is written.
pub async fn decrement_colorant_level(
    pool: &AnyPool,
    colorant_id: Uuid,
    store_id: Uuid,
    amount: Decimal,
) -> Result<ColorantLevelRow, DbError> {
    if amount <= Decimal::ZERO {
        return Err(DbError::InvalidData(format!(
            "decrement amount must be positive, got {amount}"
        )));
    }
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT * FROM colorant_levels WHERE colorant_id = ? AND store_id = ?")
        .bind(uuid_str(colorant_id))
        .bind(uuid_str(store_id))
        .fetch_one(&mut *tx)
        .await
        .map_err(not_found(&format!(
            "colorant level {colorant_id} at store {store_id}"
        )))?;
    let level = colorant_level_from_row(&row)?;
    if level.on_hand_fl_oz < amount {
        // Dropping `tx` rolls back; nothing is written.
        return Err(DbError::InsufficientColorant {
            colorant_id,
            store_id,
            needed: amount,
            on_hand: level.on_hand_fl_oz,
        });
    }
    let updated = ColorantLevelRow {
        colorant_id,
        store_id,
        on_hand_fl_oz: level.on_hand_fl_oz - amount,
        low_threshold_fl_oz: level.low_threshold_fl_oz,
        updated_at: Utc::now(),
    };
    sqlx::query(
        "UPDATE colorant_levels SET on_hand_fl_oz = ?, updated_at = ? WHERE colorant_id = ? AND store_id = ?",
    )
    .bind(dec_str(updated.on_hand_fl_oz))
    .bind(dt_str(updated.updated_at))
    .bind(uuid_str(colorant_id))
    .bind(uuid_str(store_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(updated)
}
