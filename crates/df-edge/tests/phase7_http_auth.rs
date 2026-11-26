//! Phase 7 (v1.1) exit tests, edge surface: HTTP auth enforcement
//! end to end over the real `Edge::router()`.
//!
//! (a) Unauthenticated → 401 on protected routes; `/health` stays
//!     200 without a token.
//! (b) Cashier token on a manager-only route → 403; manager
//!     succeeds.
//! (c) Portal login as contractor A: B's statement → 403, A's own
//!     data → 200, and a reorder for A's own customer → 200.
//! (d) Expired token → 401; logout revokes the token.
//!
//! The HQ half of (a)/(b) lives in `crates/df-api/tests/
//! phase7_http_auth.rs`; the df-edge unit tests in `src/http.rs`
//! cover the same flows at the handler level.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use df_auth_http::test_support::{SeededUsers, TEST_PASSWORD, seed_test_users};
use df_db::models::{Organization, Product, Store, TintFormula};
use df_domain::{Currency, Money};
use df_edge::{Edge, EdgeConfig};
use rust_decimal::Decimal;
use std::str::FromStr;
use tower::ServiceExt;
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;
const FORMULA_ID: u128 = 0x7000;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

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

async fn get_json(edge: &Edge, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = edge
        .router()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// Edge with org + store + one base product + the standard seeded
/// users (admin/manager/cashier/contractors) + a White Dove formula
/// owned by contractor A's customer, so the portal reorder flow
/// has something real to reorder.
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
            sku: "BM-REGAL-1G".to_string(),
            manufacturer_code: Some("BM".to_string()),
            name: "Regal Select Interior Gallon".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: Money::from_units(100, Currency::Usd).amount(),
            standard_cost: Money::from_units(50, Currency::Usd).amount(),
            avg_cost: Money::from_units(50, Currency::Usd).amount(),
            min_margin_pct: None,
            sheen: Some("Eggshell".to_string()),
            paint_base_code: Some("1X".to_string()),
            container_volume_ml: Some(3785),
            eco_fee_category: Some("architectural_paint".to_string()),
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            is_active: true,
        },
    )
    .await
    .unwrap();
    let users = seed_test_users(pool, Uuid::from_u128(ORG_ID)).await;
    // A past White Dove formula owned by contractor A's customer.
    df_db::insert_tint_formula(
        pool,
        &TintFormula {
            id: Uuid::from_u128(FORMULA_ID),
            org_id: Uuid::from_u128(ORG_ID),
            store_id: Uuid::from_u128(STORE_ID),
            customer_id: Some(users.customer_a),
            job_id: None,
            color_id: None,
            color_code_snapshot: "OC-17".to_string(),
            color_name_snapshot: "White Dove".to_string(),
            base_product_id: Uuid::from_u128(PRODUCT_ID),
            container_volume_ml: 3785,
            sheen: Some("Eggshell".to_string()),
            room_or_usage: None,
            is_custom_match: false,
            colorants_json: serde_json::json!([{"code": "Y3", "oz": 0, "shots": 14}]),
            total_tint_cost: dec("0.1225"),
            tint_surcharge: dec("0.0000"),
            dispenser_name: Some("virtual-simulator".to_string()),
            external_formula_ref: None,
            dispensed_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    (edge, users)
}

#[tokio::test]
async fn unauthenticated_protected_edge_routes_are_401() {
    let (edge, users) = seeded_edge().await;
    // No token on protected routes: 401, never 403.
    let (status, _) = get_json(&edge, "/inventory/balances", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = get_json(
        &edge,
        &format!("/portal/customers/{}", users.customer_a),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post_json(&edge, "/pos/transactions", serde_json::json!({}), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Health stays public.
    let (status, _) = get_json(&edge, "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    // …and a valid token still works after the 401s.
    let (status, _) = get_json(&edge, "/inventory/balances", Some(&users.manager.token)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn cashier_forbidden_manager_allowed_on_reorder_points() {
    let (edge, users) = seeded_edge().await;
    let body = serde_json::json!({
        "product_id": Uuid::from_u128(PRODUCT_ID).to_string(),
        "store_id": Uuid::from_u128(STORE_ID).to_string(),
        "min_qty": "2",
        "max_qty": "20",
        "reorder_qty": "10",
    });
    // Setting a reorder point needs InventoryAdjust: the cashier
    // gets 403, the manager succeeds.
    let (status, _) = post_json(
        &edge,
        "/inventory/reorder-points",
        body.clone(),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = post_json(
        &edge,
        "/inventory/reorder-points",
        body,
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["min_qty"], "2");
}

#[tokio::test]
async fn portal_login_scopes_contractor_to_own_customer() {
    let (edge, users) = seeded_edge().await;
    let a = users.customer_a;
    let b = users.customer_b;

    // Portal login for contractor A (email + password, not a
    // customer UUID) returns a contractor token.
    let (status, body) = post_json(
        &edge,
        "/pos/auth/portal-login",
        serde_json::json!({"email": "contractor-a@example.invalid", "password": TEST_PASSWORD}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("dfp_"));
    assert_eq!(body["role"], "PortalContractor");
    assert_eq!(body["customer_id"], a.to_string());

    // Contractor B's statement: 403.
    let (status, _) = get_json(
        &edge,
        &format!("/portal/customers/{b}/statements"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A's own statement: 200.
    let (status, body) = get_json(
        &edge,
        &format!("/portal/customers/{a}/statements"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A reorder for A's own customer succeeds…
    let (status, body) = post_json(
        &edge,
        "/portal/reorders",
        serde_json::json!({
            "customer_id": a.to_string(),
            "formula_id": Uuid::from_u128(FORMULA_ID).to_string(),
            "qty": "1",
            "job_id": null,
            "fulfillment": "Pickup",
        }),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // …for B's customer it is 403, before any formula check.
    let (status, _) = post_json(
        &edge,
        "/portal/reorders",
        serde_json::json!({
            "customer_id": b.to_string(),
            "formula_id": Uuid::from_u128(FORMULA_ID).to_string(),
            "qty": "1",
            "job_id": null,
            "fulfillment": "Pickup",
        }),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn expired_token_is_401_and_logout_revokes() {
    let (edge, users) = seeded_edge().await;
    // A session that expired a second ago.
    let (token, _) = df_db::create_session(
        edge.pool(),
        &users.cashier.user,
        chrono::Duration::seconds(-1),
        None,
    )
    .await
    .unwrap();
    let (status, _) = get_json(&edge, "/inventory/balances", Some(&token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A live token works, logout revokes it, and it stops working.
    let (status, body) = post_json(
        &edge,
        "/pos/auth/logout",
        serde_json::json!({}),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revoked"], true);
    let (status, _) = get_json(&edge, "/inventory/balances", Some(&users.cashier.token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// PortalContractor sessions are scoped to their linked customer;
// staff sessions may not use portal routes at all.
