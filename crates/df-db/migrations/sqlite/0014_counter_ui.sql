-- decor-pos counter-UI wiring (v1.2.1 follow-up) — SQLite edge schema additions.
-- Same tables/columns as migrations/postgres/0014_counter_ui.sql,
-- adapted for SQLite: UUID -> TEXT, money columns are TEXT holding
-- exact decimal strings. No f32/f64 anywhere.
--
-- * `price_levels`: the four contractor price levels (retail/gold/ws/
--   platinum), seeded idempotently by `df_db::ensure_price_levels`
--   with fixed UUIDv7 ids (see `df-db/src/repos.rs`).
-- * `products.msrp` / `products.map`: the price list's MSRP / MAP,
--   stored on import (see `df-db/src/product_import.rs`).
-- * `transactions.job_name`: the counter-entered job name for a sale
--   (the `job_id` FK points at a customer job; `job_name` is the
--   free-text name shown on the invoice).

CREATE TABLE price_levels (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL,
    code            TEXT NOT NULL,
    name            TEXT NOT NULL,
    discount_pct    TEXT NOT NULL,
    UNIQUE (org_id, code)
);

ALTER TABLE products ADD COLUMN msrp TEXT;
ALTER TABLE products ADD COLUMN map TEXT;
ALTER TABLE transactions ADD COLUMN job_name TEXT;
