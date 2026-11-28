-- decor-pos Customer Setup — PostgreSQL schema addition.
-- Same columns as migrations/sqlite/0019_customer_setup.sql: the
-- billing/mailing address and free-form account notes captured by
-- the counter's customer setup form. All columns are nullable.
-- No f32/f64 anywhere. No COLORx code. No PAN/CVV columns.

ALTER TABLE customers ADD COLUMN address_line1 TEXT;
ALTER TABLE customers ADD COLUMN address_line2 TEXT;
ALTER TABLE customers ADD COLUMN city TEXT;
ALTER TABLE customers ADD COLUMN province TEXT;
ALTER TABLE customers ADD COLUMN postal_code TEXT;
ALTER TABLE customers ADD COLUMN notes TEXT;
