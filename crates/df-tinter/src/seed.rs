//! Seed / demo catalog data for tests and demos only (Phase 2).
//!
//! These are *catalog data* — color names/codes and colorant costs —
//! not vendor protocol code. There is no Benjamin Moore COLORx code,
//! protocol, or certification work anywhere in this crate (deferred
//! post-v1.0, spec §9); the names below could equally be typed in by
//! hand at the counter.

use df_domain::tint::TintColorant;
use df_domain::{Currency, Money};
use rust_decimal::Decimal;

use crate::catalog::CatalogColor;

/// Demo color catalog: a small set of well-known off-white / neutral
/// colors, in the style of a manufacturer fan deck.
pub fn seed_catalog() -> Vec<CatalogColor> {
    let mut colors = vec![
        CatalogColor::new("Benjamin Moore", "OC-17", "White Dove"),
        CatalogColor::new("Benjamin Moore", "OC-65", "Chantilly Lace"),
        CatalogColor::new("Benjamin Moore", "OC-117", "Simply White"),
        CatalogColor::new("Benjamin Moore", "HC-172", "Revere Pewter"),
        CatalogColor::new("Benjamin Moore", "HC-154", "Hale Navy"),
        CatalogColor::new("Benjamin Moore", "HC-166", "Kendall Charcoal"),
    ];
    colors[0].collection = Some("Off-White".to_string());
    colors[0].rgb_hex = Some("#EFEDE6".to_string());
    colors[0].recommended_base = Some("1X".to_string());
    colors[1].collection = Some("Off-White".to_string());
    colors[1].rgb_hex = Some("#F2F0EB".to_string());
    colors[1].recommended_base = Some("1X".to_string());
    colors[2].collection = Some("Off-White".to_string());
    colors[2].recommended_base = Some("1X".to_string());
    colors[3].collection = Some("Historical".to_string());
    colors[3].recommended_base = Some("2X".to_string());
    colors[4].collection = Some("Historical".to_string());
    colors[4].recommended_base = Some("3X".to_string());
    colors[5].collection = Some("Historical".to_string());
    colors[5].recommended_base = Some("3X".to_string());
    colors
}

/// One seed colorant: system, code, name, cost and shots-per-ounce.
#[derive(Debug, Clone, PartialEq)]
pub struct SeedColorant {
    pub system_name: String,
    pub code: String,
    pub name: String,
    pub cost_per_fl_oz: Money,
    pub shots_per_fl_oz: u32,
}

impl SeedColorant {
    /// Build a `TintColorant` formula line from this seed colorant.
    pub fn to_tint_colorant(&self, ounces: Decimal, shots: u32) -> TintColorant {
        TintColorant {
            code: self.code.clone(),
            ounces,
            shots,
            shots_per_fl_oz: self.shots_per_fl_oz,
            cost_per_fl_oz: self.cost_per_fl_oz,
        }
    }
}

/// Demo colorant system: universal colorants with sample costs.
pub fn seed_colorants() -> Vec<SeedColorant> {
    let cost = |amount: &str| Money::from_str_amount(amount, Currency::Usd).unwrap();
    vec![
        SeedColorant {
            system_name: "Gennex".to_string(),
            code: "Y3".to_string(),
            name: "Yellow Oxide".to_string(),
            cost_per_fl_oz: cost("0.4200"),
            shots_per_fl_oz: 48,
        },
        SeedColorant {
            system_name: "Gennex".to_string(),
            code: "R1".to_string(),
            name: "Red Oxide".to_string(),
            cost_per_fl_oz: cost("0.5000"),
            shots_per_fl_oz: 48,
        },
        SeedColorant {
            system_name: "Gennex".to_string(),
            code: "BK".to_string(),
            name: "Black".to_string(),
            cost_per_fl_oz: cost("0.3800"),
            shots_per_fl_oz: 48,
        },
        SeedColorant {
            system_name: "Gennex".to_string(),
            code: "W1".to_string(),
            name: "White".to_string(),
            cost_per_fl_oz: cost("0.3000"),
            shots_per_fl_oz: 48,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::tint::tint_cost;

    #[test]
    fn seed_catalog_has_the_demo_colors() {
        let colors = seed_catalog();
        assert_eq!(colors.len(), 6);
        assert!(
            colors
                .iter()
                .any(|c| c.code == "OC-17" && c.name == "White Dove")
        );
    }

    #[test]
    fn seed_colorant_builds_a_costed_formula_line() {
        let colorants = seed_colorants();
        let y3 = colorants.iter().find(|c| c.code == "Y3").unwrap();
        // White Dove OC-17, 1X base: Y3 0 oz + 14 shots.
        let line = y3.to_tint_colorant(Decimal::ZERO, 14);
        assert_eq!(
            line.total_fl_oz().unwrap().to_string(),
            "0.2916666666666666666666666667"
        );
        assert_eq!(
            tint_cost(std::slice::from_ref(&line)).unwrap(),
            Money::from_str_amount("0.1225", Currency::Usd).unwrap()
        );
    }
}
