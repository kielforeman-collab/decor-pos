-- decor-pos Phase 1 Counter Completeness — SQLite schema addition.
-- Same table as migrations/postgres/0017_pos_holds.sql.
-- Persists parked counter holds across edge restarts (replacing the
-- Phase 1 in-memory HashMap). `payload_json` stores the serialized
-- `SaleInput` cart (SKUs, quantities, customer/job ids, tender kinds —
-- never card numbers or CVVs).
-- No f32/f64 anywhere. No COLORx code. No PAN/CVV columns.

CREATE TABLE pos_holds (
    id              TEXT PRIMARY KEY,
    store_id        TEXT NOT NULL,
    cashier_id      TEXT,
    payload_json    TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE INDEX idx_pos_holds_store ON pos_holds (store_id, created_at);
