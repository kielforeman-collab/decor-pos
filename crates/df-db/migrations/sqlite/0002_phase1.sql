-- decor-pos Phase 1 (v0.2) — SQLite edge schema additions (spec §5.5).
-- Same tables as migrations/postgres/0002_phase1.sql, adapted for SQLite:
--   UUID -> TEXT, JSONB -> TEXT, TIMESTAMPTZ -> TEXT (RFC 3339),
--   money columns are TEXT holding exact decimal strings
--   (spec §5: "scaled integer/text in SQLite"). No f32/f64 anywhere.
--   No PAN/CVV columns: payments stores only a tender, reference and
--   authorization token.
--   SQLite note: this edge schema deliberately omits FOREIGN KEY
--   references to HQ reference data (stores, users, customers,
--   jobs, products) — the edge must keep selling when that data
--   has not synced down yet. PostgreSQL (HQ) enforces the full
--   FK graph from spec section 5. Edge-owned chains (transaction_lines
--   and payments -> transactions) are still enforced.

CREATE TABLE till_sessions (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    cashier_id          TEXT NOT NULL,
    opened_at           TEXT NOT NULL,
    closed_at           TEXT,
    opening_float       TEXT NOT NULL,
    counted_cash        TEXT,
    expected_cash       TEXT,
    status              TEXT NOT NULL DEFAULT 'open'
);

CREATE INDEX idx_tills_store ON till_sessions (store_id, opened_at);

CREATE TABLE transactions (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    till_session_id     TEXT,
    doc_number          TEXT NOT NULL,
    doc_type            TEXT NOT NULL,
    status              TEXT NOT NULL,
    customer_id         TEXT,
    job_id              TEXT,
    po_number           TEXT,
    salesperson_id      TEXT,
    cashier_id          TEXT,
    currency            TEXT NOT NULL DEFAULT 'USD',
    subtotal            TEXT NOT NULL,
    tax_total           TEXT NOT NULL,
    eco_fee_total       TEXT NOT NULL DEFAULT 0,
    pos_fee_total       TEXT NOT NULL DEFAULT 0,
    grand_total         TEXT NOT NULL,
    deposit_paid        TEXT NOT NULL DEFAULT 0,
    fulfillment_mode    TEXT NOT NULL DEFAULT 'counter',
    scheduled_at        TEXT,
    completed_at        TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_transactions_store ON transactions (store_id, created_at);
CREATE INDEX idx_transactions_till ON transactions (till_session_id);

CREATE TABLE transaction_lines (
    id                  TEXT PRIMARY KEY,
    transaction_id      TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    line_no             INTEGER NOT NULL,
    product_id          TEXT NOT NULL,
    description         TEXT NOT NULL,
    qty                 TEXT NOT NULL,
    uom_code            TEXT NOT NULL,
    unit_price          TEXT NOT NULL,
    base_unit_cost      TEXT NOT NULL,
    tint_unit_cost      TEXT NOT NULL DEFAULT 0,
    discount_amount     TEXT NOT NULL DEFAULT 0,
    tax_amount          TEXT NOT NULL DEFAULT 0,
    eco_fee_amount      TEXT NOT NULL DEFAULT 0,
    line_total          TEXT NOT NULL,
    tint_formula_id     TEXT,
    prompts_json        TEXT NOT NULL DEFAULT '{}',
    vertical_spec_json  TEXT NOT NULL DEFAULT '{}',
    linked_po_line_id   TEXT,
    fulfilled_qty       TEXT NOT NULL DEFAULT 0
);

CREATE INDEX idx_transaction_lines_tx ON transaction_lines (transaction_id, line_no);

CREATE TABLE inventory_ledger (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    product_id          TEXT NOT NULL,
    lot_number          TEXT,
    delta_on_hand       TEXT NOT NULL DEFAULT 0,
    delta_committed     TEXT NOT NULL DEFAULT 0,
    delta_on_order      TEXT NOT NULL DEFAULT 0,
    delta_in_transit    TEXT NOT NULL DEFAULT 0,
    unit_cost           TEXT NOT NULL,
    reason_type         TEXT NOT NULL,
    reference_id        TEXT NOT NULL,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_inventory_product ON inventory_ledger (store_id, product_id, created_at);

CREATE TABLE payments (
    id                  TEXT PRIMARY KEY,
    transaction_id      TEXT NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    tender              TEXT NOT NULL,
    amount              TEXT NOT NULL,
    reference           TEXT NOT NULL,
    authorization       TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_payments_tx ON payments (transaction_id);

CREATE TABLE sync_outbox (
    id                  TEXT PRIMARY KEY,
    store_id            TEXT NOT NULL,
    aggregate_type      TEXT NOT NULL,
    aggregate_id        TEXT NOT NULL,
    payload_json        TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    synced_at           TEXT,
    retry_count         INTEGER NOT NULL DEFAULT 0,
    last_error          TEXT
);

CREATE INDEX idx_outbox_pending ON sync_outbox (store_id, synced_at, id);

CREATE TABLE sync_state (
    store_id            TEXT PRIMARY KEY,
    last_batch_id       TEXT,
    last_synced_at      TEXT,
    pending_count       INTEGER NOT NULL DEFAULT 0
);
