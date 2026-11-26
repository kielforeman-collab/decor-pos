-- decor-pos Phase 4 (v0.5) — SQLite edge schema additions (spec §5.5/§7).
-- Same tables as migrations/postgres/0006_phase4.sql, adapted for SQLite:
--   UUID -> TEXT, JSONB -> TEXT, TIMESTAMPTZ -> TEXT (RFC 3339),
--   DATE -> TEXT (YYYY-MM-DD). No f32/f64 anywhere.
--   SQLite note: this edge schema deliberately omits FOREIGN KEY
--   references to HQ reference data (organizations, stores,
--   customers, transactions) — the edge must keep selling when that
--   data has not synced down yet. PostgreSQL (HQ) enforces the full
--   FK graph from spec section 5. The edge-owned chain
--   (`delivery_pods` -> `deliveries`) is still enforced.

CREATE TABLE crm_activities (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    customer_id         TEXT,
    activity_type       TEXT NOT NULL,
    subject             TEXT NOT NULL,
    notes               TEXT,
    created_by          TEXT,
    due_at              TEXT,
    completed_at        TEXT,
    snoozed_until       TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_crm_activities_store ON crm_activities (store_id, created_at);
CREATE INDEX idx_crm_activities_customer ON crm_activities (customer_id, created_at);
CREATE INDEX idx_crm_activities_due ON crm_activities (due_at) WHERE completed_at IS NULL;

CREATE TABLE appointments (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    customer_id         TEXT,
    staff_id            TEXT,
    title               TEXT NOT NULL,
    starts_at           TEXT NOT NULL,
    ends_at             TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'scheduled',
    notes               TEXT,
    created_at          TEXT NOT NULL,
    CHECK (ends_at > starts_at)
);

CREATE INDEX idx_appointments_store_day ON appointments (store_id, starts_at);
CREATE INDEX idx_appointments_customer ON appointments (customer_id, starts_at);
CREATE INDEX idx_appointments_staff ON appointments (staff_id, starts_at);

CREATE TABLE crm_saved_views (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    name                TEXT NOT NULL,
    filter_json         TEXT NOT NULL DEFAULT '{}',
    created_at          TEXT NOT NULL,
    UNIQUE (store_id, name)
);

CREATE TABLE deliveries (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    transaction_id      TEXT,
    customer_id         TEXT,
    driver_id           TEXT,
    status              TEXT NOT NULL DEFAULT 'draft',
    address_json        TEXT NOT NULL DEFAULT '{}',
    scheduled_date      TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE INDEX idx_deliveries_store ON deliveries (store_id, created_at);
CREATE INDEX idx_deliveries_status ON deliveries (status, scheduled_date);
CREATE INDEX idx_deliveries_driver ON deliveries (driver_id, scheduled_date);
CREATE INDEX idx_deliveries_transaction ON deliveries (transaction_id);

CREATE TABLE delivery_pods (
    id                  TEXT PRIMARY KEY,
    delivery_id         TEXT NOT NULL REFERENCES deliveries(id) ON DELETE CASCADE,
    signature_sha256    TEXT NOT NULL,
    signature_b64       TEXT NOT NULL,
    photo_ref           TEXT,
    photo_sha256        TEXT,
    received_by         TEXT NOT NULL,
    signed_at           TEXT NOT NULL,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_pods_delivery ON delivery_pods (delivery_id);
