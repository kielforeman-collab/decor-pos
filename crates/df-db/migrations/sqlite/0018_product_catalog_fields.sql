-- decor-pos Product Catalog Completeness — SQLite edge schema addition.
-- Preserves Product UPC, category, line, color, raw size code, and full
-- lossless source spreadsheet metadata (`source_metadata_json`) from
-- imported .xls/.xlsx/.csv price lists.
-- No f32/f64 anywhere. No COLORx code. No PAN/CVV columns.

ALTER TABLE products ADD COLUMN upc TEXT;
ALTER TABLE products ADD COLUMN category TEXT;
ALTER TABLE products ADD COLUMN line_code TEXT;
ALTER TABLE products ADD COLUMN color_name TEXT;
ALTER TABLE products ADD COLUMN size_code TEXT;
ALTER TABLE products ADD COLUMN source_metadata_json TEXT;

CREATE INDEX idx_products_org_upc ON products (org_id, upc);
