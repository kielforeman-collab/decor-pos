-- decor-pos Phase 2 (v0.3) — SQLite edge schema additions (spec §5.3/§7).
-- Same tables as migrations/postgres/0003_phase2.sql, adapted for SQLite:
--   UUID -> TEXT, JSONB -> TEXT, TIMESTAMPTZ -> TEXT (RFC 3339),
--   money/volume columns are TEXT holding exact decimal strings
--   (spec §5: "scaled integer/text in SQLite"). No f32/f64 anywhere.
--   No PAN/CVV columns.
--   NOTE: `color_catalog`, `colorants` and `tint_formulas` already exist
--   from migration 0001 and are NOT recreated here.
--   SQLite FK note (as in 0002): this edge schema omits FOREIGN KEY
--   references to HQ reference data (`colorants`, `stores`) — the edge
--   must keep selling when that data has not synced down yet.
--   Edge-owned chains (`tint_jobs` -> `tint_formulas` /
--   `transactions`) are still enforced.

CREATE TABLE tint_jobs (
    id                  TEXT PRIMARY KEY,
    formula_id          TEXT NOT NULL REFERENCES tint_formulas(id),
    transaction_id      TEXT REFERENCES transactions(id),
    line_no             INTEGER,
    status              TEXT NOT NULL,
    dispensed_json      TEXT NOT NULL DEFAULT '[]',
    error               TEXT,
    created_at          TEXT NOT NULL,
    completed_at        TEXT
);

CREATE INDEX idx_tint_jobs_formula ON tint_jobs (formula_id, created_at);
CREATE INDEX idx_tint_jobs_transaction ON tint_jobs (transaction_id);

CREATE TABLE colorant_levels (
    colorant_id         TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    on_hand_fl_oz       TEXT NOT NULL DEFAULT 0,
    low_threshold_fl_oz TEXT NOT NULL DEFAULT 0,
    updated_at          TEXT NOT NULL,
    PRIMARY KEY (colorant_id, store_id)
);

CREATE INDEX idx_colorant_levels_store ON colorant_levels (store_id);
