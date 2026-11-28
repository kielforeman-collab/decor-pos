//! End-to-end edge formula-tracking test (Phase 2 rescoped Nov 26, 2026):
//!
//! 1. Color catalog search (`GET /pos/tinting/colors`)
//! 2. Colorant catalog listing (`GET /pos/tinting/colorants`)
//! 3. Formula creation with authoritative costing (`POST /pos/tinting/formulas`)
//!    and rejection of unknown colorant codes (400)
//! 4. Customer colour history lookup (`GET /pos/tinting/formulas?customer_id=`)
//! 5. Daily usage report (`GET /pos/reports/daily-usage`) reflecting the
//!    tracked formula and exact decimal colorant cost.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use df_auth_http::test_support::seed_test_users;
use df_db::models::{ColorCatalogEntry, Colorant, Customer, Organization, Product, Store};
use df_domain::Currency;
use df_edge::{Edge, EdgeConfig};
use rust_decimal::Decimal;
use std::str::FromStr;
use tower::ServiceExt;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn get_json(edge: &Edge, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let response = edge
        .router()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn post_json(
    edge: &Edge,
    uri: &str,
    body: serde_json::Value,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let response = edge
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn formula_tracking_and_daily_usage_end_to_end() {
    let org_id = Uuid::from_u128(0x2000);
    let store_id = Uuid::from_u128(0x1000);
    let product_id = Uuid::from_u128(0x3000);
    let customer_id = Uuid::from_u128(0x4000);

    let edge = Edge::open(EdgeConfig {
        store_id,
        org_id,
        db_url: "sqlite::memory:".to_string(),
        store_name: "Main Store".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    })
    .await
    .unwrap();

    df_db::insert_organization(
        edge.pool(),
        &Organization {
            id: org_id,
            name: "Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({}),
            settings_json: serde_json::json!({}),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    df_db::insert_store(
        edge.pool(),
        &Store {
            id: store_id,
            org_id,
            code: "MAIN".to_string(),
            name: "Main Store".to_string(),
            country_code: "US".to_string(),
            region_code: "CA".to_string(),
            postal_code: "90210".to_string(),
            currency: "USD".to_string(),
            tax_zone_id: Uuid::now_v7(),
            timezone: "America/Los_Angeles".to_string(),
            is_warehouse: false,
        },
    )
    .await
    .unwrap();
    df_db::insert_product(
        edge.pool(),
        &Product {
            id: product_id,
            org_id,
            sku: "BM-REGAL-1G".to_string(),
            manufacturer_code: Some("BM".to_string()),
            name: "Regal Select Interior Gallon".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: dec("60"),
            standard_cost: dec("30"),
            avg_cost: dec("30"),
            min_margin_pct: None,
            sheen: Some("Eggshell".to_string()),
            paint_base_code: Some("1X".to_string()),
            container_volume_ml: Some(3785),
            eco_fee_category: None,
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            upc: None,
            category: None,
            line_code: None,
            color_name: None,
            size_code: None,
            source_metadata: None,
            is_active: true,
            map: None,
            msrp: None,
        },
    )
    .await
    .unwrap();
    df_db::insert_customer(
        edge.pool(),
        &Customer {
            id: customer_id,
            org_id,
            account_number: "C-100".to_string(),
            customer_type: "contractor".to_string(),
            company_name: Some("Apex Painting".to_string()),
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: true,
            credit_limit: dec("5000"),
            payment_terms: "NET30".to_string(),
            terms_days: Some(30),
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
        },
    )
    .await
    .unwrap();
    df_db::insert_color(
        edge.pool(),
        &ColorCatalogEntry {
            id: Uuid::now_v7(),
            org_id,
            manufacturer: "Benjamin Moore".to_string(),
            color_code: "OC-17".to_string(),
            color_name: "White Dove".to_string(),
            collection: None,
            rgb_hex: Some("#F3EFE6".to_string()),
            recommended_base: None,
        },
    )
    .await
    .unwrap();
    for (code, name, cost) in [
        ("Y3", "Yellow Oxide", "0.4200"),
        ("R1", "Red Oxide", "0.5000"),
    ] {
        df_db::insert_colorant(
            edge.pool(),
            &Colorant {
                id: Uuid::now_v7(),
                org_id,
                system_name: "Gennex".to_string(),
                code: code.to_string(),
                name: name.to_string(),
                cost_per_fl_oz: dec(cost),
                shots_per_fl_oz: 48,
            },
        )
        .await
        .unwrap();
    }

    let users = seed_test_users(edge.pool(), org_id).await;
    let cashier = &users.cashier.token;
    let manager = &users.manager.token;

    // 1. Color search.
    let (status, colors) = get_json(
        &edge,
        "/pos/tinting/colors?manufacturer=Benjamin%20Moore&query=dove",
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{colors}");
    assert_eq!(colors.as_array().unwrap().len(), 1);
    assert_eq!(colors[0]["color_code"], "OC-17");

    // 2. Colorant catalog (sorted by code, no canister levels).
    let (status, colorants) = get_json(&edge, "/pos/tinting/colorants", cashier).await;
    assert_eq!(status, StatusCode::OK, "{colorants}");
    let list = colorants.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["code"], "R1");
    assert_eq!(list[1]["code"], "Y3");

    // 3. Reject unknown colorant code.
    let (status, bad) = post_json(
        &edge,
        "/pos/tinting/formulas",
        serde_json::json!({
            "customer_id": customer_id,
            "color_code": "OC-17",
            "color_name": "White Dove",
            "base_product_id": product_id,
            "container_volume_ml": 3785,
            "sheen": "Eggshell",
            "colorants": [{ "code": "NOPE", "oz": "0", "shots": 4 }]
        }),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");

    // 4. Create tracked formula: Y3 0 oz + 12 shots (0.25 fl oz * 0.42 = 0.1050) + R1 1 oz (0.5000) = 0.6050.
    let (status, created) = post_json(
        &edge,
        "/pos/tinting/formulas",
        serde_json::json!({
            "customer_id": customer_id,
            "color_code": "OC-17",
            "color_name": "White Dove",
            "base_product_id": product_id,
            "container_volume_ml": 3785,
            "sheen": "Eggshell",
            "colorants": [
                { "code": "Y3", "oz": "0", "shots": 12 },
                { "code": "R1", "oz": "1", "shots": 0 }
            ]
        }),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["total_tint_cost"], "0.6050");

    // 5. Customer colour history lookup returns the stored formula.
    let (status, history) = get_json(
        &edge,
        &format!("/pos/tinting/formulas?customer_id={customer_id}"),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["id"], created["id"]);

    // 6. Daily usage report summarizes the recorded formula and colorant usage.
    let today = Utc::now().date_naive();
    let (status, report) = get_json(
        &edge,
        &format!("/pos/reports/daily-usage?date={today}"),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["formulas_recorded"], 1);
    assert_eq!(report["total_tint_cost"]["amount"], "0.6050");
    assert_eq!(report["colorant_used"].as_array().unwrap().len(), 2);
}
