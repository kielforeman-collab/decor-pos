//! GUI first-run setup exit tests, HQ surface: the first admin is
//! created over the real `df_api::create_router()`.
//!
//! (a) Fresh DB → `GET /api/v1/auth/setup-status` is
//!     `{"needs_setup": true}` → `POST /api/v1/auth/setup` is 201
//!     with a `dfp_` token → login with the new credentials is
//!     200 with a `dfp_` token → setup-status is false.
//! (b) A second `POST /api/v1/auth/setup` is 409 and the user
//!     count is unchanged.
//! (c) Empty email / empty password (and a short password) are
//!     400 and create no user.
//!
//! Plus: a `role` parameter is rejected, a missing `display_name`
//! defaults to the email local part, and two concurrent setups
//! create exactly one admin. The org row is deliberately NOT
//! seeded here — setup must create it, the way a fresh HQ install
//! has no org row yet.
//!
//! The edge half lives in `crates/df-edge/tests/setup_first_admin.rs`.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use df_api::{ApiState, create_router};
use df_db::AnyPool;
use tower::ServiceExt;
use uuid::Uuid;

const ORG_ID: u128 = 0x2000;

/// HQ state on an in-memory SQLite database with no rows at all —
/// the real first-run state. The configured org id has no org row
/// yet; setup must create it.
async fn fresh_router() -> (Router, AnyPool) {
    let pool = df_db::connect("sqlite::memory:").await.unwrap();
    df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
    let state = ApiState {
        pool: Some(pool.clone()),
        org_id: Some(Uuid::from_u128(ORG_ID)),
        store_id: None,
        ..ApiState::default()
    };
    (create_router(state), pool)
}

async fn post_json(
    router: Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = router
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

async fn get_json(router: Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn user_count(pool: &AnyPool) -> i64 {
    df_db::count_users(pool).await.unwrap()
}

fn setup_body(email: &str, password: &str) -> serde_json::Value {
    serde_json::json!({"email": email, "password": password})
}

#[tokio::test]
async fn fresh_db_setup_then_login() {
    let (router, pool) = fresh_router().await;

    let (status, body) = get_json(router.clone(), "/api/v1/auth/setup-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["needs_setup"], true);

    // No display_name: it defaults to the email local part.
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/setup",
        setup_body("admin@example.invalid", "password-123"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("dfp_"), "{body}");
    assert_eq!(body["role"], "Admin");
    assert_eq!(body["email"], "admin@example.invalid");
    assert_eq!(body["display_name"], "admin");
    assert_eq!(user_count(&pool).await, 1);

    let (status, body) = get_json(router.clone(), "/api/v1/auth/setup-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["needs_setup"], false);

    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"email": "admin@example.invalid", "password": "password-123"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["token"].as_str().unwrap().starts_with("dfp_"));
    assert_eq!(body["role"], "Admin");
}

#[tokio::test]
async fn second_setup_is_409_and_creates_nothing() {
    let (router, pool) = fresh_router().await;
    let (status, _) = post_json(
        router.clone(),
        "/api/v1/auth/setup",
        serde_json::json!({
            "email": "admin@example.invalid",
            "display_name": "First Admin",
            "password": "password-123",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(user_count(&pool).await, 1);

    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/setup",
        serde_json::json!({
            "email": "second@example.invalid",
            "display_name": "Second Admin",
            "password": "password-123",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(user_count(&pool).await, 1);

    let (status, _) = post_json(
        router.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"email": "second@example.invalid", "password": "password-123"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn blank_email_password_and_short_password_are_400() {
    let (router, pool) = fresh_router().await;

    for body in [
        setup_body("", "password-123"),
        setup_body("   ", "password-123"),
        setup_body("admin@example.invalid", ""),
        setup_body("admin@example.invalid", "   "),
        setup_body("admin@example.invalid", "short"),
    ] {
        let (status, _) = post_json(router.clone(), "/api/v1/auth/setup", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(user_count(&pool).await, 0);
    }

    let (status, body) = get_json(router.clone(), "/api/v1/auth/setup-status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["needs_setup"], true);
}

#[tokio::test]
async fn role_parameter_is_rejected() {
    let (router, pool) = fresh_router().await;
    let (status, _) = post_json(
        router.clone(),
        "/api/v1/auth/setup",
        serde_json::json!({
            "email": "admin@example.invalid",
            "password": "password-123",
            "role": "Cashier",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(user_count(&pool).await, 0);
}

#[tokio::test]
async fn concurrent_setup_creates_exactly_one_admin() {
    let (router, pool) = fresh_router().await;
    let first = post_json(
        router.clone(),
        "/api/v1/auth/setup",
        serde_json::json!({
            "email": "one@example.invalid",
            "display_name": "One",
            "password": "password-123",
        }),
    );
    let second = post_json(
        router.clone(),
        "/api/v1/auth/setup",
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
    assert_eq!(user_count(&pool).await, 1);
}
