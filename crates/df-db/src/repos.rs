//! Repositories over [`sqlx::AnyPool`].
//!
//! One implementation serves both dialects: `?` placeholders are
//! rewritten to `$N` for PostgreSQL by sqlx's `Any` driver.
//!
//! Why strings? sqlx's `Any` driver implements `Encode`/`Decode` only
//! for primitive types — not for `Uuid`, `Decimal` or `DateTime`. So
//! ids, money and timestamps cross the driver boundary as text:
//!
//! * `Uuid` as hyphenated text (UUID in PostgreSQL, TEXT in SQLite).
//! * Money as an exact decimal string (NUMERIC(14,4) in PostgreSQL,
//!   TEXT in SQLite — spec §5: "scaled integer/text in SQLite").
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
//! dialect-shared, but no live-Postgres round-trip test runs in
//! Phase 0 (it needs a running PG 16+); that test is a Phase 1 item.
//! The SQLite path is tested end-to-end.

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::error::DbError;
use crate::models::{
    ArLedgerEntry, ColorCatalogEntry, Colorant, Customer, Organization, PricingRuleRow, Product,
    Store, TintFormula, UserRow,
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

fn bool_col(row: &AnyRow, name: &str) -> Result<bool, DbError> {
    // PostgreSQL returns a native bool; SQLite returns INTEGER 0/1
    // (reported as BIGINT), so try bool first, then integer.
    if let Ok(value) = row.try_get::<bool, _>(name) {
        return Ok(value);
    }
    let value: i64 = row
        .try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))?;
    Ok(value != 0)
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

fn user_from_row(row: &AnyRow) -> Result<UserRow, DbError> {
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

fn color_from_row(row: &AnyRow) -> Result<ColorCatalogEntry, DbError> {
    Ok(ColorCatalogEntry {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        manufacturer: text_col(row, "manufacturer")?,
        color_code: text_col(row, "color_code")?,
        color_name: text_col(row, "color_name")?,
        collection: opt_text_col(row, "collection")?,
        rgb_hex: opt_text_col(row, "rgb_hex")?,
        recommended_base: opt_text_col(row, "recommended_base")?,
    })
}

fn colorant_from_row(row: &AnyRow) -> Result<Colorant, DbError> {
    Ok(Colorant {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        system_name: text_col(row, "system_name")?,
        code: text_col(row, "code")?,
        name: text_col(row, "name")?,
        cost_per_fl_oz: dec_col(row, "cost_per_fl_oz")?,
        shots_per_fl_oz: i32_col(row, "shots_per_fl_oz")?,
    })
}

fn tint_formula_from_row(row: &AnyRow) -> Result<TintFormula, DbError> {
    Ok(TintFormula {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        customer_id: opt_uuid_col(row, "customer_id")?,
        job_id: opt_uuid_col(row, "job_id")?,
        color_id: opt_uuid_col(row, "color_id")?,
        color_code_snapshot: text_col(row, "color_code_snapshot")?,
        color_name_snapshot: text_col(row, "color_name_snapshot")?,
        base_product_id: uuid_col(row, "base_product_id")?,
        container_volume_ml: i32_col(row, "container_volume_ml")?,
        sheen: opt_text_col(row, "sheen")?,
        room_or_usage: opt_text_col(row, "room_or_usage")?,
        is_custom_match: bool_col(row, "is_custom_match")?,
        colorants_json: json_col(row, "colorants_json")?,
        total_tint_cost: dec_col(row, "total_tint_cost")?,
        tint_surcharge: dec_col(row, "tint_surcharge")?,
        dispenser_name: opt_text_col(row, "dispenser_name")?,
        external_formula_ref: opt_text_col(row, "external_formula_ref")?,
        dispensed_at: dt_col(row, "dispensed_at")?,
    })
}

fn customer_from_row(row: &AnyRow) -> Result<Customer, DbError> {
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

fn ar_entry_from_row(row: &AnyRow) -> Result<ArLedgerEntry, DbError> {
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

pub async fn insert_organization(pool: &AnyPool, org: &Organization) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO organizations (id, name, default_currency, enabled_modules, settings_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(org.id))
    .bind(&org.name)
    .bind(&org.default_currency)
    .bind(org.enabled_modules.to_string())
    .bind(org.settings_json.to_string())
    .bind(dt_str(org.created_at))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_organization(pool: &AnyPool, id: Uuid) -> Result<Organization, DbError> {
    let row = sqlx::query("SELECT * FROM organizations WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("organization {id}")))?;
    org_from_row(&row)
}

// --- stores ---------------------------------------------------------------

pub async fn insert_store(pool: &AnyPool, store: &Store) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO stores (id, org_id, code, name, country_code, region_code, postal_code, currency, tax_zone_id, timezone, is_warehouse) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    let row = sqlx::query("SELECT * FROM stores WHERE org_id = ? AND code = ?")
        .bind(uuid_str(org_id))
        .bind(code)
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("store {code}")))?;
    store_from_row(&row)
}

pub async fn get_store(pool: &AnyPool, id: Uuid) -> Result<Store, DbError> {
    let row = sqlx::query("SELECT * FROM stores WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("store {id}")))?;
    store_from_row(&row)
}

// --- users ----------------------------------------------------------------

pub async fn insert_user(pool: &AnyPool, user: &UserRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash, oidc_subject, counter_pin_hash, badge_token_hash, role_id, active) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(user.active)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_user_by_email(pool: &AnyPool, email: &str) -> Result<UserRow, DbError> {
    let row = sqlx::query("SELECT * FROM users WHERE email = ?")
        .bind(email)
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("user {email}")))?;
    user_from_row(&row)
}

// --- products -------------------------------------------------------------

pub async fn insert_product(pool: &AnyPool, product: &Product) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO products (id, org_id, sku, manufacturer_code, name, department_id, class_id, product_line_id, primary_vendor_id, product_type, base_uom, list_price, standard_cost, avg_cost, min_margin_pct, sheen, paint_base_code, container_volume_ml, eco_fee_category, tax_class_id, prompt_template_id, is_active) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

pub async fn get_product_by_sku(
    pool: &AnyPool,
    org_id: Uuid,
    sku: &str,
) -> Result<Product, DbError> {
    let row = sqlx::query("SELECT * FROM products WHERE org_id = ? AND sku = ?")
        .bind(uuid_str(org_id))
        .bind(sku)
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("product {sku}")))?;
    product_from_row(&row)
}

pub async fn get_product(pool: &AnyPool, id: Uuid) -> Result<Product, DbError> {
    let row = sqlx::query("SELECT * FROM products WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("product {id}")))?;
    product_from_row(&row)
}

// --- color catalog & colorants --------------------------------------------

pub async fn insert_color(pool: &AnyPool, color: &ColorCatalogEntry) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO color_catalog (id, org_id, manufacturer, color_code, color_name, collection, rgb_hex, recommended_base) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
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
        "SELECT * FROM color_catalog WHERE org_id = ? AND manufacturer = ? AND color_code = ?",
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
        "INSERT INTO colorants (id, org_id, system_name, code, name, cost_per_fl_oz, shots_per_fl_oz) VALUES (?, ?, ?, ?, ?, ?, ?)",
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
        "SELECT * FROM colorants WHERE org_id = ? AND system_name = ? ORDER BY code ASC",
    )
    .bind(uuid_str(org_id))
    .bind(system_name)
    .fetch_all(pool)
    .await?;
    rows.iter().map(colorant_from_row).collect()
}

// --- tint formulas ---------------------------------------------------------

pub async fn insert_tint_formula(pool: &AnyPool, formula: &TintFormula) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO tint_formulas (id, org_id, store_id, customer_id, job_id, color_id, color_code_snapshot, color_name_snapshot, base_product_id, container_volume_ml, sheen, room_or_usage, is_custom_match, colorants_json, total_tint_cost, tint_surcharge, dispenser_name, external_formula_ref, dispensed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    let rows =
        sqlx::query("SELECT * FROM tint_formulas WHERE customer_id = ? ORDER BY dispensed_at DESC")
            .bind(uuid_str(customer_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(tint_formula_from_row).collect()
}

// --- customers --------------------------------------------------------------

pub async fn insert_customer(pool: &AnyPool, customer: &Customer) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO customers (id, org_id, account_number, customer_type, company_name, first_name, last_name, email, phone, price_level_id, salesperson_id, tax_exempt_cert, tax_exempt_expires, charge_enabled, credit_limit, payment_terms, credit_hold, require_po_number, require_job_name, portal_enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    let row = sqlx::query("SELECT * FROM customers WHERE org_id = ? AND account_number = ?")
        .bind(uuid_str(org_id))
        .bind(account_number)
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("customer {account_number}")))?;
    customer_from_row(&row)
}

// --- pricing rules ----------------------------------------------------------

pub async fn insert_pricing_rule(pool: &AnyPool, rule: &PricingRuleRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO pricing_rules (id, org_id, priority, customer_id, job_id, price_level_id, product_id, product_line_id, class_id, department_id, vendor_id, min_qty, calc_method, calc_value, min_margin_floor, effective_from, effective_to) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    let rows = sqlx::query("SELECT * FROM pricing_rules WHERE org_id = ? ORDER BY priority ASC")
        .bind(uuid_str(org_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(pricing_rule_from_row).collect()
}

// --- AR ledger (append-only) ------------------------------------------------

pub async fn insert_ar_entry(pool: &AnyPool, entry: &ArLedgerEntry) -> Result<(), DbError> {
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
    let rows = sqlx::query("SELECT * FROM ar_ledger WHERE customer_id = ? ORDER BY created_at ASC")
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
