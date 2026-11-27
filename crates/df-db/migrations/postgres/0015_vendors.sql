-- decor-pos vendors (Products / Vendors / Dashboards screens) — PostgreSQL HQ schema additions.
-- Same tables/columns as migrations/sqlite/0015_vendors.sql.
-- Money as exact decimal TEXT, UUIDv7 ids as TEXT (see `df-db/src/lib.rs`).
-- Booleans are native BOOLEAN here (SQLite uses INTEGER 0/1); the
-- `df-db` codec reads both. No f32/f64 anywhere.
--
-- * `vendors`: back-office master data for the vendor-id string used
--   on purchase orders (e.g. "Benjamin Moore"). `name` is the PO key
--   and is not editable after creation; `display_name` is the human
--   label shown in the UI. Backfilled below from the distinct
--   `vendor_id` values already on `purchase_orders`
--   (display_name = vendor_id, no contact details — those stay NULL
--   until entered by hand).

CREATE TABLE vendors (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL,
    name            TEXT NOT NULL,
    display_name    TEXT,
    contact_name    TEXT,
    email           TEXT,
    phone           TEXT,
    edi_enabled     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TEXT NOT NULL,
    UNIQUE (org_id, name)
);

CREATE INDEX idx_vendors_org ON vendors (org_id, name);

-- Backfill: one row per distinct vendor already on a purchase order,
-- attributed to the org that owns the PO's store. `created_at` is the
-- vendor's most recent PO, so the row dates from real activity
-- rather than the migration run. POs whose store is unknown cannot
-- be attributed to an org and are skipped.
INSERT INTO vendors (id, org_id, name, display_name, edi_enabled, created_at)
SELECT
    gen_random_uuid()::text,
    s.org_id,
    po.vendor_id,
    po.vendor_id,
    FALSE,
    MAX(po.created_at)
FROM purchase_orders po
JOIN stores s ON s.id = po.store_id
GROUP BY s.org_id, po.vendor_id;
