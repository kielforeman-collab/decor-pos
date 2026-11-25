//! `TinterPlugin` trait (spec §6.2).
//!
//! The trait is synchronous and deterministic: `send_formula` either
//! dispenses (for the virtual simulator, immediately) or fails before
//! anything is decremented. Async / out-of-process adapters
//! (file-drop, TCP/serial bridges, vendor plugins — including the
//! deferred Benjamin Moore COLORx plugin, spec §9) wrap this trait
//! behind `df-edge` / `df-plugin-sdk` in a later phase; they are not
//! part of this crate.
//!
//! `drain_completed` is the in-process equivalent of spec §6.2
//! `stream_dispensed_formulas()`: it returns the dispenses completed
//! since the last drain, in dispense order, so a caller can forward
//! them to the sale / sync outbox exactly once.

use uuid::Uuid;

use crate::error::TinterError;
use crate::job::{DispenseRequest, DispensedFormula, JobAck, TintJob};

pub trait TinterPlugin {
    /// Human-readable tinter name (e.g. `"virtual-simulator"`).
    fn name(&self) -> &str;

    /// Dispensers this tinter exposes (usually one).
    fn list_dispensers(&self) -> Vec<String>;

    /// Accept a formula for dispensing and return its job ack. On
    /// failure no colorant is decremented, but a `Failed` job is
    /// recorded and retrievable via [`TinterPlugin::job`].
    fn send_formula(&mut self, request: DispenseRequest) -> Result<JobAck, TinterError>;

    /// One job by id, if it exists.
    fn job(&self, job_id: Uuid) -> Option<TintJob>;

    /// All jobs, oldest first.
    fn jobs(&self) -> Vec<TintJob>;

    /// Completed dispenses since the tinter started (non-draining
    /// snapshot, newest last).
    fn completed_dispenses(&self) -> Vec<DispensedFormula>;

    /// Completed dispenses since the last drain, in dispense order;
    /// drains the internal queue.
    fn drain_completed(&mut self) -> Vec<DispensedFormula>;
}
