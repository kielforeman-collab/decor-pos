//! Charge accounts and the credit check (spec §5.4 `customers`).
//!
//! A [`ChargeAccount`] is the credit-control view of a customer:
//! whether charging is enabled at all, the credit limit, the
//! payment terms (which set the due date of a new charge), the
//! monthly finance-charge rate and the credit-hold flag.
//!
//! The credit check is deliberately strict: a charge is rejected
//! when the account is not charge-enabled, when it is on credit
//! hold, or when the resulting balance would reach or exceed the
//! credit limit. A zero limit means no credit extended.

use chrono::{Datelike, NaiveDate};
use df_domain::{Currency, DomainError, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Payment terms as stored in `customers.payment_terms`
/// (`'COD'`, `'NET15'`, `'NET30'`, `'EOM'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Terms {
    /// Cash on delivery: due the day of the sale.
    Cod,
    /// Due N days after the sale.
    Net(u32),
    /// Due on the last day of the sale's calendar month.
    Eom,
}

/// Parse a `payment_terms` string. Case-insensitive; `NET<n>` takes
/// the number, anything unrecognised falls back to [`Terms::Net`]
/// (30) so a bad terms string never blocks a sale at the counter —
/// the fallback is documented here, not silent elsewhere.
pub fn parse_terms(s: &str) -> Terms {
    let upper = s.trim().to_ascii_uppercase();
    if upper == "COD" {
        return Terms::Cod;
    }
    if upper == "EOM" {
        return Terms::Eom;
    }
    if let Some(days) = upper.strip_prefix("NET")
        && let Ok(n) = days.parse::<u32>()
    {
        return Terms::Net(n);
    }
    Terms::Net(30)
}

impl Terms {
    /// Due date for a sale on `txn_date`.
    pub fn due_date(self, txn_date: NaiveDate) -> NaiveDate {
        match self {
            Terms::Cod => txn_date,
            Terms::Net(days) => txn_date + chrono::Duration::days(i64::from(days)),
            Terms::Eom => {
                let (year, month) = (txn_date.year(), txn_date.month());
                let (next_year, next_month) = if month == 12 {
                    (year + 1, 1)
                } else {
                    (year, month + 1)
                };
                NaiveDate::from_ymd_opt(next_year, next_month, 1).expect("first of a valid month")
                    - chrono::Duration::days(1)
            }
        }
    }
}

/// Why a charge was refused. The messages are written for the
/// counter: they name the account, the limit and the balance so the
/// operator can act on them without looking anything up.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CreditError {
    #[error("account {account_number} is not charge-enabled")]
    NotChargeEnabled { account_number: String },
    #[error("account {account_number} is on credit hold")]
    OnCreditHold { account_number: String },
    #[error(
        "account {account_number} is at/over its credit limit: open balance {open_balance} + new charge {new_charge} reaches the limit {limit}"
    )]
    OverLimit {
        account_number: String,
        limit: Decimal,
        open_balance: Decimal,
        new_charge: Decimal,
    },
    #[error("charge amount must be positive: {0}")]
    InvalidAmount(Decimal),
    #[error("currency mismatch: {0}")]
    CurrencyMismatch(#[from] DomainError),
}

/// A contractor charge account: the credit-control view of a
/// customer row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChargeAccount {
    pub id: Uuid,
    pub org_id: Uuid,
    pub account_number: String,
    pub charge_enabled: bool,
    pub credit_limit: Money,
    /// Parsed payment terms (from `customers.payment_terms`).
    pub terms: Terms,
    /// Explicit `terms_days` column override; when set it wins over
    /// the parsed `terms`.
    pub terms_days: Option<u32>,
    /// Monthly finance-charge rate in percent (e.g. `1.5` for
    /// 1.5%/month), from `customers.finance_rate_pct_monthly`.
    /// `None` means "use the rate the assessment run was given".
    pub finance_rate_pct_monthly: Option<Decimal>,
    /// `customers.credit_hold`.
    pub on_credit_hold: bool,
}

impl ChargeAccount {
    /// Effective terms in days. An explicit `terms_days` wins; else
    /// `Net(n)` gives `n`, `Cod` gives 0, `Eom` gives the days
    /// remaining in the sale month (so callers that need an exact
    /// due date should use [`ChargeAccount::due_date`] instead).
    pub fn terms_days(&self) -> u32 {
        if let Some(days) = self.terms_days {
            return days;
        }
        match self.terms {
            Terms::Cod => 0,
            Terms::Net(n) => n,
            Terms::Eom => 30,
        }
    }

    /// Due date for a charge rung up on `txn_date`: exact for
    /// `Cod` / `Net` / `Eom`; the explicit `terms_days` override
    /// behaves as `Net(days)`.
    pub fn due_date(&self, txn_date: NaiveDate) -> NaiveDate {
        if let Some(days) = self.terms_days {
            return txn_date + chrono::Duration::days(i64::from(days));
        }
        self.terms.due_date(txn_date)
    }

    /// Available credit: `limit - open_balance`. Negative when the
    /// account is already over its limit.
    pub fn available_credit(&self, open_balance: Money) -> Result<Money, CreditError> {
        Ok(self.credit_limit.checked_sub(open_balance)?)
    }

    /// The credit check. Rejects when the account is not
    /// charge-enabled, is on credit hold, or when
    /// `open_balance + new_charge` would reach or exceed the credit
    /// limit. A zero charge passes (nothing to extend); a negative
    /// charge is rejected as invalid input.
    pub fn check_charge(&self, open_balance: Money, new_charge: Money) -> Result<(), CreditError> {
        if !self.charge_enabled {
            return Err(CreditError::NotChargeEnabled {
                account_number: self.account_number.clone(),
            });
        }
        if self.on_credit_hold {
            return Err(CreditError::OnCreditHold {
                account_number: self.account_number.clone(),
            });
        }
        if new_charge.is_negative() {
            return Err(CreditError::InvalidAmount(new_charge.amount()));
        }
        if new_charge.is_zero() {
            return Ok(());
        }
        let resulting = open_balance.checked_add(new_charge)?;
        if resulting.amount() >= self.credit_limit.amount() {
            return Err(CreditError::OverLimit {
                account_number: self.account_number.clone(),
                limit: self.credit_limit.amount(),
                open_balance: open_balance.amount(),
                new_charge: new_charge.amount(),
            });
        }
        Ok(())
    }
}

/// The raw customer-row fields [`account_from_row`] needs, kept
/// as one struct so the constructor stays under clippy's
/// argument-count lint.
pub struct AccountRow<'a> {
    pub id: Uuid,
    pub org_id: Uuid,
    pub account_number: &'a str,
    pub charge_enabled: bool,
    pub credit_limit: Decimal,
    pub payment_terms: &'a str,
    pub terms_days: Option<i32>,
    pub finance_rate_pct_monthly: Option<Decimal>,
    pub on_credit_hold: bool,
}

/// Build a [`ChargeAccount`] from the raw customer-row fields, so
/// callers that own the row type (`df-db`) do not duplicate the
/// parsing. `currency` is the store currency.
pub fn account_from_row(row: AccountRow<'_>, currency: Currency) -> ChargeAccount {
    ChargeAccount {
        id: row.id,
        org_id: row.org_id,
        account_number: row.account_number.to_string(),
        charge_enabled: row.charge_enabled,
        credit_limit: Money::new(row.credit_limit, currency),
        terms: parse_terms(row.payment_terms),
        terms_days: row.terms_days.map(|d| u32::try_from(d).unwrap_or(0)),
        finance_rate_pct_monthly: row.finance_rate_pct_monthly,
        on_credit_hold: row.on_credit_hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::Currency;

    fn account(limit: &str) -> ChargeAccount {
        ChargeAccount {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            account_number: "C-1".to_string(),
            charge_enabled: true,
            credit_limit: Money::from_str_amount(limit, Currency::Usd).unwrap(),
            terms: Terms::Net(30),
            terms_days: None,
            finance_rate_pct_monthly: None,
            on_credit_hold: false,
        }
    }

    fn money(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    #[test]
    fn terms_parse() {
        assert_eq!(parse_terms("NET30"), Terms::Net(30));
        assert_eq!(parse_terms("net15"), Terms::Net(15));
        assert_eq!(parse_terms("COD"), Terms::Cod);
        assert_eq!(parse_terms("EOM"), Terms::Eom);
        assert_eq!(parse_terms("garbage"), Terms::Net(30));
    }

    #[test]
    fn due_dates() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        assert_eq!(
            Terms::Net(30).due_date(date),
            NaiveDate::from_ymd_opt(2026, 11, 4).unwrap()
        );
        assert_eq!(Terms::Cod.due_date(date), date);
        assert_eq!(
            Terms::Eom.due_date(date),
            NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()
        );
        assert_eq!(
            Terms::Eom.due_date(NaiveDate::from_ymd_opt(2026, 12, 15).unwrap()),
            NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()
        );
    }

    #[test]
    fn charge_blocked_at_or_over_limit() {
        let acc = account("1000");
        // Open 900 + new 100 reaches the limit exactly: blocked.
        assert!(matches!(
            acc.check_charge(money("900"), money("100")),
            Err(CreditError::OverLimit { .. })
        ));
        // Open 900 + new 99.9999 stays under: allowed.
        assert!(acc.check_charge(money("900"), money("99.9999")).is_ok());
        // Zero limit: any positive charge is blocked.
        assert!(matches!(
            account("0").check_charge(money("0"), money("0.0001")),
            Err(CreditError::OverLimit { .. })
        ));
    }

    #[test]
    fn charge_blocked_on_hold_and_when_not_enabled() {
        let mut acc = account("1000");
        acc.on_credit_hold = true;
        assert!(matches!(
            acc.check_charge(money("0"), money("1")),
            Err(CreditError::OnCreditHold { .. })
        ));
        acc.on_credit_hold = false;
        acc.charge_enabled = false;
        assert!(matches!(
            acc.check_charge(money("0"), money("1")),
            Err(CreditError::NotChargeEnabled { .. })
        ));
    }

    #[test]
    fn available_credit_goes_negative_over_limit() {
        let acc = account("1000");
        assert_eq!(acc.available_credit(money("1200")).unwrap(), money("-200"));
    }

    #[test]
    fn explicit_terms_days_wins() {
        let mut acc = account("1000");
        acc.terms_days = Some(15);
        let date = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        assert_eq!(
            acc.due_date(date),
            NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()
        );
        assert_eq!(acc.terms_days(), 15);
    }
}
