//! ZPL paint-can label with formula shots (Phase 2, spec §6.1
//! `POST /tinting/formulas/{id}/print-label`).
//!
//! This extends `df-hw`'s [`df_hw::can_label_zpl`] rather than
//! duplicating it: the header (product, color, sheen/base, formula
//! reference) and the Code 128 barcode come from `df-hw`, and this
//! crate appends one line per colorant shot below the barcode before
//! the closing `^XZ`. `df-hw` remains the single owner of the base
//! can-label layout.

use crate::job::DispensedShot;

/// A tinted-can label: the `df-hw` can-label fields plus the exact
/// colorant shots dispensed.
#[derive(Debug, Clone, PartialEq)]
pub struct TintCanLabel {
    pub product_name: String,
    pub color_code: String,
    pub color_name: String,
    pub sheen: String,
    pub base: String,
    pub formula_ref: String,
    pub barcode: String,
    pub shots: Vec<DispensedShot>,
}

/// ZPL II for a tinted paint can: `df-hw` can label + one
/// `CODE: <fl oz> fl oz` line per colorant shot.
pub fn tint_can_label_zpl(label: &TintCanLabel) -> String {
    let base_zpl = df_hw::can_label_zpl(
        &label.product_name,
        &label.color_code,
        &label.color_name,
        &label.sheen,
        &label.base,
        &label.formula_ref,
        &label.barcode,
    );
    if label.shots.is_empty() {
        return base_zpl;
    }
    // The df-hw label ends "^XZ\n" with the barcode at y=200 (80 high);
    // shot lines start below it at y=300, 40 apart.
    let mut shots_zpl = String::new();
    for (index, shot) in label.shots.iter().enumerate() {
        let y = 300 + index * 40;
        shots_zpl.push_str(&format!(
            "^FO20,{y}^A0N,28,28^FD{}: {} fl oz^FS\n",
            zpl_text(&shot.code),
            shot.fl_oz,
        ));
    }
    base_zpl
        .trim_end()
        .strip_suffix("^XZ")
        .map(|without_end| format!("{without_end}{shots_zpl}^XZ\n"))
        .unwrap_or(base_zpl)
}

/// Strip ZPL control characters (`^`, `~`, C0 controls) so
/// user-supplied values cannot break out of a `^FD` field — same rule
/// as `df-hw`'s label sanitiser.
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
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn label() -> TintCanLabel {
        TintCanLabel {
            product_name: "Regal Select Interior Gallon".to_string(),
            color_code: "OC-17".to_string(),
            color_name: "White Dove".to_string(),
            sheen: "Eggshell".to_string(),
            base: "1X".to_string(),
            formula_ref: "F-2026-0042".to_string(),
            barcode: "012345678905".to_string(),
            shots: vec![
                DispensedShot {
                    code: "Y3".to_string(),
                    fl_oz: Decimal::from_str("14").unwrap() / Decimal::from(48),
                },
                DispensedShot {
                    code: "R1".to_string(),
                    fl_oz: Decimal::from(1),
                },
            ],
        }
    }

    #[test]
    fn tint_label_extends_hw_can_label_with_shot_lines() {
        let zpl = tint_can_label_zpl(&label());
        assert!(zpl.starts_with("^XA"));
        assert!(zpl.trim_end().ends_with("^XZ"));
        // df-hw header fields are present verbatim.
        assert!(zpl.contains("OC-17 White Dove"));
        assert!(zpl.contains("Eggshell / 1X"));
        assert!(zpl.contains("Formula: F-2026-0042"));
        assert!(zpl.contains("^BCN,80,Y,N,N^FD012345678905^FS"));
        // Shot lines follow the barcode.
        assert!(zpl.contains("Y3: 0.2916666666666666666666666667 fl oz"));
        assert!(zpl.contains("R1: 1 fl oz"));
        assert!(zpl.find("Y3:").unwrap() > zpl.find("^BCN").unwrap());
    }

    #[test]
    fn tint_label_without_shots_is_the_hw_label() {
        let mut no_shots = label();
        no_shots.shots.clear();
        let zpl = tint_can_label_zpl(&no_shots);
        assert_eq!(
            zpl,
            df_hw::can_label_zpl(
                "Regal Select Interior Gallon",
                "OC-17",
                "White Dove",
                "Eggshell",
                "1X",
                "F-2026-0042",
                "012345678905",
            )
        );
    }

    #[test]
    fn tint_label_sanitises_shot_codes() {
        let mut bad = label();
        bad.shots[0].code = "Y^3".to_string();
        let zpl = tint_can_label_zpl(&bad);
        assert!(zpl.contains("Y 3:"));
        assert!(!zpl.contains("Y^3:"));
    }
}
