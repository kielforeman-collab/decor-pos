-- decor-pos Phase 0 (v0.1) — PostgreSQL HQ schema (spec §5).
-- Financial invariants: money as exact decimal TEXT, UUIDv7 ids as TEXT,
-- append-only ledgers (ar_ledger). No f32/f64 anywhere.

CREATE TABLE organizations (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    default_currency TEXT NOT NULL DEFAULT 'USD',
    enabled_modules TEXT NOT NULL DEFAULT '{}',
    settings_json   TEXT NOT NULL DEFAULT '{}',
    created_at      TEXT NOT NULL
);

CREATE TABLE stores (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL REFERENCES organizations(id),
    code            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    country_code    TEXT NOT NULL,
    region_code     TEXT NOT NULL,
    postal_code     TEXT NOT NULL,
    currency        TEXT NOT NULL DEFAULT 'USD',
    tax_zone_id     TEXT NOT NULL,
    timezone        TEXT NOT NULL,
    is_warehouse    BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE users (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL REFERENCES organizations(id),
    email           TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    password_hash   TEXT,
    oidc_subject    TEXT UNIQUE,
    counter_pin_hash TEXT,
    badge_token_hash TEXT UNIQUE,
    role_id         TEXT NOT NULL,
    active          BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE audit_logs (
    id              TEXT PRIMARY KEY,
    org_id          TEXT NOT NULL,
    store_id        TEXT,
    actor_user_id   TEXT NOT NULL,
    approver_user_id TEXT,
    action          TEXT NOT NULL,
    entity_type     TEXT NOT NULL,
    entity_id       TEXT NOT NULL,
    reason_code     TEXT,
    before_json     TEXT,
    after_json      TEXT,
    created_at      TEXT NOT NULL
);

CREATE TABLE products (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    sku                 TEXT NOT NULL,
    manufacturer_code   TEXT,
    name                TEXT NOT NULL,
    department_id       TEXT NOT NULL,
    class_id            TEXT,
    product_line_id     TEXT,
    primary_vendor_id   TEXT,
    product_type        TEXT NOT NULL,
    base_uom            TEXT NOT NULL,
    list_price          TEXT NOT NULL,
    standard_cost       TEXT NOT NULL,
    avg_cost            TEXT NOT NULL,
    min_margin_pct      TEXT,
    sheen               TEXT,
    paint_base_code     TEXT,
    container_volume_ml INTEGER,
    eco_fee_category    TEXT,
    tax_class_id        TEXT NOT NULL,
    prompt_template_id  TEXT,
    is_active           BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE (org_id, sku)
);

CREATE TABLE product_barcodes (
    barcode             TEXT NOT NULL,
    org_id              TEXT NOT NULL,
    product_id          TEXT NOT NULL REFERENCES products(id),
    uom_code            TEXT NOT NULL,
    qty_multiplier      TEXT NOT NULL DEFAULT 1,
    PRIMARY KEY (org_id, barcode)
);

CREATE TABLE product_uoms (
    id                  TEXT PRIMARY KEY,
    product_id          TEXT NOT NULL REFERENCES products(id),
    uom_code            TEXT NOT NULL,
    to_base_factor      TEXT NOT NULL,
    is_sellable         BOOLEAN NOT NULL DEFAULT TRUE,
    is_purchasable      BOOLEAN NOT NULL DEFAULT FALSE,
    default_waste_pct   TEXT DEFAULT 0,
    UNIQUE (product_id, uom_code)
);

CREATE TABLE wallpaper_book_index (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    book_code           TEXT NOT NULL,
    book_title          TEXT NOT NULL,
    page_number         TEXT NOT NULL,
    pattern_number      TEXT NOT NULL,
    product_id          TEXT NOT NULL REFERENCES products(id),
    dye_lot_required    BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE (org_id, book_code, page_number, pattern_number)
);

CREATE TABLE eco_fee_rules (
    id                  TEXT PRIMARY KEY,
    country_code        TEXT NOT NULL,
    region_code         TEXT NOT NULL,
    eco_fee_category    TEXT NOT NULL,
    min_volume_ml       INTEGER NOT NULL,
    max_volume_ml       INTEGER NOT NULL,
    fee_amount          TEXT NOT NULL,
    is_taxable          BOOLEAN NOT NULL DEFAULT TRUE,
    effective_from      TEXT NOT NULL,
    effective_to        TEXT
);

CREATE TABLE color_catalog (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    manufacturer        TEXT NOT NULL,
    color_code          TEXT NOT NULL,
    color_name          TEXT NOT NULL,
    collection          TEXT,
    rgb_hex             TEXT,
    recommended_base    TEXT,
    UNIQUE (org_id, manufacturer, color_code)
);

CREATE TABLE colorants (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    system_name         TEXT NOT NULL,
    code                TEXT NOT NULL,
    name                TEXT NOT NULL,
    cost_per_fl_oz      TEXT NOT NULL,
    shots_per_fl_oz     INTEGER NOT NULL DEFAULT 48,
    UNIQUE (org_id, system_name, code)
);

CREATE TABLE tint_formulas (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    customer_id         TEXT,
    job_id              TEXT,
    color_id            TEXT REFERENCES color_catalog(id),
    color_code_snapshot TEXT NOT NULL,
    color_name_snapshot TEXT NOT NULL,
    base_product_id     TEXT NOT NULL REFERENCES products(id),
    container_volume_ml INTEGER NOT NULL,
    sheen               TEXT,
    room_or_usage       TEXT,
    is_custom_match     BOOLEAN NOT NULL DEFAULT FALSE,
    colorants_json      TEXT NOT NULL,
    total_tint_cost     TEXT NOT NULL DEFAULT 0,
    tint_surcharge      TEXT NOT NULL DEFAULT 0,
    dispenser_name      TEXT,
    external_formula_ref TEXT,
    dispensed_at        TEXT NOT NULL
);

CREATE INDEX idx_formulas_customer_search
    ON tint_formulas (org_id, customer_id, job_id, color_code_snapshot, dispensed_at DESC);

CREATE TABLE customers (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    account_number      TEXT NOT NULL,
    customer_type       TEXT NOT NULL,
    company_name        TEXT,
    first_name          TEXT,
    last_name           TEXT,
    email               TEXT,
    phone               TEXT,
    price_level_id      TEXT,
    salesperson_id      TEXT REFERENCES users(id),
    tax_exempt_cert     TEXT,
    tax_exempt_expires  TEXT,
    charge_enabled      BOOLEAN NOT NULL DEFAULT FALSE,
    credit_limit        TEXT NOT NULL DEFAULT 0,
    payment_terms       TEXT DEFAULT 'NET30',
    credit_hold         BOOLEAN NOT NULL DEFAULT FALSE,
    require_po_number   BOOLEAN NOT NULL DEFAULT FALSE,
    require_job_name    BOOLEAN NOT NULL DEFAULT FALSE,
    portal_enabled      BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE (org_id, account_number)
);

CREATE TABLE customer_jobs (
    id                  TEXT PRIMARY KEY,
    customer_id         TEXT NOT NULL REFERENCES customers(id),
    job_code            TEXT NOT NULL,
    job_name            TEXT NOT NULL,
    site_address_json   TEXT,
    tax_zone_override   TEXT,
    is_active           BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE (customer_id, job_code)
);

CREATE TABLE pricing_rules (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    priority            INTEGER NOT NULL,
    customer_id         TEXT REFERENCES customers(id),
    job_id              TEXT REFERENCES customer_jobs(id),
    price_level_id      TEXT,
    product_id          TEXT REFERENCES products(id),
    product_line_id     TEXT,
    class_id            TEXT,
    department_id       TEXT,
    vendor_id           TEXT,
    min_qty             TEXT NOT NULL DEFAULT 1,
    calc_method         TEXT NOT NULL,
    calc_value          TEXT NOT NULL,
    min_margin_floor    TEXT,
    effective_from      TEXT,
    effective_to        TEXT
);

CREATE INDEX idx_pricing_rules_lookup
    ON pricing_rules (org_id, priority);

CREATE TABLE ar_ledger (
    id                  TEXT PRIMARY KEY,
    org_id              TEXT NOT NULL,
    store_id            TEXT NOT NULL,
    customer_id         TEXT NOT NULL REFERENCES customers(id),
    job_id              TEXT REFERENCES customer_jobs(id),
    entry_type          TEXT NOT NULL,
    reference_type      TEXT NOT NULL,
    reference_id        TEXT NOT NULL,
    amount              TEXT NOT NULL,
    due_date            TEXT,
    statement_id        TEXT,
    created_at          TEXT NOT NULL
);

CREATE INDEX idx_ar_customer
    ON ar_ledger (customer_id, created_at);

CREATE TABLE ar_allocations (
    id                  TEXT PRIMARY KEY,
    payment_entry_id    TEXT NOT NULL REFERENCES ar_ledger(id),
    charge_entry_id     TEXT NOT NULL REFERENCES ar_ledger(id),
    allocated_amount    TEXT NOT NULL,
    created_at          TEXT NOT NULL
);
