//! SQLite round-trip tests for the Phase 2 tint formula-tracking
//! tables (spec §5.3/§7): color search, colorant catalog CRUD, and
//! formula lookup by id / color / customer history with job filter.

use chrono::Utc;
use df_db::models::{ColorCatalogEntry, Colorant, Organization, Product, TintFormula};
use df_db::{DbError, Dialect, connect, migrate};
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

fn product(org_id: Uuid) -> Product {
    Product {
        id: Uuid::now_v7(),
        org_id,
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
    }
}

fn color(org_id: Uuid, manufacturer: &str, code: &str, name: &str) -> ColorCatalogEntry {
    ColorCatalogEntry {
        id: Uuid::now_v7(),
        org_id,
        manufacturer: manufacturer.to_string(),
        color_code: code.to_string(),
        color_name: name.to_string(),
        collection: Some("Off-White".to_string()),
        rgb_hex: Some("#EFEDE6".to_string()),
        recommended_base: Some("1X".to_string()),
    }
}

fn colorant(org_id: Uuid, system: &str, code: &str, cost: &str) -> Colorant {
    Colorant {
        id: Uuid::now_v7(),
        org_id,
        system_name: system.to_string(),
        code: code.to_string(),
        name: format!("{code} colorant"),
        cost_per_fl_oz: dec(cost),
        shots_per_fl_oz: 48,
    }
}

#[allow(clippy::too_many_arguments)]
fn formula(
    org_id: Uuid,
    store_id: Uuid,
    product_id: Uuid,
    customer_id: Option<Uuid>,
    job_id: Option<Uuid>,
    code: &str,
    name: &str,
    dispensed_at: chrono::DateTime<Utc>,
) -> TintFormula {
    TintFormula {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        customer_id,
        job_id,
        color_id: None,
        color_code_snapshot: code.to_string(),
        color_name_snapshot: name.to_string(),
        base_product_id: product_id,
        container_volume_ml: 3785,
        sheen: Some("Eggshell".to_string()),
        room_or_usage: Some("Master Bedroom Trim".to_string()),
        is_custom_match: false,
        colorants_json: serde_json::json!([
            {"code": "Y3", "oz": 0, "shots": 14, "sub_shots": 2, "unit_cost": "0.42"}
        ]),
        total_tint_cost: dec("0.1225"),
        tint_surcharge: dec("0.0000"),
        dispenser_name: Some("virtual-simulator".to_string()),
        external_formula_ref: None,
        dispensed_at,
    }
}

#[tokio::test]
async fn color_search_is_case_insensitive_and_manufacturer_scoped() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let bm_dove = color(org.id, "Benjamin Moore", "OC-17", "White Dove");
    df_db::insert_color(&pool, &bm_dove).await.unwrap();
    df_db::insert_color(
        &pool,
        &color(org.id, "Benjamin Moore", "HC-172", "Revere Pewter"),
    )
    .await
    .unwrap();
    // Same color name, different manufacturer — must not leak across.
    df_db::insert_color(&pool, &color(org.id, "PPG", "PPG1012-1", "White Dove"))
        .await
        .unwrap();

    let found = df_db::search_colors(&pool, org.id, "Benjamin Moore", "white dove")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].color_code, "OC-17");

    let found = df_db::search_colors(&pool, org.id, "benjamin moore", "OC-")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);

    let found = df_db::search_colors(&pool, org.id, "Benjamin Moore", "ew")
        .await
        .unwrap();
    assert_eq!(found.len(), 1); // Revere Pewter
    assert_eq!(found[0].color_code, "HC-172");

    let found = df_db::search_colors(&pool, org.id, "PPG", "White Dove")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].color_code, "PPG1012-1");

    // LIKE metacharacters in the query are literal, not wildcards.
    let found = df_db::search_colors(&pool, org.id, "Benjamin Moore", "%")
        .await
        .unwrap();
    assert!(found.is_empty());

    let back = df_db::get_color(&pool, bm_dove.id).await.unwrap();
    assert_eq!(back.color_name, "White Dove");
}

#[tokio::test]
async fn colorant_crud() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let mut y3 = colorant(org.id, "Gennex", "Y3", "0.420000");
    df_db::insert_colorant(&pool, &y3).await.unwrap();

    let back = df_db::get_colorant(&pool, y3.id).await.unwrap();
    assert_eq!(back.code, "Y3");
    assert_eq!(back.cost_per_fl_oz, dec("0.420000"));
    let by_code = df_db::get_colorant_by_code(&pool, org.id, "Gennex", "Y3")
        .await
        .unwrap();
    assert_eq!(by_code.id, y3.id);

    y3.cost_per_fl_oz = dec("0.450000");
    y3.name = "Yellow Oxide".to_string();
    df_db::update_colorant(&pool, &y3).await.unwrap();
    assert_eq!(
        df_db::get_colorant(&pool, y3.id)
            .await
            .unwrap()
            .cost_per_fl_oz,
        dec("0.450000")
    );

    let system = df_db::list_colorants_for_system(&pool, org.id, "Gennex")
        .await
        .unwrap();
    assert_eq!(system.len(), 1);

    df_db::delete_colorant(&pool, y3.id).await.unwrap();
    assert!(matches!(
        df_db::get_colorant(&pool, y3.id).await,
        Err(DbError::NotFound(_))
    ));
}

#[tokio::test]
async fn formula_lookup_by_id_color_and_customer_history() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_id = Uuid::now_v7();
    let product = product(org.id);
    df_db::insert_product(&pool, &product).await.unwrap();
    let customer_id = Uuid::now_v7();
    let other_customer = Uuid::now_v7();
    let job_id = Uuid::now_v7();
    let t0 = Utc::now();
    let t1 = t0 + chrono::Duration::seconds(10);
    let t2 = t0 + chrono::Duration::seconds(20);

    let f1 = formula(
        org.id,
        store_id,
        product.id,
        Some(customer_id),
        Some(job_id),
        "OC-17",
        "White Dove",
        t0,
    );
    let f2 = formula(
        org.id,
        store_id,
        product.id,
        Some(customer_id),
        None,
        "HC-172",
        "Revere Pewter",
        t2,
    );
    let f3 = formula(
        org.id,
        store_id,
        product.id,
        Some(other_customer),
        None,
        "OC-65",
        "Chantilly Lace",
        t1,
    );
    for f in [&f1, &f2, &f3] {
        df_db::insert_tint_formula(&pool, f).await.unwrap();
    }

    let back = df_db::get_tint_formula(&pool, f1.id).await.unwrap();
    assert_eq!(back.color_code_snapshot, "OC-17");
    assert_eq!(back.total_tint_cost, dec("0.1225"));

    let found = df_db::search_tint_formulas_by_color(&pool, org.id, "white dove")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, f1.id);

    // Customer history, newest first.
    let history = df_db::list_tint_formulas_for_customer_job(&pool, customer_id, None)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].id, f2.id); // t2
    assert_eq!(history[1].id, f1.id); // t0

    // Scoped to one job: only f1.
    let for_job = df_db::list_tint_formulas_for_customer_job(&pool, customer_id, Some(job_id))
        .await
        .unwrap();
    assert_eq!(for_job.len(), 1);
    assert_eq!(for_job[0].id, f1.id);

    // The pre-existing customer-only listing still works.
    let legacy = df_db::list_tint_formulas_for_customer(&pool, customer_id)
        .await
        .unwrap();
    assert_eq!(legacy.len(), 2);
}
