//! ZPL label generation (pure string building — no printer attached).
//!
//! Emits ZPL II for paint-can labels and shelf tags. `^` and `~` are the
//! ZPL control prefixes, so they are stripped from user-supplied text
//! before embedding.

use crate::receipt::format_money;
use df_domain::Money;

/// ZPL paint-can label: product, color, sheen/base, formula reference,
/// and a Code 128 barcode of `barcode`.
///
/// Layout (203 dpi, ~4 in wide): product name large at the top, color
/// code + name, sheen / base, formula reference, then the barcode.
pub fn can_label_zpl(
    product_name: &str,
    color_code: &str,
    color_name: &str,
    sheen: &str,
    base: &str,
    formula_ref: &str,
    barcode: &str,
) -> String {
    format!(
        "^XA\n\
         ^CI28\n\
         ^FO20,20^A0N,40,40^FD{product}^FS\n\
         ^FO20,70^A0N,30,30^FD{color_code} {color_name}^FS\n\
         ^FO20,110^A0N,28,28^FD{sheen} / {base}^FS\n\
         ^FO20,150^A0N,28,28^FDFormula: {formula_ref}^FS\n\
         ^FO20,200^BCN,80,Y,N,N^FD{barcode}^FS\n\
         ^XZ\n",
        product = zpl_text(product_name),
        color_code = zpl_text(color_code),
        color_name = zpl_text(color_name),
        sheen = zpl_text(sheen),
        base = zpl_text(base),
        formula_ref = zpl_text(formula_ref),
        barcode = zpl_text(barcode),
    )
}

/// ZPL shelf tag: product name, SKU, price, and a Code 128 barcode.
pub fn shelf_label_zpl(sku: &str, name: &str, price: Money, barcode: &str) -> String {
    format!(
        "^XA\n\
         ^CI28\n\
         ^FO20,20^A0N,36,36^FD{name}^FS\n\
         ^FO20,70^A0N,28,28^FDSKU: {sku}^FS\n\
         ^FO20,110^A0N,40,40^FD{price}^FS\n\
         ^FO20,170^BCN,60,Y,N,N^FD{barcode}^FS\n\
         ^XZ\n",
        name = zpl_text(name),
        sku = zpl_text(sku),
        price = format_money(price),
        barcode = zpl_text(barcode),
    )
}

/// Strip ZPL control characters (`^`, `~`, and C0 controls) from text so
/// user-supplied values cannot break out of a `^FD` field.
fn zpl_text(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '^' | '~' => ' ',
            c if (c as u32) < 0x20 => ' ',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::Currency;

    #[test]
    fn can_label_contains_all_fields_and_barcode() {
        let zpl = can_label_zpl(
            "Premium Interior Eggshell",
            "OC-17",
            "White Dove",
            "Eggshell",
            "2X",
            "F-2026-0042",
            "012345678905",
        );
        assert!(zpl.starts_with("^XA"));
        assert!(zpl.trim_end().ends_with("^XZ"));
        assert!(zpl.contains("Premium Interior Eggshell"));
        assert!(zpl.contains("OC-17 White Dove"));
        assert!(zpl.contains("Eggshell / 2X"));
        assert!(zpl.contains("Formula: F-2026-0042"));
        assert!(zpl.contains("^BCN,80,Y,N,N^FD012345678905^FS"));
        assert!(zpl.contains("^FO"));
    }

    #[test]
    fn shelf_label_contains_sku_name_price_and_barcode() {
        let price = Money::from_str_amount("45.9900", Currency::Usd).unwrap();
        let zpl = shelf_label_zpl(
            "PAINT-EGG-1G",
            "Premium Interior Eggshell",
            price,
            "012345678905",
        );
        assert!(zpl.starts_with("^XA"));
        assert!(zpl.trim_end().ends_with("^XZ"));
        assert!(zpl.contains("PAINT-EGG-1G"));
        assert!(zpl.contains("Premium Interior Eggshell"));
        assert!(zpl.contains("$45.99"));
        assert!(zpl.contains("^BCN,60,Y,N,N^FD012345678905^FS"));
    }

    #[test]
    fn zpl_text_strips_control_prefixes() {
        let zpl = can_label_zpl(
            "Bad ^XA Name",
            "OC-17",
            "White ~ Dove",
            "Eggshell",
            "2X",
            "F-1",
            "123",
        );
        // No user-supplied ^XA / ~ may survive inside a field: the ^ and
        // ~ are replaced with spaces, the surrounding letters stay.
        assert!(!zpl.contains("Bad ^XA Name"));
        assert!(zpl.contains("Bad  XA Name"));
        assert!(zpl.contains("White   Dove"));
    }
}
