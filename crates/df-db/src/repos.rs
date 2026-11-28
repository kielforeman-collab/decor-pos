//! Repositories over [`sqlx::AnyPool`].
//!
//! One implementation serves both dialects with one SQL text. All
//! queries use `$N` placeholders: sqlx's `Any` driver does NOT rewrite
//! `?` — a literal `?` reaches PostgreSQL, where it is the JSONB
//! operator, so every `?` query syntax-errors on PG (proven live
//! 2026-11-26). `$N` works on both: natively on PG, and as named
//! parameters bound positionally on SQLite.
//!
//! Why strings? sqlx's `Any` driver implements `Encode`/`Decode` only
//! for primitive types — not for `Uuid`, `Decimal` or `DateTime`. So
//! ids, money and timestamps cross the driver boundary as text:
//!
//! * `Uuid` as hyphenated text (TEXT on both dialects).
//! * Money as an exact decimal string (TEXT on both dialects —
//!   spec §5: "scaled integer/text in SQLite").
//! * Timestamps as RFC 3339 text, dates as `YYYY-MM-DD` text.
//!
//! Booleans and integers cross natively. Call [`crate::connect`]
//! (which installs the drivers) before opening a pool.
//!
//! Phase 0 covers the spec §5 tables needed by the core engine.
//! Balances (AR, inventory) are derived in Rust from the append-only
//! ledger rows, never from a stored mutable balance.
//!
//! PostgreSQL note: the migrations are embedded and the SQL is
//! dialect-shared. TEXT columns on PG are deliberate (see `lib.rs`):
//! the `Any` driver cannot decode uuid / numeric / timestamptz /
//! jsonb columns and cannot bind text into them, and the CASTs that
//! would fix that are destructive on SQLite.

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Executor, Row};
use uuid::Uuid;

use crate::codec::{bool_col, color_from_row, colorant_from_row, tint_formula_from_row};
use crate::error::DbError;
use crate::models::{
    ArLedgerEntry, ColorCatalogEntry, Colorant, Customer, Organization, PriceLevel, PricingRuleRow,
    Product, Store, TintFormula, UserRow,
};

// --- text encode helpers --------------------------------------------------

fn uuid_str(id: Uuid) -> String {
    id.to_string()
}

fn opt_uuid_str(id: Option<Uuid>) -> Option<String> {
    id.map(|i| i.to_string())
}

fn dec_str(amount: Decimal) -> String {
    amount.to_string()
}

fn opt_dec_str(amount: Option<Decimal>) -> Option<String> {
    amount.map(|a| a.to_string())
}

fn dt_str(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn opt_dt_str(dt: Option<DateTime<Utc>>) -> Option<String> {
    dt.map(|d| d.to_rfc3339())
}

fn opt_date_str(date: Option<NaiveDate>) -> Option<String> {
    date.map(|d| d.to_string())
}

// --- row decoding helpers -------------------------------------------------

fn text_col(row: &AnyRow, name: &str) -> Result<String, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

fn opt_text_col(row: &AnyRow, name: &str) -> Result<Option<String>, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

fn uuid_col(row: &AnyRow, name: &str) -> Result<Uuid, DbError> {
    Uuid::parse_str(&text_col(row, name)?)
        .map_err(|e| DbError::InvalidData(format!("column {name} is not a UUID: {e}")))
}

fn opt_uuid_col(row: &AnyRow, name: &str) -> Result<Option<Uuid>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => Uuid::parse_str(&text)
            .map(Some)
            .map_err(|e| DbError::InvalidData(format!("column {name} is not a UUID: {e}"))),
    }
}

fn dec_col(row: &AnyRow, name: &str) -> Result<Decimal, DbError> {
    Decimal::from_str(&text_col(row, name)?)
        .map_err(|e| DbError::InvalidData(format!("column {name} is not a decimal: {e}")))
}

fn opt_dec_col(row: &AnyRow, name: &str) -> Result<Option<Decimal>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => Decimal::from_str(&text)
            .map(Some)
            .map_err(|e| DbError::InvalidData(format!("column {name} is not a decimal: {e}"))),
    }
}

fn dt_col(row: &AnyRow, name: &str) -> Result<DateTime<Utc>, DbError> {
    let text = text_col(row, name)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| DbError::InvalidData(format!("column {name} is not RFC 3339: {e}")))
}

fn opt_dt_col(row: &AnyRow, name: &str) -> Result<Option<DateTime<Utc>>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => DateTime::parse_from_rfc3339(&text)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|e| DbError::InvalidData(format!("column {name} is not RFC 3339: {e}"))),
    }
}

fn opt_date_col(row: &AnyRow, name: &str) -> Result<Option<NaiveDate>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .map(Some)
            .map_err(|e| DbError::InvalidData(format!("column {name} is not a date: {e}"))),
    }
}

fn i32_col(row: &AnyRow, name: &str) -> Result<i32, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

fn opt_i32_col(row: &AnyRow, name: &str) -> Result<Option<i32>, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

fn json_col(row: &AnyRow, name: &str) -> Result<serde_json::Value, DbError> {
    let text = text_col(row, name)?;
    serde_json::from_str(&text)
        .map_err(|e| DbError::InvalidData(format!("column {name} is not JSON: {e}")))
}

fn org_from_row(row: &AnyRow) -> Result<Organization, DbError> {
    Ok(Organization {
        id: uuid_col(row, "id")?,
        name: text_col(row, "name")?,
        default_currency: text_col(row, "default_currency")?,
        enabled_modules: json_col(row, "enabled_modules")?,
        settings_json: json_col(row, "settings_json")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn store_from_row(row: &AnyRow) -> Result<Store, DbError> {
    Ok(Store {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        code: text_col(row, "code")?,
        name: text_col(row, "name")?,
        country_code: text_col(row, "country_code")?,
        region_code: text_col(row, "region_code")?,
        postal_code: text_col(row, "postal_code")?,
        currency: text_col(row, "currency")?,
        tax_zone_id: uuid_col(row, "tax_zone_id")?,
        timezone: text_col(row, "timezone")?,
        is_warehouse: bool_col(row, "is_warehouse")?,
    })
}

pub(crate) fn user_from_row(row: &AnyRow) -> Result<UserRow, DbError> {
    Ok(UserRow {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        email: text_col(row, "email")?,
        display_name: text_col(row, "display_name")?,
        password_hash: opt_text_col(row, "password_hash")?,
        oidc_subject: opt_text_col(row, "oidc_subject")?,
        counter_pin_hash: opt_text_col(row, "counter_pin_hash")?,
        badge_token_hash: opt_text_col(row, "badge_token_hash")?,
        role_id: uuid_col(row, "role_id")?,
        portal_customer_id: opt_uuid_col(row, "portal_customer_id")?,
        active: bool_col(row, "active")?,
    })
}

fn product_from_row(row: &AnyRow) -> Result<Product, DbError> {
    Ok(Product {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        sku: text_col(row, "sku")?,
        manufacturer_code: opt_text_col(row, "manufacturer_code")?,
        name: text_col(row, "name")?,
        department_id: uuid_col(row, "department_id")?,
        class_id: opt_uuid_col(row, "class_id")?,
        product_line_id: opt_uuid_col(row, "product_line_id")?,
        primary_vendor_id: opt_uuid_col(row, "primary_vendor_id")?,
        product_type: text_col(row, "product_type")?,
        base_uom: text_col(row, "base_uom")?,
        list_price: dec_col(row, "list_price")?,
        standard_cost: dec_col(row, "standard_cost")?,
        avg_cost: dec_col(row, "avg_cost")?,
        msrp: opt_dec_col(row, "msrp")?,
        map: opt_dec_col(row, "map")?,
        min_margin_pct: opt_dec_col(row, "min_margin_pct")?,
        sheen: opt_text_col(row, "sheen")?,
        paint_base_code: opt_text_col(row, "paint_base_code")?,
        container_volume_ml: opt_i32_col(row, "container_volume_ml")?,
        eco_fee_category: opt_text_col(row, "eco_fee_category")?,
        tax_class_id: uuid_col(row, "tax_class_id")?,
        prompt_template_id: opt_uuid_col(row, "prompt_template_id")?,
        is_active: bool_col(row, "is_active")?,
    })
}

pub(crate) fn customer_from_row(row: &AnyRow) -> Result<Customer, DbError> {
    Ok(Customer {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        account_number: text_col(row, "account_number")?,
        customer_type: text_col(row, "customer_type")?,
        company_name: opt_text_col(row, "company_name")?,
        first_name: opt_text_col(row, "first_name")?,
        last_name: opt_text_col(row, "last_name")?,
        email: opt_text_col(row, "email")?,
        phone: opt_text_col(row, "phone")?,
        price_level_id: opt_uuid_col(row, "price_level_id")?,
        salesperson_id: opt_uuid_col(row, "salesperson_id")?,
        tax_exempt_cert: opt_text_col(row, "tax_exempt_cert")?,
        tax_exempt_expires: opt_date_col(row, "tax_exempt_expires")?,
        charge_enabled: bool_col(row, "charge_enabled")?,
        credit_limit: dec_col(row, "credit_limit")?,
        payment_terms: text_col(row, "payment_terms")?,
        terms_days: opt_i32_col(row, "terms_days")?,
        finance_rate_pct_monthly: opt_dec_col(row, "finance_rate_pct_monthly")?,
        credit_hold: bool_col(row, "credit_hold")?,
        require_po_number: bool_col(row, "require_po_number")?,
        require_job_name: bool_col(row, "require_job_name")?,
        portal_enabled: bool_col(row, "portal_enabled")?,
    })
}

fn pricing_rule_from_row(row: &AnyRow) -> Result<PricingRuleRow, DbError> {
    Ok(PricingRuleRow {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        priority: i32_col(row, "priority")?,
        customer_id: opt_uuid_col(row, "customer_id")?,
        job_id: opt_uuid_col(row, "job_id")?,
        price_level_id: opt_uuid_col(row, "price_level_id")?,
        product_id: opt_uuid_col(row, "product_id")?,
        product_line_id: opt_uuid_col(row, "product_line_id")?,
        class_id: opt_uuid_col(row, "class_id")?,
        department_id: opt_uuid_col(row, "department_id")?,
        vendor_id: opt_uuid_col(row, "vendor_id")?,
        min_qty: dec_col(row, "min_qty")?,
        calc_method: text_col(row, "calc_method")?,
        calc_value: dec_col(row, "calc_value")?,
        min_margin_floor: opt_dec_col(row, "min_margin_floor")?,
        effective_from: opt_dt_col(row, "effective_from")?,
        effective_to: opt_dt_col(row, "effective_to")?,
    })
}

pub(crate) fn ar_entry_from_row(row: &AnyRow) -> Result<ArLedgerEntry, DbError> {
    Ok(ArLedgerEntry {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        customer_id: uuid_col(row, "customer_id")?,
        job_id: opt_uuid_col(row, "job_id")?,
        entry_type: text_col(row, "entry_type")?,
        reference_type: text_col(row, "reference_type")?,
        reference_id: uuid_col(row, "reference_id")?,
        amount: dec_col(row, "amount")?,
        due_date: opt_date_col(row, "due_date")?,
        statement_id: opt_uuid_col(row, "statement_id")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn not_found(what: &str) -> impl Fn(sqlx::Error) -> DbError + '_ {
    move |e| match e {
        sqlx::Error::RowNotFound => DbError::NotFound(what.to_string()),
        other => DbError::Sqlx(other),
    }
}

// --- organizations --------------------------------------------------------

/// Insert an organization over any executor, so a caller holding
/// a transaction (e.g. first-run setup) can join the insert to it.
/// [`insert_organization`] is the pool shorthand.
pub async fn insert_organization_exec<'e, E>(executor: E, org: &Organization) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO organizations (id, name, default_currency, enabled_modules, settings_json, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(uuid_str(org.id))
    .bind(&org.name)
    .bind(&org.default_currency)
    .bind(org.enabled_modules.to_string())
    .bind(org.settings_json.to_string())
    .bind(dt_str(org.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn insert_organization(pool: &AnyPool, org: &Organization) -> Result<(), DbError> {
    insert_organization_exec(pool, org).await
}

pub async fn get_organization(pool: &AnyPool, id: Uuid) -> Result<Organization, DbError> {
    let row = sqlx::query("SELECT * FROM organizations WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("organization {id}")))?;
    org_from_row(&row)
}

// --- stores ---------------------------------------------------------------

pub async fn insert_store(pool: &AnyPool, store: &Store) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO stores (id, org_id, code, name, country_code, region_code, postal_code, currency, tax_zone_id, timezone, is_warehouse) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(uuid_str(store.id))
    .bind(uuid_str(store.org_id))
    .bind(&store.code)
    .bind(&store.name)
    .bind(&store.country_code)
    .bind(&store.region_code)
    .bind(&store.postal_code)
    .bind(&store.currency)
    .bind(uuid_str(store.tax_zone_id))
    .bind(&store.timezone)
    .bind(store.is_warehouse)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_store_by_code(pool: &AnyPool, org_id: Uuid, code: &str) -> Result<Store, DbError> {
    let row = sqlx::query("SELECT * FROM stores WHERE org_id = $1 AND code = $2")
        .bind(uuid_str(org_id))
        .bind(code)
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("store {code}")))?;
    store_from_row(&row)
}

pub async fn get_store(pool: &AnyPool, id: Uuid) -> Result<Store, DbError> {
    let row = sqlx::query("SELECT * FROM stores WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("store {id}")))?;
    store_from_row(&row)
}

/// Every store of an org, ordered by code — the raw material for
/// org-wide reports (e.g. the eco-fee remittance, which groups by
/// each store's country/region jurisdiction).
pub async fn list_stores_for_org(pool: &AnyPool, org_id: Uuid) -> Result<Vec<Store>, DbError> {
    let rows = sqlx::query("SELECT * FROM stores WHERE org_id = $1 ORDER BY code ASC")
        .bind(uuid_str(org_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(store_from_row).collect()
}

/// Update an existing store's editable configuration fields (name,
/// code, jurisdiction, postal code, currency, timezone, warehouse flag).
/// Returns [`DbError::NotFound`] if the store does not exist in `org_id`.
pub async fn update_store(pool: &AnyPool, store: &Store) -> Result<(), DbError> {
    let result = sqlx::query(
        "UPDATE stores \
         SET code = $1, name = $2, country_code = $3, region_code = $4, \
             postal_code = $5, currency = $6, timezone = $7, is_warehouse = $8 \
         WHERE id = $9 AND org_id = $10",
    )
    .bind(&store.code)
    .bind(&store.name)
    .bind(&store.country_code)
    .bind(&store.region_code)
    .bind(&store.postal_code)
    .bind(&store.currency)
    .bind(&store.timezone)
    .bind(store.is_warehouse)
    .bind(uuid_str(store.id))
    .bind(uuid_str(store.org_id))
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound(format!("store {}", store.id)));
    }
    Ok(())
}

// --- users ----------------------------------------------------------------

/// Insert a user over any executor, so a caller holding a
/// transaction (e.g. first-run setup) can join the insert to it.
/// [`insert_user`] is the pool shorthand.
pub async fn insert_user_exec<'e, E>(executor: E, user: &UserRow) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash, oidc_subject, counter_pin_hash, badge_token_hash, role_id, portal_customer_id, active) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(uuid_str(user.id))
    .bind(uuid_str(user.org_id))
    .bind(&user.email)
    .bind(&user.display_name)
    .bind(&user.password_hash)
    .bind(&user.oidc_subject)
    .bind(&user.counter_pin_hash)
    .bind(&user.badge_token_hash)
    .bind(uuid_str(user.role_id))
    .bind(opt_uuid_str(user.portal_customer_id))
    .bind(user.active)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn insert_user(pool: &AnyPool, user: &UserRow) -> Result<(), DbError> {
    insert_user_exec(pool, user).await
}

pub async fn get_user_by_email(pool: &AnyPool, email: &str) -> Result<UserRow, DbError> {
    let row = sqlx::query("SELECT * FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("user {email}")))?;
    user_from_row(&row)
}

// --- products -------------------------------------------------------------

pub async fn insert_product(pool: &AnyPool, product: &Product) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO products (id, org_id, sku, manufacturer_code, name, department_id, class_id, product_line_id, primary_vendor_id, product_type, base_uom, list_price, standard_cost, avg_cost, msrp, map, min_margin_pct, sheen, paint_base_code, container_volume_ml, eco_fee_category, tax_class_id, prompt_template_id, is_active) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24)",
    )
    .bind(uuid_str(product.id))
    .bind(uuid_str(product.org_id))
    .bind(&product.sku)
    .bind(&product.manufacturer_code)
    .bind(&product.name)
    .bind(uuid_str(product.department_id))
    .bind(opt_uuid_str(product.class_id))
    .bind(opt_uuid_str(product.product_line_id))
    .bind(opt_uuid_str(product.primary_vendor_id))
    .bind(&product.product_type)
    .bind(&product.base_uom)
    .bind(dec_str(product.list_price))
    .bind(dec_str(product.standard_cost))
    .bind(dec_str(product.avg_cost))
    .bind(opt_dec_str(product.msrp))
    .bind(opt_dec_str(product.map))
    .bind(opt_dec_str(product.min_margin_pct))
    .bind(&product.sheen)
    .bind(&product.paint_base_code)
    .bind(product.container_volume_ml)
    .bind(&product.eco_fee_category)
    .bind(uuid_str(product.tax_class_id))
    .bind(opt_uuid_str(product.prompt_template_id))
    .bind(product.is_active)
    .execute(pool)
    .await?;
    Ok(())
}

/// The fields a re-import may update on an existing product, as
/// one struct so [`update_imported_product`] stays under clippy's
/// argument limit.
pub struct ImportedProductUpdate<'a> {
    pub org_id: Uuid,
    pub sku: &'a str,
    pub name: &'a str,
    pub manufacturer_code: Option<&'a str>,
    pub list_price: Decimal,
    pub standard_cost: Decimal,
    pub msrp: Option<Decimal>,
    pub map: Option<Decimal>,
    pub container_volume_ml: Option<i32>,
}

/// Update the import-controlled fields of an existing product:
/// name, manufacturer code, prices, MSRP / MAP and container
/// volume. Department, tax class, product type and UOM are left as
/// they were — a re-import reprices, it does not reclassify. A
/// `None` MSRP / MAP / volume leaves the stored value untouched
/// (the file may leave MSRP blank on most rows); pass `Some` to
/// overwrite it.
pub async fn update_imported_product(
    pool: &AnyPool,
    update: ImportedProductUpdate<'_>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE products SET name = $1, manufacturer_code = $2, list_price = $3, standard_cost = $4, avg_cost = $5, msrp = COALESCE($6, msrp), map = COALESCE($7, map), container_volume_ml = COALESCE($8, container_volume_ml) WHERE org_id = $9 AND sku = $10",
    )
    .bind(update.name)
    .bind(update.manufacturer_code)
    .bind(dec_str(update.list_price))
    .bind(dec_str(update.standard_cost))
    .bind(dec_str(update.standard_cost))
    .bind(opt_dec_str(update.msrp))
    .bind(opt_dec_str(update.map))
    .bind(update.container_volume_ml)
    .bind(uuid_str(update.org_id))
    .bind(update.sku)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_product_by_sku(
    pool: &AnyPool,
    org_id: Uuid,
    sku: &str,
) -> Result<Product, DbError> {
    let row = sqlx::query("SELECT * FROM products WHERE org_id = $1 AND sku = $2")
        .bind(uuid_str(org_id))
        .bind(sku)
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("product {sku}")))?;
    product_from_row(&row)
}

pub async fn get_product(pool: &AnyPool, id: Uuid) -> Result<Product, DbError> {
    let row = sqlx::query("SELECT * FROM products WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("product {id}")))?;
    product_from_row(&row)
}

/// Every product of an org, ordered by SKU. The back-office
/// inventory browser builds on this (balances come from the ledger).
pub async fn list_products_for_org(pool: &AnyPool, org_id: Uuid) -> Result<Vec<Product>, DbError> {
    let rows = sqlx::query("SELECT * FROM products WHERE org_id = $1 ORDER BY sku ASC")
        .bind(uuid_str(org_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(product_from_row).collect()
}

// --- color catalog & colorants --------------------------------------------

pub async fn insert_color(pool: &AnyPool, color: &ColorCatalogEntry) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO color_catalog (id, org_id, manufacturer, color_code, color_name, collection, rgb_hex, recommended_base) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(uuid_str(color.id))
    .bind(uuid_str(color.org_id))
    .bind(&color.manufacturer)
    .bind(&color.color_code)
    .bind(&color.color_name)
    .bind(&color.collection)
    .bind(&color.rgb_hex)
    .bind(&color.recommended_base)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_color_by_code(
    pool: &AnyPool,
    org_id: Uuid,
    manufacturer: &str,
    color_code: &str,
) -> Result<ColorCatalogEntry, DbError> {
    let row = sqlx::query(
        "SELECT * FROM color_catalog WHERE org_id = $1 AND manufacturer = $2 AND color_code = $3",
    )
    .bind(uuid_str(org_id))
    .bind(manufacturer)
    .bind(color_code)
    .fetch_one(pool)
    .await
    .map_err(not_found(&format!("color {manufacturer} {color_code}")))?;
    color_from_row(&row)
}

pub async fn insert_colorant(pool: &AnyPool, colorant: &Colorant) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO colorants (id, org_id, system_name, code, name, cost_per_fl_oz, shots_per_fl_oz) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid_str(colorant.id))
    .bind(uuid_str(colorant.org_id))
    .bind(&colorant.system_name)
    .bind(&colorant.code)
    .bind(&colorant.name)
    .bind(dec_str(colorant.cost_per_fl_oz))
    .bind(colorant.shots_per_fl_oz)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_colorants_for_system(
    pool: &AnyPool,
    org_id: Uuid,
    system_name: &str,
) -> Result<Vec<Colorant>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM colorants WHERE org_id = $1 AND system_name = $2 ORDER BY code ASC",
    )
    .bind(uuid_str(org_id))
    .bind(system_name)
    .fetch_all(pool)
    .await?;
    rows.iter().map(colorant_from_row).collect()
}

/// Every colorant for an org, ordered by system then code. Reports
/// and the edge tinter use this to resolve a colorant *code* (the
/// only thing a stored formula or a dispensed shot carries) to its
/// id, cost and shots-per-ounce without knowing the system name.
/// Codes are unique per (org, system), not per org: if two systems
/// share a code, callers take the first row in this order and should
/// prefer the system-scoped lookups above when the system is known.
pub async fn list_colorants_for_org(
    pool: &AnyPool,
    org_id: Uuid,
) -> Result<Vec<Colorant>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM colorants WHERE org_id = $1 ORDER BY system_name ASC, code ASC")
            .bind(uuid_str(org_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(colorant_from_row).collect()
}

// --- tint formulas ---------------------------------------------------------

pub async fn insert_tint_formula(pool: &AnyPool, formula: &TintFormula) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO tint_formulas (id, org_id, store_id, customer_id, job_id, color_id, color_code_snapshot, color_name_snapshot, base_product_id, container_volume_ml, sheen, room_or_usage, is_custom_match, colorants_json, total_tint_cost, tint_surcharge, dispenser_name, external_formula_ref, dispensed_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)",
    )
    .bind(uuid_str(formula.id))
    .bind(uuid_str(formula.org_id))
    .bind(uuid_str(formula.store_id))
    .bind(opt_uuid_str(formula.customer_id))
    .bind(opt_uuid_str(formula.job_id))
    .bind(opt_uuid_str(formula.color_id))
    .bind(&formula.color_code_snapshot)
    .bind(&formula.color_name_snapshot)
    .bind(uuid_str(formula.base_product_id))
    .bind(formula.container_volume_ml)
    .bind(&formula.sheen)
    .bind(&formula.room_or_usage)
    .bind(formula.is_custom_match)
    .bind(formula.colorants_json.to_string())
    .bind(dec_str(formula.total_tint_cost))
    .bind(dec_str(formula.tint_surcharge))
    .bind(&formula.dispenser_name)
    .bind(&formula.external_formula_ref)
    .bind(dt_str(formula.dispensed_at))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_tint_formulas_for_customer(
    pool: &AnyPool,
    customer_id: Uuid,
) -> Result<Vec<TintFormula>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM tint_formulas WHERE customer_id = $1 ORDER BY dispensed_at DESC",
    )
    .bind(uuid_str(customer_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(tint_formula_from_row).collect()
}

/// Every stored formula for a store, newest first — the raw material
/// for formula-usage reporting (tracking only: formulas as recorded,
/// never a dispense/machine record).
pub async fn list_tint_formulas_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<TintFormula>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM tint_formulas WHERE store_id = $1 ORDER BY dispensed_at DESC")
            .bind(uuid_str(store_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(tint_formula_from_row).collect()
}

// --- customers --------------------------------------------------------------

pub async fn insert_customer(pool: &AnyPool, customer: &Customer) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO customers (id, org_id, account_number, customer_type, company_name, first_name, last_name, email, phone, price_level_id, salesperson_id, tax_exempt_cert, tax_exempt_expires, charge_enabled, credit_limit, payment_terms, terms_days, finance_rate_pct_monthly, credit_hold, require_po_number, require_job_name, portal_enabled) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)",
    )
    .bind(uuid_str(customer.id))
    .bind(uuid_str(customer.org_id))
    .bind(&customer.account_number)
    .bind(&customer.customer_type)
    .bind(&customer.company_name)
    .bind(&customer.first_name)
    .bind(&customer.last_name)
    .bind(&customer.email)
    .bind(&customer.phone)
    .bind(opt_uuid_str(customer.price_level_id))
    .bind(opt_uuid_str(customer.salesperson_id))
    .bind(&customer.tax_exempt_cert)
    .bind(opt_date_str(customer.tax_exempt_expires))
    .bind(customer.charge_enabled)
    .bind(dec_str(customer.credit_limit))
    .bind(&customer.payment_terms)
    .bind(customer.terms_days)
    .bind(opt_dec_str(customer.finance_rate_pct_monthly))
    .bind(customer.credit_hold)
    .bind(customer.require_po_number)
    .bind(customer.require_job_name)
    .bind(customer.portal_enabled)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_customer_by_account(
    pool: &AnyPool,
    org_id: Uuid,
    account_number: &str,
) -> Result<Customer, DbError> {
    let row = sqlx::query("SELECT * FROM customers WHERE org_id = $1 AND account_number = $2")
        .bind(uuid_str(org_id))
        .bind(account_number)
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("customer {account_number}")))?;
    customer_from_row(&row)
}

pub async fn list_customers_for_org(
    pool: &AnyPool,
    org_id: Uuid,
) -> Result<Vec<Customer>, DbError> {
    let rows = sqlx::query("SELECT * FROM customers WHERE org_id = $1 ORDER BY account_number ASC")
        .bind(uuid_str(org_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(customer_from_row).collect()
}

/// Replace a customer's price level (the counter's price-level
/// picker, `POST /pos/customers/{id}/price-level`). `None` clears
/// it back to list pricing.
pub async fn update_customer_price_level(
    pool: &AnyPool,
    id: Uuid,
    price_level_id: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE customers SET price_level_id = $1 WHERE id = $2")
        .bind(opt_uuid_str(price_level_id))
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- price levels -----------------------------------------------------------
/// Fixed UUIDv7 ids for the four seeded price levels
/// (`price_levels`, migration `0014_counter_ui`). Deterministic so
/// the counter UI, tests and any synced copy agree on which level
/// "Gold" is without a lookup by code. The timestamp half is
/// 2026-11-27T00:00:00Z (the day the levels were introduced);
/// the last 12 hex digits are the level number (1–4).
pub const PRICE_LEVEL_RETAIL_ID: Uuid = Uuid::from_u128(0x01a21a4d_4000_7000_8000_000000000001);
pub const PRICE_LEVEL_GOLD_ID: Uuid = Uuid::from_u128(0x01a21a4d_4000_7000_8000_000000000002);
pub const PRICE_LEVEL_WS_ID: Uuid = Uuid::from_u128(0x01a21a4d_4000_7000_8000_000000000003);
pub const PRICE_LEVEL_PLATINUM_ID: Uuid = Uuid::from_u128(0x01a21a4d_4000_7000_8000_000000000004);

fn price_level_from_row(row: &AnyRow) -> Result<PriceLevel, DbError> {
    Ok(PriceLevel {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        code: text_col(row, "code")?,
        name: text_col(row, "name")?,
        discount_pct: dec_col(row, "discount_pct")?,
    })
}

/// Insert a price level, ignoring the insert when the id already
/// exists (the seed below is idempotent).
pub async fn insert_price_level(pool: &AnyPool, level: &PriceLevel) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO price_levels (id, org_id, code, name, discount_pct) VALUES ($1, $2, $3, $4, $5) ON CONFLICT(id) DO NOTHING",
    )
    .bind(uuid_str(level.id))
    .bind(uuid_str(level.org_id))
    .bind(&level.code)
    .bind(&level.name)
    .bind(dec_str(level.discount_pct))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_price_levels(pool: &AnyPool, org_id: Uuid) -> Result<Vec<PriceLevel>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM price_levels WHERE org_id = $1 ORDER BY discount_pct ASC, code ASC",
    )
    .bind(uuid_str(org_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(price_level_from_row).collect()
}

pub async fn get_price_level(pool: &AnyPool, id: Uuid) -> Result<PriceLevel, DbError> {
    let row = sqlx::query("SELECT * FROM price_levels WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("price level {id}")))?;
    price_level_from_row(&row)
}

/// Seed the four price levels (fixed ids above) plus the matching
/// matrix rules, idempotently — safe to run on every
/// [`crate::migrate`] / edge open. `discount_pct` is a fraction
/// (Gold `0.20`); the rule stores the same discount as percent
/// points (`20`) because `df-domain::pricing::CalcMethod::PctOffList`
/// takes percent. Retail needs no rule (list price is the fallback).
/// A rule is seeded only when no rule exists yet for that level, so
/// an operator-edited rule is never overwritten.
pub async fn ensure_price_levels(pool: &AnyPool, org_id: Uuid) -> Result<(), DbError> {
    let levels = [
        (
            PRICE_LEVEL_RETAIL_ID,
            "retail",
            "Retail",
            Decimal::ZERO,
            None,
        ),
        (
            PRICE_LEVEL_GOLD_ID,
            "gold",
            "Gold",
            Decimal::from_str("0.20").unwrap(),
            Some(Decimal::from(20)),
        ),
        (
            PRICE_LEVEL_WS_ID,
            "ws",
            "WS",
            Decimal::from_str("0.25").unwrap(),
            Some(Decimal::from(25)),
        ),
        (
            PRICE_LEVEL_PLATINUM_ID,
            "platinum",
            "Platinum",
            Decimal::from_str("0.30").unwrap(),
            Some(Decimal::from(30)),
        ),
    ];
    for (id, code, name, discount_pct, rule_pct) in levels {
        insert_price_level(
            pool,
            &PriceLevel {
                id,
                org_id,
                code: code.to_string(),
                name: name.to_string(),
                discount_pct,
            },
        )
        .await?;
        if let Some(rule_pct) = rule_pct {
            let existing = list_pricing_rules(pool, org_id).await?;
            if existing.iter().any(|rule| rule.price_level_id == Some(id)) {
                continue;
            }
            insert_pricing_rule(
                pool,
                &PricingRuleRow {
                    id: Uuid::now_v7(),
                    org_id,
                    priority: 100,
                    customer_id: None,
                    job_id: None,
                    price_level_id: Some(id),
                    product_id: None,
                    product_line_id: None,
                    class_id: None,
                    department_id: None,
                    vendor_id: None,
                    min_qty: Decimal::ONE,
                    calc_method: "pct_off_list".to_string(),
                    calc_value: rule_pct,
                    min_margin_floor: None,
                    effective_from: None,
                    effective_to: None,
                },
            )
            .await?;
        }
    }
    Ok(())
}

// --- pricing rules ----------------------------------------------------------

pub async fn insert_pricing_rule(pool: &AnyPool, rule: &PricingRuleRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO pricing_rules (id, org_id, priority, customer_id, job_id, price_level_id, product_id, product_line_id, class_id, department_id, vendor_id, min_qty, calc_method, calc_value, min_margin_floor, effective_from, effective_to) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
    )
    .bind(uuid_str(rule.id))
    .bind(uuid_str(rule.org_id))
    .bind(rule.priority)
    .bind(opt_uuid_str(rule.customer_id))
    .bind(opt_uuid_str(rule.job_id))
    .bind(opt_uuid_str(rule.price_level_id))
    .bind(opt_uuid_str(rule.product_id))
    .bind(opt_uuid_str(rule.product_line_id))
    .bind(opt_uuid_str(rule.class_id))
    .bind(opt_uuid_str(rule.department_id))
    .bind(opt_uuid_str(rule.vendor_id))
    .bind(dec_str(rule.min_qty))
    .bind(&rule.calc_method)
    .bind(dec_str(rule.calc_value))
    .bind(opt_dec_str(rule.min_margin_floor))
    .bind(opt_dt_str(rule.effective_from))
    .bind(opt_dt_str(rule.effective_to))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_pricing_rules(
    pool: &AnyPool,
    org_id: Uuid,
) -> Result<Vec<PricingRuleRow>, DbError> {
    let rows = sqlx::query("SELECT * FROM pricing_rules WHERE org_id = $1 ORDER BY priority ASC")
        .bind(uuid_str(org_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(pricing_rule_from_row).collect()
}

pub async fn get_pricing_rule(pool: &AnyPool, id: Uuid) -> Result<PricingRuleRow, DbError> {
    let row = sqlx::query("SELECT * FROM pricing_rules WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("pricing rule {id}")))?;
    pricing_rule_from_row(&row)
}

/// Update a rule's calculation (method / value / margin floor) in
/// place. Dimensions, priority and validity window are not editable
/// through the bulk pricing editor — they are set at rule creation.
/// The caller validates the new calculation (including the
/// min-margin guardrail) before calling this.
pub async fn update_pricing_rule(pool: &AnyPool, rule: &PricingRuleRow) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE pricing_rules SET calc_method = $1, calc_value = $2, min_margin_floor = $3 WHERE id = $4",
    )
    .bind(&rule.calc_method)
    .bind(dec_str(rule.calc_value))
    .bind(opt_dec_str(rule.min_margin_floor))
    .bind(uuid_str(rule.id))
    .execute(pool)
    .await?;
    Ok(())
}

// --- AR ledger (append-only) ------------------------------------------------

pub async fn insert_ar_entry(pool: &AnyPool, entry: &ArLedgerEntry) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO ar_ledger (id, org_id, store_id, customer_id, job_id, entry_type, reference_type, reference_id, amount, due_date, statement_id, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
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
    .bind(opt_date_str(entry.due_date))
    .bind(opt_uuid_str(entry.statement_id))
    .bind(dt_str(entry.created_at))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_ar_entries_for_customer(
    pool: &AnyPool,
    customer_id: Uuid,
) -> Result<Vec<ArLedgerEntry>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM ar_ledger WHERE customer_id = $1 ORDER BY created_at ASC")
            .bind(uuid_str(customer_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(ar_entry_from_row).collect()
}

/// AR balance for a customer, derived by summing the append-only ledger
/// in Rust (never a stored mutable balance).
pub async fn ar_balance_for_customer(
    pool: &AnyPool,
    customer_id: Uuid,
) -> Result<Decimal, DbError> {
    let entries = list_ar_entries_for_customer(pool, customer_id).await?;
    Ok(entries
        .iter()
        .map(|e| e.amount)
        .fold(Decimal::ZERO, |acc, amount| acc + amount))
}

/// Every AR ledger entry for a store, oldest first — the raw material
/// for the aging and collections views (bucketed in Rust with
/// `df-domain::ar`, never by SQL `SUM` over text decimals).
pub async fn list_ar_entries_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<ArLedgerEntry>, DbError> {
    let rows = sqlx::query("SELECT * FROM ar_ledger WHERE store_id = $1 ORDER BY created_at ASC")
        .bind(uuid_str(store_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(ar_entry_from_row).collect()
}
