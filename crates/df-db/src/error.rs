//! Database errors.

use rust_decimal::Decimal;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("invalid data in row: {0}")]
    InvalidData(String),
    #[error("row not found: {0}")]
    NotFound(String),
    #[error(
        "insufficient colorant {colorant_id} at store {store_id}: need {needed} fl oz, have {on_hand} fl oz"
    )]
    InsufficientColorant {
        colorant_id: Uuid,
        store_id: Uuid,
        needed: Decimal,
        on_hand: Decimal,
    },
}
