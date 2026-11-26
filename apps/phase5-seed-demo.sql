-- Demo seed for the Phase 5 apps (mobile-pwa, web-portal, web-storefront).
-- Applies to the SQLite database used by `df-server edge`, on top of
-- apps/pos-desktop/seed-demo.sql (same org/store/product ids).
-- Adds: one portal-enabled contractor (C-PORTAL), two open charges
-- (400.00 + 300.00 = 700.00), one stored formula (OC-17 White Dove),
-- and 10 gallons on hand for BOPIS pickup.
-- Run: sqlite3 edge.db < apps/phase5-seed-demo.sql
-- (or: python3 -c "import sqlite3; sqlite3.connect('edge.db').executescript(open('apps/phase5-seed-demo.sql').read())")
-- Re-running is safe: INSERT OR IGNORE on keyed ids.
--
-- Portal demo contractor id (paste into the portal's customer field,
-- or open the portal with ?customer=00000000-0000-7000-8000-000000000101):
--   00000000-0000-7000-8000-000000000101

INSERT OR IGNORE INTO customers (id, org_id, account_number, customer_type, company_name, first_name, last_name, email, phone, price_level_id, salesperson_id, tax_exempt_cert, tax_exempt_expires, charge_enabled, credit_limit, payment_terms, terms_days, finance_rate_pct_monthly, credit_hold, require_po_number, require_job_name, portal_enabled)
VALUES ('00000000-0000-7000-8000-000000000101', '00000000-0000-0000-0000-00000000b000', 'C-PORTAL', 'contractor', 'Portal Painting', NULL, NULL, 'portal@example.invalid', NULL, NULL, NULL, NULL, NULL, 1, '5000.0000', 'NET30', NULL, '1.500', 0, 0, 0, 1);

INSERT OR IGNORE INTO ar_ledger (id, org_id, store_id, customer_id, job_id, entry_type, reference_type, reference_id, amount, due_date, statement_id, created_at)
VALUES
('00000000-0000-7000-8000-000000000111', '00000000-0000-0000-0000-00000000b000', '00000000-0000-0000-0000-00000000a000', '00000000-0000-7000-8000-000000000101', NULL, 'charge', 'transaction', '00000000-0000-7000-8000-000000000121', '400.0000', '2026-11-20', NULL, '2026-11-10T00:00:00Z'),
('00000000-0000-7000-8000-000000000112', '00000000-0000-0000-0000-00000000b000', '00000000-0000-0000-0000-00000000a000', '00000000-0000-7000-8000-000000000101', NULL, 'charge', 'transaction', '00000000-0000-7000-8000-000000000122', '300.0000', '2026-12-05', NULL, '2026-11-12T00:00:00Z');

INSERT OR IGNORE INTO tint_formulas (id, org_id, store_id, customer_id, job_id, color_id, color_code_snapshot, color_name_snapshot, base_product_id, container_volume_ml, sheen, room_or_usage, is_custom_match, colorants_json, total_tint_cost, tint_surcharge, dispenser_name, external_formula_ref, dispensed_at)
VALUES ('00000000-0000-7000-8000-000000000131', '00000000-0000-0000-0000-00000000b000', '00000000-0000-0000-0000-00000000a000', '00000000-0000-7000-8000-000000000101', NULL, NULL, 'OC-17', 'White Dove', '00000000-0000-7000-8000-000000000001', 3785, 'Eggshell', 'Living Room', 0, '[{"code": "Y3", "oz": 0, "shots": 14}]', '0.6225', '0', NULL, NULL, '2026-11-09T00:00:00Z');

INSERT OR IGNORE INTO inventory_ledger (id, org_id, store_id, product_id, lot_number, delta_on_hand, delta_committed, delta_on_order, delta_in_transit, unit_cost, reason_type, reference_id, created_at)
VALUES ('00000000-0000-7000-8000-000000000141', '00000000-0000-0000-0000-00000000b000', '00000000-0000-0000-0000-00000000a000', '00000000-0000-7000-8000-000000000001', NULL, '10', '0', '0', '0', '50.0000', 'po_receive', '00000000-0000-7000-8000-000000000151', '2026-11-25T00:00:00Z');
