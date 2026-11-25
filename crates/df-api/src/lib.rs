//! `df-api` — Axum REST API with utoipa OpenAPI 3.1 (Phase 0, v0.1).
//!
//! Skeleton only: the routes from spec §6.1 exist and are documented,
//! but only `/health` and the OpenAPI document do anything. Auth routes
//! return `501 Not Implemented` — there is no database wired in Phase 0,
//! and no credential is ever accepted, logged, or stored here. The real
//! handlers land with `df-db` wiring in Phase 1.

use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::OpenApi;
use utoipa::ToSchema;

/// Shared, cheaply-cloned API state.
#[derive(Debug, Clone)]
pub struct ApiState {
    pub version: &'static str,
}

impl Default for ApiState {
    fn default() -> Self {
        Self { version: "0.1.0" }
    }
}

// --- schemas ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CounterSwitchRequest {
    /// 4–6 digit counter PIN, or a badge token.
    pub pin: Option<String>,
    pub badge_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PasskeyVerifyRequest {
    pub user_id: String,
    pub response_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OverrideVerifyRequest {
    pub user_id: String,
    pub pin: String,
    pub reason_code: String,
}

// --- handlers --------------------------------------------------------------

fn not_implemented(feature: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorResponse {
            error: format!("{feature} is not implemented in Phase 0 (v0.1)"),
        }),
    )
}

#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Service is up", body = HealthResponse))
)]
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 501, description = "Not implemented in Phase 0", body = ErrorResponse),
        (status = 200, description = "Authenticated (future)", body = ErrorResponse)
    )
)]
pub async fn login(Json(_body): Json<LoginRequest>) -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("password login")
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/counter-switch",
    request_body = CounterSwitchRequest,
    responses((status = 501, description = "Not implemented in Phase 0", body = ErrorResponse))
)]
pub async fn counter_switch(
    Json(_body): Json<CounterSwitchRequest>,
) -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("counter PIN/badge switch")
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/passkey/verify",
    request_body = PasskeyVerifyRequest,
    responses((status = 501, description = "Not implemented in Phase 0", body = ErrorResponse))
)]
pub async fn passkey_verify(
    Json(_body): Json<PasskeyVerifyRequest>,
) -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("passkey verification")
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/callback",
    responses((status = 501, description = "Not implemented in Phase 0", body = ErrorResponse))
)]
pub async fn oidc_callback() -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("OIDC callback")
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/override-verify",
    request_body = OverrideVerifyRequest,
    responses((status = 501, description = "Not implemented in Phase 0", body = ErrorResponse))
)]
pub async fn override_verify(
    Json(_body): Json<OverrideVerifyRequest>,
) -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("manager override verification")
}

// --- OpenAPI document ------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    paths(health, login, counter_switch, passkey_verify, oidc_callback, override_verify),
    components(schemas(
        HealthResponse,
        ErrorResponse,
        LoginRequest,
        CounterSwitchRequest,
        PasskeyVerifyRequest,
        OverrideVerifyRequest
    )),
    tags((name = "auth", description = "Authentication & counter switching"))
)]
pub struct ApiDoc;

/// Serve the generated OpenAPI 3.1 document as JSON.
pub async fn openapi_json() -> Json<serde_json::Value> {
    let mut doc = ApiDoc::openapi();
    doc.info.title = "decor-pos API".to_string();
    doc.info.version = env!("CARGO_PKG_VERSION").to_string();
    Json(serde_json::to_value(doc).expect("OpenAPI document serialises"))
}

/// Build the Phase 0 router.
pub fn create_router(_state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api-docs/openapi.json", get(openapi_json))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/counter-switch", post(counter_switch))
        .route("/api/v1/auth/passkey/verify", post(passkey_verify))
        .route("/api/v1/auth/oidc/callback", get(oidc_callback))
        .route("/api/v1/auth/override-verify", post(override_verify))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let router = create_router(ApiState::default());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["version"], "0.1.0");
    }

    #[tokio::test]
    async fn openapi_document_is_3_1() {
        let router = create_router(ApiState::default());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api-docs/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["openapi"], "3.1.0");
        assert!(value["paths"]["/health"].is_object());
        assert!(value["paths"]["/api/v1/auth/login"].is_object());
    }

    #[tokio::test]
    async fn auth_routes_are_501_in_phase_0() {
        let router = create_router(ApiState::default());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/login")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"email":"a@example.invalid","password":"not-a-real-password"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
