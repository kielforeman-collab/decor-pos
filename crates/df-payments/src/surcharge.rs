//! Card-surcharge / POS-fee rules (spec §1).
//!
//! A rule is per-tender: `pct` percent of the sale amount plus a `flat`
//! fee, capped at `max` when set. All math is `rust_decimal` — zero
//! floating-point. A rule whose `flat` or `max` is in a different
//! currency than the sale amount has that part ignored rather than
//! failing the sale; rules are store configuration and are expected to
//! match the store currency.

use crate::provider::TenderKind;
use df_domain::Money;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Surcharge rule for one tender kind. `pct` is a percent value, e.g.
/// `2.5` means 2.5%. Serializable: rules are store configuration held
/// by `df-edge`'s `EdgeConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurchargeRule {
    pub tender: TenderKind,
    pub pct: Decimal,
    pub flat: Money,
    pub max: Option<Money>,
}

/// Surcharge for `amount` under `rule`: `amount * pct / 100 + flat`,
/// capped at `max` when set. Zero when there is no rule. Never negative.
pub fn compute_surcharge(rule: Option<&SurchargeRule>, amount: Money) -> Money {
    let Some(rule) = rule else {
        return Money::zero(amount.currency());
    };
    let mut total = amount.mul_decimal(rule.pct / Decimal::from(100));
    if rule.flat.currency() == amount.currency() {
        total = total.checked_add(rule.flat).unwrap_or(total);
    }
    if let Some(max) = rule.max {
        if max.currency() == amount.currency() && total.amount() > max.amount() {
            total = max;
        }
    }
    if total.is_negative() {
        total = Money::zero(amount.currency());
    }
    total
}

/// The first rule in `rules` for `tender`, if any.
pub fn surcharge_rule_for(tender: TenderKind, rules: &[SurchargeRule]) -> Option<&SurchargeRule> {
    rules.iter().find(|rule| rule.tender == tender)
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::Currency;
    use std::str::FromStr;

    fn usd(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    fn pct_rule(pct: &str, flat: &str, max: Option<&str>) -> SurchargeRule {
        SurchargeRule {
            tender: TenderKind::Card,
            pct: Decimal::from_str(pct).unwrap(),
            flat: usd(flat),
            max: max.map(usd),
        }
    }

    #[test]
    fn no_rule_means_zero_surcharge() {
        assert_eq!(compute_surcharge(None, usd("100.0000")), usd("0.0000"));
    }

    #[test]
    fn percent_only() {
        // 2.5% of $100 = $2.50.
        assert_eq!(
            compute_surcharge(Some(&pct_rule("2.5", "0.0000", None)), usd("100.0000")),
            usd("2.5000")
        );
    }

    #[test]
    fn flat_only() {
        assert_eq!(
            compute_surcharge(Some(&pct_rule("0", "1.5000", None)), usd("100.0000")),
            usd("1.5000")
        );
    }

    #[test]
    fn percent_plus_flat() {
        // 3% of $50 = $1.50, plus $0.30 flat = $1.80.
        assert_eq!(
            compute_surcharge(Some(&pct_rule("3", "0.3000", None)), usd("50.0000")),
            usd("1.8000")
        );
    }

    #[test]
    fn cap_limits_the_surcharge() {
        // 10% of $100 = $10, capped at $5.
        assert_eq!(
            compute_surcharge(
                Some(&pct_rule("10", "0.0000", Some("5.0000"))),
                usd("100.0000")
            ),
            usd("5.0000")
        );
    }

    #[test]
    fn cap_does_not_apply_below_the_max() {
        // 2% of $100 = $2, under the $5 cap.
        assert_eq!(
            compute_surcharge(
                Some(&pct_rule("2", "0.0000", Some("5.0000"))),
                usd("100.0000")
            ),
            usd("2.0000")
        );
    }

    #[test]
    fn surcharge_never_goes_negative() {
        assert_eq!(
            compute_surcharge(Some(&pct_rule("-5", "0.0000", None)), usd("100.0000")),
            usd("0.0000")
        );
    }

    #[test]
    fn rule_for_finds_by_tender() {
        let rules = vec![
            pct_rule("2.5", "0.0000", None),
            SurchargeRule {
                tender: TenderKind::GiftCard,
                pct: Decimal::ZERO,
                flat: usd("0.0000"),
                max: None,
            },
        ];
        assert_eq!(
            surcharge_rule_for(TenderKind::Card, &rules),
            Some(&rules[0])
        );
        assert_eq!(
            surcharge_rule_for(TenderKind::GiftCard, &rules),
            Some(&rules[1])
        );
        assert_eq!(surcharge_rule_for(TenderKind::Cash, &rules), None);
    }
}
