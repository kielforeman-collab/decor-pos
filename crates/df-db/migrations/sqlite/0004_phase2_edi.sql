-- decor-pos Phase 2 (v0.3) — SQLite edge schema additions (spec §5.5, §7 Phase 2 row, file-based EDI pulled forward from Phase 4).
-- Same tables as migrations/postgres/0004_phase2_edi.sql, adapted for SQLite:
--   UUID -> TEXT, JSONB -> TEXT, TIMESTAMPTZ -> TEXT (RFC 3339),
--   money columns are TEXT holding exact decimal strings
--   (spec §5: "scaled integer/text in SQLite"). No f32/f64 anywhere.
--   No PAN/CVV columns. No live vendor transport (SFTP/AS2 is
--   Phase 4); edi_documents stores file-based drops only.
--   SQLite note: following 0002, FKs to HQ reference data are omitted
--   (store_id / product_id); the edge-owned chains
--   (purchase_order_lines and edi_documents -> purchase_orders) are
--   still enforced.

CREATE TABLE purchase_orders (
    id                  TEXT PRIMARY KEY,
    po_number           TEXT NOT NULL,
    vendor_id           TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'draft',
    total               TEXT NOT NULL,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_pos_vendor ON purchase_orders (vendor_id, created_at);
CREATE INDEX idx_pos_store ON purchase_orders (store_id, created_at);

CREATE TABLE purchase_order_lines (
    id                  TEXT PRIMARY KEY,
    po_id               TEXT NOT NULL REFERENCES purchase_orders(id) ON DELETE CASCADE,
    product_id          TEXT NOT NULL,
    line_no             INTEGER NOT NULL,
    vendor_sku          TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    qty                 TEXT NOT NULL,
    uom                 TEXT NOT NULL,
    unit_cost           TEXT NOT NULL
);

CREATE INDEX idx_po_lines_po ON purchase_order_lines (po_id, line_no);

CREATE TABLE edi_documents (
    id                  TEXT PRIMARY KEY,
    doc_type            TEXT NOT NULL,
    direction           TEXT NOT NULL,
    vendor_id           TEXT NOT NULL,
    related_po_id       TEXT REFERENCES purchase_orders(id),
    raw_text            TEXT NOT NULL,
    parsed_json         TEXT NOT NULL DEFAULT '{}',
    received_at         TEXT NOT NULL
);

CREATE INDEX idx_edi_vendor ON edi_documents (vendor_id, received_at);
CREATE INDEX idx_edi_po ON edi_documents (related_po_id, received_at);
CREATE INDEX idx_edi_type ON edi_documents (doc_type, received_at);
