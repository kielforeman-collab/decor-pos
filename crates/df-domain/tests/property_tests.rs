//! Property tests for the deterministic financial & pricing suite.
//!
//! No floats anywhere: strategies generate integer-scaled `Decimal`
//! values, so these tests exercise the exact arithmetic the counter
//! will use.

use chrono::NaiveDate;
use df_domain::money::{Currency, Money};
use df_domain::pricing::{CalcMethod, PricingInput, PricingRule, evaluate_price};
use df_domain::tax::{
    Country, EcoFeeCategory, compute_tax, eco_fee_for, eco_fee_presets, find_tax_rule, tax_presets,
};
use df_domain::uom::{Uom, convert_qty, flooring_boxes_needed};
use proptest::prelude::*;
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

/// A Decimal with 4 decimal places, built from an integer so the
/// strategy never touches a float.
fn money_decimal() -> impl Strategy<Value = Decimal> {
    (-1_000_000_000i64..1_000_000_000i64)
        .prop_map(|n| Decimal::from_i128_with_scale(i128::from(n), 4))
}

fn positive_money_decimal() -> impl Strategy<Value = Decimal> {
    (1i64..1_000_000_000i64).prop_map(|n| Decimal::from_i128_with_scale(i128::from(n), 4))
}

fn money_strategy() -> impl Strategy<Value = Money> {
    money_decimal().prop_map(|d| Money::new(d, Currency::Usd))
}

proptest! {
    #[test]
    fn money_addition_is_commutative_and_reversible(a in money_strategy(), b in money_strategy()) {
        prop_assert_eq!(a.checked_add(b).unwrap(), b.checked_add(a).unwrap());
        prop_assert_eq!(a.checked_add(b).unwrap().checked_sub(b).unwrap(), a);
        prop_assert_eq!(a.checked_add(b).unwrap().checked_sub(a).unwrap(), b);
    }

    #[test]
    fn money_addition_is_associative(a in money_strategy(), b in money_strategy(), c in money_strategy()) {
        let left = a.checked_add(b).unwrap().checked_add(c).unwrap();
        let right = a.checked_add(b.checked_add(c).unwrap()).unwrap();
        prop_assert_eq!(left, right);
    }

    #[test]
    fn money_mul_by_one_is_identity(a in money_strategy()) {
        prop_assert_eq!(a.mul_decimal(Decimal::ONE), a);
    }

    #[test]
    fn money_mul_distributes_over_addition(a in money_strategy(), b in money_strategy(), qty in positive_money_decimal()) {
        // Distributivity holds only up to scale-4 rounding: (a+b)*q is
        // rounded once, a*q + b*q is rounded twice, so the two sides can
        // differ by at most one unit in the last place (0.0001).
        let left = a.checked_add(b).unwrap().mul_decimal(qty);
        let right = a.mul_decimal(qty).checked_add(b.mul_decimal(qty)).unwrap();
        let diff = (left.amount() - right.amount()).abs();
        prop_assert!(diff <= Decimal::from_str("0.0001").unwrap());
    }

    #[test]
    fn money_never_has_more_than_4_decimal_places(a in money_strategy()) {
        // Scale is 4 by construction; rescaling to 4 must be a no-op.
        let mut amount = a.amount();
        amount.rescale(4);
        prop_assert_eq!(amount, a.amount());
    }

    #[test]
    fn uom_volume_roundtrip_is_exact(qty in 0i64..1_000_000i64) {
        let q = Decimal::from(qty);
        let quarts = convert_qty(q, Uom::Gal, Uom::Qt).unwrap();
        let back = convert_qty(quarts, Uom::Qt, Uom::Gal).unwrap();
        prop_assert_eq!(back, q);
    }

    #[test]
    fn uom_area_roundtrip_is_exact(qty in 0i64..1_000_000i64) {
        // Multiples of 9 sq ft round-trip exactly through sq yd.
        let q = Decimal::from(qty * 9);
        let yards = convert_qty(q, Uom::SqFt, Uom::SqYd).unwrap();
        let back = convert_qty(yards, Uom::SqYd, Uom::SqFt).unwrap();
        prop_assert_eq!(back, q);
    }

    #[test]
    fn flooring_boxes_cover_and_are_minimal(area in 1i64..10_000i64, coverage in 1i64..100i64, waste in 0i64..50i64) {
        let calc = flooring_boxes_needed(
            Decimal::from(area),
            Decimal::from(coverage),
            Decimal::from(waste),
        ).unwrap();
        // Purchased area covers the waste-adjusted area...
        prop_assert!(calc.total_sqft_purchased >= calc.waste_adjusted_sqft);
        // ...and one fewer box would not.
        if calc.boxes > 0 {
            let one_less = Decimal::from(calc.boxes - 1) * Decimal::from(coverage);
            prop_assert!(one_less < calc.waste_adjusted_sqft);
        }
    }

    #[test]
    fn tax_total_is_sum_of_lines(amount in positive_money_decimal()) {
        let presets = tax_presets();
        for rule in &presets {
            let result = compute_tax(rule, Money::new(amount, Currency::Cad), false).unwrap();
            let mut sum = Money::zero(Currency::Cad);
            for line in &result.lines {
                sum = sum.checked_add(line.amount).unwrap();
            }
            prop_assert_eq!(sum, result.total);
        }
    }

    #[test]
    fn oregon_never_charges_sales_tax(amount in positive_money_decimal()) {
        let presets = tax_presets();
        let or = find_tax_rule(&presets, Country::Us, "OR").unwrap();
        let result = compute_tax(or, Money::new(amount, Currency::Usd), false).unwrap();
        prop_assert!(result.total.is_zero());
    }

    #[test]
    fn eco_fee_is_per_container_and_non_negative(qty in 1i64..100i64, volume in 1u32..5000u32) {
        let presets = eco_fee_presets();
        let on = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        if let Some(fee) = eco_fee_for(
            &presets,
            Country::Us,
            "CA",
            EcoFeeCategory::ArchitecturalPaint,
            volume,
            Decimal::from(qty),
            on,
        ) {
            prop_assert!(!fee.is_negative());
            // Fee scales linearly with qty: fee(qty) == fee(1) * qty.
            let one = eco_fee_for(
                &presets,
                Country::Us,
                "CA",
                EcoFeeCategory::ArchitecturalPaint,
                volume,
                Decimal::ONE,
                on,
            ).unwrap();
            prop_assert_eq!(fee, one.mul_decimal(Decimal::from(qty)));
        }
    }

    #[test]
    fn pct_off_zero_is_list_and_hundred_is_zero(list in positive_money_decimal()) {
        let on = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let base_input = PricingInput {
            list_price: Money::new(list, Currency::Usd),
            cost: Money::new(list / Decimal::from(2), Currency::Usd),
            qty: Decimal::ONE,
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: None,
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            on,
        };
        let zero_off = PricingRule {
            id: Uuid::from_u128(1),
            priority: 1,
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: None,
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            min_qty: Decimal::ONE,
            calc_method: CalcMethod::PctOffList,
            calc_value: Decimal::ZERO,
            min_margin_floor: None,
            effective_from: None,
            effective_to: None,
        };
        prop_assert_eq!(
            evaluate_price(std::slice::from_ref(&zero_off), &base_input).unit_price,
            Money::new(list, Currency::Usd)
        );
        let mut hundred_off = zero_off.clone();
        hundred_off.calc_value = Decimal::from(100);
        prop_assert!(
            evaluate_price(std::slice::from_ref(&hundred_off), &base_input)
                .unit_price
                .is_zero()
        );
    }

    #[test]
    fn pricing_is_deterministic(list in positive_money_decimal(), qty in 1i64..100i64) {
        let on = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let input = PricingInput {
            list_price: Money::new(list, Currency::Usd),
            cost: Money::new(list / Decimal::from(2), Currency::Usd),
            qty: Decimal::from(qty),
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: None,
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            on,
        };
        let rule = PricingRule {
            id: Uuid::from_u128(9),
            priority: 1,
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: None,
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            min_qty: Decimal::ONE,
            calc_method: CalcMethod::MarkupCost,
            calc_value: Decimal::from_str("25").unwrap(),
            min_margin_floor: Some(Decimal::from_str("10").unwrap()),
            effective_from: None,
            effective_to: None,
        };
        let first = evaluate_price(std::slice::from_ref(&rule), &input);
        let second = evaluate_price(std::slice::from_ref(&rule), &input);
        prop_assert_eq!(first, second);
    }
}
