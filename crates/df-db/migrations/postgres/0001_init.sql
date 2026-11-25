-- decor-pos Phase 0 (v0.1) — PostgreSQL HQ schema (spec §5).
-- Financial invariants: NUMERIC(14,4) money, UUIDv7 primary keys,
-- append-only ledgers (ar_ledger). No f32/f64 anywhere.

CREATE TABLE organizations (
    id              UUID PRIMARY KEY,
    name            TEXT NOT NULL,
    default_currency CHAR(3) NOT NULL DEFAULT 'USD',
    enabled_modules JSONB NOT NULL DEFAULT '{}',
    settings_json   JSONB NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE stores (
    id              UUID PRIMARY KEY,
    org_id          UUID NOT NULL REFERENCES organizations(id),
    code            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    country_code    CHAR(2) NOT NULL,
    region_code     TEXT NOT NULL,
    postal_code     TEXT NOT NULL,
    currency        CHAR(3) NOT NULL DEFAULT 'USD',
    tax_zone_id     UUID NOT NULL,
    timezone        TEXT NOT NULL,
    is_warehouse    BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE users (
    id              UUID PRIMARY KEY,
    org_id          UUID NOT NULL REFERENCES organizations(id),
    email           TEXT NOT NULL UNIQUE,
    display_name    TEXT NOT NULL,
    password_hash   TEXT,
    oidc_subject    TEXT UNIQUE,
    counter_pin_hash TEXT,
    badge_token_hash TEXT UNIQUE,
    role_id         UUID NOT NULL,
    active          BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE audit_logs (
    id              UUID PRIMARY KEY,
    org_id          UUID NOT NULL,
    store_id        UUID,
    actor_user_id   UUID NOT NULL,
    approver_user_id UUID,
    action          TEXT NOT NULL,
    entity_type     TEXT NOT NULL,
    entity_id       UUID NOT NULL,
    reason_code     TEXT,
    before_json     JSONB,
    after_json      JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE products (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    sku                 TEXT NOT NULL,
    manufacturer_code   TEXT,
    name                TEXT NOT NULL,
    department_id       UUID NOT NULL,
    class_id            UUID,
    product_line_id     UUID,
    primary_vendor_id   UUID,
    product_type        TEXT NOT NULL,
    base_uom            TEXT NOT NULL,
    list_price          NUMERIC(14,4) NOT NULL,
    standard_cost       NUMERIC(14,4) NOT NULL,
    avg_cost            NUMERIC(14,4) NOT NULL,
    min_margin_pct      NUMERIC(6,3),
    sheen               TEXT,
    paint_base_code     TEXT,
    container_volume_ml INTEGER,
    eco_fee_category    TEXT,
    tax_class_id        UUID NOT NULL,
    prompt_template_id  UUID,
    is_active           BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE (org_id, sku)
);

CREATE TABLE product_barcodes (
    barcode             TEXT NOT NULL,
    org_id              UUID NOT NULL,
    product_id          UUID NOT NULL REFERENCES products(id),
    uom_code            TEXT NOT NULL,
    qty_multiplier      NUMERIC(12,4) NOT NULL DEFAULT 1,
    PRIMARY KEY (org_id, barcode)
);

CREATE TABLE product_uoms (
    id                  UUID PRIMARY KEY,
    product_id          UUID NOT NULL REFERENCES products(id),
    uom_code            TEXT NOT NULL,
    to_base_factor      NUMERIC(14,6) NOT NULL,
    is_sellable         BOOLEAN NOT NULL DEFAULT TRUE,
    is_purchasable      BOOLEAN NOT NULL DEFAULT FALSE,
    default_waste_pct   NUMERIC(5,2) DEFAULT 0,
    UNIQUE (product_id, uom_code)
);

CREATE TABLE wallpaper_book_index (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    book_code           TEXT NOT NULL,
    book_title          TEXT NOT NULL,
    page_number         TEXT NOT NULL,
    pattern_number      TEXT NOT NULL,
    product_id          UUID NOT NULL REFERENCES products(id),
    dye_lot_required    BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE (org_id, book_code, page_number, pattern_number)
);

CREATE TABLE eco_fee_rules (
    id                  UUID PRIMARY KEY,
    country_code        CHAR(2) NOT NULL,
    region_code         TEXT NOT NULL,
    eco_fee_category    TEXT NOT NULL,
    min_volume_ml       INTEGER NOT NULL,
    max_volume_ml       INTEGER NOT NULL,
    fee_amount          NUMERIC(10,4) NOT NULL,
    is_taxable          BOOLEAN NOT NULL DEFAULT TRUE,
    effective_from      DATE NOT NULL,
    effective_to        DATE
);

CREATE TABLE color_catalog (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    manufacturer        TEXT NOT NULL,
    color_code          TEXT NOT NULL,
    color_name          TEXT NOT NULL,
    collection          TEXT,
    rgb_hex             CHAR(7),
    recommended_base    TEXT,
    UNIQUE (org_id, manufacturer, color_code)
);

CREATE TABLE colorants (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    system_name         TEXT NOT NULL,
    code                TEXT NOT NULL,
    name                TEXT NOT NULL,
    cost_per_fl_oz      NUMERIC(12,6) NOT NULL,
    shots_per_fl_oz     INTEGER NOT NULL DEFAULT 48,
    UNIQUE (org_id, system_name, code)
);

CREATE TABLE tint_formulas (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    store_id            UUID NOT NULL,
    customer_id         UUID,
    job_id              UUID,
    color_id            UUID REFERENCES color_catalog(id),
    color_code_snapshot TEXT NOT NULL,
    color_name_snapshot TEXT NOT NULL,
    base_product_id     UUID NOT NULL REFERENCES products(id),
    container_volume_ml INTEGER NOT NULL,
    sheen               TEXT,
    room_or_usage       TEXT,
    is_custom_match     BOOLEAN NOT NULL DEFAULT FALSE,
    colorants_json      JSONB NOT NULL,
    total_tint_cost     NUMERIC(14,4) NOT NULL DEFAULT 0,
    tint_surcharge      NUMERIC(14,4) NOT NULL DEFAULT 0,
    dispenser_name      TEXT,
    external_formula_ref TEXT,
    dispensed_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_formulas_customer_search
    ON tint_formulas (org_id, customer_id, job_id, color_code_snapshot, dispensed_at DESC);

CREATE TABLE customers (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    account_number      TEXT NOT NULL,
    customer_type       TEXT NOT NULL,
    company_name        TEXT,
    first_name          TEXT,
    last_name           TEXT,
    email               TEXT,
    phone               TEXT,
    price_level_id      UUID,
    salesperson_id      UUID REFERENCES users(id),
    tax_exempt_cert     TEXT,
    tax_exempt_expires  DATE,
    charge_enabled      BOOLEAN NOT NULL DEFAULT FALSE,
    credit_limit        NUMERIC(14,4) NOT NULL DEFAULT 0,
    payment_terms       TEXT DEFAULT 'NET30',
    credit_hold         BOOLEAN NOT NULL DEFAULT FALSE,
    require_po_number   BOOLEAN NOT NULL DEFAULT FALSE,
    require_job_name    BOOLEAN NOT NULL DEFAULT FALSE,
    portal_enabled      BOOLEAN NOT NULL DEFAULT FALSE,
    UNIQUE (org_id, account_number)
);

CREATE TABLE customer_jobs (
    id                  UUID PRIMARY KEY,
    customer_id         UUID NOT NULL REFERENCES customers(id),
    job_code            TEXT NOT NULL,
    job_name            TEXT NOT NULL,
    site_address_json   JSONB,
    tax_zone_override   UUID,
    is_active           BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE (customer_id, job_code)
);

CREATE TABLE pricing_rules (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    priority            INTEGER NOT NULL,
    customer_id         UUID REFERENCES customers(id),
    job_id              UUID REFERENCES customer_jobs(id),
    price_level_id      UUID,
    product_id          UUID REFERENCES products(id),
    product_line_id     UUID,
    class_id            UUID,
    department_id       UUID,
    vendor_id           UUID,
    min_qty             NUMERIC(12,4) NOT NULL DEFAULT 1,
    calc_method         TEXT NOT NULL,
    calc_value          NUMERIC(14,4) NOT NULL,
    min_margin_floor    NUMERIC(6,3),
    effective_from      TIMESTAMPTZ,
    effective_to        TIMESTAMPTZ
);

CREATE INDEX idx_pricing_rules_lookup
    ON pricing_rules (org_id, priority);

CREATE TABLE ar_ledger (
    id                  UUID PRIMARY KEY,
    org_id              UUID NOT NULL,
    store_id            UUID NOT NULL,
    customer_id         UUID NOT NULL REFERENCES customers(id),
    job_id              UUID REFERENCES customer_jobs(id),
    entry_type          TEXT NOT NULL,
    reference_type      TEXT NOT NULL,
    reference_id        UUID NOT NULL,
    amount              NUMERIC(14,4) NOT NULL,
    due_date            DATE,
    statement_id        UUID,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_ar_customer
    ON ar_ledger (customer_id, created_at);

CREATE TABLE ar_allocations (
    id                  UUID PRIMARY KEY,
    payment_entry_id    UUID NOT NULL REFERENCES ar_ledger(id),
    charge_entry_id     UUID NOT NULL REFERENCES ar_ledger(id),
    allocated_amount    NUMERIC(14,4) NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
