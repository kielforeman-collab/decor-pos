//! Errors for `df-edi`.

use thiserror::Error;

/// Errors from EDI document building, parsing, catalog import and repos.
#[derive(Debug, Error)]
pub enum EdiError {
    /// A document is not well-formed X12 / CSV, or a required segment is
    /// missing or has the wrong tag.
    #[error("EDI parse error: {0}")]
    Parse(String),

    /// A required X12 segment (e.g. `BEG`, `BSN`, `BIG`) is absent.
    #[error("missing required segment: {0}")]
    MissingSegment(&'static str),

    /// A quantity / price field is not a valid decimal.
    #[error("invalid decimal {0:?}: {1}")]
    InvalidDecimal(String, #[source] rust_decimal::Error),

    /// A date field is not a valid X12 date (`CCYYMMDD`).
    #[error("invalid date {0:?}: {1}")]
    InvalidDate(String, #[source] chrono::ParseError),

    /// A database error from the `df-db` layer.
    #[error("database error: {0}")]
    Db(#[from] df_db::DbError),

    /// A raw sqlx error (repo queries).
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// JSON (de)serialization of a parsed EDI document failed.
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// A capability that is honestly not implemented in this phase
    /// (e.g. AS2 signing/encryption — real crypto is out of scope
    /// for this VM build). Never used to fake a capability.
    #[error("not supported: {0}")]
    NotSupported(String),

    /// A file-transport (drop-folder / SFTP stand-in) I/O failure.
    #[error("transport I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience result type for this crate.
pub type Result<T> = std::result::Result<T, EdiError>;
