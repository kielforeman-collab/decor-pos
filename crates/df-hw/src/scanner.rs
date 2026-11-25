//! HID barcode-scanner input parsing (pure — no device attached).
//!
//! Scanners behave like keyboards: they type the barcode followed by a
//! newline (often `\r\n`). [`parse_scanner_input`] trims that framing,
//! handles an optional `qty*barcode` quantity prefix typed at the
//! counter, looks the barcode up in a catalog, and applies the catalog's
//! quantity multiplier (a case barcode scans as N base units).

use crate::error::HwError;
use rust_decimal::Decimal;
use std::str::FromStr;

/// One catalog barcode entry (mirrors `product_barcodes` in spec §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarcodeEntry {
    /// The exact barcode string the scanner types.
    pub barcode: String,
    /// Unit of measure this barcode sells in, e.g. `"ea"` or `"case"`.
    pub uom_code: String,
    /// Base units per scan of this barcode: 1 for a single, 12 for a
    /// case of 12.
    pub qty_multiplier: Decimal,
}

/// Result of a successful scan, already converted to base units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    pub barcode: String,
    /// Quantity in base units: quantity prefix × catalog multiplier.
    pub qty: Decimal,
    pub uom_code: String,
}

/// Parse raw scanner input against a barcode catalog.
///
/// Trims surrounding whitespace / CR / LF, applies
/// [`parse_quantity_prefix`], then looks the barcode up. Returns
/// [`HwError::UnknownBarcode`] when the barcode is not in the catalog
/// and [`HwError::InvalidInput`] when nothing scannable remains.
pub fn parse_scanner_input(input: &str, catalog: &[BarcodeEntry]) -> Result<Scan, HwError> {
    let (qty_prefix, barcode) = parse_quantity_prefix(input);
    let barcode = barcode.trim();
    if barcode.is_empty() {
        return Err(HwError::InvalidInput("empty barcode".to_string()));
    }
    match catalog.iter().find(|entry| entry.barcode == barcode) {
        Some(entry) => Ok(Scan {
            barcode: barcode.to_string(),
            qty: qty_prefix * entry.qty_multiplier,
            uom_code: entry.uom_code.clone(),
        }),
        None => Err(HwError::UnknownBarcode(barcode.to_string())),
    }
}

/// Split an optional quantity prefix off scanner input: `3*123456` means
/// "3 of barcode 123456". Returns `(quantity, barcode)`.
///
/// * No `*` present: quantity is 1 and the whole (trimmed) input is the
///   barcode.
/// * The part before `*` is not a positive decimal: no prefix is assumed
///   and the whole input is treated as the barcode, so a barcode that
///   legitimately contains `*` still scans.
pub fn parse_quantity_prefix(input: &str) -> (Decimal, &str) {
    let trimmed = input.trim();
    match trimmed.split_once('*') {
        Some((qty_part, rest)) => match Decimal::from_str(qty_part.trim()) {
            Ok(qty) if qty > Decimal::ZERO => (qty, rest.trim()),
            _ => (Decimal::ONE, trimmed),
        },
        None => (Decimal::ONE, trimmed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Vec<BarcodeEntry> {
        vec![
            BarcodeEntry {
                barcode: "012345678905".to_string(),
                uom_code: "ea".to_string(),
                qty_multiplier: Decimal::ONE,
            },
            BarcodeEntry {
                barcode: "112345678902".to_string(),
                uom_code: "case".to_string(),
                qty_multiplier: Decimal::from(12),
            },
        ]
    }

    #[test]
    fn single_barcode_scans_as_one_base_unit() {
        let scan = parse_scanner_input("012345678905", &catalog()).unwrap();
        assert_eq!(scan.barcode, "012345678905");
        assert_eq!(scan.qty, Decimal::ONE);
        assert_eq!(scan.uom_code, "ea");
    }

    #[test]
    fn scanner_framing_whitespace_and_crlf_is_trimmed() {
        let scan = parse_scanner_input("  012345678905\r\n", &catalog()).unwrap();
        assert_eq!(scan.qty, Decimal::ONE);
        assert_eq!(scan.barcode, "012345678905");
    }

    #[test]
    fn case_barcode_applies_qty_multiplier() {
        let scan = parse_scanner_input("112345678902", &catalog()).unwrap();
        assert_eq!(scan.qty, Decimal::from(12));
        assert_eq!(scan.uom_code, "case");
    }

    #[test]
    fn quantity_prefix_multiplies_with_case_multiplier() {
        // 2 cases of 12 = 24 base units.
        let scan = parse_scanner_input("2*112345678902", &catalog()).unwrap();
        assert_eq!(scan.qty, Decimal::from(24));
        assert_eq!(scan.uom_code, "case");
    }

    #[test]
    fn quantity_prefix_on_single_barcode() {
        let scan = parse_scanner_input("3*012345678905", &catalog()).unwrap();
        assert_eq!(scan.qty, Decimal::from(3));
    }

    #[test]
    fn unknown_barcode_is_an_error() {
        assert_eq!(
            parse_scanner_input("999999999999", &catalog()),
            Err(HwError::UnknownBarcode("999999999999".to_string()))
        );
    }

    #[test]
    fn empty_input_is_invalid_not_unknown() {
        assert_eq!(
            parse_scanner_input("  \r\n", &catalog()),
            Err(HwError::InvalidInput("empty barcode".to_string()))
        );
    }

    #[test]
    fn parse_quantity_prefix_variants() {
        assert_eq!(
            parse_quantity_prefix("3*123456"),
            (Decimal::from(3), "123456")
        );
        assert_eq!(parse_quantity_prefix("123456"), (Decimal::ONE, "123456"));
        assert_eq!(
            parse_quantity_prefix("  2 * 123456  "),
            (Decimal::from(2), "123456")
        );
        // Non-numeric prefix: whole input is the barcode.
        assert_eq!(
            parse_quantity_prefix("ABC*123456"),
            (Decimal::ONE, "ABC*123456")
        );
        // Zero prefix is not a quantity.
        assert_eq!(
            parse_quantity_prefix("0*123456"),
            (Decimal::ONE, "0*123456")
        );
    }
}
