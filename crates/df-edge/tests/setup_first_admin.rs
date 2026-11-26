//! GUI first-run setup exit tests, edge surface: the first admin
//! is created over the real `Edge::router()`.
//!
//! (a) Fresh DB → `GET /pos/auth/setup-status` is
//!     `{"needs_setup": true}` → `POST /pos/auth/setup` is 201
//!     with a `dfp_` token → login with the new credentials is
//!     200 with a `dfp_` token → setup-status is false.
//! (b) A second `POST /pos/auth/setup` is 409 and the user count
//!     is unchanged.
//! (c) Empty email / empty password (and a short password) are
//!     400 and create no user.
//!
//! Plus: a `role` parameter is rejected (the endpoint never
//! accepts one — the created user is always Admin), a missing
//! `display_name` defaults to the email local part, and two
//! concurrent setups create exactly one admin.
//!
//! The HQ half lives in `crates/df-api/tests/setup_first_admin.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use df_domain::Currency;
use df_edge::{Edge, EdgeConfig};
use tower::ServiceExt;
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;

/// Fresh edge on an in-memory SQLite database with no org, store
/// or user rows — the real first-run state (`Edge::open` seeds
/// nothing). Setup must work from exactly this state.
async fn fresh_edge() -> Edge {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    Edge::open(config).await.unwrap()
}

async fn post_json(
    edge: &Edge,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = edge
        .router()
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    // A rejected body (e.g. axum's 422 for an unknown field) is
    // plain text, not JSON — keep the status, not the parse.
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn get_json(edge: &Edge, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = edge
        .router()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn user_count(edge: &Edge) -> i64 {
    df_db::count_users(edge.pool()).await.unwrap()
}

fn setup_body(email: &str, password: &str) -> serde_json::Value {
    serde_json::json!({"email": email, "password": password})
}

#[tokio::test]
async fn fresh_db_setup_then_login() {
    let edge = fresh_edge().await;

    let (status, body) = get_json(&edge, "/pos/auth/setup-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["needs_setup"], true);

    // No display_name: it defaults to the email local part.
    let (status, body) = post_json(
        &edge,
        "/pos/auth/setup",
        setup_body("admin@example.invalid", "password-123"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("dfp_"), "{body}");
    assert_eq!(body["role"], "Admin");
    assert_eq!(body["email"], "admin@example.invalid");
    assert_eq!(body["display_name"], "admin");
    assert_eq!(user_count(&edge).await, 1);

    let (status, body) = get_json(&edge, "/pos/auth/setup-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["needs_setup"], false);

    // The new admin logs in with the same credentials — no second
    // setup call needed, but login itself works.
    let (status, body) = post_json(
        &edge,
        "/pos/auth/login",
        serde_json::json!({"email": "admin@example.invalid", "password": "password-123"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["token"].as_str().unwrap().starts_with("dfp_"));
    assert_eq!(body["role"], "Admin");
}

#[tokio::test]
async fn second_setup_is_409_and_creates_nothing() {
    let edge = fresh_edge().await;
    let (status, _) = post_json(
        &edge,
        "/pos/auth/setup",
        serde_json::json!({
            "email": "admin@example.invalid",
            "display_name": "First Admin",
            "password": "password-123",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(user_count(&edge).await, 1);

    // A second setup — different email, so the UNIQUE constraint
    // is not what stops it — is 409 and creates nothing.
    let (status, body) = post_json(
        &edge,
        "/pos/auth/setup",
        serde_json::json!({
            "email": "second@example.invalid",
            "display_name": "Second Admin",
            "password": "password-123",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(user_count(&edge).await, 1);

    // The first admin still logs in; the second never existed.
    let (status, _) = post_json(
        &edge,
        "/pos/auth/login",
        serde_json::json!({"email": "second@example.invalid", "password": "password-123"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn blank_email_password_and_short_password_are_400() {
    let edge = fresh_edge().await;

    for body in [
        setup_body("", "password-123"),
        setup_body("   ", "password-123"),
        setup_body("admin@example.invalid", ""),
        setup_body("admin@example.invalid", "   "),
        setup_body("admin@example.invalid", "short"),
    ] {
        let (status, _) = post_json(&edge, "/pos/auth/setup", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(user_count(&edge).await, 0);
    }

    // Setup still works after the rejected attempts.
    let (status, body) = get_json(&edge, "/pos/auth/setup-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["needs_setup"], true);
}

#[tokio::test]
async fn role_parameter_is_rejected() {
    let edge = fresh_edge().await;
    // `role` is not a field of the setup request: unknown fields
    // are rejected, so a caller cannot smuggle in a non-Admin role.
    let (status, _) = post_json(
        &edge,
        "/pos/auth/setup",
        serde_json::json!({
            "email": "admin@example.invalid",
            "password": "password-123",
            "role": "Cashier",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(user_count(&edge).await, 0);
}

#[tokio::test]
async fn concurrent_setup_creates_exactly_one_admin() {
    let edge = fresh_edge().await;
    let first = post_json(
        &edge,
        "/pos/auth/setup",
        serde_json::json!({
            "email": "one@example.invalid",
            "display_name": "One",
            "password": "password-123",
        }),
    );
    let second = post_json(
        &edge,
        "/pos/auth/setup",
        serde_json::json!({
            "email": "two@example.invalid",
            "display_name": "Two",
            "password": "password-123",
        }),
    );
    let ((first_status, _), (second_status, _)) = tokio::join!(first, second);
    let mut statuses = [first_status, second_status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::CREATED, StatusCode::CONFLICT],
        "exactly one setup must win"
    );
    assert_eq!(user_count(&edge).await, 1);
}
