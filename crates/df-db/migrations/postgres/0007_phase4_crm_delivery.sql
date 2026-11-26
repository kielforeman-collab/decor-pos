-- decor-pos Phase 4 (v0.5) — CRM + delivery additions on top of the
-- wiring worker's 0006_phase4 (spec §4 crm_marketing /
-- delivery_logistics, §7 Phase 4; domain crates df-crm /
-- df-delivery, repos in df-db::{delivery, crm_reports}).
-- Same tables as migrations/sqlite/0007_phase4_crm_delivery.sql,
-- adapted for PostgreSQL: TEXT encoding for ids/money/timestamps. No f32/f64
-- anywhere. No PAN/CVV anywhere.
--   Adds only what 0006 did not have: the delivery status-change
--   audit trail (`delivery_events`), persisted packing slips
--   (`packing_slips`), the dispatch priority + vehicle columns on
--   `deliveries`, and the photo capture time on `delivery_pods`.
--   No EDI tables here — those belong to the df-edi worker's part
--   of Phase 4.

CREATE TABLE delivery_events (
    id                  TEXT PRIMARY KEY,
    delivery_id         TEXT NOT NULL REFERENCES deliveries(id) ON DELETE CASCADE,
    from_status         TEXT NOT NULL,
    to_status           TEXT NOT NULL,
    actor_user_id       TEXT NOT NULL REFERENCES users(id),
    note                TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_delivery_events_delivery ON delivery_events (delivery_id, created_at);

CREATE TABLE packing_slips (
    id                  TEXT PRIMARY KEY,
    delivery_id         TEXT NOT NULL REFERENCES deliveries(id) ON DELETE CASCADE,
    rendered_text       TEXT NOT NULL,
    rendered_zpl        TEXT NOT NULL,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_packing_slips_delivery ON packing_slips (delivery_id);

ALTER TABLE deliveries ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
ALTER TABLE deliveries ADD COLUMN vehicle TEXT;
ALTER TABLE delivery_pods ADD COLUMN photo_at TEXT;
