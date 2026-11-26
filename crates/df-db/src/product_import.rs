//! Bulk product import from a vendor price list (GUI product import,
//! v1.2.1 follow-up; spec §6.1).
//!
//! The back office parses the spreadsheet in the browser and POSTs the
//! rows as JSON; this module validates each row and upserts it into
//! `products` by `(org_id, sku)`:
//!
//! * a SKU that does not exist yet is **created** (list price from the
//!   file's List Price, standard cost from its NET Price);
//! * a SKU that already exists is **updated** — name and prices only,
//!   never duplicated, so re-importing the same file is safe;
//! * one bad row never fails the batch: it is collected in
//!   [`ProductImportResult::failures`] with its spreadsheet row number
//!   and a named reason.
//!
//! Column mapping (Benjamin Moore price-list layout; the browser
//! sends the columns under these names):
//!
//! | File column      | Stored as                                  |
//! |------------------|--------------------------------------------|
//! | SKU              | `sku` (lookup key)                         |
//! | Product Name     | `name`                                     |
//! | List Price       | `list_price`                               |
//! | NET Price        | `standard_cost` (and `avg_cost` on create)|
//! | Brand Code       | `manufacturer_code`                        |
//! | Category / Line / Color / Size / Product UPC / MSRP / MAP / French Description / Brand Discount… | **not stored** — `products` has no columns for them (see [`ImportProductRow`]) |
//!
//! Defaults for the columns the file cannot supply follow the
//! existing seed pattern (`Edge::seed_product`): `product_type`
//! `"paint_base"`, `base_uom` `"gal"`, fresh `Uuid::now_v7()` values
//! for `department_id` / `tax_class_id` (no departments or tax-class
//! tables exist — the columns are free-form UUIDs with no FK).
//! Prices are `rust_decimal::Decimal` — never floats.

use std::collections::HashMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

use crate::AnyPool;
use crate::error::DbError;
use crate::models::Product;
use crate::repos::{get_product_by_sku, insert_product, update_imported_product};

/// One parsed spreadsheet row, as posted by the back office. Every
/// field is a string (prices arrive like `"$39.34"`); `row` is the
/// 1-based spreadsheet row number (header is row 1) so failures point
/// at the exact line in the file. Fields the `products` table has no
/// column for are accepted and ignored — see the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportProductRow {
    pub row: usize,
    #[serde(default)]
    pub sku: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub line: String,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub size: String,
    #[serde(default)]
    pub upc: String,
    #[serde(default)]
    pub msrp: String,
    #[serde(default)]
    pub map: String,
    #[serde(default)]
    pub net_price: String,
    #[serde(default)]
    pub list_price: String,
    #[serde(default)]
    pub brand_code: String,
}

/// A row that passed validation, with prices parsed to [`Decimal`].
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedImportRow {
    pub row: usize,
    pub sku: String,
    pub name: String,
    pub list_price: Decimal,
    pub net_price: Decimal,
    pub brand_code: String,
}

/// One row that could not be imported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductImportFailure {
    pub row: usize,
    pub sku: String,
    pub reason: String,
}

/// Outcome of [`import_products`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductImportResult {
    pub created: usize,
    pub updated: usize,
    pub failed: usize,
    pub failures: Vec<ProductImportFailure>,
}

/// Parse a money string from the price list: `"$39.34"`,
/// `"1,234.56"`, `"39.34"` → `Decimal`. Empty / blank → `None`
/// (no value supplied). Anything else — including a shifted row
/// whose price cell holds a SKU like `"K7931X-001"` — is an error.
/// Negative values are rejected.
pub fn parse_money(value: &str, what: &str) -> Result<Option<Decimal>, String> {
    let cleaned: String = value
        .trim()
        .chars()
        .filter(|c| *c != '$' && *c != ',')
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return Ok(None);
    }
    let parsed =
        Decimal::from_str(cleaned).map_err(|_| format!("{what} {value:?} is not a valid price"))?;
    if parsed < Decimal::ZERO {
        return Err(format!("{what} {value:?} must not be negative"));
    }
    Ok(Some(parsed))
}

/// Validate one posted row. SKU and name are required; List Price
/// and NET Price are required and must parse as decimals >= 0.
/// MSRP / MAP are optional (the real price list leaves MSRP blank
/// on most rows) but must parse when present.
pub fn validate_import_row(row: &ImportProductRow) -> Result<ValidatedImportRow, String> {
    let sku = row.sku.trim().to_string();
    if sku.is_empty() {
        return Err("SKU is empty".to_string());
    }
    let name = row.name.trim().to_string();
    if name.is_empty() {
        return Err(format!("{sku}: Product Name is empty"));
    }
    let list_price = parse_money(&row.list_price, "list_price")?
        .ok_or_else(|| format!("{sku}: List Price is missing"))?;
    let net_price = parse_money(&row.net_price, "net_price")?
        .ok_or_else(|| format!("{sku}: NET Price is missing"))?;
    // Parsed for validation only — MSRP / MAP are not stored (no
    // columns); a garbage value here is still a bad row.
    parse_money(&row.msrp, "msrp")?;
    parse_money(&row.map, "map")?;
    Ok(ValidatedImportRow {
        row: row.row,
        sku,
        name,
        list_price,
        net_price,
        brand_code: row.brand_code.trim().to_string(),
    })
}

/// Upsert every row into `products` for `org_id`. Returns per-row
/// counts plus a failure entry (row, SKU, reason) for each row that
/// could not be imported — validation failures, a duplicate SKU
/// inside this one file, or a database error on that row. A second
/// import of the same file updates prices instead of duplicating.
pub async fn import_products(
    pool: &AnyPool,
    org_id: Uuid,
    rows: &[ImportProductRow],
) -> Result<ProductImportResult, DbError> {
    let mut created = 0usize;
    let mut updated = 0usize;
    let mut failures: Vec<ProductImportFailure> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();

    for row in rows {
        let fail = |reason: String| ProductImportFailure {
            row: row.row,
            sku: row.sku.trim().to_string(),
            reason,
        };
        let validated = match validate_import_row(row) {
            Ok(v) => v,
            Err(reason) => {
                failures.push(fail(reason));
                continue;
            }
        };
        if let Some(first_row) = seen.get(&validated.sku) {
            failures.push(fail(format!(
                "duplicate SKU in this import (first seen at row {first_row})"
            )));
            continue;
        }
        seen.insert(validated.sku.clone(), validated.row);

        match get_product_by_sku(pool, org_id, &validated.sku).await {
            Ok(_) => {
                let manufacturer_code = if validated.brand_code.is_empty() {
                    None
                } else {
                    Some(validated.brand_code.clone())
                };
                match update_imported_product(
                    pool,
                    org_id,
                    &validated.sku,
                    &validated.name,
                    manufacturer_code.as_deref(),
                    validated.list_price,
                    validated.net_price,
                )
                .await
                {
                    Ok(()) => updated += 1,
                    Err(e) => failures.push(fail(format!("database error: {e}"))),
                }
            }
            Err(DbError::NotFound(_)) => {
                // Seed defaults: the price list carries no
                // department, tax class, product type or UOM.
                let product = Product {
                    id: Uuid::now_v7(),
                    org_id,
                    sku: validated.sku.clone(),
                    manufacturer_code: if validated.brand_code.is_empty() {
                        None
                    } else {
                        Some(validated.brand_code.clone())
                    },
                    name: validated.name.clone(),
                    department_id: Uuid::now_v7(),
                    class_id: None,
                    product_line_id: None,
                    primary_vendor_id: None,
                    product_type: "paint_base".to_string(),
                    base_uom: "gal".to_string(),
                    list_price: validated.list_price,
                    standard_cost: validated.net_price,
                    avg_cost: validated.net_price,
                    min_margin_pct: None,
                    sheen: None,
                    paint_base_code: None,
                    container_volume_ml: None,
                    eco_fee_category: None,
                    tax_class_id: Uuid::now_v7(),
                    prompt_template_id: None,
                    is_active: true,
                };
                match insert_product(pool, &product).await {
                    Ok(()) => created += 1,
                    Err(e) => failures.push(fail(format!("database error: {e}"))),
                }
            }
            Err(e) => failures.push(fail(format!("database error: {e}"))),
        }
    }

    let failed = failures.len();
    Ok(ProductImportResult {
        created,
        updated,
        failed,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sku: &str, name: &str, net: &str, list: &str) -> ImportProductRow {
        ImportProductRow {
            row: 2,
            sku: sku.to_string(),
            name: name.to_string(),
            category: String::new(),
            line: String::new(),
            color: String::new(),
            size: String::new(),
            upc: String::new(),
            msrp: String::new(),
            map: String::new(),
            net_price: net.to_string(),
            list_price: list.to_string(),
            brand_code: String::new(),
        }
    }

    #[test]
    fn parse_money_strips_dollar_and_commas() {
        assert_eq!(
            parse_money("$39.34", "net_price").unwrap(),
            Some(Decimal::from_str("39.34").unwrap())
        );
        assert_eq!(
            parse_money("1,234.56", "list_price").unwrap(),
            Some(Decimal::from_str("1234.56").unwrap())
        );
        assert_eq!(
            parse_money("  $0.00 ", "net_price").unwrap(),
            Some(Decimal::ZERO)
        );
        assert_eq!(parse_money("", "msrp").unwrap(), None);
        assert_eq!(parse_money("   ", "msrp").unwrap(), None);
    }

    #[test]
    fn parse_money_rejects_garbage_and_negatives() {
        // The malformed shifted row: every cell holds a SKU.
        assert!(parse_money("K7931X-001", "net_price").is_err());
        assert!(parse_money("-5.00", "list_price").is_err());
        assert!(parse_money("abc", "net_price").is_err());
    }

    #[test]
    fn validate_ok_row() {
        let v = validate_import_row(&row(
            "0229B1-004",
            "GENNEX WB CLRNT -THALO BLUE",
            "$39.34",
            "$39.64",
        ))
        .unwrap();
        assert_eq!(v.sku, "0229B1-004");
        assert_eq!(v.list_price, Decimal::from_str("39.64").unwrap());
        assert_eq!(v.net_price, Decimal::from_str("39.34").unwrap());
    }

    #[test]
    fn validate_missing_sku_and_name() {
        assert!(validate_import_row(&row("", "Name", "$1.00", "$2.00")).is_err());
        assert!(validate_import_row(&row("SKU-1", "", "$1.00", "$2.00")).is_err());
        assert!(validate_import_row(&row("   ", "Name", "$1.00", "$2.00")).is_err());
    }

    #[test]
    fn validate_missing_and_bad_prices() {
        assert!(validate_import_row(&row("SKU-1", "Name", "", "$2.00")).is_err());
        assert!(validate_import_row(&row("SKU-1", "Name", "$1.00", "")).is_err());
        assert!(validate_import_row(&row("SKU-1", "Name", "K7931X-001", "K7931X-001")).is_err());
        assert!(validate_import_row(&row("SKU-1", "Name", "$1.00", "-$2.00")).is_err());
    }

    #[test]
    fn validate_malformed_shifted_row_is_rejected_with_reason() {
        let mut bad = row("K7931X-001", "K7931X-001", "K7931X-001", "K7931X-001");
        bad.row = 1012;
        let err = validate_import_row(&bad).unwrap_err();
        assert!(
            err.contains("K7931X-001"),
            "reason names the bad value: {err}"
        );
        assert!(
            err.contains("price"),
            "reason says it is a price problem: {err}"
        );
    }

    #[test]
    fn validate_optional_msrp_map_may_be_blank_but_not_garbage() {
        let mut r = row("SKU-1", "Name", "$1.00", "$2.00");
        r.msrp = "$108.99".to_string();
        assert!(validate_import_row(&r).is_ok());
        r.msrp = "K7931X-001".to_string();
        assert!(validate_import_row(&r).is_err());
    }
}
