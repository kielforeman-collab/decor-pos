//! Vendor catalog CSV import.
//!
//! Maps vendor catalog rows (`vendor_sku`, `description`, `cost`,
//! `list_price`, `uom`, `barcode`) into [`CatalogItem`]s. Validation
//! errors are collected per bad row — one bad row never fails the
//! whole file.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::csv::{field_by_name, parse_csv};

/// One validated vendor catalog row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogItem {
    /// The vendor's SKU / part number.
    pub vendor_sku: String,
    pub description: String,
    /// Vendor cost (Decimal — no floats).
    pub cost: Decimal,
    /// Vendor list price (Decimal — no floats).
    pub list_price: Decimal,
    /// Unit of measure, e.g. `EA`, `GAL`.
    pub uom: String,
    /// Barcode, when the vendor supplies one.
    pub barcode: Option<String>,
}

/// One rejected row, with its 1-based data-row number (header is row 1).
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogRowError {
    pub row: usize,
    pub message: String,
}

/// The result of a catalog import: the valid items plus every bad row.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogImport {
    pub items: Vec<CatalogItem>,
    pub errors: Vec<CatalogRowError>,
}

/// Import a vendor catalog CSV.
///
/// Expected header (case-insensitive):
/// `vendor_sku,description,cost,list_price,uom,barcode`
///
/// Row rules: `vendor_sku`, `description` and `uom` must be non-empty;
/// `cost` and `list_price` must parse as decimals and be >= 0;
/// `barcode` is optional (empty means `None`). A wrong header is a
/// single row-1 error with no items.
pub fn import_vendor_catalog_csv(text: &str) -> CatalogImport {
    let rows = match parse_csv(text) {
        Ok(rows) => rows,
        Err(e) => {
            return CatalogImport {
                items: Vec::new(),
                errors: vec![CatalogRowError {
                    row: 1,
                    message: format!("CSV parse error: {e}"),
                }],
            };
        }
    };
    if rows.is_empty() {
        return CatalogImport {
            items: Vec::new(),
            errors: vec![CatalogRowError {
                row: 1,
                message: "catalog file is empty".to_string(),
            }],
        };
    }
    let header = &rows[0];
    for required in [
        "vendor_sku",
        "description",
        "cost",
        "list_price",
        "uom",
        "barcode",
    ] {
        if crate::csv::header_index(header, required).is_none() {
            return CatalogImport {
                items: Vec::new(),
                errors: vec![CatalogRowError {
                    row: 1,
                    message: format!("catalog header is missing column {required:?}"),
                }],
            };
        }
    }
    let mut items = Vec::new();
    let mut errors = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(1) {
        let row_no = index + 1;
        let get = |name: &str| field_by_name(header, row, name).unwrap_or("").trim();
        let vendor_sku = get("vendor_sku");
        let description = get("description");
        let uom = get("uom");
        let barcode = get("barcode");
        let fail = |message: String| CatalogRowError {
            row: row_no,
            message,
        };
        if vendor_sku.is_empty() {
            errors.push(fail("vendor_sku is empty".to_string()));
            continue;
        }
        if description.is_empty() {
            errors.push(fail(format!("{vendor_sku}: description is empty")));
            continue;
        }
        if uom.is_empty() {
            errors.push(fail(format!("{vendor_sku}: uom is empty")));
            continue;
        }
        let cost = match Decimal::from_str(get("cost")) {
            Ok(value) if value >= Decimal::ZERO => value,
            Ok(_) => {
                errors.push(fail(format!("{vendor_sku}: cost must not be negative")));
                continue;
            }
            Err(_) => {
                errors.push(fail(format!(
                    "{vendor_sku}: cost {:?} is not a decimal",
                    get("cost")
                )));
                continue;
            }
        };
        let list_price = match Decimal::from_str(get("list_price")) {
            Ok(value) if value >= Decimal::ZERO => value,
            Ok(_) => {
                errors.push(fail(format!(
                    "{vendor_sku}: list_price must not be negative"
                )));
                continue;
            }
            Err(_) => {
                errors.push(fail(format!(
                    "{vendor_sku}: list_price {:?} is not a decimal",
                    get("list_price")
                )));
                continue;
            }
        };
        items.push(CatalogItem {
            vendor_sku: vendor_sku.to_string(),
            description: description.to_string(),
            cost,
            list_price,
            uom: uom.to_string(),
            barcode: if barcode.is_empty() {
                None
            } else {
                Some(barcode.to_string())
            },
        });
    }
    CatalogImport { items, errors }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small sample vendor catalog CSV, used by the tests (and as the
    /// documented format for vendor drops).
    pub const SAMPLE_CATALOG_CSV: &str = "vendor_sku,description,cost,list_price,uom,barcode\n\
         PPG-1001,Interior Eggshell Gallon,24.5000,49.9900,GAL,012345678901\n\
         PPG-1002,Interior Flat Quart,9.2500,18.4900,QT,\n\
         BAD-ROW,Missing cost row,not-a-number,10.0000,EA,\n\
         ,No SKU row,1.0000,2.0000,EA,\n\
         PPG-1003,\"Primer, Stain Blocking\",15.0000,29.9900,GAL,098765432109\n";

    #[test]
    fn imports_valid_rows_and_collects_bad_rows() {
        let result = import_vendor_catalog_csv(SAMPLE_CATALOG_CSV);
        assert_eq!(result.items.len(), 3);
        assert_eq!(result.items[0].vendor_sku, "PPG-1001");
        assert_eq!(result.items[0].cost, Decimal::from_str("24.5000").unwrap());
        assert_eq!(result.items[0].barcode.as_deref(), Some("012345678901"));
        assert_eq!(result.items[1].barcode, None);
        assert_eq!(result.items[2].description, "Primer, Stain Blocking");
        assert_eq!(result.errors.len(), 2);
        assert_eq!(result.errors[0].row, 4); // BAD-ROW: bad cost
        assert!(result.errors[0].message.contains("BAD-ROW"));
        assert_eq!(result.errors[1].row, 5); // empty SKU
    }

    #[test]
    fn wrong_header_is_a_single_error() {
        let result = import_vendor_catalog_csv("sku,desc\nX,Thing\n");
        assert!(result.items.is_empty());
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].row, 1);
    }

    #[test]
    fn negative_money_is_rejected_per_row() {
        let result = import_vendor_catalog_csv(
            "vendor_sku,description,cost,list_price,uom,barcode\nNEG-1,Thing,-1.0000,2.0000,EA,\nOK-1,Thing,1.0000,2.0000,EA,\n",
        );
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].vendor_sku, "OK-1");
        assert_eq!(result.errors.len(), 1);
    }
}
