//! Contractor AR aging and finance charges (spec §4 `contractor_ar`).
//!
//! Balances are derived from the append-only `ar_ledger`; this module is
//! the pure math: bucket an entry by days past due, and compute a simple
//! monthly finance charge. All amounts are [`Money`].

use crate::money::{Currency, Money};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An AR ledger entry that is still open (unpaid balance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArEntry {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub job_id: Option<Uuid>,
    /// Amount still owed (positive). Payments reduce it before it gets here.
    pub balance: Money,
    pub due_date: NaiveDate,
}

/// Aging buckets: 30 / 60 / 90 / 120+ (spec §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgingBucket {
    Current,
    Days31_60,
    Days61_90,
    Days91_120,
    Over120,
}

impl AgingBucket {
    pub fn label(self) -> &'static str {
        match self {
            AgingBucket::Current => "current",
            AgingBucket::Days31_60 => "31-60",
            AgingBucket::Days61_90 => "61-90",
            AgingBucket::Days91_120 => "91-120",
            AgingBucket::Over120 => "120+",
        }
    }
}

/// Bucket an entry as of `as_of`. Not-yet-due entries are `Current`.
pub fn bucket_for(due_date: NaiveDate, as_of: NaiveDate) -> AgingBucket {
    let days_past_due = (as_of - due_date).num_days();
    if days_past_due <= 30 {
        AgingBucket::Current
    } else if days_past_due <= 60 {
        AgingBucket::Days31_60
    } else if days_past_due <= 90 {
        AgingBucket::Days61_90
    } else if days_past_due <= 120 {
        AgingBucket::Days91_120
    } else {
        AgingBucket::Over120
    }
}

/// Totals per bucket, in the order
/// [current, 31-60, 61-90, 91-120, 120+].
/// All entries must share a currency; mixed currencies are summed per
/// entry's own currency only when they match `currency`, others are
/// skipped (callers should age one currency at a time).
pub fn aging_totals(entries: &[ArEntry], as_of: NaiveDate, currency: Currency) -> [Money; 5] {
    let mut totals = [
        Money::zero(currency),
        Money::zero(currency),
        Money::zero(currency),
        Money::zero(currency),
        Money::zero(currency),
    ];
    for entry in entries {
        if entry.balance.currency() != currency {
            continue;
        }
        let idx = match bucket_for(entry.due_date, as_of) {
            AgingBucket::Current => 0,
            AgingBucket::Days31_60 => 1,
            AgingBucket::Days61_90 => 2,
            AgingBucket::Days91_120 => 3,
            AgingBucket::Over120 => 4,
        };
        totals[idx] = totals[idx]
            .checked_add(entry.balance)
            .expect("same currency, addition cannot fail");
    }
    totals
}

/// Simple monthly finance charge: `balance * (monthly_rate_pct / 100) *
/// months`. `monthly_rate_pct` is a percent, e.g. `1.5` for 1.5%/month.
pub fn finance_charge(balance: Money, monthly_rate_pct: Decimal, months: u32) -> Money {
    balance.mul_decimal(monthly_rate_pct / Decimal::from(100) * Decimal::from(months))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Currency;

    fn entry(due: &str, amount: &str) -> ArEntry {
        ArEntry {
            id: Uuid::now_v7(),
            customer_id: Uuid::now_v7(),
            job_id: None,
            balance: Money::from_str_amount(amount, Currency::Usd).unwrap(),
            due_date: NaiveDate::parse_from_str(due, "%Y-%m-%d").unwrap(),
        }
    }

    #[test]
    fn aging_buckets_are_correct() {
        let as_of = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        assert_eq!(
            bucket_for(NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(), as_of),
            AgingBucket::Current
        );
        assert_eq!(
            bucket_for(NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(), as_of),
            AgingBucket::Current
        ); // 30 days
        assert_eq!(
            bucket_for(NaiveDate::from_ymd_opt(2026, 10, 25).unwrap(), as_of),
            AgingBucket::Days31_60
        ); // 31 days
        assert_eq!(
            bucket_for(NaiveDate::from_ymd_opt(2026, 8, 27).unwrap(), as_of),
            AgingBucket::Days61_90
        ); // 90 days
        assert_eq!(
            bucket_for(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(), as_of),
            AgingBucket::Over120
        );
    }

    #[test]
    fn aging_totals_sum_per_bucket() {
        let as_of = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let entries = vec![
            entry("2026-11-01", "100.0000"),
            entry("2026-10-01", "50.0000"),
            entry("2026-06-01", "25.0000"),
        ];
        let totals = aging_totals(&entries, as_of, Currency::Usd);
        assert_eq!(
            totals[0],
            Money::from_str_amount("100.0000", Currency::Usd).unwrap()
        );
        assert_eq!(
            totals[1],
            Money::from_str_amount("50.0000", Currency::Usd).unwrap()
        );
        assert_eq!(
            totals[4],
            Money::from_str_amount("25.0000", Currency::Usd).unwrap()
        );
        assert!(totals[2].is_zero() && totals[3].is_zero());
    }

    #[test]
    fn finance_charge_is_simple_monthly() {
        let balance = Money::from_units(1000, Currency::Usd);
        let charge = finance_charge(balance, Decimal::from_str_exact("1.5").unwrap(), 2);
        assert_eq!(charge, Money::from_units(30, Currency::Usd));
    }
}
