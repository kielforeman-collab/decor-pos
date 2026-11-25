//! Domain errors. All money paths use [`rust_decimal::Decimal`]; there is
//! no floating-point arithmetic anywhere in this crate.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("currency mismatch: {0} vs {1}")]
    CurrencyMismatch(String, String),
    #[error("invalid quantity: {0}")]
    InvalidQuantity(String),
    #[error("invalid price: {0}")]
    InvalidPrice(String),
    #[error("invalid UOM conversion: {0}")]
    InvalidUomConversion(String),
    #[error("invalid percentage: {0}")]
    InvalidPercentage(String),
    #[error("division by zero")]
    DivisionByZero,
    #[error("invalid date range: {0}")]
    InvalidDateRange(String),
}
