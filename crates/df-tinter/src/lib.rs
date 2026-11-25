//! `df-tinter` — paint tinting (Phase 2, v0.3; spec §6.2, §7).
//!
//! * [`TinterPlugin`] — the tinter trait from spec §6.2:
//!   `list_dispensers()`, `send_formula(DispenseRequest) -> JobAck`,
//!   plus job lookup and `drain_completed()` as the in-process
//!   equivalent of `stream_dispensed_formulas()`.
//! * [`VirtualTinter`] — deterministic in-memory simulator: dispenses
//!   compute each colorant's shot volumes with
//!   `df-domain::TintColorant::total_fl_oz`, decrement on-hand levels
//!   exactly, and fail atomically (nothing decremented) on a short or
//!   unknown colorant. No randomness, no wall-clock dependence in the
//!   dispense math.
//! * [`TintJob`] — job lifecycle `Queued -> Dispensing -> Done /
//!   Failed`, carrying the sale/transaction line reference so a tinted
//!   sale line traces to its dispense record.
//! * [`catalog`] — color lookup by name/code (case-insensitive
//!   substring, scoped by manufacturer) and per-customer colour
//!   history, newest first (optionally for one job).
//! * [`label`] — ZPL paint-can label with formula shots, extending
//!   `df-hw::can_label_zpl`.
//! * [`seed`] — demo catalog/colorant data for tests only.
//!
//! Benjamin Moore COLORx integration is deferred post-v1.0 (spec §9):
//! there is no COLORx code, protocol, or certification work in this
//! crate — only catalog *data* (color names/codes) in [`seed`].
//!
//! Money and colorant volumes are `rust_decimal::Decimal` throughout
//! — never floats.

pub mod catalog;
pub mod error;
pub mod job;
pub mod label;
pub mod plugin;
pub mod seed;
pub mod virtual_tinter;

pub use catalog::{CatalogColor, FormulaHistory, StoredFormula, search_colors};
pub use error::TinterError;
pub use job::{
    ColorantLevel, DispenseRequest, DispensedFormula, DispensedShot, JobAck, TintJob, TintJobStatus,
};
pub use label::{TintCanLabel, tint_can_label_zpl};
pub use plugin::TinterPlugin;
pub use seed::{SeedColorant, seed_catalog, seed_colorants};
pub use virtual_tinter::VirtualTinter;
