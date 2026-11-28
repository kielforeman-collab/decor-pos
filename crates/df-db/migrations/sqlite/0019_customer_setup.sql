-- decor-pos Customer Setup — SQLite edge schema addition.
-- Adds the billing/mailing address and free-form account notes the
-- counter's "New customer" / "Edit customer" form captures. All
-- columns are nullable so existing customers stay valid.
-- No f32/f64 anywhere. No COLORx code. No PAN/CVV columns.

ALTER TABLE customers ADD COLUMN address_line1 TEXT;
ALTER TABLE customers ADD COLUMN address_line2 TEXT;
ALTER TABLE customers ADD COLUMN city TEXT;
ALTER TABLE customers ADD COLUMN province TEXT;
ALTER TABLE customers ADD COLUMN postal_code TEXT;
ALTER TABLE customers ADD COLUMN notes TEXT;
