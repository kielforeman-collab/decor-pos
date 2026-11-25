-- Demo seed for the pos-desktop dev path.
-- Applies to the SQLite database used by `df-server edge`.
-- Store/org ids match `PosApp::default_config` (and the
-- `--store-id` / `--org-id` in the README quickstart).
-- Product ids match `apps/pos-desktop/src/catalog.ts`.
-- Run: sqlite3 edge.db < apps/pos-desktop/seed-demo.sql
-- (or: python3 -c "import sqlite3; sqlite3.connect('edge.db').executescript(open('apps/pos-desktop/seed-demo.sql').read())")
-- Re-running is safe: rows are inserted with INSERT OR IGNORE /
-- the tables are keyed by id, so a second run changes nothing
-- except re-inserting the same ids (ignored).

INSERT OR IGNORE INTO organizations (id, name, default_currency, enabled_modules, settings_json, created_at)
VALUES ('00000000-0000-0000-0000-00000000b000', 'Demo Paint Co', 'USD', '{"core_pos": true}', '{}', '2026-11-25T00:00:00Z');

INSERT OR IGNORE INTO stores (id, org_id, code, name, country_code, region_code, postal_code, currency, tax_zone_id, timezone, is_warehouse)
VALUES ('00000000-0000-0000-0000-00000000a000', '00000000-0000-0000-0000-00000000b000', 'MAIN', 'Demo Paint Co', 'US', 'CA', '90210', 'USD', '00000000-0000-7000-8000-0000000000ff', 'America/Los_Angeles', 0);

INSERT OR IGNORE INTO products (id, org_id, sku, manufacturer_code, name, department_id, class_id, product_line_id, primary_vendor_id, product_type, base_uom, list_price, standard_cost, avg_cost, min_margin_pct, sheen, paint_base_code, container_volume_ml, eco_fee_category, tax_class_id, prompt_template_id, is_active)
VALUES
('00000000-0000-7000-8000-000000000001', '00000000-0000-0000-0000-00000000b000', 'BM-REGAL-1G', 'BM', 'Regal Select Interior Gallon', '00000000-0000-7000-8000-0000000000d1', NULL, NULL, NULL, 'paint_base', 'gal', '100.0000', '50.0000', '50.0000', NULL, 'Eggshell', '1X', 3785, 'architectural_paint', '00000000-0000-7000-8000-0000000000d2', NULL, 1),
('00000000-0000-7000-8000-000000000002', '00000000-0000-0000-0000-00000000b000', 'BRUSH-2IN', NULL, '2 in Angle Brush', '00000000-0000-7000-8000-0000000000d1', NULL, NULL, NULL, 'stock', 'ea', '12.5000', '6.0000', '6.0000', NULL, NULL, NULL, NULL, 'none', '00000000-0000-7000-8000-0000000000d2', NULL, 1),
('00000000-0000-7000-8000-000000000003', '00000000-0000-0000-0000-00000000b000', 'ROLLER-9IN', NULL, '9 in Roller Cover', '00000000-0000-7000-8000-0000000000d1', NULL, NULL, NULL, 'stock', 'ea', '8.9900', '4.0000', '4.0000', NULL, NULL, NULL, NULL, 'none', '00000000-0000-7000-8000-0000000000d2', NULL, 1);
