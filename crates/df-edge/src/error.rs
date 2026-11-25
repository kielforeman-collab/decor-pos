//! Edge errors.

use df_db::DbError;
use df_domain::DomainError;
use df_sync::SyncError;
use thiserror::Error;

use crate::hw::HwError;
use crate::payments::PaymentError;

#[derive(Debug, Error)]
pub enum EdgeError {
    #[error("database error: {0}")]
    Db(#[from] DbError),
    #[error("domain error: {0}")]
    Domain(#[from] DomainError),
    #[error("sync error: {0}")]
    Sync(#[from] SyncError),
    #[error("payment error: {0}")]
    Payment(#[from] PaymentError),
    #[error("hardware error: {0}")]
    Hw(#[from] HwError),
    #[error("tinter error: {0}")]
    Tinter(#[from] df_tinter::TinterError),
    #[error("report error: {0}")]
    Report(#[from] df_reports::ReportError),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("till error: {0}")]
    Till(String),
    #[error("payment declined: {0}")]
    PaymentDeclined(String),
    #[error("no payment provider configured: {0}")]
    NoPaymentProvider(String),
    #[error("no HQ sink configured: {0}")]
    NoHq(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
