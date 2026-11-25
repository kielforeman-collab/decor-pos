-- decor-pos Phase 2 (v0.3) — PostgreSQL HQ schema additions (spec §5.3/§7).
--
-- NOTE: `color_catalog`, `colorants` and `tint_formulas` already exist
-- from migration 0001 (spec §5.3) and are NOT recreated here. This
-- migration adds only the two tables Phase 2 needs that 0001 did not
-- have: `tint_jobs` (the dispense record a tinted sale line traces
-- to) and `colorant_levels` (on-hand colorant inventory per store).
--
-- Financial invariants: NUMERIC money, UUIDv7 primary keys, no
-- floating-point anywhere. No PAN/CVV columns. PostgreSQL enforces
-- the FK graph; SQLite (edge) omits FKs to HQ reference data — see
-- migrations/sqlite/0003_phase2.sql.

CREATE TABLE tint_jobs (
    id                  UUID PRIMARY KEY,
    formula_id          UUID NOT NULL REFERENCES tint_formulas(id),
    transaction_id      UUID REFERENCES transactions(id),
    line_no             INTEGER,
    status              TEXT NOT NULL,               -- 'queued','dispensing','done','failed'
    dispensed_json      JSONB NOT NULL DEFAULT '[]', -- [{ "code": "Y3", "fl_oz": "0.2916..." }]
    error               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at        TIMESTAMPTZ
);

CREATE INDEX idx_tint_jobs_formula ON tint_jobs (formula_id, created_at);
CREATE INDEX idx_tint_jobs_transaction ON tint_jobs (transaction_id);

CREATE TABLE colorant_levels (
    colorant_id         UUID NOT NULL REFERENCES colorants(id),
    store_id            UUID NOT NULL REFERENCES stores(id),
    on_hand_fl_oz       NUMERIC(12,4) NOT NULL DEFAULT 0,
    low_threshold_fl_oz NUMERIC(12,4) NOT NULL DEFAULT 0,
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (colorant_id, store_id)
);

CREATE INDEX idx_colorant_levels_store ON colorant_levels (store_id);
