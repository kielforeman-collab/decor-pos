//! Phase 7 (v1.1) exit tests, HQ surface: HTTP auth enforcement
//! end to end over the real `df_api::create_router()`.
//!
//! (a) Unauthenticated → 401 on a protected HQ route; `/health`
//!     and `/api-docs/openapi.json` stay 200 without a token, and
//!     the OpenAPI document contains no data rows.
//! (b) Cashier token on a manager-only HQ route → 403; manager
//!     succeeds.
//! (d) Expired token → 401; logout revokes the token.
//!
//! The edge half of (a)/(b)/(d) lives in
//! `crates/df-edge/tests/phase7_http_auth.rs`; (c) (portal scope)
//! is edge-only — the HQ API has no portal surface.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use df_api::{ApiState, create_router};
use df_auth_http::test_support::{SeededUsers, TEST_PASSWORD, seed_test_users};
use df_db::models::{Colorant, Organization, Store};
use rust_decimal::Decimal;
use std::str::FromStr;
use tower::ServiceExt;
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const Y3_ID: u128 = 0x5001;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn post_json(
    router: axum::Router,
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
    let response = router
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn get_json(
    router: axum::Router,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// HQ state on an in-memory SQLite database: org + store + one
/// colorant + the standard seeded users.
async fn seeded_state() -> (ApiState, SeededUsers) {
    let pool = df_db::connect("sqlite::memory:").await.unwrap();
    df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
    let org_id = Uuid::from_u128(ORG_ID);
    let store_id = Uuid::from_u128(STORE_ID);
    df_db::insert_organization(
        &pool,
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
        &pool,
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
    df_db::insert_colorant(
        &pool,
        &Colorant {
            id: Uuid::from_u128(Y3_ID),
            org_id,
            system_name: "Gennex".to_string(),
            code: "Y3".to_string(),
            name: "Yellow Oxide".to_string(),
            cost_per_fl_oz: dec("0.4200"),
            shots_per_fl_oz: 48,
        },
    )
    .await
    .unwrap();
    let users = seed_test_users(&pool, org_id).await;
    (
        ApiState {
            version: "1.2.0",
            pool: Some(pool),
            org_id: Some(org_id),
            store_id: Some(store_id),
        },
        users,
    )
}

#[tokio::test]
async fn hq_unauthenticated_is_401_health_and_openapi_public() {
    let (state, _) = seeded_state().await;
    let store_id = state.store_id.unwrap();
    let router = create_router(state);

    // No token on a protected HQ route: 401, never 403.
    let (status, _) = get_json(
        router.clone(),
        &format!("/api/v1/inventory/balances?store_id={store_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = get_json(router.clone(), "/api/v1/crm/activities", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Health stays public.
    let (status, _) = get_json(router.clone(), "/health", None).await;
    assert_eq!(status, StatusCode::OK);

    // The OpenAPI document stays public and contains no data rows.
    let (status, body) = get_json(router, "/api-docs/openapi.json", None).await;
    assert_eq!(status, StatusCode::OK);
    let text = serde_json::to_string(&body).unwrap();
    for leak in ["BM-REGAL-1G", "example.invalid", "dfp_", "C-PORTAL-A"] {
        assert!(!text.contains(leak), "openapi doc leaks {leak}");
    }
}

#[tokio::test]
async fn hq_cashier_forbidden_manager_allowed_on_colorant_levels() {
    let (state, users) = seeded_state().await;
    let store_id = state.store_id.unwrap();
    let router = create_router(state);
    let body = serde_json::json!({
        "colorant_id": Uuid::from_u128(Y3_ID).to_string(),
        "store_id": store_id.to_string(),
        "on_hand_fl_oz": "10",
        "low_threshold_fl_oz": "2",
    });
    // Setting a colorant level needs InventoryAdjust: the cashier
    // gets 403, the manager succeeds.
    let (status, _) = post_json(
        router.clone(),
        "/api/v1/tinting/colorants/levels",
        body.clone(),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/tinting/colorants/levels",
        body,
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["on_hand_fl_oz"], "10");
}

#[tokio::test]
async fn hq_expired_token_is_401_and_logout_revokes() {
    let (state, users) = seeded_state().await;
    let pool = state.pool.clone().unwrap();
    let store_id = state.store_id.unwrap();
    let router = create_router(state);

    // A session that expired a second ago.
    let (token, _) = df_db::create_session(
        &pool,
        &users.admin.user,
        chrono::Duration::seconds(-1),
        None,
    )
    .await
    .unwrap();
    let (status, _) = get_json(
        router.clone(),
        &format!("/api/v1/inventory/balances?store_id={store_id}"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Login, logout, and the token stops working.
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"email": "admin@example.invalid", "password": TEST_PASSWORD}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("dfp_"));
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/logout",
        serde_json::json!({}),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revoked"], true);
    let (status, _) = get_json(
        router.clone(),
        &format!("/api/v1/inventory/balances?store_id={store_id}"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
