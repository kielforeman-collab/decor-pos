-- decor-pos Phase 4 (v0.5) — PostgreSQL HQ schema additions (spec §6.2, §7 Phase 4 row, df-edi half).
-- Adds PO acknowledgments (855), three-way invoice match reports (810 vs 850 vs 856),
-- reviewed price-catalog applications (832), and EDI transport records.
-- (The CRM/delivery half of Phase 4 lives in 0006_phase4.sql, owned by
-- the other Phase 4 worker — this file only adds the EDI tables.)
-- Financial invariants: money as exact decimal TEXT, UUIDv7 ids as TEXT.
-- No f32/f64 anywhere. No PAN/CVV columns. No live vendor transport:
-- edi_transport_records logs file drops (local-sftp-stand-in / AS2 file
-- packaging) only; there is no network endpoint recorded here.

CREATE TABLE po_acknowledgments (
    id                  TEXT PRIMARY KEY,
    po_id               TEXT NOT NULL REFERENCES purchase_orders(id),
    po_number           TEXT NOT NULL,
    ack_date            TEXT,
    received_at         TEXT NOT NULL
);

CREATE INDEX idx_acks_po ON po_acknowledgments (po_id, received_at);

CREATE TABLE po_acknowledgment_lines (
    id                  TEXT PRIMARY KEY,
    ack_id              TEXT NOT NULL REFERENCES po_acknowledgments(id) ON DELETE CASCADE,
    line_no             INTEGER NOT NULL,
    vendor_sku          TEXT NOT NULL,
    ack_code            TEXT NOT NULL,               -- 'IA','DR','IQ'
    qty_acknowledged    TEXT NOT NULL,
    uom                 TEXT NOT NULL,
    unit_cost           TEXT,
    promised_date       TEXT
);

CREATE INDEX idx_ack_lines_ack ON po_acknowledgment_lines (ack_id, line_no);

CREATE TABLE invoice_match_reports (
    id                  TEXT PRIMARY KEY,
    po_id               TEXT NOT NULL REFERENCES purchase_orders(id),
    po_number           TEXT NOT NULL,
    invoice_number      TEXT NOT NULL,
    shipment_id         TEXT NOT NULL,
    status              TEXT NOT NULL,               -- 'matched','variance'
    po_total            TEXT NOT NULL,
    invoiced_total      TEXT NOT NULL,
    total_variance      TEXT NOT NULL,
    stated_total_difference TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_match_reports_po ON invoice_match_reports (po_id, created_at);
CREATE INDEX idx_match_reports_invoice ON invoice_match_reports (invoice_number);

CREATE TABLE invoice_match_lines (
    id                  TEXT PRIMARY KEY,
    match_id            TEXT NOT NULL REFERENCES invoice_match_reports(id) ON DELETE CASCADE,
    po_line_no          INTEGER,
    vendor_sku          TEXT NOT NULL,
    po_qty              TEXT NOT NULL,
    received_qty        TEXT NOT NULL,
    invoiced_qty        TEXT NOT NULL,
    po_unit_cost        TEXT NOT NULL,
    invoiced_unit_price TEXT NOT NULL,
    qty_variance        TEXT NOT NULL,
    price_variance_per_unit TEXT NOT NULL,
    qty_variance_amount TEXT NOT NULL,
    price_variance_amount TEXT NOT NULL,
    total_variance_amount TEXT NOT NULL,
    status              TEXT NOT NULL,               -- 'matched','variance'
    variance_kind       TEXT NOT NULL                -- 'none','quantity','price','quantity_and_price','missing_received','missing_invoiced','not_on_po'
);

CREATE INDEX idx_match_lines_match ON invoice_match_lines (match_id);

CREATE TABLE price_catalog_applications (
    id                  TEXT PRIMARY KEY,
    vendor_id           TEXT NOT NULL,
    catalog_id          TEXT NOT NULL,
    catalog_date        TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending_review',  -- 'pending_review','applied'
    reviewed_by         TEXT,
    applied_at          TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_catalog_apps_vendor ON price_catalog_applications (vendor_id, created_at);

CREATE TABLE price_catalog_lines (
    id                  TEXT PRIMARY KEY,
    application_id      TEXT NOT NULL REFERENCES price_catalog_applications(id) ON DELETE CASCADE,
    vendor_sku          TEXT NOT NULL,
    action              TEXT NOT NULL,               -- 'ADD','CHG','DROP'
    old_cost            TEXT,
    old_list_price      TEXT,
    new_cost            TEXT,
    new_list_price      TEXT,
    result              TEXT NOT NULL,               -- 'apply','add','drop','skipped'
    note                TEXT NOT NULL DEFAULT ''
);

CREATE INDEX idx_catalog_lines_app ON price_catalog_lines (application_id);

CREATE TABLE edi_transport_records (
    id                  TEXT PRIMARY KEY,
    vendor_id           TEXT NOT NULL,
    direction           TEXT NOT NULL,               -- 'in','out'
    transport           TEXT NOT NULL,               -- e.g. 'local-sftp-stand-in', 'as2-file'
    file_name           TEXT NOT NULL,
    doc_type            TEXT,                        -- '850','855','856','810','832' or NULL
    status              TEXT NOT NULL,               -- 'sent','received','archived','failed'
    error               TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_transport_vendor ON edi_transport_records (vendor_id, created_at);
