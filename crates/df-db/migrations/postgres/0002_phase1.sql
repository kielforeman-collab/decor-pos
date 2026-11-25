-- decor-pos Phase 1 (v0.2) — PostgreSQL HQ schema additions (spec §5.5).
-- Adds transactions, transaction_lines, inventory_ledger, till_sessions,
-- payments and sync_state, plus sync_outbox (spec §5.5; not created in
-- 0001). Financial invariants: NUMERIC(14,4) money, UUIDv7 primary keys,
-- append-only ledgers. No f32/f64 anywhere. No PAN/CVV columns: payments
-- stores only a tender, reference and authorization token.

CREATE TABLE till_sessions (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    store_id            UUID NOT NULL REFERENCES stores(id),
    cashier_id          UUID NOT NULL REFERENCES users(id),
    opened_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    closed_at           TIMESTAMPTZ,
    opening_float       NUMERIC(14,4) NOT NULL,
    counted_cash        NUMERIC(14,4),
    expected_cash       NUMERIC(14,4),
    status              TEXT NOT NULL DEFAULT 'open'  -- 'open','closed'
);

CREATE INDEX idx_tills_store ON till_sessions (store_id, opened_at);

CREATE TABLE transactions (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    store_id            UUID NOT NULL REFERENCES stores(id),
    till_session_id     UUID REFERENCES till_sessions(id),
    doc_number          TEXT NOT NULL,
    doc_type            TEXT NOT NULL,               -- 'sale','return','quote','order','layaway'
    status              TEXT NOT NULL,               -- 'open','parked','completed','voided'
    customer_id         UUID REFERENCES customers(id),
    job_id              UUID REFERENCES customer_jobs(id),
    po_number           TEXT,
    salesperson_id      UUID REFERENCES users(id),
    cashier_id          UUID REFERENCES users(id),
    currency            CHAR(3) NOT NULL DEFAULT 'USD',
    subtotal            NUMERIC(14,4) NOT NULL,
    tax_total           NUMERIC(14,4) NOT NULL,
    eco_fee_total       NUMERIC(14,4) NOT NULL DEFAULT 0,
    pos_fee_total       NUMERIC(14,4) NOT NULL DEFAULT 0,
    grand_total         NUMERIC(14,4) NOT NULL,
    deposit_paid        NUMERIC(14,4) NOT NULL DEFAULT 0,
    fulfillment_mode    TEXT NOT NULL DEFAULT 'counter',
    scheduled_at        TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_transactions_store ON transactions (store_id, created_at);
CREATE INDEX idx_transactions_till ON transactions (till_session_id);

CREATE TABLE transaction_lines (
    id                  UUID PRIMARY KEY,
    transaction_id      UUID NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    line_no             INTEGER NOT NULL,
    product_id          UUID NOT NULL REFERENCES products(id),
    description         TEXT NOT NULL,
    qty                 NUMERIC(14,4) NOT NULL,
    uom_code            TEXT NOT NULL,
    unit_price          NUMERIC(14,4) NOT NULL,
    base_unit_cost      NUMERIC(14,4) NOT NULL,
    tint_unit_cost      NUMERIC(14,4) NOT NULL DEFAULT 0,
    discount_amount     NUMERIC(14,4) NOT NULL DEFAULT 0,
    tax_amount          NUMERIC(14,4) NOT NULL DEFAULT 0,
    eco_fee_amount      NUMERIC(14,4) NOT NULL DEFAULT 0,
    line_total          NUMERIC(14,4) NOT NULL,
    tint_formula_id     UUID REFERENCES tint_formulas(id),
    prompts_json        JSONB NOT NULL DEFAULT '{}',
    vertical_spec_json  JSONB NOT NULL DEFAULT '{}',
    linked_po_line_id   UUID,
    fulfilled_qty       NUMERIC(14,4) NOT NULL DEFAULT 0
);

CREATE INDEX idx_transaction_lines_tx ON transaction_lines (transaction_id, line_no);

CREATE TABLE inventory_ledger (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    store_id            UUID NOT NULL REFERENCES stores(id),
    product_id          UUID NOT NULL REFERENCES products(id),
    lot_number          TEXT,
    delta_on_hand       NUMERIC(14,4) NOT NULL DEFAULT 0,
    delta_committed     NUMERIC(14,4) NOT NULL DEFAULT 0,
    delta_on_order      NUMERIC(14,4) NOT NULL DEFAULT 0,
    delta_in_transit    NUMERIC(14,4) NOT NULL DEFAULT 0,
    unit_cost           NUMERIC(14,4) NOT NULL,
    reason_type         TEXT NOT NULL,               -- 'sale','return','po_receive','transfer','count_adj','mistint'
    reference_id        UUID NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_inventory_product ON inventory_ledger (store_id, product_id, created_at);

CREATE TABLE payments (
    id                  UUID PRIMARY KEY,
    transaction_id      UUID NOT NULL REFERENCES transactions(id) ON DELETE CASCADE,
    tender              TEXT NOT NULL,               -- 'cash','card','charge_account','gift_card'
    amount              NUMERIC(14,4) NOT NULL,
    reference           TEXT NOT NULL,
    authorization       TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_payments_tx ON payments (transaction_id);

CREATE TABLE sync_outbox (
    id                  UUID PRIMARY KEY,            -- UUIDv7 ordered sync sequence
    store_id            UUID NOT NULL,
    aggregate_type      TEXT NOT NULL,               -- 'transaction','tint_formula','ar_ledger','inventory_ledger'
    aggregate_id        UUID NOT NULL,
    payload_json        JSONB NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    synced_at           TIMESTAMPTZ,
    retry_count         INTEGER NOT NULL DEFAULT 0,
    last_error          TEXT
);

CREATE INDEX idx_outbox_pending ON sync_outbox (store_id, synced_at, id);

CREATE TABLE sync_state (
    store_id            UUID PRIMARY KEY,
    last_batch_id       UUID,
    last_synced_at      TIMESTAMPTZ,
    pending_count       INTEGER NOT NULL DEFAULT 0
);
