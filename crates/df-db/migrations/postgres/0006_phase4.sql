-- decor-pos Phase 4 (v0.5) — PostgreSQL HQ schema additions (spec §5.5/§7).
-- CRM activity log + appointments + saved views, delivery dispatch +
-- proof-of-delivery, for the Phase 4 wiring (df-db repos in `crm.rs`,
-- edge endpoints in `df-edge::{crm,delivery,edi}`).
--
-- No f32/f64 anywhere. No PAN/CVV columns. `delivery_pods` stores
-- the signature as base64 text plus its SHA-256 hex digest (the
-- signature is captured bytes, not an image file — no camera
-- hardware in this repo); the photo is a reference string (path or
-- URL) plus an optional SHA-256, never the photo bytes.
--
-- SQLite note: the edge schema deliberately omits FOREIGN KEY
-- references to HQ reference data (organizations, stores,
-- customers, transactions) — the edge must keep selling when that
-- data has not synced down yet. PostgreSQL (HQ) enforces the full
-- FK graph. Edge-owned chains (`delivery_pods` -> `deliveries`)
-- are enforced in both dialects.

CREATE TABLE crm_activities (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL REFERENCES organizations(id),
    store_id            UUID NOT NULL REFERENCES stores(id),
    customer_id         UUID REFERENCES customers(id),
    activity_type       TEXT NOT NULL, -- 'call' | 'email' | 'visit' | 'quote' | 'note' | 'follow_up'
    subject             TEXT NOT NULL,
    notes               TEXT,
    created_by          UUID,
    due_at              TIMESTAMPTZ,
    completed_at        TIMESTAMPTZ,
    snoozed_until       TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_crm_activities_store ON crm_activities (store_id, created_at);
CREATE INDEX idx_crm_activities_customer ON crm_activities (customer_id, created_at);
CREATE INDEX idx_crm_activities_due ON crm_activities (due_at) WHERE completed_at IS NULL;

CREATE TABLE appointments (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL REFERENCES organizations(id),
    store_id            UUID NOT NULL REFERENCES stores(id),
    customer_id         UUID REFERENCES customers(id),
    staff_id            UUID,
    title               TEXT NOT NULL,
    starts_at           TIMESTAMPTZ NOT NULL,
    ends_at             TIMESTAMPTZ NOT NULL,
    status              TEXT NOT NULL DEFAULT 'scheduled', -- 'scheduled' | 'completed' | 'cancelled' | 'no_show'
    notes               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at)
);

CREATE INDEX idx_appointments_store_day ON appointments (store_id, starts_at);
CREATE INDEX idx_appointments_customer ON appointments (customer_id, starts_at);
CREATE INDEX idx_appointments_staff ON appointments (staff_id, starts_at);

CREATE TABLE crm_saved_views (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL REFERENCES organizations(id),
    store_id            UUID NOT NULL REFERENCES stores(id),
    name                TEXT NOT NULL,
    filter_json         JSONB NOT NULL DEFAULT '{}',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (store_id, name)
);

CREATE TABLE deliveries (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL REFERENCES organizations(id),
    store_id            UUID NOT NULL REFERENCES stores(id),
    transaction_id      UUID REFERENCES transactions(id),
    customer_id         UUID REFERENCES customers(id),
    driver_id           UUID,
    status              TEXT NOT NULL DEFAULT 'draft', -- 'draft' | 'assigned' | 'out_for_delivery' | 'delivered' | 'cancelled'
    address_json        JSONB NOT NULL DEFAULT '{}',
    scheduled_date      DATE,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_deliveries_store ON deliveries (store_id, created_at);
CREATE INDEX idx_deliveries_status ON deliveries (status, scheduled_date);
CREATE INDEX idx_deliveries_driver ON deliveries (driver_id, scheduled_date);
CREATE INDEX idx_deliveries_transaction ON deliveries (transaction_id);

CREATE TABLE delivery_pods (
    id                  UUID PRIMARY KEY,
    delivery_id         UUID NOT NULL REFERENCES deliveries(id) ON DELETE CASCADE,
    signature_sha256    TEXT NOT NULL,
    signature_b64       TEXT NOT NULL,
    photo_ref           TEXT,
    photo_sha256        TEXT,
    received_by         TEXT NOT NULL,
    signed_at           TIMESTAMPTZ NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_pods_delivery ON delivery_pods (delivery_id);
