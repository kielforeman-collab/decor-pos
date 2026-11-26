//! GUI product import (`POST /pos/inventory/import`) end to end
//! over the real `Edge::router()`.
//!
//! (a) Unauthenticated → 401.
//! (b) RBAC: the route is gated on `PricingManage` (Manager /
//!     Admin only) — Cashier and Warehouse (who holds
//!     `InventoryAdjust`) both get 403; Manager and Admin succeed.
//! (c) A mixed batch creates new SKUs, reprices an existing SKU,
//!     and reports bad rows (bad price, missing SKU, the malformed
//!     shifted row) without failing the batch; re-import updates
//!     instead of duplicating.
//! (d) An unknown store_id → 404.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use df_auth_http::test_support::{SeededUsers, seed_test_users};
use df_db::models::{Organization, Product, Store};
use df_domain::Currency;
use df_edge::{Edge, EdgeConfig};
use std::str::FromStr;
use tower::ServiceExt;
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;

async fn post_json(
    edge: &Edge,
    uri: &str,
    body: serde_json::Value,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = edge
        .router()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// Edge with org + store + one existing product (SKU `F40301-001`,
/// list $58.18 / cost $57.74) + the standard seeded users.
async fn seeded_edge() -> (Edge, SeededUsers) {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    let edge = Edge::open(config).await.unwrap();
    let pool = edge.pool();
    df_db::insert_organization(
        pool,
        &Organization {
            id: Uuid::from_u128(ORG_ID),
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
        pool,
        &Store {
            id: Uuid::from_u128(STORE_ID),
            org_id: Uuid::from_u128(ORG_ID),
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
        pool,
        &Product {
            id: Uuid::from_u128(PRODUCT_ID),
            org_id: Uuid::from_u128(ORG_ID),
            sku: "F40301-001".to_string(),
            manufacturer_code: Some("BMC".to_string()),
            name: "REGAL SELECT HB SG-WHITE".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: rust_decimal::Decimal::from_str("58.18").unwrap(),
            standard_cost: rust_decimal::Decimal::from_str("57.74").unwrap(),
            avg_cost: rust_decimal::Decimal::from_str("57.74").unwrap(),
            min_margin_pct: None,
            sheen: None,
            paint_base_code: None,
            container_volume_ml: None,
            eco_fee_category: None,
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            is_active: true,
        },
    )
    .await
    .unwrap();
    let users = seed_test_users(pool, Uuid::from_u128(ORG_ID)).await;
    (edge, users)
}

fn import_body(rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "store_id": Uuid::from_u128(STORE_ID).to_string(),
        "rows": rows,
    })
}

fn good_rows() -> serde_json::Value {
    serde_json::json!([
        // Existing SKU, new prices → update.
        {"row": 2, "sku": "F40301-001", "name": "REGAL SELECT HB SG-WHITE",
         "net_price": "$59.00", "list_price": "$60.00", "brand_code": "BMC"},
        // New SKU → create.
        {"row": 3, "sku": "0229B1-004", "name": "GENNEX WB CLRNT -THALO BLUE",
         "net_price": "$39.34", "list_price": "$39.64", "brand_code": "BMN"},
        // Bad price → failure, batch continues.
        {"row": 4, "sku": "SKU-BADPRICE", "name": "Bad price",
         "net_price": "not-a-price", "list_price": "$2.00"},
        // Missing SKU → failure.
        {"row": 5, "sku": "", "name": "Missing SKU",
         "net_price": "$1.00", "list_price": "$2.00"},
        // The malformed shifted row from the real price list
        // (sheet row 1012): every cell holds the same string.
        {"row": 1012, "sku": "K7931X-001", "name": "K7931X-001",
         "net_price": "K7931X-001", "list_price": "K7931X-001"},
    ])
}

#[tokio::test]
async fn unauthenticated_import_is_401() {
    let (edge, _) = seeded_edge().await;
    let (status, _) = post_json(
        &edge,
        "/pos/inventory/import",
        import_body(good_rows()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cashier_and_warehouse_forbidden_manager_and_admin_allowed() {
    let (edge, users) = seeded_edge().await;
    // Cashier holds no pricing permission.
    let (status, _) = post_json(
        &edge,
        "/pos/inventory/import",
        import_body(good_rows()),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Warehouse holds InventoryAdjust but NOT PricingManage — the
    // import gate is Manager/Admin only, so this must also be 403.
    let (status, _) = post_json(
        &edge,
        "/pos/inventory/import",
        import_body(good_rows()),
        Some(&users.warehouse.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Manager succeeds.
    let (status, body) = post_json(
        &edge,
        "/pos/inventory/import",
        import_body(good_rows()),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], 1);
    assert_eq!(body["updated"], 1);
    // Admin succeeds on the re-import (now both rows are updates).
    let (status, body) = post_json(
        &edge,
        "/pos/inventory/import",
        import_body(good_rows()),
        Some(&users.admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], 0);
    assert_eq!(body["updated"], 2);
}

#[tokio::test]
async fn manager_import_creates_updates_and_reports_failures() {
    let (edge, users) = seeded_edge().await;
    let (status, body) = post_json(
        &edge,
        "/pos/inventory/import",
        import_body(good_rows()),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["created"], 1);
    assert_eq!(body["updated"], 1);
    assert_eq!(body["failed"], 3);
    let failures = body["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 3);
    assert_eq!(
        failures
            .iter()
            .map(|f| f["row"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![4, 5, 1012]
    );
    assert!(
        failures[2]["reason"].as_str().unwrap().contains("price"),
        "malformed row names the price problem: {}",
        failures[2]["reason"]
    );
    // The existing SKU was repriced, not duplicated.
    let product = df_db::get_product_by_sku(edge.pool(), Uuid::from_u128(ORG_ID), "F40301-001")
        .await
        .unwrap();
    assert_eq!(product.list_price.to_string(), "60.00");
    assert_eq!(product.standard_cost.to_string(), "59.00");
    assert_eq!(
        df_db::list_products_for_org(edge.pool(), Uuid::from_u128(ORG_ID))
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn import_rejects_unknown_store() {
    let (edge, users) = seeded_edge().await;
    let body = serde_json::json!({
        "store_id": Uuid::now_v7().to_string(),
        "rows": good_rows(),
    });
    let (status, _) = post_json(
        &edge,
        "/pos/inventory/import",
        body,
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
