-- decor-pos Phase 3 (v0.4) — PostgreSQL HQ schema additions (spec §5.4/§7).
-- Adds the monthly statement batch table, inter-store transfers,
-- physical counts and reorder points, plus the two Phase 3 credit
-- columns on customers. Financial invariants: money as decimal TEXT
-- (TEXT for the monthly finance-charge percent), UUIDv7 ids as TEXT,
-- primary keys, append-only ledgers. No f32/f64 anywhere.
--   `customers` already carries credit_limit / credit_hold since
--   0001; this migration adds only the two Phase 3 credit columns
--   that did not exist yet (terms_days, finance_rate_pct_monthly),
--   both nullable so existing rows keep working.

ALTER TABLE customers ADD COLUMN terms_days INTEGER;
ALTER TABLE customers ADD COLUMN finance_rate_pct_monthly TEXT;

CREATE TABLE statements (
    id                  TEXT PRIMARY KEY,
    customer_id         TEXT NOT NULL REFERENCES customers(id),
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
    org_id              TEXT NOT NULL REFERENCES organizations(id),
    from_store_id       TEXT NOT NULL REFERENCES stores(id),
    to_store_id         TEXT NOT NULL REFERENCES stores(id),
    status              TEXT NOT NULL DEFAULT 'draft',
    reference           TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_transfers_stores ON inventory_transfers (from_store_id, to_store_id);

CREATE TABLE transfer_lines (
    id                  TEXT PRIMARY KEY,
    transfer_id         TEXT NOT NULL REFERENCES inventory_transfers(id) ON DELETE CASCADE,
    product_id          TEXT NOT NULL REFERENCES products(id),
    qty                 TEXT NOT NULL,
    unit_cost           TEXT NOT NULL
);

CREATE INDEX idx_transfer_lines_transfer ON transfer_lines (transfer_id);

CREATE TABLE physical_counts (
    id                  TEXT PRIMARY KEY,
    store_id            TEXT NOT NULL REFERENCES stores(id),
    status              TEXT NOT NULL DEFAULT 'open',
    counted_at          TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_counts_store ON physical_counts (store_id, created_at);

CREATE TABLE count_lines (
    id                  TEXT PRIMARY KEY,
    count_id            TEXT NOT NULL REFERENCES physical_counts(id) ON DELETE CASCADE,
    product_id          TEXT NOT NULL REFERENCES products(id),
    counted_qty         TEXT NOT NULL,
    system_qty          TEXT NOT NULL,
    variance_qty        TEXT NOT NULL,
    unit_cost           TEXT NOT NULL
);

CREATE INDEX idx_count_lines_count ON count_lines (count_id);

CREATE TABLE reorder_points (
    id                  TEXT PRIMARY KEY,
    store_id            TEXT NOT NULL REFERENCES stores(id),
    product_id          TEXT NOT NULL REFERENCES products(id),
    min_qty             TEXT NOT NULL,
    max_qty             TEXT NOT NULL,
    reorder_qty         TEXT NOT NULL,
    UNIQUE (store_id, product_id)
);

CREATE INDEX idx_reorder_store ON reorder_points (store_id);
