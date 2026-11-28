-- decor-pos Phase 1 Counter Completeness — PostgreSQL schema addition.
-- Same table as migrations/sqlite/0017_pos_holds.sql, in the TEXT
-- encoding both dialects use for ids/timestamps (see df-db lib.rs).
-- Persists parked counter holds across restarts.
-- No f32/f64 anywhere. No COLORx code. No PAN/CVV columns.

CREATE TABLE pos_holds (
    id              TEXT PRIMARY KEY,
    store_id        TEXT NOT NULL,
    cashier_id      TEXT,
    payload_json    TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_pos_holds_store ON pos_holds (store_id, created_at);
