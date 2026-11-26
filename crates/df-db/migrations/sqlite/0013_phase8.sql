-- decor-pos Phase 8 (v1.2) — SQLite edge schema additions.
-- Same tables as migrations/postgres/0013_phase8.sql, adapted for SQLite:
--   UUID -> TEXT, TIMESTAMPTZ -> TEXT (RFC 3339).
-- Sync ingest idempotency (see `df-sync::db::DbHq`): the ingest
-- endpoint records each applied outbox entry id in
-- `sync_applied_entries` (entry-id PRIMARY KEY = replay guard) and
-- skipped transaction conflicts in `sync_conflicts`. The edge never
-- writes these tables (it never applies batches); they exist here
-- only so both dialects stay in lockstep, same tables.
-- `payload_json` is TEXT in both dialects on purpose: the sink
-- stores the raw payload verbatim for audit and never queries into
-- it. No f32/f64 anywhere. No COLORx code. No PAN/CVV columns.

CREATE TABLE sync_applied_entries (
    entry_id        TEXT PRIMARY KEY,
    store_id        TEXT NOT NULL,
    batch_id        TEXT NOT NULL,
    aggregate_type  TEXT NOT NULL,
    aggregate_id    TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    applied_at      TEXT NOT NULL
);

CREATE INDEX idx_sync_applied_store ON sync_applied_entries (store_id, applied_at);
CREATE INDEX idx_sync_applied_aggregate ON sync_applied_entries (aggregate_type, aggregate_id);

CREATE TABLE sync_conflicts (
    entry_id        TEXT PRIMARY KEY,
    store_id        TEXT NOT NULL,
    aggregate_type  TEXT NOT NULL,
    aggregate_id    TEXT NOT NULL,
    reason          TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_sync_conflicts_store ON sync_conflicts (store_id, created_at);
