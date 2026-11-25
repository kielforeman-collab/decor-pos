//! Hardware errors for store peripherals (spec §7, Phase 1).
//!
//! All errors are plain values — no hardware is touched while producing
//! them; byte generation and scanner parsing in this crate are pure.

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HwError {
    #[error("unknown barcode: {0}")]
    UnknownBarcode(String),
    #[error("invalid scanner input: {0}")]
    InvalidInput(String),
    #[error("printer error: {0}")]
    Printer(String),
    #[error("cash drawer error: {0}")]
    Drawer(String),
}
