//! `df-hw` — store hardware for decor-pos (Phase 1, v0.2).
//!
//! ESC/POS receipts, ZPL labels, HID scanner parsing, and cash-drawer
//! control (spec §7). All byte generation and parsing in this crate is
//! pure and unit-testable with no printer, label printer, scanner, or
//! drawer attached — the only I/O is the [`Printer`] trait, implemented
//! in-memory by [`MockPrinter`] and by real drivers in `df-edge`.
//!
//! * [`receipt`] — [`ReceiptBuilder`] turns a [`ReceiptData`] into
//!   ESC/POS bytes for a 42-column thermal printer.
//! * [`label`] — ZPL II paint-can labels ([`can_label_zpl`]) and shelf
//!   tags ([`shelf_label_zpl`]).
//! * [`scanner`] — [`parse_scanner_input`] resolves a scanned barcode
//!   (with optional `qty*barcode` prefix) against a catalog.
//! * [`printer`] — the [`Printer`] trait, [`MockPrinter`],
//!   [`MockDrawer`], and [`drawer_kick_bytes`].
//!
//! Money is [`df_domain::Money`] throughout — zero floating-point, and
//! only the printed text is rounded to 2 decimals.

pub mod error;
pub mod label;
pub mod printer;
pub mod receipt;
pub mod scanner;

pub use error::HwError;
pub use label::{can_label_zpl, shelf_label_zpl};
pub use printer::{MockDrawer, MockPrinter, Printer, drawer_kick_bytes};
pub use receipt::{
    PaymentSummary, RECEIPT_WIDTH, ReceiptBuilder, ReceiptData, ReceiptLine, format_money,
};
pub use scanner::{BarcodeEntry, Scan, parse_quantity_prefix, parse_scanner_input};
