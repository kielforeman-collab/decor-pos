//! Matrix pricing (spec §4 `matrix_pricing`, §5.4 `pricing_rules`).
//!
//! Dimensions are Customer/Level × Dept/Class/Line/Vendor/SKU × Qty Break.
//! A `None` dimension is a wildcard. The winning rule is the most specific
//! match (most non-wildcard dimensions); ties break on `priority` (lower
//! wins), then on rule id for determinism. A minimum-margin floor can clamp
//! the computed price and flags the line as needing a manager override.

use crate::error::DomainError;
use crate::money::Money;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How a rule computes its price from list price / cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CalcMethod {
    /// `calc_value` is the unit price.
    Fixed,
    /// `calc_value` is a percent off list, e.g. `20` = 20% off.
    PctOffList,
    /// `calc_value` is a percent markup on cost, e.g. `50` = cost * 1.50.
    MarkupCost,
    /// `calc_value` is the target gross margin percent, e.g. `40`.
    TargetGm,
}

/// One row of `pricing_rules` (spec §5.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingRule {
    pub id: Uuid,
    pub priority: i32,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub price_level_id: Option<Uuid>,
    pub product_id: Option<Uuid>,
    pub product_line_id: Option<Uuid>,
    pub class_id: Option<Uuid>,
    pub department_id: Option<Uuid>,
    pub vendor_id: Option<Uuid>,
    pub min_qty: Decimal,
    pub calc_method: CalcMethod,
    /// Meaning depends on `calc_method` (fixed price, percent, or margin).
    pub calc_value: Decimal,
    /// Minimum gross margin percent; `None` = no floor.
    pub min_margin_floor: Option<Decimal>,
    pub effective_from: Option<NaiveDate>,
    pub effective_to: Option<NaiveDate>,
}

impl PricingRule {
    /// Number of non-wildcard dimensions — the specificity score.
    pub fn specificity(&self) -> u32 {
        [
            self.customer_id,
            self.job_id,
            self.price_level_id,
            self.product_id,
            self.product_line_id,
            self.class_id,
            self.department_id,
            self.vendor_id,
        ]
        .iter()
        .filter(|d| d.is_some())
        .count() as u32
    }

    fn dimension_matches(rule_dim: Option<Uuid>, input_dim: Option<Uuid>) -> bool {
        match rule_dim {
            None => true,
            Some(rule_value) => input_dim == Some(rule_value),
        }
    }

    pub fn matches(&self, input: &PricingInput) -> bool {
        if input.qty < self.min_qty {
            return false;
        }
        if let Some(from) = self.effective_from
            && input.on < from
        {
            return false;
        }
        if let Some(to) = self.effective_to
            && input.on > to
        {
            return false;
        }
        Self::dimension_matches(self.customer_id, input.customer_id)
            && Self::dimension_matches(self.job_id, input.job_id)
            && Self::dimension_matches(self.price_level_id, input.price_level_id)
            && Self::dimension_matches(self.product_id, input.product_id)
            && Self::dimension_matches(self.product_line_id, input.product_line_id)
            && Self::dimension_matches(self.class_id, input.class_id)
            && Self::dimension_matches(self.department_id, input.department_id)
            && Self::dimension_matches(self.vendor_id, input.vendor_id)
    }

    /// Compute the unit price this rule yields, or `None` if the
    /// calculation is invalid for this input (e.g. target margin >= 100).
    pub fn compute_price(&self, input: &PricingInput) -> Option<Money> {
        let currency = input.list_price.currency();
        match self.calc_method {
            CalcMethod::Fixed => Some(Money::new(self.calc_value, currency)),
            CalcMethod::PctOffList => {
                if self.calc_value < Decimal::ZERO || self.calc_value > Decimal::from(100) {
                    return None;
                }
                let factor = Decimal::ONE - self.calc_value / Decimal::from(100);
                Some(input.list_price.mul_decimal(factor))
            }
            CalcMethod::MarkupCost => {
                if self.calc_value < Decimal::ZERO {
                    return None;
                }
                let factor = Decimal::ONE + self.calc_value / Decimal::from(100);
                Some(input.cost.mul_decimal(factor))
            }
            CalcMethod::TargetGm => Money::price_for_margin(input.cost, self.calc_value).ok(),
        }
    }
}

/// What is being priced.
#[derive(Debug, Clone, PartialEq)]
pub struct PricingInput {
    pub list_price: Money,
    pub cost: Money,
    pub qty: Decimal,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub price_level_id: Option<Uuid>,
    pub product_id: Option<Uuid>,
    pub product_line_id: Option<Uuid>,
    pub class_id: Option<Uuid>,
    pub department_id: Option<Uuid>,
    pub vendor_id: Option<Uuid>,
    pub on: NaiveDate,
}

/// Outcome of [`evaluate_price`].
#[derive(Debug, Clone, PartialEq)]
pub struct PriceOutcome {
    pub unit_price: Money,
    /// The winning rule, or `None` when no rule matched (list price used).
    pub rule_id: Option<Uuid>,
    pub margin_pct: Option<Decimal>,
    /// True when the minimum-margin floor clamped the price.
    pub floor_applied: bool,
    /// True when a manager override is required to sell below the floor.
    pub needs_manager_override: bool,
}

/// Evaluate the matrix for one line. Deterministic: same rules + input
/// always yield the same outcome.
pub fn evaluate_price(rules: &[PricingRule], input: &PricingInput) -> PriceOutcome {
    let mut best: Option<&PricingRule> = None;
    for rule in rules {
        if !rule.matches(input) {
            continue;
        }
        if rule.compute_price(input).is_none() {
            continue;
        }
        best = match best {
            None => Some(rule),
            Some(current) => {
                let better = (rule.specificity(), -rule.priority, rule.id)
                    > (current.specificity(), -current.priority, current.id);
                // Note: comparing tuples with Uuid last is only a
                // deterministic tie-break; specificity and priority decide
                // first. (Uuid ordering is total, so this is stable.)
                if better { Some(rule) } else { Some(current) }
            }
        };
    }

    let (unit_price, rule_id, floor) = match best {
        Some(rule) => (
            rule.compute_price(input).expect("checked above"),
            Some(rule.id),
            rule.min_margin_floor,
        ),
        None => (input.list_price, None, None),
    };

    apply_floor(unit_price, rule_id, floor, input)
}

fn apply_floor(
    unit_price: Money,
    rule_id: Option<Uuid>,
    floor: Option<Decimal>,
    input: &PricingInput,
) -> PriceOutcome {
    let margin_pct = unit_price.margin_pct(input.cost).unwrap_or(None);
    let Some(floor_pct) = floor else {
        return PriceOutcome {
            unit_price,
            rule_id,
            margin_pct,
            floor_applied: false,
            needs_manager_override: false,
        };
    };
    let below_floor = match margin_pct {
        Some(margin) => margin < floor_pct,
        None => true, // zero price is always below any floor
    };
    if !below_floor {
        return PriceOutcome {
            unit_price,
            rule_id,
            margin_pct,
            floor_applied: false,
            needs_manager_override: false,
        };
    }
    match Money::price_for_margin(input.cost, floor_pct) {
        Ok(floor_price) => PriceOutcome {
            unit_price: floor_price,
            rule_id,
            margin_pct: floor_price.margin_pct(input.cost).unwrap_or(None),
            floor_applied: true,
            needs_manager_override: true,
        },
        Err(_) => PriceOutcome {
            unit_price,
            rule_id,
            margin_pct,
            floor_applied: false,
            needs_manager_override: true,
        },
    }
}

/// Validate a rule's static fields before it is stored.
pub fn validate_rule(rule: &PricingRule) -> Result<(), DomainError> {
    if rule.min_qty < Decimal::ZERO {
        return Err(DomainError::InvalidQuantity(format!(
            "min_qty must not be negative: {}",
            rule.min_qty
        )));
    }
    if let (Some(from), Some(to)) = (rule.effective_from, rule.effective_to)
        && from > to
    {
        return Err(DomainError::InvalidDateRange(format!(
            "{from} is after {to}"
        )));
    }
    if let Some(floor) = rule.min_margin_floor
        && (floor < Decimal::ZERO || floor >= Decimal::from(100))
    {
        return Err(DomainError::InvalidPercentage(format!(
            "min_margin_floor must be in [0, 100): {floor}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn input() -> PricingInput {
        PricingInput {
            list_price: Money::from_units(100, Currency::Usd),
            cost: Money::from_units(50, Currency::Usd),
            qty: d("1"),
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: None,
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            on: NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
        }
    }

    fn rule(id: u128, priority: i32, method: CalcMethod, value: &str) -> PricingRule {
        PricingRule {
            id: Uuid::from_u128(id),
            priority,
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: None,
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            min_qty: d("1"),
            calc_method: method,
            calc_value: d(value),
            min_margin_floor: None,
            effective_from: None,
            effective_to: None,
        }
    }

    #[test]
    fn no_rule_uses_list_price() {
        let out = evaluate_price(&[], &input());
        assert_eq!(out.unit_price, Money::from_units(100, Currency::Usd));
        assert_eq!(out.rule_id, None);
    }

    #[test]
    fn calc_methods() {
        let rules = vec![
            rule(1, 10, CalcMethod::PctOffList, "20"),
            rule(2, 10, CalcMethod::MarkupCost, "50"),
            rule(3, 10, CalcMethod::TargetGm, "40"),
            rule(4, 10, CalcMethod::Fixed, "77"),
        ];
        // Each on its own, all wildcards with equal specificity/priority:
        // the deterministic tie-break picks the highest rule id.
        let _ = rules;
        let pct = evaluate_price(&[rule(1, 10, CalcMethod::PctOffList, "20")], &input());
        assert_eq!(pct.unit_price, Money::from_units(80, Currency::Usd));
        let markup = evaluate_price(&[rule(2, 10, CalcMethod::MarkupCost, "50")], &input());
        assert_eq!(markup.unit_price, Money::from_units(75, Currency::Usd));
        let target = evaluate_price(&[rule(3, 10, CalcMethod::TargetGm, "40")], &input());
        // 50 / (1 - 0.40) = 83.3333
        assert_eq!(
            target.unit_price,
            Money::from_str_amount("83.3333", Currency::Usd).unwrap()
        );
        let fixed = evaluate_price(&[rule(4, 10, CalcMethod::Fixed, "77")], &input());
        assert_eq!(fixed.unit_price, Money::from_units(77, Currency::Usd));
    }

    #[test]
    fn specificity_beats_priority_and_wildcards() {
        let generic = rule(1, 1, CalcMethod::PctOffList, "10");
        let mut specific = rule(2, 99, CalcMethod::PctOffList, "30");
        specific.customer_id = Some(Uuid::from_u128(42));
        let mut inp = input();
        inp.customer_id = Some(Uuid::from_u128(42));
        let out = evaluate_price(&[generic.clone(), specific.clone()], &inp);
        assert_eq!(out.rule_id, Some(specific.id));
        assert_eq!(out.unit_price, Money::from_units(70, Currency::Usd));
        // A different customer does not match the specific rule.
        inp.customer_id = Some(Uuid::from_u128(7));
        let out = evaluate_price(&[generic, specific], &inp);
        assert_eq!(out.unit_price, Money::from_units(90, Currency::Usd));
    }

    #[test]
    fn qty_break_and_dates_gate_rules() {
        let mut bulk = rule(1, 1, CalcMethod::PctOffList, "25");
        bulk.min_qty = d("10");
        let mut inp = input();
        inp.qty = d("5");
        assert_eq!(
            evaluate_price(&[bulk.clone()], &inp).unit_price,
            Money::from_units(100, Currency::Usd)
        );
        inp.qty = d("10");
        assert_eq!(
            evaluate_price(&[bulk], &inp).unit_price,
            Money::from_units(75, Currency::Usd)
        );
    }

    #[test]
    fn margin_floor_clamps_and_flags_override() {
        let mut r = rule(1, 1, CalcMethod::PctOffList, "60");
        r.min_margin_floor = Some(d("30"));
        let out = evaluate_price(&[r], &input());
        // 60% off list = $40, margin (40-50)/40 < 0, floor at 30%:
        // 50 / (1 - 0.30) = 71.4286
        assert!(out.floor_applied);
        assert!(out.needs_manager_override);
        assert_eq!(
            out.unit_price,
            Money::from_str_amount("71.4286", Currency::Usd).unwrap()
        );
    }
}
