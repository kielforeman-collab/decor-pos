-- decor-pos Phase 3 (v0.4) — PostgreSQL HQ schema additions (spec §5.4/§7).
-- Adds the monthly statement batch table, inter-store transfers,
-- physical counts and reorder points, plus the two Phase 3 credit
-- columns on customers. Financial invariants: NUMERIC(14,4) money
-- (NUMERIC(6,3) for the monthly finance-charge percent), UUIDv7
-- primary keys, append-only ledgers. No f32/f64 anywhere.
--   `customers` already carries credit_limit / credit_hold since
--   0001; this migration adds only the two Phase 3 credit columns
--   that did not exist yet (terms_days, finance_rate_pct_monthly),
--   both nullable so existing rows keep working.

ALTER TABLE customers ADD COLUMN terms_days INTEGER;
ALTER TABLE customers ADD COLUMN finance_rate_pct_monthly NUMERIC(6,3);

CREATE TABLE statements (
    id                  UUID PRIMARY KEY,
    customer_id         UUID NOT NULL REFERENCES customers(id),
    period_start        DATE NOT NULL,
    period_end          DATE NOT NULL,
    opening_balance     NUMERIC(14,4) NOT NULL,
    charges_total       NUMERIC(14,4) NOT NULL,
    payments_total      NUMERIC(14,4) NOT NULL,
    finance_charge_total NUMERIC(14,4) NOT NULL,
    closing_balance     NUMERIC(14,4) NOT NULL,
    aging_json          JSONB NOT NULL,
    pdf_sha256          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (customer_id, period_start, period_end)
);

CREATE INDEX idx_statements_period ON statements (period_start, period_end);

CREATE TABLE inventory_transfers (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL REFERENCES organizations(id),
    from_store_id       UUID NOT NULL REFERENCES stores(id),
    to_store_id         UUID NOT NULL REFERENCES stores(id),
    status              TEXT NOT NULL DEFAULT 'draft',
    reference           TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_transfers_stores ON inventory_transfers (from_store_id, to_store_id);

CREATE TABLE transfer_lines (
    id                  UUID PRIMARY KEY,
    transfer_id         UUID NOT NULL REFERENCES inventory_transfers(id) ON DELETE CASCADE,
    product_id          UUID NOT NULL REFERENCES products(id),
    qty                 NUMERIC(14,4) NOT NULL,
    unit_cost           NUMERIC(14,4) NOT NULL
);

CREATE INDEX idx_transfer_lines_transfer ON transfer_lines (transfer_id);

CREATE TABLE physical_counts (
    id                  UUID PRIMARY KEY,
    store_id            UUID NOT NULL REFERENCES stores(id),
    status              TEXT NOT NULL DEFAULT 'open',
    counted_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_counts_store ON physical_counts (store_id, created_at);

CREATE TABLE count_lines (
    id                  UUID PRIMARY KEY,
    count_id            UUID NOT NULL REFERENCES physical_counts(id) ON DELETE CASCADE,
    product_id          UUID NOT NULL REFERENCES products(id),
    counted_qty         NUMERIC(14,4) NOT NULL,
    system_qty          NUMERIC(14,4) NOT NULL,
    variance_qty        NUMERIC(14,4) NOT NULL,
    unit_cost           NUMERIC(14,4) NOT NULL
);

CREATE INDEX idx_count_lines_count ON count_lines (count_id);

CREATE TABLE reorder_points (
    id                  UUID PRIMARY KEY,
    store_id            UUID NOT NULL REFERENCES stores(id),
    product_id          UUID NOT NULL REFERENCES products(id),
    min_qty             NUMERIC(14,4) NOT NULL,
    max_qty             NUMERIC(14,4) NOT NULL,
    reorder_qty         NUMERIC(14,4) NOT NULL,
    UNIQUE (store_id, product_id)
);

CREATE INDEX idx_reorder_store ON reorder_points (store_id);
