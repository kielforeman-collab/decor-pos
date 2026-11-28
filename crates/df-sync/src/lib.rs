//! `df-sync` — UUIDv7 idempotent delta sync (Phase 1, v0.2; spec §7).
//!
//! The edge daemon writes every mutation to the local `sync_outbox`
//! table (spec §5.5) and later drains it as [`DeltaBatch`]es to HQ.
//! Replay safety is structural, not best-effort:
//!
//! * Every outbox entry id is a UUIDv7, so batches drain in creation
//!   order and a replayed entry is recognisable by id alone.
//! * [`HqSink::apply_batch`] must record applied entry ids and list
//!   every resolved id in [`ApplyReport::applied_ids`]; a replayed
//!   batch changes nothing, and a conflicting transaction entry is
//!   skipped (counted in `ApplyReport::conflicts`) without blocking
//!   the rest of the batch.
//! * Conflict rules live in [`resolve`] and are documented in
//!   [`resolve::RESOLUTION_RULES`]: append-only ledgers merge by union,
//!   completed transactions are immutable, reference data resolves
//!   newest-`updated_at`-wins with the loser kept for audit.
//!
//! [`MemoryHq`] is the in-memory HQ used by tests and by the edge
//! daemon's local sync endpoint; [`DbHq`](crate::db::DbHq) is the
//! DB-backed HQ behind the live ingest endpoint, and
//! [`RemoteHq`](crate::remote::RemoteHq) pushes batches to it over
//! HTTPS. No floating-point money anywhere: ledger deltas are
//! `rust_decimal::Decimal`.

pub mod db;
pub mod error;
pub mod outbox;
pub mod remote;
pub mod resolve;
pub mod sink;

pub use db::{DbHq, SyncConflictRow};
pub use error::SyncError;
pub use outbox::{DEFAULT_MAX_RETRIES, DeltaBatch, Outbox, OutboxEntry, SyncState};
pub use remote::RemoteHq;
pub use sink::{ApplyReport, HqSink, MemoryHq};
