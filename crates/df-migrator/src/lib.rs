//! `df-migrator` — legacy Decor Fusion CSV import for decor-pos
//! (Phase 6, v1.0; spec §7 Phase 6 row).
//!
//! Imports customers, products, tint formulas (tracking records
//! only), AR balances and inventory on-hand from legacy CSV
//! exports, with a reconciliation manifest for every run: every
//! row is counted and checksummed (SHA-256 over canonical row
//! bytes), and the manifest reports rows read / imported /
//! skipped / failed with reasons plus per-file checksums.
//! Mismatches are reported, never silently dropped, and a
//! re-import of the same files is a no-op (deterministic ids
//! deduplicate on the natural keys).
//!
//! Local only: reads CSV files and writes to the database the
//! caller opens. No network, no vendor API, no payment API.
//! Tinting is formula tracking only — the import stores formulas
//! and their colorant lists; it never drives a tint machine and
//! never touches colorant inventory. Benjamin Moore COLORx is
//! deferred post-v1.0 (spec §9) and appears nowhere in this crate.
//!
//! ## Use
//!
//! As a library: [`import::import_directory`] (or
//! [`import::import_file`] for one file) against a pool the
//! caller opened and migrated. As a binary: `df-migrator import
//! --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --dir
//! ./legacy --out manifest.json` (also wired into `df-server
//! migrate import`). Exit code is 1 when any row or file failed;
//! the manifest is still written, so the failures are visible.
//!
//! Sample legacy files live under `fixtures/` and are exercised
//! by `tests/fixtures_import.rs`, which asserts exact counts and
//! checksums.

pub mod csv;
pub mod import;
pub mod manifest;

pub use csv::{CsvError, parse_csv};
pub use import::{MigratorError, import_directory, import_file};
pub use manifest::{
    FileManifest, ImportManifest, RowOutcome, RowStatus, row_checksum, sha256_hex, uuid_from_seed,
};
