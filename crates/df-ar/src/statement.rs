//! Statement batch data model (spec §7 Phase 3: monthly Typst PDF
//! statements — this crate is the **data**; another worker renders
//! the PDF from [`Statement`]).
//!
//! One statement per charge account per period:
//! opening balance, charges, payments, finance charges, closing
//! balance and the aging summary as of the period end. The identity
//!
//! ```text
//! opening + charges - payments + finance charges == closing
//! ```
//!
//! must hold exactly (see [`Statement::check_identity`]). Credit
//! memos applied in the period count inside `payments_total`
//! (they reduce the balance like a payment); the statement lines
//! keep the original entry types so the PDF can show them.

use chrono::NaiveDate;
use df_domain::ar::aging_totals;
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ledger::{OpenCharge, aging_entries};

/// Aging totals as of a date, in the order
/// [current, 31-60, 61-90, 91-120, 120+].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AgingSummary {
    pub current: Money,
    pub days_31_60: Money,
    pub days_61_90: Money,
    pub days_91_120: Money,
    pub over_120: Money,
}

impl AgingSummary {
    pub fn zero(currency: Currency) -> Self {
        Self {
            current: Money::zero(currency),
            days_31_60: Money::zero(currency),
            days_61_90: Money::zero(currency),
            days_91_120: Money::zero(currency),
            over_120: Money::zero(currency),
        }
    }

    /// From open charges as of `as_of`, reusing the Phase 0
    /// [`aging_totals`] math.
    pub fn from_open(open: &[OpenCharge], as_of: NaiveDate, currency: Currency) -> Self {
        let totals = aging_totals(&aging_entries(open, currency), as_of, currency);
        Self {
            current: totals[0],
            days_31_60: totals[1],
            days_61_90: totals[2],
            days_91_120: totals[3],
            over_120: totals[4],
        }
    }

    /// Total across all buckets (== the open balance).
    pub fn total(&self) -> Result<Money, df_domain::DomainError> {
        self.current
            .checked_add(self.days_31_60)?
            .checked_add(self.days_61_90)?
            .checked_add(self.days_91_120)?
            .checked_add(self.over_120)
    }

    /// Buckets as `[Money; 5]` in the canonical order.
    pub fn as_array(&self) -> [Money; 5] {
        [
            self.current,
            self.days_31_60,
            self.days_61_90,
            self.days_91_120,
            self.over_120,
        ]
    }
}

/// One ledger entry inside the statement period, for the PDF
/// line-item table. `amount` keeps its ledger sign (charges
/// positive, payments negative).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementLine {
    pub entry_id: Uuid,
    pub entry_date: NaiveDate,
    pub entry_type: String,
    pub reference: String,
    pub amount: Money,
    pub due_date: Option<NaiveDate>,
}

/// One charge account's statement for one period. Pure data — the
/// PDF worker consumes this struct directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub account_number: String,
    pub currency: Currency,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub opening_balance: Money,
    /// Charges posted in the period (positive magnitude).
    pub charges_total: Money,
    /// Payments **and credit memos** in the period (positive
    /// magnitude — subtracted in the identity).
    pub payments_total: Money,
    /// Finance charges posted in the period (positive magnitude).
    pub finance_charge_total: Money,
    pub closing_balance: Money,
    /// Aging as of `period_end`.
    pub aging: AgingSummary,
    /// Ledger entries dated inside the period, oldest first.
    pub lines: Vec<StatementLine>,
}

impl Statement {
    /// Exact identity check: opening + charges − payments +
    /// finance charges == closing. Returns the computed closing
    /// balance when the identity fails, so a caller can report
    /// both numbers.
    pub fn check_identity(&self) -> Result<(), Decimal> {
        let computed = self
            .opening_balance
            .amount()
            .checked_add(self.charges_total.amount())
            .and_then(|v| v.checked_sub(self.payments_total.amount()))
            .and_then(|v| v.checked_add(self.finance_charge_total.amount()));
        match computed {
            Some(value) if value == self.closing_balance.amount() => Ok(()),
            Some(value) => Err(value),
            None => Err(Decimal::ZERO),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono::Utc;

    fn money(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    #[test]
    fn identity_holds_and_fails_loudly() {
        let stmt = Statement {
            id: Uuid::now_v7(),
            customer_id: Uuid::now_v7(),
            account_number: "C-1".to_string(),
            currency: Currency::Usd,
            period_start: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
            opening_balance: money("400"),
            charges_total: money("300"),
            payments_total: money("150"),
            finance_charge_total: money("3.75"),
            closing_balance: money("553.75"),
            aging: AgingSummary::zero(Currency::Usd),
            lines: vec![],
        };
        assert!(stmt.check_identity().is_ok());
        let mut broken = stmt.clone();
        broken.closing_balance = money("553.74");
        assert_eq!(
            broken.check_identity(),
            Err(Decimal::from_str_exact("553.75").unwrap())
        );
    }

    #[test]
    fn aging_summary_matches_open_charges() {
        let as_of = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let open = vec![
            OpenCharge {
                id: Uuid::now_v7(),
                customer_id: Uuid::now_v7(),
                job_id: None,
                due_date: NaiveDate::from_ymd_opt(2026, 9, 14).unwrap(),
                created_at: Utc.with_ymd_and_hms(2026, 8, 15, 0, 0, 0).unwrap(),
                open_amount: Decimal::from_str_exact("250.0000").unwrap(),
            },
            OpenCharge {
                id: Uuid::now_v7(),
                customer_id: Uuid::now_v7(),
                job_id: None,
                due_date: NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
                created_at: Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0).unwrap(),
                open_amount: Decimal::from_str_exact("300.0000").unwrap(),
            },
        ];
        let aging = AgingSummary::from_open(&open, as_of, Currency::Usd);
        assert_eq!(aging.current, money("300"));
        assert_eq!(aging.days_31_60, money("250"));
        assert_eq!(aging.total().unwrap(), money("550"));
    }
}
