//! Tint formula costing (spec §5.3 `tint_formulas`, `colorants`).
//!
//! A colorant is dispensed in fluid ounces plus shots (48ths of a fluid
//! ounce by default, per `colorants.shots_per_fl_oz`). Cost is exact
//! decimal arithmetic on `cost_per_fl_oz`.
//!
//! Note: this is the *formula cost* math only. No tinter hardware, no
//! vendor protocol — the Benjamin Moore COLORx integration is deferred
//! post-v1.0 (spec §9) and there is no COLORx code in this crate.

use crate::error::DomainError;
use crate::money::Money;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// One colorant line of a tint formula.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TintColorant {
    pub code: String,
    /// Whole fluid ounces.
    pub ounces: Decimal,
    /// Shots (subdivisions of a fluid ounce).
    pub shots: u32,
    /// Shots per fluid ounce for this colorant system (usually 48).
    pub shots_per_fl_oz: u32,
    /// Cost per fluid ounce.
    pub cost_per_fl_oz: Money,
}

impl TintColorant {
    /// Total fluid ounces dispensed: `ounces + shots / shots_per_fl_oz`.
    pub fn total_fl_oz(&self) -> Result<Decimal, DomainError> {
        if self.shots_per_fl_oz == 0 {
            return Err(DomainError::DivisionByZero);
        }
        Ok(self.ounces + Decimal::from(self.shots) / Decimal::from(self.shots_per_fl_oz))
    }

    /// Cost of this colorant line.
    pub fn cost(&self) -> Result<Money, DomainError> {
        Ok(self.cost_per_fl_oz.mul_decimal(self.total_fl_oz()?))
    }
}

/// Total tint cost of a formula: the sum of its colorant lines. All
/// lines must share a currency.
pub fn tint_cost(colorants: &[TintColorant]) -> Result<Money, DomainError> {
    let mut total: Option<Money> = None;
    for line in colorants {
        let cost = line.cost()?;
        total = Some(match total {
            None => cost,
            Some(acc) => acc.checked_add(cost)?,
        });
    }
    Ok(total.unwrap_or_else(|| Money::zero(crate::money::Currency::Usd)))
}

/// Total tint cost plus an optional deep-base surcharge.
pub fn tint_cost_with_surcharge(
    colorants: &[TintColorant],
    surcharge: Money,
) -> Result<Money, DomainError> {
    tint_cost(colorants)?.checked_add(surcharge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn colorant(code: &str, oz: &str, shots: u32, cost: &str) -> TintColorant {
        TintColorant {
            code: code.to_string(),
            ounces: Decimal::from_str_exact(oz).unwrap(),
            shots,
            shots_per_fl_oz: 48,
            cost_per_fl_oz: Money::from_str_amount(cost, Currency::Usd).unwrap(),
        }
    }

    #[test]
    fn tint_cost_sums_ounces_and_shots() {
        // Y3: 0 oz + 14 shots @ $0.42/fl oz = 14/48 * 0.42 = 0.1225
        // R1: 1 oz + 0 shots @ $0.50/fl oz = 0.5000
        let lines = vec![
            colorant("Y3", "0", 14, "0.4200"),
            colorant("R1", "1", 0, "0.5000"),
        ];
        assert_eq!(
            tint_cost(&lines).unwrap(),
            Money::from_str_amount("0.6225", Currency::Usd).unwrap()
        );
    }

    #[test]
    fn empty_formula_costs_nothing() {
        assert!(tint_cost(&[]).unwrap().is_zero());
    }
}
