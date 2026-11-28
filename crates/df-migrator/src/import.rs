//! Legacy Decor Fusion CSV import (spec §7 Phase 6, `df-migrator`).
//!
//! Import order is fixed — customers, products, formulas,
//! AR balances, inventory — because formulas reference customers
//! and products, and AR/inventory reference customers and
//! products. Every row of every file lands in exactly one bucket
//! in the manifest: imported, skipped (duplicate, with the reason
//! named) or failed (with the reason named). A file whose kind is
//! unknown, that cannot be read, or whose CSV does not parse is
//! reported as a file-level failure — never silently ignored.
//!
//! Idempotency: re-importing the same files imports nothing new.
//! Customers deduplicate on `account_number`, products on `sku`,
//! formulas/AR/inventory on a deterministic UUID derived from
//! the row's canonical bytes (see
//! [`crate::manifest::uuid_from_seed`]), so the same row always
//! maps to the same id and the second run finds it.
//!
//! ## Legacy CSV shapes
//!
//! * `customers.csv` — `account_number,customer_type,company_name,
//!   first_name,last_name,email,phone,charge_enabled,credit_limit,
//!   payment_terms`
//! * `products.csv` — `sku,manufacturer_code,name,product_type,
//!   base_uom,list_price,standard_cost,department_code`
//! * `formulas.csv` — `customer_account_number,manufacturer,
//!   color_code,color_name,base_sku,container_volume_ml,sheen,
//!   room_or_usage,colorants,total_tint_cost` where `colorants` is
//!   `CODE:shots` pairs separated by `;` (e.g. `Y3:14;R1:2`).
//!   Formulas are **tracking records only**: the import stores
//!   the formula and its colorant list, it never drives a tint
//!   machine and never touches colorant inventory.
//! * `ar_balances.csv` — `account_number,amount,entry_type,
//!   due_date,reference`. `entry_type` is `charge` (amount as
//!   given) or `payment` (stored as `-abs(amount)`); entries are
//!   posted to `ar_ledger` with `reference_type = 'migration'`.
//! * `inventory.csv` — `sku,store_code,on_hand,unit_cost`.
//!   On-hand is posted as one `inventory_ledger` row with
//!   `reason_type = 'migration'`; an empty `store_code` means the
//!   target store. A non-empty `store_code` must match a seeded
//!   store row for the org (`get_store_by_code`) — otherwise the
//!   row fails with `unknown store code` rather than silently
//!   landing in the wrong store.
//!
//! Only the columns a file needs are required; extra columns are
//! ignored. Money is `rust_decimal::Decimal` — never floats.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, Utc};
use df_db::AnyPool;
use df_db::models::{
    ArLedgerEntry, ColorCatalogEntry, Customer, InventoryLedgerEntry, Product, TintFormula,
};
use rust_decimal::Decimal;
use serde_json::json;
use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

use crate::csv::{CsvError, parse_csv};
use crate::manifest::{
    FileManifest, ImportManifest, RowOutcome, RowStatus, row_checksum, uuid_from_seed,
};

/// Errors that abort a whole import run (as opposed to per-row /
/// per-file failures, which are recorded in the manifest).
#[derive(Debug, Error)]
pub enum MigratorError {
    #[error("database error: {0}")]
    Db(#[from] df_db::DbError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("csv error: {0}")]
    Csv(#[from] CsvError),
    #[error("directory {0} does not exist or is not a directory")]
    DirectoryNotFound(PathBuf),
}

/// Import order: customers → products → formulas → AR balances →
/// inventory. Formulas reference customers and products; AR and
/// inventory reference customers and products.
const KIND_ORDER: [&str; 5] = [
    "customers",
    "products",
    "formulas",
    "ar_balances",
    "inventory",
];

fn kind_for_file_name(file_name: &str) -> &'static str {
    let stem = Path::new(file_name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    match stem.as_str() {
        "customers" => "customers",
        "products" => "products",
        "formulas" => "formulas",
        "ar_balances" => "ar_balances",
        "inventory" => "inventory",
        _ => "unknown",
    }
}

/// Column lookup by header name (case-insensitive, trimmed).
struct Columns {
    map: HashMap<String, usize>,
}

impl Columns {
    fn from_header(header: &[String]) -> Self {
        let mut map = HashMap::new();
        for (index, name) in header.iter().enumerate() {
            map.insert(name.trim().to_ascii_lowercase(), index);
        }
        Columns { map }
    }

    fn get<'a>(&self, row: &'a [String], name: &str) -> Option<&'a str> {
        let index = *self.map.get(name)?;
        row.get(index).map(|value| value.trim())
    }

    fn required(&self, row: &[String], name: &str) -> Result<String, String> {
        match self.get(row, name) {
            Some(value) if !value.is_empty() => Ok(value.to_string()),
            _ => Err(format!("missing {name}")),
        }
    }

    fn missing_required(&self, names: &[&str]) -> Vec<String> {
        names
            .iter()
            .filter(|name| !self.map.contains_key(**name))
            .map(|name| (*name).to_string())
            .collect()
    }
}

fn parse_decimal(value: &str, column: &str) -> Result<Decimal, String> {
    Decimal::from_str(value).map_err(|_| format!("{column} is not a decimal: {value:?}"))
}

fn parse_bool(value: &str, column: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" => Ok(true),
        "false" | "0" | "no" | "n" | "" => Ok(false),
        _ => Err(format!("{column} is not true/false: {value:?}")),
    }
}

fn parse_int(value: &str, column: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .map_err(|_| format!("{column} is not an integer: {value:?}"))
}

fn imported(row_number: usize, checksum: String) -> RowOutcome {
    RowOutcome {
        row_number,
        status: RowStatus::Imported,
        reason: None,
        sha256: checksum,
    }
}

fn skipped(row_number: usize, checksum: String, reason: String) -> RowOutcome {
    RowOutcome {
        row_number,
        status: RowStatus::Skipped,
        reason: Some(reason),
        sha256: checksum,
    }
}

fn failed(row_number: usize, checksum: String, reason: String) -> RowOutcome {
    RowOutcome {
        row_number,
        status: RowStatus::Failed,
        reason: Some(reason),
        sha256: checksum,
    }
}

async fn import_customers(
    pool: &AnyPool,
    org_id: Uuid,
    rows: &[Vec<String>],
    manifest: &mut FileManifest,
) -> Result<(), MigratorError> {
    let (header, data) = match rows.split_first() {
        Some((header, data)) => (header, data),
        None => return Ok(()),
    };
    let columns = Columns::from_header(header);
    let missing = columns.missing_required(&["account_number"]);
    if !missing.is_empty() {
        manifest.file_error = Some(format!(
            "missing required column(s): {}",
            missing.join(", ")
        ));
        return Ok(());
    }
    for (index, row) in data.iter().enumerate() {
        let row_number = index + 1;
        let checksum = row_checksum(row);
        let outcome = match import_customer_row(pool, org_id, &columns, row).await {
            Ok(Some(reason)) => skipped(row_number, checksum, reason),
            Ok(None) => imported(row_number, checksum),
            Err(reason) => failed(row_number, checksum, reason),
        };
        manifest.record(outcome);
    }
    Ok(())
}

/// Returns `Ok(Some(reason))` when the row was skipped,
/// `Ok(None)` when imported, `Err(reason)` when it failed.
async fn import_customer_row(
    pool: &AnyPool,
    org_id: Uuid,
    columns: &Columns,
    row: &[String],
) -> Result<Option<String>, String> {
    let account_number = columns.required(row, "account_number")?;
    if df_db::get_customer_by_account(pool, org_id, &account_number)
        .await
        .is_ok()
    {
        return Ok(Some(format!("duplicate account_number {account_number}")));
    }
    let credit_limit = match columns.get(row, "credit_limit") {
        Some(value) if !value.is_empty() => parse_decimal(value, "credit_limit")?,
        _ => Decimal::ZERO,
    };
    let charge_enabled = match columns.get(row, "charge_enabled") {
        Some(value) => parse_bool(value, "charge_enabled")?,
        None => false,
    };
    let customer = Customer {
        id: Uuid::now_v7(),
        org_id,
        account_number,
        customer_type: columns
            .get(row, "customer_type")
            .filter(|value| !value.is_empty())
            .unwrap_or("retail")
            .to_string(),
        company_name: columns
            .get(row, "company_name")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        first_name: columns
            .get(row, "first_name")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        last_name: columns
            .get(row, "last_name")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        email: columns
            .get(row, "email")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        phone: columns
            .get(row, "phone")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        price_level_id: None,
        salesperson_id: None,
        tax_exempt_cert: None,
        tax_exempt_expires: None,
        charge_enabled,
        credit_limit,
        payment_terms: columns
            .get(row, "payment_terms")
            .filter(|value| !value.is_empty())
            .unwrap_or("NET30")
            .to_string(),
        terms_days: None,
        finance_rate_pct_monthly: None,
        credit_hold: false,
        require_po_number: false,
        require_job_name: false,
        portal_enabled: false,
        address_line1: None,
        address_line2: None,
        city: None,
        province: None,
        postal_code: None,
        notes: None,
    };
    df_db::insert_customer(pool, &customer)
        .await
        .map_err(|err| err.to_string())?;
    Ok(None)
}

async fn import_products(
    pool: &AnyPool,
    org_id: Uuid,
    rows: &[Vec<String>],
    manifest: &mut FileManifest,
) -> Result<(), MigratorError> {
    let (header, data) = match rows.split_first() {
        Some((header, data)) => (header, data),
        None => return Ok(()),
    };
    let columns = Columns::from_header(header);
    let missing = columns.missing_required(&["sku", "name"]);
    if !missing.is_empty() {
        manifest.file_error = Some(format!(
            "missing required column(s): {}",
            missing.join(", ")
        ));
        return Ok(());
    }
    for (index, row) in data.iter().enumerate() {
        let row_number = index + 1;
        let checksum = row_checksum(row);
        let outcome = match import_product_row(pool, org_id, &columns, row).await {
            Ok(Some(reason)) => skipped(row_number, checksum, reason),
            Ok(None) => imported(row_number, checksum),
            Err(reason) => failed(row_number, checksum, reason),
        };
        manifest.record(outcome);
    }
    Ok(())
}

async fn import_product_row(
    pool: &AnyPool,
    org_id: Uuid,
    columns: &Columns,
    row: &[String],
) -> Result<Option<String>, String> {
    let sku = columns.required(row, "sku")?;
    let name = columns.required(row, "name")?;
    if df_db::get_product_by_sku(pool, org_id, &sku).await.is_ok() {
        return Ok(Some(format!("duplicate sku {sku}")));
    }
    let list_price = match columns.get(row, "list_price") {
        Some(value) if !value.is_empty() => parse_decimal(value, "list_price")?,
        _ => Decimal::ZERO,
    };
    let standard_cost = match columns.get(row, "standard_cost") {
        Some(value) if !value.is_empty() => parse_decimal(value, "standard_cost")?,
        _ => Decimal::ZERO,
    };
    let department_code = columns
        .get(row, "department_code")
        .filter(|value| !value.is_empty())
        .unwrap_or("GENERAL");
    let product = Product {
        id: Uuid::now_v7(),
        org_id,
        sku,
        manufacturer_code: columns
            .get(row, "manufacturer_code")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        name,
        department_id: uuid_from_seed(&format!("department:{department_code}")),
        class_id: None,
        product_line_id: None,
        primary_vendor_id: None,
        product_type: columns
            .get(row, "product_type")
            .filter(|value| !value.is_empty())
            .unwrap_or("stock")
            .to_string(),
        base_uom: columns
            .get(row, "base_uom")
            .filter(|value| !value.is_empty())
            .unwrap_or("ea")
            .to_string(),
        list_price,
        standard_cost,
        avg_cost: standard_cost,
        msrp: match columns.get(row, "msrp").filter(|v| !v.is_empty()) {
            Some(value) => Some(parse_decimal(value, "msrp")?),
            None => None,
        },
        map: match columns.get(row, "map").filter(|v| !v.is_empty()) {
            Some(value) => Some(parse_decimal(value, "map")?),
            None => None,
        },
        min_margin_pct: None,
        sheen: None,
        paint_base_code: None,
        container_volume_ml: None,
        eco_fee_category: None,
        tax_class_id: uuid_from_seed("tax-class:default"),
        prompt_template_id: None,
        upc: None,
        category: None,
        line_code: None,
        color_name: None,
        size_code: None,
        source_metadata: None,
        is_active: true,
    };
    df_db::insert_product(pool, &product)
        .await
        .map_err(|err| err.to_string())?;
    Ok(None)
}

async fn import_formulas(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    rows: &[Vec<String>],
    manifest: &mut FileManifest,
) -> Result<(), MigratorError> {
    let (header, data) = match rows.split_first() {
        Some((header, data)) => (header, data),
        None => return Ok(()),
    };
    let columns = Columns::from_header(header);
    let missing = columns.missing_required(&["color_code", "color_name", "base_sku"]);
    if !missing.is_empty() {
        manifest.file_error = Some(format!(
            "missing required column(s): {}",
            missing.join(", ")
        ));
        return Ok(());
    }
    for (index, row) in data.iter().enumerate() {
        let row_number = index + 1;
        let checksum = row_checksum(row);
        let outcome =
            match import_formula_row(pool, org_id, store_id, &columns, row, &checksum).await {
                Ok(Some(reason)) => skipped(row_number, checksum, reason),
                Ok(None) => imported(row_number, checksum),
                Err(reason) => failed(row_number, checksum, reason),
            };
        manifest.record(outcome);
    }
    Ok(())
}

fn parse_colorants(value: &str) -> Result<serde_json::Value, String> {
    if value.trim().is_empty() {
        return Ok(json!([]));
    }
    let mut colorants = Vec::new();
    for pair in value.split(';') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (code, shots) = pair
            .split_once(':')
            .ok_or_else(|| format!("colorants is not CODE:shots pairs: {value:?}"))?;
        let shots: i32 = shots
            .trim()
            .parse()
            .map_err(|_| format!("colorants is not CODE:shots pairs: {value:?}"))?;
        colorants.push(json!({"code": code.trim(), "shots": shots}));
    }
    Ok(json!(colorants))
}

async fn import_formula_row(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    columns: &Columns,
    row: &[String],
    checksum: &str,
) -> Result<Option<String>, String> {
    let color_code = columns.required(row, "color_code")?;
    let color_name = columns.required(row, "color_name")?;
    let base_sku = columns.required(row, "base_sku")?;
    let manufacturer = columns
        .get(row, "manufacturer")
        .filter(|value| !value.is_empty())
        .unwrap_or("Custom")
        .to_string();
    let base_product = df_db::get_product_by_sku(pool, org_id, &base_sku)
        .await
        .map_err(|_| format!("unknown base sku {base_sku}"))?;
    let customer_id = match columns.get(row, "customer_account_number") {
        Some(account) if !account.is_empty() => {
            let customer = df_db::get_customer_by_account(pool, org_id, account)
                .await
                .map_err(|_| format!("unknown customer account {account}"))?;
            Some(customer.id)
        }
        _ => None,
    };
    let container_volume_ml = match columns.get(row, "container_volume_ml") {
        Some(value) if !value.is_empty() => parse_int(value, "container_volume_ml")?,
        _ => 0,
    };
    let total_tint_cost = match columns.get(row, "total_tint_cost") {
        Some(value) if !value.is_empty() => parse_decimal(value, "total_tint_cost")?,
        _ => Decimal::ZERO,
    };
    let colorants_json = parse_colorants(columns.get(row, "colorants").unwrap_or(""))?;
    // The color catalog entry is created on demand (deterministic
    // id, so a re-import finds it instead of duplicating it).
    let color_id = match df_db::get_color_by_code(pool, org_id, &manufacturer, &color_code).await {
        Ok(color) => Some(color.id),
        Err(_) => {
            let color = ColorCatalogEntry {
                id: uuid_from_seed(&format!("color:{manufacturer}:{color_code}")),
                org_id,
                manufacturer: manufacturer.clone(),
                color_code: color_code.clone(),
                color_name: color_name.clone(),
                collection: None,
                rgb_hex: None,
                recommended_base: None,
            };
            df_db::insert_color(pool, &color)
                .await
                .map_err(|err| err.to_string())?;
            Some(color.id)
        }
    };
    let id = uuid_from_seed(&format!("formula:{checksum}"));
    if df_db::get_tint_formula(pool, id).await.is_ok() {
        return Ok(Some(format!(
            "duplicate formula {manufacturer} {color_code} for base {base_sku}"
        )));
    }
    let formula = TintFormula {
        id,
        org_id,
        store_id,
        customer_id,
        job_id: None,
        color_id,
        color_code_snapshot: color_code,
        color_name_snapshot: color_name,
        base_product_id: base_product.id,
        container_volume_ml,
        sheen: columns
            .get(row, "sheen")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        room_or_usage: columns
            .get(row, "room_or_usage")
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        is_custom_match: false,
        colorants_json,
        total_tint_cost,
        tint_surcharge: Decimal::ZERO,
        dispenser_name: None,
        external_formula_ref: None,
        dispensed_at: Utc::now(),
    };
    df_db::insert_tint_formula(pool, &formula)
        .await
        .map_err(|err| err.to_string())?;
    Ok(None)
}

async fn import_ar_balances(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    rows: &[Vec<String>],
    manifest: &mut FileManifest,
) -> Result<(), MigratorError> {
    let (header, data) = match rows.split_first() {
        Some((header, data)) => (header, data),
        None => return Ok(()),
    };
    let columns = Columns::from_header(header);
    let missing = columns.missing_required(&["account_number", "amount"]);
    if !missing.is_empty() {
        manifest.file_error = Some(format!(
            "missing required column(s): {}",
            missing.join(", ")
        ));
        return Ok(());
    }
    for (index, row) in data.iter().enumerate() {
        let row_number = index + 1;
        let checksum = row_checksum(row);
        let outcome = match import_ar_row(pool, org_id, store_id, &columns, row, &checksum).await {
            Ok(Some(reason)) => skipped(row_number, checksum, reason),
            Ok(None) => imported(row_number, checksum),
            Err(reason) => failed(row_number, checksum, reason),
        };
        manifest.record(outcome);
    }
    Ok(())
}

async fn import_ar_row(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    columns: &Columns,
    row: &[String],
    checksum: &str,
) -> Result<Option<String>, String> {
    let account_number = columns.required(row, "account_number")?;
    let amount = parse_decimal(&columns.required(row, "amount")?, "amount")?;
    let entry_type = columns
        .get(row, "entry_type")
        .filter(|value| !value.is_empty())
        .unwrap_or("charge")
        .to_ascii_lowercase();
    if entry_type != "charge" && entry_type != "payment" {
        return Err(format!(
            "entry_type must be charge or payment, got {entry_type:?}"
        ));
    }
    let customer = df_db::get_customer_by_account(pool, org_id, &account_number)
        .await
        .map_err(|_| format!("unknown customer account {account_number}"))?;
    let due_date = match columns.get(row, "due_date") {
        Some(value) if !value.is_empty() => Some(
            NaiveDate::from_str(value)
                .map_err(|_| format!("due_date is not YYYY-MM-DD: {value:?}"))?,
        ),
        _ => None,
    };
    let reference = columns
        .get(row, "reference")
        .filter(|value| !value.is_empty())
        .unwrap_or("LEGACY-BALANCE")
        .to_string();
    let reference_id = uuid_from_seed(&format!("ar:{checksum}"));
    if df_db::find_ar_entry_by_reference(pool, customer.id, &entry_type, "migration", reference_id)
        .await
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Ok(Some(format!(
            "duplicate AR {entry_type} {reference} for {account_number}"
        )));
    }
    // A payment reduces the balance: store it negative. A charge
    // stores the amount as given.
    let signed_amount = if entry_type == "payment" {
        -amount.abs()
    } else {
        amount
    };
    let entry = ArLedgerEntry {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        customer_id: customer.id,
        job_id: None,
        entry_type,
        reference_type: "migration".to_string(),
        reference_id,
        amount: signed_amount,
        due_date,
        statement_id: None,
        created_at: Utc::now(),
    };
    df_db::insert_ar_entry(pool, &entry)
        .await
        .map_err(|err| err.to_string())?;
    Ok(None)
}

async fn import_inventory(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    rows: &[Vec<String>],
    manifest: &mut FileManifest,
) -> Result<(), MigratorError> {
    let (header, data) = match rows.split_first() {
        Some((header, data)) => (header, data),
        None => return Ok(()),
    };
    let columns = Columns::from_header(header);
    let missing = columns.missing_required(&["sku", "on_hand"]);
    if !missing.is_empty() {
        manifest.file_error = Some(format!(
            "missing required column(s): {}",
            missing.join(", ")
        ));
        return Ok(());
    }
    for (index, row) in data.iter().enumerate() {
        let row_number = index + 1;
        let checksum = row_checksum(row);
        let outcome =
            match import_inventory_row(pool, org_id, store_id, &columns, row, &checksum).await {
                Ok(Some(reason)) => skipped(row_number, checksum, reason),
                Ok(None) => imported(row_number, checksum),
                Err(reason) => failed(row_number, checksum, reason),
            };
        manifest.record(outcome);
    }
    Ok(())
}

async fn import_inventory_row(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    columns: &Columns,
    row: &[String],
    checksum: &str,
) -> Result<Option<String>, String> {
    let sku = columns.required(row, "sku")?;
    let on_hand = parse_decimal(&columns.required(row, "on_hand")?, "on_hand")?;
    let unit_cost = match columns.get(row, "unit_cost") {
        Some(value) if !value.is_empty() => parse_decimal(value, "unit_cost")?,
        _ => Decimal::ZERO,
    };
    let product = df_db::get_product_by_sku(pool, org_id, &sku)
        .await
        .map_err(|_| format!("unknown sku {sku}"))?;
    let target_store_id = match columns.get(row, "store_code") {
        Some(code) if !code.is_empty() => {
            let store = df_db::get_store_by_code(pool, org_id, code)
                .await
                .map_err(|_| format!("unknown store code {code}"))?;
            store.id
        }
        _ => store_id,
    };
    let reference_id = uuid_from_seed(&format!("inventory:{checksum}"));
    if df_db::find_inventory_entry_by_reference(pool, target_store_id, reference_id)
        .await
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Ok(Some(format!("duplicate inventory row for {sku}")));
    }
    let entry = InventoryLedgerEntry {
        id: Uuid::now_v7(),
        org_id,
        store_id: target_store_id,
        product_id: product.id,
        lot_number: None,
        delta_on_hand: on_hand,
        delta_committed: Decimal::ZERO,
        delta_on_order: Decimal::ZERO,
        delta_in_transit: Decimal::ZERO,
        unit_cost,
        reason_type: "migration".to_string(),
        reference_id,
        created_at: Utc::now(),
    };
    df_db::insert_inventory_delta(pool, &entry)
        .await
        .map_err(|err| err.to_string())?;
    Ok(None)
}

/// Import one CSV file. The kind is taken from the file name
/// (`customers.csv`, `products.csv`, `formulas.csv`,
/// `ar_balances.csv`, `inventory.csv`); anything else is reported
/// as a file-level failure with kind `unknown`.
pub async fn import_file(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    path: &Path,
) -> Result<FileManifest, MigratorError> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let kind = kind_for_file_name(&file_name);
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(err) => {
            let mut manifest = FileManifest::new(file_name, kind.to_string(), None);
            manifest.file_error = Some(format!("cannot read file: {err}"));
            return Ok(manifest);
        }
    };
    let file_sha256 = crate::manifest::sha256_hex(&raw);
    let mut manifest = FileManifest::new(file_name.clone(), kind.to_string(), Some(file_sha256));
    if kind == "unknown" {
        manifest.file_error = Some(format!(
            "unknown file kind {file_name:?} (expected customers.csv, products.csv, formulas.csv, ar_balances.csv or inventory.csv)"
        ));
        return Ok(manifest);
    }
    let text = match String::from_utf8(raw) {
        Ok(text) => text,
        Err(_) => {
            manifest.file_error = Some("file is not valid UTF-8".to_string());
            return Ok(manifest);
        }
    };
    let rows = match parse_csv(&text) {
        Ok(rows) => rows,
        Err(err) => {
            manifest.file_error = Some(format!("csv parse error: {err}"));
            return Ok(manifest);
        }
    };
    match kind {
        "customers" => import_customers(pool, org_id, &rows, &mut manifest).await?,
        "products" => import_products(pool, org_id, &rows, &mut manifest).await?,
        "formulas" => import_formulas(pool, org_id, store_id, &rows, &mut manifest).await?,
        "ar_balances" => import_ar_balances(pool, org_id, store_id, &rows, &mut manifest).await?,
        "inventory" => import_inventory(pool, org_id, store_id, &rows, &mut manifest).await?,
        _ => unreachable!("kind is one of the five known kinds here"),
    }
    manifest.finish();
    Ok(manifest)
}

/// Import every CSV file in `dir`, in the fixed dependency order
/// (customers → products → formulas → AR balances → inventory),
/// and return the reconciliation manifest. Unknown files are
/// reported, not skipped silently.
pub async fn import_directory(
    pool: &AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    dir: &Path,
) -> Result<ImportManifest, MigratorError> {
    if !dir.is_dir() {
        return Err(MigratorError::DirectoryNotFound(dir.to_path_buf()));
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            paths.push(path);
        }
    }
    // Known kinds first, in dependency order; unknown files last,
    // sorted by name so the run is deterministic.
    paths.sort_by_key(|path| {
        let kind = kind_for_file_name(&path.to_string_lossy());
        let order = KIND_ORDER
            .iter()
            .position(|known| *known == kind)
            .unwrap_or(KIND_ORDER.len());
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        (order, name)
    });
    let mut files = Vec::new();
    for path in &paths {
        files.push(import_file(pool, org_id, store_id, path).await?);
    }
    let manifest = ImportManifest {
        imported_at: Utc::now(),
        org_id,
        store_id,
        total_rows_read: files.iter().map(|file| file.rows_read).sum(),
        total_imported: files.iter().map(|file| file.imported).sum(),
        total_skipped: files.iter().map(|file| file.skipped).sum(),
        total_failed: files.iter().map(|file| file.failed).sum(),
        files,
    };
    Ok(manifest)
}
