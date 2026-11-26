-- decor-pos Phase 5 (v0.6) — PostgreSQL HQ schema additions (spec §6.1, §7 Phase 5 row).
-- Same tables as migrations/sqlite/0010_phase5.sql, in PostgreSQL types:
--   UUID, TIMESTAMPTZ, NUMERIC(14,4) money (spec §5). No PAN/CVV columns:
--   portal_payments stores only the provider reference string returned by
--   the (mock) payment provider, never card data. Idempotency: a retried
--   bill payment with the same key returns the original payment record
--   and posts nothing twice.

CREATE TABLE portal_payments (
    idempotency_key    TEXT PRIMARY KEY,
    customer_id        UUID NOT NULL REFERENCES customers(id),
    payment_entry_id   UUID NOT NULL REFERENCES ar_ledger(id),
    amount             NUMERIC(14,4) NOT NULL,
    method             TEXT NOT NULL,
    provider_reference TEXT NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_portal_payments_customer ON portal_payments (customer_id, created_at);

CREATE TABLE bopis_orders (
    order_id        UUID PRIMARY KEY REFERENCES transactions(id),
    pickup_name     TEXT NOT NULL,
    contact         TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
