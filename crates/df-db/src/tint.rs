//! Tint repositories (formula tracking only — spec §5.3/§6.1).
//!
//! Covers the lookups and CRUD over `color_catalog`, `colorants`, and
//! `tint_formulas`: color search by name/code, colorant catalog CRUD,
//! and formula lookup by id / color / customer colour history.
//!
//! Same conventions as [`crate::repos`] / [`crate::transactions`]:
//! one implementation serves both dialects over [`sqlx::AnyPool`]
//! with `$N` placeholders; ids, money/volumes and timestamps cross
//! the driver as text (UUID hyphenated, `Decimal` exact strings,
//! RFC 3339); write functions are generic over any sqlx executor so
//! a caller can run them inside one transaction (`&mut *tx`) or
//! standalone (`&pool`). Reads take `&AnyPool`.
//!
//! Case-insensitive substring search uses `LOWER(col) LIKE $N ESCAPE '\'`
//! because plain `LIKE` is case-sensitive in PostgreSQL and `ILIKE` does
//! not exist in SQLite.

use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{ColorCatalogEntry, Colorant, TintFormula};

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
    let row = sqlx::query("SELECT * FROM color_catalog WHERE id = $1")
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
        "SELECT * FROM color_catalog WHERE org_id = $1 AND LOWER(manufacturer) = LOWER($2) AND (LOWER(color_code) LIKE $3 ESCAPE '\\' OR LOWER(color_name) LIKE $4 ESCAPE '\\') ORDER BY color_code ASC",
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
    let row = sqlx::query("SELECT * FROM colorants WHERE id = $1")
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
        sqlx::query("SELECT * FROM colorants WHERE org_id = $1 AND system_name = $2 AND code = $3")
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
        "UPDATE colorants SET system_name = $1, code = $2, name = $3, cost_per_fl_oz = $4, shots_per_fl_oz = $5 WHERE id = $6",
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
    sqlx::query("DELETE FROM colorants WHERE id = $1")
        .bind(uuid_str(id))
        .execute(executor)
        .await?;
    Ok(())
}

// --- tint formulas (lookup) ---------------------------------------------------

pub async fn get_tint_formula(pool: &AnyPool, id: Uuid) -> Result<TintFormula, DbError> {
    let row = sqlx::query("SELECT * FROM tint_formulas WHERE id = $1")
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
        "SELECT * FROM tint_formulas WHERE org_id = $1 AND (LOWER(color_code_snapshot) LIKE $2 ESCAPE '\\' OR LOWER(color_name_snapshot) LIKE $3 ESCAPE '\\') ORDER BY dispensed_at DESC",
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
            "SELECT * FROM tint_formulas WHERE customer_id = $1 ORDER BY dispensed_at DESC",
        )
        .bind(uuid_str(customer_id))
        .fetch_all(pool)
        .await?,
        Some(job) => sqlx::query(
            "SELECT * FROM tint_formulas WHERE customer_id = $1 AND job_id = $2 ORDER BY dispensed_at DESC",
        )
        .bind(uuid_str(customer_id))
        .bind(uuid_str(job))
        .fetch_all(pool)
        .await?,
    };
    rows.iter().map(tint_formula_from_row).collect()
}
