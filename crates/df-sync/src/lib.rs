//! `df-sync` — UUIDv7 idempotent delta sync (Phase 1, v0.2; spec §7).
//!
//! The edge daemon writes every mutation to the local `sync_outbox`
//! table (spec §5.5) and later drains it as [`DeltaBatch`]es to HQ.
//! Replay safety is structural, not best-effort:
//!
//! * Every outbox entry id is a UUIDv7, so batches drain in creation
//!   order and a replayed entry is recognisable by id alone.
//! * [`HqSink::apply_batch`] must record applied entry ids and report
//!   replays in [`ApplyReport::duplicates`]; a replayed batch changes
//!   nothing.
//! * Conflict rules live in [`resolve`] and are documented in
//!   [`resolve::RESOLUTION_RULES`]: append-only ledgers merge by union,
//!   completed transactions are immutable, reference data resolves
//!   newest-`updated_at`-wins with the loser kept for audit.
//!
//! [`MemoryHq`] is the in-memory HQ used by tests and by the edge
//! daemon's local sync endpoint; a real PostgreSQL HQ sink replaces it
//! in a later phase. No floating-point money anywhere: ledger deltas
//! are `rust_decimal::Decimal`.

pub mod error;
pub mod outbox;
pub mod resolve;
pub mod sink;

pub use error::SyncError;
pub use outbox::{DeltaBatch, Outbox, OutboxEntry, SyncState};
pub use sink::{ApplyReport, HqSink, MemoryHq};
