-- decor-pos vendors (Products / Vendors / Dashboards screens) — SQLite edge schema additions.
-- Same tables/columns as migrations/postgres/0015_vendors.sql,
-- adapted for SQLite: UUID -> TEXT, money columns are TEXT holding
-- exact decimal strings, booleans are INTEGER 0/1. No f32/f64 anywhere.
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
    edi_enabled     INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    UNIQUE (org_id, name)
);

CREATE INDEX idx_vendors_org ON vendors (org_id, name);

-- Backfill: one row per distinct vendor already on a purchase order,
-- attributed to the org that owns the PO's store. New ids are random
-- UUID-shaped text (SQLite has no uuid function; the version/variant
-- nibbles are unset, which still parses as a UUID). `created_at` is
-- the vendor's most recent PO, so the row dates from real activity
-- rather than the migration run. POs whose store is unknown cannot
-- be attributed to an org and are skipped.
INSERT INTO vendors (id, org_id, name, display_name, edi_enabled, created_at)
SELECT
    lower(substr(h, 1, 8) || '-' || substr(h, 9, 4) || '-' || substr(h, 13, 4) || '-' || substr(h, 17, 4) || '-' || substr(h, 21, 12)),
    org_id,
    vendor_id,
    vendor_id,
    0,
    latest_po_at
FROM (
    SELECT
        hex(randomblob(16)) AS h,
        s.org_id AS org_id,
        po.vendor_id AS vendor_id,
        MAX(po.created_at) AS latest_po_at
    FROM purchase_orders po
    JOIN stores s ON s.id = po.store_id
    GROUP BY s.org_id, po.vendor_id
);
