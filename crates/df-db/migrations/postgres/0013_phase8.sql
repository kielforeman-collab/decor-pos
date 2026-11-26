-- decor-pos Phase 8 (v1.2) — PostgreSQL HQ schema additions.
-- Sync ingest idempotency (see `df-sync::db::DbHq`): the ingest
-- endpoint applies each outbox entry inside one transaction and
-- records the entry id in `sync_applied_entries` first; the
-- entry-id PRIMARY KEY is the DB-layer replay guard, so a replayed
-- batch inserts nothing twice and counts no money twice.
-- `sync_conflicts` records transaction entries skipped as conflicts
-- (same aggregate id, different content) so a skipped entry stays
-- visible to an operator instead of vanishing; it is never in the
-- sink's `applied_ids`, so the edge leaves it pending.
-- Same tables as migrations/sqlite/0013_phase8.sql, in the TEXT
-- encoding both dialects use for ids/timestamps (see df-db lib.rs:
-- the `Any` driver cannot decode uuid / timestamptz columns).
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
