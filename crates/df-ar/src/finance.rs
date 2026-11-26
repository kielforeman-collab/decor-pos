//! Finance charges (spec §4 `contractor_ar`).
//!
//! A finance charge is a simple monthly charge on the **overdue**
//! balance only (buckets 31-60 and older — never the current
//! bucket), at the account's `finance_rate_pct_monthly` when set,
//! else the rate the assessment run was given.
//!
//! Idempotency: one assessment per customer per statement period
//! (calendar month). The ledger entry carries
//! `reference_type = 'statement_run'` and a deterministic
//! `reference_id` from [`statement_period_key`]; re-running the
//! same period finds that entry and posts nothing.

use chrono::{Datelike, NaiveDate};
use df_domain::ar::finance_charge;
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ledger::{OpenCharge, is_overdue};

/// Calendar-month statement period containing `as_of`:
/// (first day, last day).
pub fn period_for(as_of: NaiveDate) -> (NaiveDate, NaiveDate) {
    let start =
        NaiveDate::from_ymd_opt(as_of.year(), as_of.month(), 1).expect("first of a valid month");
    let end = if as_of.month() == 12 {
        NaiveDate::from_ymd_opt(as_of.year() + 1, 1, 1).expect("Jan 1 of next year")
    } else {
        NaiveDate::from_ymd_opt(as_of.year(), as_of.month() + 1, 1).expect("first of next month")
    } - chrono::Duration::days(1);
    (start, end)
}

/// Deterministic idempotency key for one customer's assessment in
/// one statement period: a UUID derived (FNV-1a, two independent
/// 64-bit hashes) from the customer id and the period start. It is
/// **not** random — the same (customer, period) always yields the
/// same key, which is what makes a re-run a no-op. Stored as the
/// `finance_charge` entry's `reference_id`.
pub fn statement_period_key(customer_id: Uuid, period_start: NaiveDate) -> Uuid {
    fn fnv1a_64(bytes: &[u8], seed: u64) -> u64 {
        let mut hash = seed;
        for &b in bytes {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    let mut input = Vec::with_capacity(32);
    input.extend_from_slice(customer_id.as_bytes());
    input.extend_from_slice(period_start.to_string().as_bytes());
    let hi = fnv1a_64(&input, 0xcbf2_9ce4_8422_2325);
    let lo = fnv1a_64(&input, 0x8422_2325_cbf2_9ce4);
    Uuid::from_u128((u128::from(hi) << 64) | u128::from(lo))
}

/// Overdue open balance as of `as_of`: open charges more than 30
/// days past due. Charges without a due date never count (see
/// [`OpenCharge`]).
pub fn overdue_balance(open: &[OpenCharge], as_of: NaiveDate, currency: Currency) -> Money {
    open.iter()
        .filter(|c| is_overdue(c, as_of))
        .map(|c| Money::new(c.open_amount, currency))
        .fold(Money::zero(currency), |acc, m| {
            acc.checked_add(m).unwrap_or(acc)
        })
}

/// Simple monthly finance charge on an overdue balance, reusing the
/// Phase 0 math ([`finance_charge`] with one month).
pub fn finance_charge_for(overdue: Money, monthly_rate_pct: Decimal) -> Money {
    finance_charge(overdue, monthly_rate_pct, 1)
}

/// One finance charge actually posted by an assessment run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinanceChargePosted {
    pub customer_id: Uuid,
    pub entry_id: Uuid,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub overdue_balance: Money,
    pub rate_pct: Decimal,
    pub amount: Money,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono::Utc;

    fn charge(id: u128, amount: &str, due: NaiveDate) -> OpenCharge {
        OpenCharge {
            id: Uuid::from_u128(id),
            customer_id: Uuid::from_u128(1),
            job_id: None,
            due_date: due,
            created_at: Utc.with_ymd_and_hms(2026, 8, 15, 0, 0, 0).unwrap(),
            open_amount: Decimal::from_str_exact(amount).unwrap(),
        }
    }

    #[test]
    fn period_for_is_calendar_month() {
        assert_eq!(
            period_for(NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()),
            (
                NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()
            )
        );
        assert_eq!(
            period_for(NaiveDate::from_ymd_opt(2026, 2, 10).unwrap()),
            (
                NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 2, 28).unwrap()
            )
        );
    }

    #[test]
    fn period_key_is_deterministic_and_scoped() {
        let key_a = statement_period_key(
            Uuid::from_u128(1),
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        );
        let key_b = statement_period_key(
            Uuid::from_u128(1),
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
        );
        assert_eq!(key_a, key_b);
        assert_ne!(
            key_a,
            statement_period_key(
                Uuid::from_u128(1),
                NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
            )
        );
        assert_ne!(
            key_a,
            statement_period_key(
                Uuid::from_u128(2),
                NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()
            )
        );
    }

    #[test]
    fn overdue_balance_counts_only_past_30_days() {
        let as_of = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let open = vec![
            charge(
                10,
                "250.0000",
                NaiveDate::from_ymd_opt(2026, 9, 14).unwrap(),
            ), // 47d overdue
            charge(
                11,
                "300.0000",
                NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
            ), // not due
            charge(
                12,
                "100.0000",
                NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            ), // 30d: current
        ];
        assert_eq!(
            overdue_balance(&open, as_of, Currency::Usd),
            Money::from_str_amount("250.0000", Currency::Usd).unwrap()
        );
        assert_eq!(
            finance_charge_for(
                Money::from_str_amount("250.0000", Currency::Usd).unwrap(),
                Decimal::from_str_exact("1.5").unwrap()
            ),
            Money::from_str_amount("3.7500", Currency::Usd).unwrap()
        );
    }
}
