//! Hardware shapes: receipt printer + cash drawer (Phase 1, v0.2).
//!
//! Re-exports of the real [`df_hw`] crate: the placeholder copies
//! that used to live here were deleted in the Phase 1 integration and
//! `df-edge` now depends on `df-hw` directly. Receipts render through
//! `df-hw`'s ESC/POS [`ReceiptBuilder`]; receipt payments are
//! [`PaymentSummary`] values (tender label + amount — `df-hw` does
//! not depend on `df-payments`, so callers map a [`PaymentResult`]
//! into a summary; see `payment_summary` in [`crate::edge`]).

pub use df_hw::{
    HwError, MockDrawer, MockPrinter, PaymentSummary, Printer, ReceiptBuilder, ReceiptData,
    ReceiptLine, drawer_kick_bytes,
};
pub use df_hw::{format_money, parse_scanner_input};
