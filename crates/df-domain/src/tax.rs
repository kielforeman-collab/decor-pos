//! US + Canada tax and PaintCare / Canadian eco-fee engine (spec §1, §5.2).
//!
//! Rates in [`tax_presets`] / [`eco_fee_presets`] are **sample presets**
//! shipped for development and testing: statutory sales-tax rates and
//! stewardship fees change, and the PaintCare / provincial schedules must
//! be verified against the current official schedule before production
//! use. The engine itself is data-driven — a rule set loaded from the
//! database replaces these presets without code changes.

use crate::error::DomainError;
use crate::money::{Currency, Money};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Country {
    Us,
    Ca,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaxKind {
    SalesTax,
    Gst,
    Pst,
    Qst,
    Hst,
}

impl TaxKind {
    pub fn code(self) -> &'static str {
        match self {
            TaxKind::SalesTax => "SALES",
            TaxKind::Gst => "GST",
            TaxKind::Pst => "PST",
            TaxKind::Qst => "QST",
            TaxKind::Hst => "HST",
        }
    }
}

/// One tax component of a jurisdiction, e.g. BC PST 7% or QC QST 9.975%.
/// `rate` is a fraction: `0.13` means 13%.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TaxComponent {
    pub kind: TaxKind,
    /// Fraction, e.g. `0.13` for 13%.
    pub rate: Decimal,
}

/// A jurisdiction's tax rule set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaxRule {
    pub country: Country,
    /// State / province code: `CA`, `OR`, `TX`, `WA`, `ON`, `BC`, `QC`, ...
    pub region: String,
    pub components: Vec<TaxComponent>,
}

impl TaxRule {
    pub fn total_rate(&self) -> Decimal {
        self.components
            .iter()
            .map(|c| c.rate)
            .fold(Decimal::ZERO, |a, b| a + b)
    }
}

/// One computed tax line.
#[derive(Debug, Clone, PartialEq)]
pub struct TaxLine {
    pub kind: TaxKind,
    pub amount: Money,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaxResult {
    pub lines: Vec<TaxLine>,
    pub total: Money,
}

/// Compute tax on a taxable amount. Tax-exempt customers pay nothing.
/// Each component is computed on the same base amount (no tax-on-tax,
/// matching the US/CA presets shipped here).
pub fn compute_tax(
    rule: &TaxRule,
    taxable_amount: Money,
    tax_exempt: bool,
) -> Result<TaxResult, DomainError> {
    if tax_exempt {
        return Ok(TaxResult {
            lines: Vec::new(),
            total: Money::zero(taxable_amount.currency()),
        });
    }
    let mut lines = Vec::new();
    let mut total = Money::zero(taxable_amount.currency());
    for component in &rule.components {
        let amount = taxable_amount.mul_decimal(component.rate);
        lines.push(TaxLine {
            kind: component.kind,
            amount,
        });
        total = total.checked_add(amount)?;
    }
    Ok(TaxResult { lines, total })
}

fn rate(s: &str) -> Decimal {
    Decimal::from_str_exact(s).expect("preset rate is a valid decimal")
}

/// Sample US + CA tax presets (see module docs — verify before production).
pub fn tax_presets() -> Vec<TaxRule> {
    vec![
        TaxRule {
            country: Country::Us,
            region: "CA".to_string(),
            components: vec![TaxComponent {
                kind: TaxKind::SalesTax,
                rate: rate("0.0725"), // California state base rate
            }],
        },
        TaxRule {
            country: Country::Us,
            region: "OR".to_string(),
            components: vec![], // Oregon: no statewide sales tax
        },
        TaxRule {
            country: Country::Us,
            region: "TX".to_string(),
            components: vec![TaxComponent {
                kind: TaxKind::SalesTax,
                rate: rate("0.0625"), // Texas state rate
            }],
        },
        TaxRule {
            country: Country::Us,
            region: "WA".to_string(),
            components: vec![TaxComponent {
                kind: TaxKind::SalesTax,
                rate: rate("0.065"), // Washington state rate
            }],
        },
        TaxRule {
            country: Country::Ca,
            region: "ON".to_string(),
            components: vec![TaxComponent {
                kind: TaxKind::Hst,
                rate: rate("0.13"),
            }],
        },
        TaxRule {
            country: Country::Ca,
            region: "BC".to_string(),
            components: vec![
                TaxComponent {
                    kind: TaxKind::Gst,
                    rate: rate("0.05"),
                },
                TaxComponent {
                    kind: TaxKind::Pst,
                    rate: rate("0.07"),
                },
            ],
        },
        TaxRule {
            country: Country::Ca,
            region: "QC".to_string(),
            components: vec![
                TaxComponent {
                    kind: TaxKind::Gst,
                    rate: rate("0.05"),
                },
                TaxComponent {
                    kind: TaxKind::Qst,
                    rate: rate("0.09975"),
                },
            ],
        },
    ]
}

pub fn find_tax_rule<'a>(
    rules: &'a [TaxRule],
    country: Country,
    region: &str,
) -> Option<&'a TaxRule> {
    rules
        .iter()
        .find(|r| r.country == country && r.region.eq_ignore_ascii_case(region))
}

/// Eco-fee / PaintCare category (spec §5.2 `eco_fee_category`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EcoFeeCategory {
    ArchitecturalPaint,
    Stain,
    Aerosol,
    None,
}

/// One row of `eco_fee_rules` (spec §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EcoFeeRule {
    pub country: Country,
    pub region: String,
    pub category: EcoFeeCategory,
    pub min_volume_ml: u32,
    pub max_volume_ml: u32,
    pub fee: Money,
    pub is_taxable: bool,
    pub effective_from: NaiveDate,
    pub effective_to: Option<NaiveDate>,
}

impl EcoFeeRule {
    pub fn applies(
        &self,
        country: Country,
        region: &str,
        category: EcoFeeCategory,
        volume_ml: u32,
        on: NaiveDate,
    ) -> bool {
        self.country == country
            && self.region.eq_ignore_ascii_case(region)
            && self.category == category
            && volume_ml >= self.min_volume_ml
            && volume_ml <= self.max_volume_ml
            && on >= self.effective_from
            && self.effective_to.is_none_or(|to| on <= to)
    }
}

fn money(s: &str, currency: Currency) -> Money {
    Money::from_str_amount(s, currency).expect("preset fee is a valid decimal")
}

fn eco(
    country: Country,
    region: &str,
    category: EcoFeeCategory,
    min_ml: u32,
    max_ml: u32,
    fee: &str,
    currency: Currency,
) -> EcoFeeRule {
    EcoFeeRule {
        country,
        region: region.to_string(),
        category,
        min_volume_ml: min_ml,
        max_volume_ml: max_ml,
        fee: money(fee, currency),
        is_taxable: true,
        effective_from: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        effective_to: None,
    }
}

/// Sample eco-fee presets (see module docs — verify before production).
/// Bands are by container volume in ml.
pub fn eco_fee_presets() -> Vec<EcoFeeRule> {
    vec![
        // PaintCare California — architectural paint, per container.
        eco(
            Country::Us,
            "CA",
            EcoFeeCategory::ArchitecturalPaint,
            1,
            946,
            "0.3500",
            Currency::Usd,
        ),
        eco(
            Country::Us,
            "CA",
            EcoFeeCategory::ArchitecturalPaint,
            947,
            3785,
            "0.7500",
            Currency::Usd,
        ),
        eco(
            Country::Us,
            "CA",
            EcoFeeCategory::ArchitecturalPaint,
            3786,
            18927,
            "1.6000",
            Currency::Usd,
        ),
        // PaintCare Oregon — architectural paint, per container.
        eco(
            Country::Us,
            "OR",
            EcoFeeCategory::ArchitecturalPaint,
            1,
            946,
            "0.3500",
            Currency::Usd,
        ),
        eco(
            Country::Us,
            "OR",
            EcoFeeCategory::ArchitecturalPaint,
            947,
            3785,
            "0.7500",
            Currency::Usd,
        ),
        eco(
            Country::Us,
            "OR",
            EcoFeeCategory::ArchitecturalPaint,
            3786,
            18927,
            "1.6000",
            Currency::Usd,
        ),
        // Canadian eco fees — architectural paint, per container.
        eco(
            Country::Ca,
            "ON",
            EcoFeeCategory::ArchitecturalPaint,
            1,
            1000,
            "0.5000",
            Currency::Cad,
        ),
        eco(
            Country::Ca,
            "ON",
            EcoFeeCategory::ArchitecturalPaint,
            1001,
            5000,
            "1.0000",
            Currency::Cad,
        ),
        eco(
            Country::Ca,
            "ON",
            EcoFeeCategory::ArchitecturalPaint,
            5001,
            20000,
            "2.0000",
            Currency::Cad,
        ),
        eco(
            Country::Ca,
            "BC",
            EcoFeeCategory::ArchitecturalPaint,
            1,
            1000,
            "0.5000",
            Currency::Cad,
        ),
        eco(
            Country::Ca,
            "BC",
            EcoFeeCategory::ArchitecturalPaint,
            1001,
            5000,
            "1.0000",
            Currency::Cad,
        ),
        eco(
            Country::Ca,
            "BC",
            EcoFeeCategory::ArchitecturalPaint,
            5001,
            20000,
            "2.0000",
            Currency::Cad,
        ),
        eco(
            Country::Ca,
            "QC",
            EcoFeeCategory::ArchitecturalPaint,
            1,
            1000,
            "0.5000",
            Currency::Cad,
        ),
        eco(
            Country::Ca,
            "QC",
            EcoFeeCategory::ArchitecturalPaint,
            1001,
            5000,
            "1.0000",
            Currency::Cad,
        ),
        eco(
            Country::Ca,
            "QC",
            EcoFeeCategory::ArchitecturalPaint,
            5001,
            20000,
            "2.0000",
            Currency::Cad,
        ),
    ]
}

/// Total eco fee for `qty` containers of the given volume. `None` when no
/// rule applies (e.g. category `None`, or an uncovered region/volume).
pub fn eco_fee_for(
    rules: &[EcoFeeRule],
    country: Country,
    region: &str,
    category: EcoFeeCategory,
    volume_ml: u32,
    qty: Decimal,
    on: NaiveDate,
) -> Option<Money> {
    let rule = rules
        .iter()
        .find(|r| r.applies(country, region, category, volume_ml, on))?;
    Some(rule.fee.mul_decimal(qty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    #[test]
    fn tax_presets_cover_us_and_ca() {
        let presets = tax_presets();
        let ca = find_tax_rule(&presets, Country::Us, "CA").unwrap();
        assert_eq!(ca.total_rate(), rate("0.0725"));
        let or = find_tax_rule(&presets, Country::Us, "OR").unwrap();
        assert_eq!(or.total_rate(), Decimal::ZERO);
        let bc = find_tax_rule(&presets, Country::Ca, "BC").unwrap();
        assert_eq!(bc.total_rate(), rate("0.12"));
        let qc = find_tax_rule(&presets, Country::Ca, "QC").unwrap();
        assert_eq!(qc.total_rate(), rate("0.14975"));
        assert!(find_tax_rule(&presets, Country::Ca, "MB").is_none());
    }

    #[test]
    fn bc_tax_splits_gst_and_pst() {
        let presets = tax_presets();
        let bc = find_tax_rule(&presets, Country::Ca, "BC").unwrap();
        let result = compute_tax(bc, Money::from_units(100, Currency::Cad), false).unwrap();
        assert_eq!(result.lines.len(), 2);
        assert_eq!(result.lines[0].kind, TaxKind::Gst);
        assert_eq!(
            result.lines[0].amount,
            Money::from_str_amount("5.0000", Currency::Cad).unwrap()
        );
        assert_eq!(
            result.lines[1].amount,
            Money::from_str_amount("7.0000", Currency::Cad).unwrap()
        );
        assert_eq!(
            result.total,
            Money::from_str_amount("12.0000", Currency::Cad).unwrap()
        );
    }

    #[test]
    fn exempt_customer_pays_no_tax() {
        let presets = tax_presets();
        let on = find_tax_rule(&presets, Country::Ca, "ON").unwrap();
        let result = compute_tax(on, Money::from_units(100, Currency::Cad), true).unwrap();
        assert!(result.total.is_zero());
        assert!(result.lines.is_empty());
    }

    #[test]
    fn eco_fee_bands_by_volume() {
        let presets = eco_fee_presets();
        let on = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        // 1 gallon (3785 ml) in California: $0.75 per container, x2 = $1.50.
        let fee = eco_fee_for(
            &presets,
            Country::Us,
            "CA",
            EcoFeeCategory::ArchitecturalPaint,
            3785,
            Decimal::from(2),
            on,
        )
        .unwrap();
        assert_eq!(
            fee,
            Money::from_str_amount("1.5000", Currency::Usd).unwrap()
        );
        // A pint (473 ml) is the small band.
        let fee = eco_fee_for(
            &presets,
            Country::Us,
            "CA",
            EcoFeeCategory::ArchitecturalPaint,
            473,
            Decimal::ONE,
            on,
        )
        .unwrap();
        assert_eq!(
            fee,
            Money::from_str_amount("0.3500", Currency::Usd).unwrap()
        );
        // No rule for an uncovered region.
        assert!(
            eco_fee_for(
                &presets,
                Country::Us,
                "TX",
                EcoFeeCategory::ArchitecturalPaint,
                3785,
                Decimal::ONE,
                on
            )
            .is_none()
        );
        // Category None never has a fee.
        assert!(
            eco_fee_for(
                &presets,
                Country::Us,
                "CA",
                EcoFeeCategory::None,
                3785,
                Decimal::ONE,
                on
            )
            .is_none()
        );
    }
}
