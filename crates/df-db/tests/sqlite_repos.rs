//! SQLite round-trip tests for df-db (Phase 0).
//!
//! Covers: migrations apply, UUIDv7 ids, NUMERIC(14,4) money round-trip
//! with no float drift, pricing-rule persistence + domain evaluation,
//! tint-formula JSON, and append-only AR ledger balances.

use chrono::{NaiveDate, Utc};
use df_db::models::{
    ArLedgerEntry, ColorCatalogEntry, Customer, Organization, PricingRuleRow, Product, Store,
    TintFormula,
};
use df_db::{Dialect, connect, migrate};
use df_domain::pricing::evaluate_price;
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn test_pool() -> df_db::AnyPool {
    let pool = connect("sqlite::memory:").await.unwrap();
    migrate(&pool, Dialect::Sqlite).await.unwrap();
    pool
}

fn org() -> Organization {
    Organization {
        id: Uuid::now_v7(),
        name: "Test Paint Co".to_string(),
        default_currency: "USD".to_string(),
        enabled_modules: serde_json::json!({"core_pos": true}),
        settings_json: serde_json::json!({}),
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn migrations_apply_and_org_round_trips() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let back = df_db::get_organization(&pool, org.id).await.unwrap();
    assert_eq!(back.name, "Test Paint Co");
    assert_eq!(back.default_currency, "USD");
    assert_eq!(back.enabled_modules, serde_json::json!({"core_pos": true}));
    // UUIDv7 ids are time-ordered: a later id sorts after an earlier one.
    assert!(Uuid::now_v7() > org.id);
}

#[tokio::test]
async fn store_and_product_round_trip_with_exact_money() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = Store {
        id: Uuid::now_v7(),
        org_id: org.id,
        code: "MAIN".to_string(),
        name: "Main Store".to_string(),
        country_code: "US".to_string(),
        region_code: "CA".to_string(),
        postal_code: "90210".to_string(),
        currency: "USD".to_string(),
        tax_zone_id: Uuid::now_v7(),
        timezone: "America/Los_Angeles".to_string(),
        is_warehouse: false,
    };
    df_db::insert_store(&pool, &store).await.unwrap();
    let back = df_db::get_store_by_code(&pool, org.id, "MAIN")
        .await
        .unwrap();
    assert_eq!(back.name, "Main Store");
    assert!(!back.is_warehouse);

    let product = Product {
        id: Uuid::now_v7(),
        org_id: org.id,
        sku: "BM-REGAL-1G".to_string(),
        manufacturer_code: Some("BM".to_string()),
        name: "Regal Select Interior Gallon".to_string(),
        department_id: Uuid::now_v7(),
        class_id: None,
        product_line_id: None,
        primary_vendor_id: None,
        product_type: "paint_base".to_string(),
        base_uom: "gal".to_string(),
        list_price: dec("64.9900"),
        standard_cost: dec("38.5000"),
        avg_cost: dec("38.5000"),
        min_margin_pct: Some(dec("25.000")),
        sheen: Some("Eggshell".to_string()),
        paint_base_code: Some("1X".to_string()),
        container_volume_ml: Some(3785),
        eco_fee_category: Some("architectural_paint".to_string()),
        tax_class_id: Uuid::now_v7(),
        prompt_template_id: None,
        is_active: true,
        map: None,
        msrp: None,
    };
    df_db::insert_product(&pool, &product).await.unwrap();
    let back = df_db::get_product_by_sku(&pool, org.id, "BM-REGAL-1G")
        .await
        .unwrap();
    assert_eq!(back.list_price, dec("64.9900"));
    assert_eq!(back.standard_cost, dec("38.5000"));
    assert_eq!(back.container_volume_ml, Some(3785));
    assert!(back.is_active);
}

#[tokio::test]
async fn color_and_tint_formula_round_trip() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_id = Uuid::now_v7();
    let product_id = Uuid::now_v7();
    // Minimal product row so the tint formula FK holds.
    let product = Product {
        id: product_id,
        org_id: org.id,
        sku: "BASE-1G".to_string(),
        manufacturer_code: None,
        name: "Base Gallon".to_string(),
        department_id: Uuid::now_v7(),
        class_id: None,
        product_line_id: None,
        primary_vendor_id: None,
        product_type: "paint_base".to_string(),
        base_uom: "gal".to_string(),
        list_price: dec("50.0000"),
        standard_cost: dec("30.0000"),
        avg_cost: dec("30.0000"),
        min_margin_pct: None,
        sheen: None,
        paint_base_code: Some("1X".to_string()),
        container_volume_ml: Some(3785),
        eco_fee_category: Some("architectural_paint".to_string()),
        tax_class_id: Uuid::now_v7(),
        prompt_template_id: None,
        is_active: true,
        map: None,
        msrp: None,
    };
    df_db::insert_product(&pool, &product).await.unwrap();

    let color = ColorCatalogEntry {
        id: Uuid::now_v7(),
        org_id: org.id,
        manufacturer: "Benjamin Moore".to_string(),
        color_code: "OC-17".to_string(),
        color_name: "White Dove".to_string(),
        collection: Some("Off-White".to_string()),
        rgb_hex: Some("#EFEDE6".to_string()),
        recommended_base: Some("1X".to_string()),
    };
    df_db::insert_color(&pool, &color).await.unwrap();
    let back = df_db::get_color_by_code(&pool, org.id, "Benjamin Moore", "OC-17")
        .await
        .unwrap();
    assert_eq!(back.color_name, "White Dove");

    let customer_id = Uuid::now_v7();
    let formula = TintFormula {
        id: Uuid::now_v7(),
        org_id: org.id,
        store_id,
        customer_id: Some(customer_id),
        job_id: None,
        color_id: Some(color.id),
        color_code_snapshot: "OC-17".to_string(),
        color_name_snapshot: "White Dove".to_string(),
        base_product_id: product_id,
        container_volume_ml: 3785,
        sheen: Some("Eggshell".to_string()),
        room_or_usage: Some("Master Bedroom Trim".to_string()),
        is_custom_match: false,
        colorants_json: serde_json::json!([
            {"code": "Y3", "oz": 0, "shots": 14, "sub_shots": 2, "unit_cost": "0.42"}
        ]),
        total_tint_cost: dec("0.6225"),
        tint_surcharge: dec("0.0000"),
        dispenser_name: Some("virtual-simulator".to_string()),
        external_formula_ref: None,
        dispensed_at: Utc::now(),
    };
    df_db::insert_tint_formula(&pool, &formula).await.unwrap();
    let formulas = df_db::list_tint_formulas_for_customer(&pool, customer_id)
        .await
        .unwrap();
    assert_eq!(formulas.len(), 1);
    assert_eq!(formulas[0].total_tint_cost, dec("0.6225"));
    assert_eq!(
        formulas[0].colorants_json[0]["code"],
        serde_json::json!("Y3")
    );
}

#[tokio::test]
async fn pricing_rule_persists_and_evaluates() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let rule = PricingRuleRow {
        id: Uuid::now_v7(),
        org_id: org.id,
        priority: 10,
        customer_id: None,
        job_id: None,
        price_level_id: None,
        product_id: None,
        product_line_id: None,
        class_id: None,
        department_id: None,
        vendor_id: None,
        min_qty: dec("1"),
        calc_method: "pct_off_list".to_string(),
        calc_value: dec("20"),
        min_margin_floor: Some(dec("30")),
        effective_from: None,
        effective_to: None,
    };
    df_db::insert_pricing_rule(&pool, &rule).await.unwrap();
    let rules = df_db::list_pricing_rules(&pool, org.id).await.unwrap();
    assert_eq!(rules.len(), 1);
    let domain_rules: Vec<_> = rules.iter().map(|r| r.to_domain().unwrap()).collect();
    let input = df_domain::pricing::PricingInput {
        list_price: df_domain::Money::from_units(100, df_domain::Currency::Usd),
        cost: df_domain::Money::from_units(50, df_domain::Currency::Usd),
        qty: dec("1"),
        customer_id: None,
        job_id: None,
        price_level_id: None,
        product_id: None,
        product_line_id: None,
        class_id: None,
        department_id: None,
        vendor_id: None,
        on: NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
    };
    let outcome = evaluate_price(&domain_rules, &input);
    assert_eq!(
        outcome.unit_price,
        df_domain::Money::from_units(80, df_domain::Currency::Usd)
    );
    assert!(!outcome.floor_applied);
}

#[tokio::test]
async fn ar_ledger_balance_is_derived_from_append_only_rows() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let customer = Customer {
        id: Uuid::now_v7(),
        org_id: org.id,
        account_number: "C-1001".to_string(),
        customer_type: "contractor".to_string(),
        company_name: Some("Acme Painting".to_string()),
        first_name: None,
        last_name: None,
        email: None,
        phone: None,
        price_level_id: None,
        salesperson_id: None,
        tax_exempt_cert: None,
        tax_exempt_expires: None,
        charge_enabled: true,
        credit_limit: dec("5000.0000"),
        payment_terms: "NET30".to_string(),
        terms_days: None,
        finance_rate_pct_monthly: None,
        credit_hold: false,
        require_po_number: false,
        require_job_name: false,
        portal_enabled: false,
    };
    df_db::insert_customer(&pool, &customer).await.unwrap();
    let back = df_db::get_customer_by_account(&pool, org.id, "C-1001")
        .await
        .unwrap();
    assert_eq!(back.credit_limit, dec("5000.0000"));
    assert!(back.charge_enabled);

    let store_id = Uuid::now_v7();
    let charge = ArLedgerEntry {
        id: Uuid::now_v7(),
        org_id: org.id,
        store_id,
        customer_id: customer.id,
        job_id: None,
        entry_type: "charge".to_string(),
        reference_type: "transaction".to_string(),
        reference_id: Uuid::now_v7(),
        amount: dec("250.0000"),
        due_date: Some(NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()),
        statement_id: None,
        created_at: Utc::now(),
    };
    let payment = ArLedgerEntry {
        amount: dec("-100.0000"),
        entry_type: "payment".to_string(),
        reference_type: "payment".to_string(),
        id: Uuid::now_v7(),
        ..charge.clone()
    };
    df_db::insert_ar_entry(&pool, &charge).await.unwrap();
    df_db::insert_ar_entry(&pool, &payment).await.unwrap();
    let balance = df_db::ar_balance_for_customer(&pool, customer.id)
        .await
        .unwrap();
    assert_eq!(balance, dec("150.0000"));
}
