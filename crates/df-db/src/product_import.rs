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
//! | MSRP / MAP       | `msrp` / `map` (migration `0014_counter_ui`) |
//! | Size             | `container_volume_ml` (see [`container_volume_ml_for_size`]) |
//! | Line `00229` / `COLORANT` category | also a `colorants` row (`Gennex`) |
//! | Category / Line / Color / Product UPC / French Description / Brand Discount… | **not stored** — `products` has no columns for them (see [`ImportProductRow`]) |
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
use crate::repos::{
    ImportedProductUpdate, get_product_by_sku, insert_product, update_imported_product,
};

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
    pub msrp: Option<Decimal>,
    pub map: Option<Decimal>,
    /// Container volume in ml, derived from the Size column (see
    /// [`container_volume_ml_for_size`]); `None` when the size is
    /// unknown / not a volume (kits, eaches, packaging).
    pub container_volume_ml: Option<i32>,
    pub line: String,
    pub category: String,
    pub color: String,
    pub size: String,
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
/// on most rows) but must parse when present — and are now stored
/// (migration `0014_counter_ui`), not just validated.
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
    let msrp = parse_money(&row.msrp, "msrp")?;
    let map = parse_money(&row.map, "map")?;
    Ok(ValidatedImportRow {
        row: row.row,
        sku,
        name,
        list_price,
        net_price,
        brand_code: row.brand_code.trim().to_string(),
        msrp,
        map,
        container_volume_ml: container_volume_ml_for_size(&row.size),
        line: row.line.trim().to_string(),
        category: row.category.trim().to_string(),
        color: row.color.trim().to_string(),
        size: row.size.trim().to_string(),
    })
}

/// Container volume (ml) for a price-list Size value.
///
/// The Benjamin Moore price book's Size column carries short codes,
/// verified against the 10005510 book effective July 17, 2026:
/// `001` is the gallon (the `-001` colorant SKU's NET, $157.41, is
/// 4x the `-004` quart's $39.34; a gallon paint SKU's NET matches
/// its gallon price), `004` is the quart, `005` is the 5-gallon
/// pail (a `-005` paint NET is ~5x its gallon NET), and `006` is
/// the colour-sample pot ("COLOUR SAMPLE", ~$3). The word forms
/// (`GAL`, `QT`, `PAIL`, `SAMPLE`) are accepted too, for rows the
/// back office (or another vendor file) sends that way. Anything
/// else — kits (`012`, `022`), eaches (`000`), packaging (`014`),
/// the malformed shifted row's `K7931X-001` — is not a volume and
/// yields `None`.
pub fn container_volume_ml_for_size(size: &str) -> Option<i32> {
    let normalized = size.trim().to_uppercase();
    let normalized = normalized.trim_start_matches("1 ").trim_end_matches('S');
    match normalized {
        "001" | "GAL" | "GALLON" => Some(3785),
        "004" | "QT" | "QUART" => Some(946),
        "005" | "PAIL" | "5-GAL" | "5GAL" | "5 GAL" => Some(18927),
        "006" | "SAMPLE" | "SAMPLE POT" => Some(237),
        _ => None,
    }
}

/// Container volume in US fluid ounces for a Size value, for
/// colorant costing (`cost_per_fl_oz = NET / fl oz`). Same code
/// table as [`container_volume_ml_for_size`]: gallon 128, quart 32,
/// 5-gal pail 640, sample pot 8. `None` when the size has no
/// volume (the colorant is then skipped, not guessed).
pub fn container_fl_oz_for_size(size: &str) -> Option<Decimal> {
    match container_volume_ml_for_size(size) {
        Some(3785) => Some(Decimal::from(128)),
        Some(946) => Some(Decimal::from(32)),
        Some(18927) => Some(Decimal::from(640)),
        Some(237) => Some(Decimal::from(8)),
        _ => None,
    }
}

/// Whether an import row is a Gennex colorant: Line `00229` only.
///
/// The brief says "Line 00229 (or Category COLORANT)", but the
/// real price book's `COLORANT` category spans three lines —
/// `00229` Gennex (16 rows), `00233` (12 rows) and `0TL56`
/// ColorTech (12 rows) — and only the 00229 SKUs have the `0229…`
/// shape the code derivation strips. Upserting the other two
/// lines as `system_name "Gennex"` would mislabel 24 rows from
/// other systems and break the "real 16" outcome, so the category
/// is not a fallback: a row is Gennex iff its Line is 00229.
/// (The 00233 / 0TL56 rows still import as products; they just
/// get no colorant row until their own systems are modeled.)
pub fn is_colorant_row(line: &str, _category: &str) -> bool {
    line.trim() == "00229"
}

/// The Gennex colorant code for a colorant SKU: the part of the SKU
/// after the `0229` line prefix and before the `-` size suffix —
/// `0229S1-004` → `S1`, `0229B1-004` → `B1`. Falls back to the
/// row's Color column (which carries the same code, e.g. `S1`)
/// when the SKU does not have the `0229…-…` shape.
pub fn gennex_colorant_code(sku: &str, color: &str) -> Option<String> {
    let sku = sku.trim();
    if let Some(rest) = sku.strip_prefix("0229") {
        let code = rest.split('-').next().unwrap_or("").trim();
        if !code.is_empty() {
            return Some(code.to_string());
        }
    }
    let color = color.trim();
    if color.is_empty() {
        None
    } else {
        Some(color.to_string())
    }
}

/// Upsert every row into `products` for `org_id`. Returns per-row
/// counts plus a failure entry (row, SKU, reason) for each row that
/// could not be imported — validation failures, a duplicate SKU
/// inside this one file, or a database error on that row. A second
/// import of the same file updates prices instead of duplicating.
///
/// Gennex colorants (Line `00229` — see [`is_colorant_row`] for
/// why the `COLORANT` category is not a fallback) are also
/// upserted into `colorants` (`system_name "Gennex"`, code from the
/// SKU, `cost_per_fl_oz = NET / container fl oz`, 48 shots/fl oz).
/// A colorant failure never fails its product row; the product
/// import is authoritative.
pub async fn import_products(
    pool: &AnyPool,
    org_id: Uuid,
    _store_id: Option<Uuid>,
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

        // A colorant row is only catalogued when its product row
        // imported: a failed product row catalogues nothing.
        let mut product_ok = false;
        match get_product_by_sku(pool, org_id, &validated.sku).await {
            Ok(_) => {
                let manufacturer_code = if validated.brand_code.is_empty() {
                    None
                } else {
                    Some(validated.brand_code.clone())
                };
                match update_imported_product(
                    pool,
                    ImportedProductUpdate {
                        org_id,
                        sku: &validated.sku,
                        name: &validated.name,
                        manufacturer_code: manufacturer_code.as_deref(),
                        list_price: validated.list_price,
                        standard_cost: validated.net_price,
                        msrp: validated.msrp,
                        map: validated.map,
                        container_volume_ml: validated.container_volume_ml,
                    },
                )
                .await
                {
                    Ok(()) => {
                        updated += 1;
                        product_ok = true;
                    }
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
                    msrp: validated.msrp,
                    map: validated.map,
                    min_margin_pct: None,
                    sheen: None,
                    paint_base_code: None,
                    container_volume_ml: validated.container_volume_ml,
                    eco_fee_category: None,
                    tax_class_id: Uuid::now_v7(),
                    prompt_template_id: None,
                    is_active: true,
                };
                match insert_product(pool, &product).await {
                    Ok(()) => {
                        created += 1;
                        product_ok = true;
                    }
                    Err(e) => failures.push(fail(format!("database error: {e}"))),
                }
            }
            Err(e) => failures.push(fail(format!("database error: {e}"))),
        }

        // Gennex colorant rows double as colorant catalog rows.
        // Best-effort: a colorant problem is not a product problem.
        if product_ok && is_colorant_row(&validated.line, &validated.category) {
            let _ = upsert_gennex_colorant(pool, org_id, &validated).await;
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

/// Upsert one Gennex colorant from a validated import row:
/// `cost_per_fl_oz = NET / container fl oz` (quart 32, gallon 128
/// — see [`container_fl_oz_for_size`]), 48 shots per fl oz. A code
/// sold in both sizes (e.g. S1 as `0229S1-001` and `0229S1-004`)
/// keeps one colorant row: the gallon and quart book prices derive
/// ~$1.23/fl oz each, and the last row in file order wins.
async fn upsert_gennex_colorant(
    pool: &AnyPool,
    org_id: Uuid,
    validated: &ValidatedImportRow,
) -> Result<(), DbError> {
    use crate::models::Colorant;
    use crate::{get_colorant_by_code, insert_colorant, update_colorant};
    let Some(code) = gennex_colorant_code(&validated.sku, &validated.color) else {
        return Ok(());
    };
    let Some(fl_oz) = container_fl_oz_for_size(&validated.size) else {
        return Ok(());
    };
    if fl_oz <= Decimal::ZERO {
        return Ok(());
    }
    let cost_per_fl_oz = validated.net_price / fl_oz;
    match get_colorant_by_code(pool, org_id, "Gennex", &code).await {
        Ok(existing) => {
            let mut updated = existing.clone();
            updated.name = validated.name.clone();
            updated.cost_per_fl_oz = cost_per_fl_oz;
            updated.shots_per_fl_oz = 48;
            update_colorant(pool, &updated).await?;
        }
        Err(DbError::NotFound(_)) => {
            insert_colorant(
                pool,
                &Colorant {
                    id: Uuid::now_v7(),
                    org_id,
                    system_name: "Gennex".to_string(),
                    code,
                    name: validated.name.clone(),
                    cost_per_fl_oz,
                    shots_per_fl_oz: 48,
                },
            )
            .await?;
        }
        Err(e) => return Err(e),
    }
    Ok(())
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
