-- decor-pos Phase 5 (v0.6) — PostgreSQL HQ schema additions (spec §6.1, §7 Phase 5 row).
-- Same tables as migrations/sqlite/0010_phase5.sql, in PostgreSQL types:
--   TEXT for ids/timestamps/money (spec §5). No PAN/CVV columns:
--   portal_payments stores only the provider reference string returned by
--   the (mock) payment provider, never card data. Idempotency: a retried
--   bill payment with the same key returns the original payment record
--   and posts nothing twice.

CREATE TABLE portal_payments (
    idempotency_key    TEXT PRIMARY KEY,
    customer_id        TEXT NOT NULL REFERENCES customers(id),
    payment_entry_id   TEXT NOT NULL REFERENCES ar_ledger(id),
    amount             TEXT NOT NULL,
    method             TEXT NOT NULL,
    provider_reference TEXT NOT NULL,
    created_at         TEXT NOT NULL
);

CREATE INDEX idx_portal_payments_customer ON portal_payments (customer_id, created_at);

CREATE TABLE bopis_orders (
    order_id        TEXT PRIMARY KEY REFERENCES transactions(id),
    pickup_name     TEXT NOT NULL,
    contact         TEXT NOT NULL,
    created_at      TEXT NOT NULL
);
