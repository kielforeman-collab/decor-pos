//! Shared text encode/decode helpers for repositories.
//!
//! sqlx's `Any` driver implements `Encode`/`Decode` only for primitive
//! types — not for `Uuid`, `Decimal` or `DateTime`. So ids, money and
//! timestamps cross the driver boundary as text (see `repos.rs`):
//! UUIDs as hyphenated text, money as exact decimal strings,
//! timestamps as RFC 3339 text. Booleans cross natively on PostgreSQL
//! and as INTEGER 0/1 on SQLite.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::Row;
use sqlx::any::AnyRow;
use uuid::Uuid;

use crate::error::DbError;

pub(crate) fn uuid_str(id: Uuid) -> String {
    id.to_string()
}

pub(crate) fn opt_uuid_str(id: Option<Uuid>) -> Option<String> {
    id.map(|i| i.to_string())
}

pub(crate) fn dec_str(amount: Decimal) -> String {
    amount.to_string()
}

pub(crate) fn opt_dec_str(amount: Option<Decimal>) -> Option<String> {
    amount.map(|a| a.to_string())
}

pub(crate) fn dt_str(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

pub(crate) fn opt_dt_str(dt: Option<DateTime<Utc>>) -> Option<String> {
    dt.map(|d| d.to_rfc3339())
}

pub(crate) fn text_col(row: &AnyRow, name: &str) -> Result<String, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

pub(crate) fn opt_text_col(row: &AnyRow, name: &str) -> Result<Option<String>, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

pub(crate) fn uuid_col(row: &AnyRow, name: &str) -> Result<Uuid, DbError> {
    Uuid::parse_str(&text_col(row, name)?)
        .map_err(|e| DbError::InvalidData(format!("column {name} is not a UUID: {e}")))
}

pub(crate) fn opt_uuid_col(row: &AnyRow, name: &str) -> Result<Option<Uuid>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => Uuid::parse_str(&text)
            .map(Some)
            .map_err(|e| DbError::InvalidData(format!("column {name} is not a UUID: {e}"))),
    }
}

pub(crate) fn dec_col(row: &AnyRow, name: &str) -> Result<Decimal, DbError> {
    Decimal::from_str(&text_col(row, name)?)
        .map_err(|e| DbError::InvalidData(format!("column {name} is not a decimal: {e}")))
}

pub(crate) fn opt_dec_col(row: &AnyRow, name: &str) -> Result<Option<Decimal>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => Decimal::from_str(&text)
            .map(Some)
            .map_err(|e| DbError::InvalidData(format!("column {name} is not a decimal: {e}"))),
    }
}

pub(crate) fn dt_col(row: &AnyRow, name: &str) -> Result<DateTime<Utc>, DbError> {
    let text = text_col(row, name)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| DbError::InvalidData(format!("column {name} is not RFC 3339: {e}")))
}

pub(crate) fn opt_dt_col(row: &AnyRow, name: &str) -> Result<Option<DateTime<Utc>>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => DateTime::parse_from_rfc3339(&text)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|e| DbError::InvalidData(format!("column {name} is not RFC 3339: {e}"))),
    }
}

pub(crate) fn i32_col(row: &AnyRow, name: &str) -> Result<i32, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

pub(crate) fn i64_col(row: &AnyRow, name: &str) -> Result<i64, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

pub(crate) fn json_col(row: &AnyRow, name: &str) -> Result<serde_json::Value, DbError> {
    let text = text_col(row, name)?;
    serde_json::from_str(&text)
        .map_err(|e| DbError::InvalidData(format!("column {name} is not JSON: {e}")))
}

pub(crate) fn not_found(what: &str) -> impl Fn(sqlx::Error) -> DbError + '_ {
    move |e| match e {
        sqlx::Error::RowNotFound => DbError::NotFound(what.to_string()),
        other => DbError::Sqlx(other),
    }
}
