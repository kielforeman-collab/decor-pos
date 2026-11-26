//! `df-accounting` — file-based accounting exports (Phase 3, v0.4;
//! spec §4 `contractor_ar`, §6.2 `AccountingPlugin`, §7).
//!
//! * [`csv`] — CSV export of the AR ledger and the inventory ledger
//!   (header row, one row per entry, exact decimal amounts,
//!   deterministic ordering) with round-trip parsers;
//! * [`iif`] — QuickBooks IIF export (`!TRNS` / `!SPLNS` /
//!   `!ENDTRNS`, balanced transactions) with a validating parser;
//! * [`plugin`] — the [`AccountingPlugin`] trait and its file-based
//!   adapters [`CsvAccountingPlugin`] and [`IifAccountingPlugin`]
//!   (journal, customers, ledgers, payment import);
//! * [`load`] — load ledger entries from the database for export.
//!
//! **File-based only.** No live QuickBooks/Xero API calls, no
//! credentials, no network. Live sync (`push_daily_journal`,
//! `sync_customers`, `sync_ar_payments`, `health_check` in spec
//! §6.2) is a **Phase 4+ / vendor-pack** concern: it needs OAuth
//! credentials and a vendor plugin, and does not belong in the core
//! crate. The trait here is the seam a vendor pack implements
//! against — it works on the same ledger data, so a live plugin can
//! reuse the export shapes.
//!
//! All money is `rust_decimal::Decimal` — never floats.

pub mod csv;
pub mod error;
pub mod iif;
pub mod load;
pub mod plugin;

pub use csv::{
    export_ar_ledger_csv, export_inventory_ledger_csv, parse_ar_ledger_csv,
    parse_inventory_ledger_csv,
};
pub use error::AccountingError;
pub use iif::{
    IifAccounts, IifSplit, IifTransaction, export_ar_ledger_iif, export_inventory_ledger_iif,
    parse_iif_transactions,
};
pub use load::{load_ar_entries_for_store, load_inventory_entries_for_store};
pub use plugin::{
    AccountingPlugin, CsvAccountingPlugin, ExportSummary, IifAccountingPlugin, ImportedPayment,
};
