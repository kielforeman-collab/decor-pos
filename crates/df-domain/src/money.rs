//! Zero-floating-point money.
//!
//! Every monetary value is a [`Money`] newtype over `rust_decimal::Decimal`
//! at scale 4 (matching `NUMERIC(14,4)` in PostgreSQL, spec §5). Raw
//! single/double-precision floats never appear in this module — CI greps
//! for the float type names (see `.github/workflows/ci.yml`).

use crate::error::DomainError;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Supported currencies (spec §1: USD/CAD multi-currency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Currency {
    Usd,
    Cad,
}

impl Currency {
    pub fn code(self) -> &'static str {
        match self {
            Currency::Usd => "USD",
            Currency::Cad => "CAD",
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Currency {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "USD" => Ok(Currency::Usd),
            "CAD" => Ok(Currency::Cad),
            other => Err(DomainError::InvalidPrice(format!(
                "unsupported currency: {other}"
            ))),
        }
    }
}

/// Money at scale 4. Arithmetic is checked and currency-aware.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    amount: Decimal,
    currency: Currency,
}

impl Money {
    /// Create money, rescaling the amount to 4 decimal places
    /// (round-half-away-from-zero, matching `rust_decimal`'s default).
    pub fn new(amount: Decimal, currency: Currency) -> Self {
        let mut amount = amount;
        amount.rescale(4);
        Self { amount, currency }
    }

    /// Parse from a decimal string, e.g. `"12.3400"`.
    pub fn from_str_amount(s: &str, currency: Currency) -> Result<Self, DomainError> {
        let amount = Decimal::from_str(s)
            .map_err(|_| DomainError::InvalidPrice(format!("not a decimal: {s}")))?;
        Ok(Self::new(amount, currency))
    }

    /// Whole units, e.g. `Money::from_units(12, Currency::Usd)` is $12.0000.
    pub fn from_units(units: i64, currency: Currency) -> Self {
        Self::new(Decimal::from(units), currency)
    }

    /// Amount in minor units at scale 4 (i.e. amount * 10000).
    pub fn from_minor_units(minor: i64, currency: Currency) -> Self {
        Self::new(Decimal::from(minor) / Decimal::from(10_000), currency)
    }

    pub fn zero(currency: Currency) -> Self {
        Self::new(Decimal::ZERO, currency)
    }

    pub fn amount(self) -> Decimal {
        self.amount
    }

    pub fn currency(self) -> Currency {
        self.currency
    }

    pub fn is_zero(self) -> bool {
        self.amount.is_zero()
    }

    pub fn is_negative(self) -> bool {
        self.amount.is_sign_negative()
    }

    fn check_currency(self, other: Self) -> Result<(), DomainError> {
        if self.currency != other.currency {
            return Err(DomainError::CurrencyMismatch(
                self.currency.code().to_string(),
                other.currency.code().to_string(),
            ));
        }
        Ok(())
    }

    pub fn checked_add(self, other: Self) -> Result<Self, DomainError> {
        self.check_currency(other)?;
        Ok(Self::new(self.amount + other.amount, self.currency))
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, DomainError> {
        self.check_currency(other)?;
        Ok(Self::new(self.amount - other.amount, self.currency))
    }

    /// Multiply by an exact decimal factor (e.g. a quantity). Result is
    /// rounded to scale 4.
    pub fn mul_decimal(self, factor: Decimal) -> Self {
        Self::new(self.amount * factor, self.currency)
    }

    /// Divide by an exact decimal divisor.
    pub fn div_decimal(self, divisor: Decimal) -> Result<Self, DomainError> {
        if divisor.is_zero() {
            return Err(DomainError::DivisionByZero);
        }
        Ok(Self::new(self.amount / divisor, self.currency))
    }

    /// `self * (percent / 100)` — percent is a Decimal like `8.25` for 8.25%.
    pub fn percent_of(self, percent: Decimal) -> Result<Self, DomainError> {
        if percent < Decimal::from(-100) || percent > Decimal::from(10_000) {
            return Err(DomainError::InvalidPercentage(format!(
                "percent out of range: {percent}"
            )));
        }
        Ok(Self::new(
            self.amount * percent / Decimal::from(100),
            self.currency,
        ))
    }

    /// Gross margin percent of this (selling) price against a cost:
    /// `(price - cost) / price * 100`. `None` when the price is zero.
    pub fn margin_pct(self, cost: Self) -> Result<Option<Decimal>, DomainError> {
        self.check_currency(cost)?;
        if self.amount.is_zero() {
            return Ok(None);
        }
        let margin = (self.amount - cost.amount) / self.amount * Decimal::from(100);
        Ok(Some(margin))
    }

    /// Price that yields exactly `target_margin_pct` gross margin over `cost`:
    /// `cost / (1 - margin / 100)`.
    pub fn price_for_margin(cost: Self, target_margin_pct: Decimal) -> Result<Self, DomainError> {
        if target_margin_pct < Decimal::ZERO || target_margin_pct >= Decimal::from(100) {
            return Err(DomainError::InvalidPercentage(format!(
                "target margin must be in [0, 100): {target_margin_pct}"
            )));
        }
        let denom = Decimal::ONE - target_margin_pct / Decimal::from(100);
        Ok(Self::new(cost.amount / denom, cost.currency))
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.currency.code(), self.amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn decimal_arithmetic_has_no_float_drift() {
        // The classic 0.1 + 0.2 float trap: exact in Decimal.
        let a = Money::from_str_amount("0.1000", Currency::Usd).unwrap();
        let b = Money::from_str_amount("0.2000", Currency::Usd).unwrap();
        assert_eq!(
            a.checked_add(b).unwrap(),
            Money::from_str_amount("0.3000", Currency::Usd).unwrap()
        );
    }

    #[test]
    fn currency_mismatch_is_an_error() {
        let usd = Money::from_units(10, Currency::Usd);
        let cad = Money::from_units(10, Currency::Cad);
        assert_eq!(
            usd.checked_add(cad),
            Err(DomainError::CurrencyMismatch(
                "USD".to_string(),
                "CAD".to_string()
            ))
        );
    }

    #[test]
    fn percent_and_margin() {
        let price = Money::from_units(100, Currency::Usd);
        assert_eq!(
            price
                .percent_of(Decimal::from_str("8.25").unwrap())
                .unwrap(),
            Money::from_str_amount("8.2500", Currency::Usd).unwrap()
        );
        let cost = Money::from_units(60, Currency::Usd);
        assert_eq!(
            price.margin_pct(cost).unwrap(),
            Some(Decimal::from_str("40").unwrap())
        );
        assert_eq!(
            Money::price_for_margin(cost, Decimal::from_str("40").unwrap()).unwrap(),
            price
        );
    }
}
