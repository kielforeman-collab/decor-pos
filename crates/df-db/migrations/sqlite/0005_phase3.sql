-- decor-pos Phase 3 (v0.4) — SQLite edge schema additions (spec §5.4/§7).
-- Same tables as migrations/postgres/0005_phase3.sql, adapted for SQLite:
--   UUID -> TEXT, JSONB -> TEXT, TIMESTAMPTZ -> TEXT (RFC 3339),
--   DATE -> TEXT (YYYY-MM-DD), BOOLEAN -> INTEGER (0/1),
--   money columns are TEXT holding exact decimal strings
--   (spec §5: "scaled integer/text in SQLite"). No f32/f64 anywhere.
--   SQLite note: this edge schema deliberately omits FOREIGN KEY
--   references to HQ reference data (stores, users, customers,
--   jobs, products) — the edge must keep selling when that data
--   has not synced down yet. PostgreSQL (HQ) enforces the full
--   FK graph from spec section 5. Edge-owned chains
--   (transfer_lines -> inventory_transfers, count_lines ->
--   physical_counts) are still enforced.
--   `customers` already carries credit_limit / credit_hold since
--   0001; this migration adds only the two Phase 3 credit columns
--   that did not exist yet (terms_days, finance_rate_pct_monthly),
--   both nullable so existing rows keep working.

ALTER TABLE customers ADD COLUMN terms_days INTEGER;
ALTER TABLE customers ADD COLUMN finance_rate_pct_monthly TEXT;

CREATE TABLE statements (
    id                  TEXT PRIMARY KEY,
    customer_id         TEXT NOT NULL,
    period_start        TEXT NOT NULL,
    period_end          TEXT NOT NULL,
    opening_balance     TEXT NOT NULL,
    charges_total       TEXT NOT NULL,
    payments_total      TEXT NOT NULL,
    finance_charge_total TEXT NOT NULL,
    closing_balance     TEXT NOT NULL,
    aging_json          TEXT NOT NULL,
    pdf_sha256          TEXT,
    created_at          TEXT NOT NULL,
    UNIQUE (customer_id, period_start, period_end)
);

CREATE INDEX idx_statements_period ON statements (period_start, period_end);

CREATE TABLE inventory_transfers (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    from_store_id       TEXT NOT NULL,
    to_store_id         TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'draft',
    reference           TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_transfers_stores ON inventory_transfers (from_store_id, to_store_id);

CREATE TABLE transfer_lines (
    id                  TEXT PRIMARY KEY,
    transfer_id         TEXT NOT NULL REFERENCES inventory_transfers(id) ON DELETE CASCADE,
    product_id          TEXT NOT NULL,
    qty                 TEXT NOT NULL,
    unit_cost           TEXT NOT NULL
);

CREATE INDEX idx_transfer_lines_transfer ON transfer_lines (transfer_id);

CREATE TABLE physical_counts (
    id                  TEXT PRIMARY KEY,
    store_id            TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'open',
    counted_at          TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_counts_store ON physical_counts (store_id, created_at);

CREATE TABLE count_lines (
    id                  TEXT PRIMARY KEY,
    count_id            TEXT NOT NULL REFERENCES physical_counts(id) ON DELETE CASCADE,
    product_id          TEXT NOT NULL,
    counted_qty         TEXT NOT NULL,
    system_qty          TEXT NOT NULL,
    variance_qty        TEXT NOT NULL,
    unit_cost           TEXT NOT NULL
);

CREATE INDEX idx_count_lines_count ON count_lines (count_id);

CREATE TABLE reorder_points (
    id                  TEXT PRIMARY KEY,
    store_id            TEXT NOT NULL,
    product_id          TEXT NOT NULL,
    min_qty             TEXT NOT NULL,
    max_qty             TEXT NOT NULL,
    reorder_qty         TEXT NOT NULL,
    UNIQUE (store_id, product_id)
);

CREATE INDEX idx_reorder_store ON reorder_points (store_id);
