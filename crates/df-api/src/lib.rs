//! `df-api` — Axum REST API with utoipa OpenAPI 3.1 (Phase 2, v0.3).
//!
//! Working: `/health` and the OpenAPI document. Phase 7 (v1.1)
//! implements the auth endpoints (`POST /api/v1/auth/login`,
//! `/auth/counter-switch`, `/auth/logout`,
//! `/auth/override-verify`) over the `df-auth` + `df-db::auth`
//! session primitives, and gates every other route on a `Bearer
//! dfp_...` session token with per-route RBAC (see `docs/auth.md`).
//! Passkey/OIDC stay 501 stubs — no credential is ever accepted,
//! logged, or stored here beyond the login handlers' credential
//! check (passwords/PINs are verified against Argon2id hashes and
//! never persisted or logged).
//!
//! Phase 2 admin surface (spec §6.1), backed by `df-db` repos over
//! the pool in [`ApiState`]: color search, tint formulas
//! (list/search by color and by customer, create), colorant levels
//! (list, set level/threshold), and reports (daily usage, low
//! stock, shortage alerts). The same repos run over `AnyPool`, so
//! these handlers work against SQLite (edge/dev) and PostgreSQL
//! (HQ) alike — the live-Postgres round-trip is covered by
//! `df-db/tests/pg_repos.rs` and `df-sync/tests/db_hq.rs` (Phase 8)
//! plus `df-server/tests/postgres_hq_round_trip.rs`.
//!
//! Phase 4 (v0.5) HQ reads (spec §6.1/§7) on the same pattern:
//! `GET /api/v1/crm/activities`, `GET /api/v1/crm/appointments`,
//! `GET /api/v1/deliveries/dispatch` and
//! `GET /api/v1/edi/documents` — read-only views of the edge's
//! CRM, delivery and EDI rows for HQ. The lapsed-contractor
//! report is edge-only (it needs the summary assembly in
//! `df-db::crm_reports` over the edge's org); writes stay on the
//! edge. Like the Phase 2 inventory/AR reads, these four are
//! routes but not OpenAPI `paths`: their row types do not
//! implement utoipa's `ToSchema`.
//!
//! When no pool is configured (the default `df-api` binary with no

pub mod auth;
pub mod reads;
pub mod reports;
pub mod schemas;
#[cfg(test)]
mod tests;
pub mod tinting;

pub use auth::{
    CreateServiceTokenRequest, CreateServiceTokenResponse, ResolveConflictRequest,
    ResolveConflictResponse, RevokeServiceTokenResponse, ServiceTokenListQuery,
    ServiceTokenSummary, SyncConflictsQuery, counter_switch, create_service_token,
    list_service_tokens, login, logout, oidc_callback, override_verify, passkey_verify,
    resolve_sync_conflict, revoke_service_token, setup, setup_status, sync_conflicts, sync_ingest,
};
pub use reads::{
    ar_aging, ar_customer_summary, crm_activities, crm_appointments, delivery_dispatch,
    edi_documents, inventory_balances,
};
pub use reports::{daily_usage, low_stock};
pub use schemas::{
    AgingCustomerResponse, AgingResponse, AppointmentListQuery, ArAgingQuery,
    ArCustomerSummaryResponse, ColorSearchQuery, CounterSwitchRequest, DispatchQuery,
    EdiDocumentsQuery, ErrorResponse, FormulaQuery, HealthResponse, LoginRequest,
    OverrideVerifyRequest, PasskeyVerifyRequest, ReportQuery, StoreQuery,
};
pub use tinting::{create_formula, list_colorants, list_formulas, search_colors};

use std::str::FromStr;

use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use df_auth::Permission;
use df_auth_http::{
    AuthRateLimiter, AuthRule, ErrorBody, Gate, HttpHardeningConfig, LoginResponse, LogoutResponse,
    OverrideVerifyResponse, SetupRequest, SetupStatusResponse, apply_http_hardening,
    auth_rate_limit,
};
use df_db::AnyPool;
use utoipa::OpenApi;
use uuid::Uuid;

/// Shared, cheaply-cloned API state.
///
/// `pool` / `org_id` / `store_id` are `None` until the binary is
/// started with `--db` / `--org-id` / `--store-id` (see `main.rs`);
/// the admin handlers 503 until then.
#[derive(Debug, Clone)]
pub struct ApiState {
    pub version: &'static str,
    pub pool: Option<AnyPool>,
    pub org_id: Option<Uuid>,
    pub store_id: Option<Uuid>,
}

impl Default for ApiState {
    fn default() -> Self {
        Self {
            version: "1.2.4",
            pool: None,
            org_id: None,
            store_id: None,
        }
    }
}

// --- helpers ---------------------------------------------------------------

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
}

fn not_implemented(feature: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorResponse {
            error: format!("{feature} is not implemented in Phase 0 (v0.1)"),
        }),
    )
}

/// The configured pool, or a 503 explaining how to configure one.
fn pool(state: &ApiState) -> Result<AnyPool, (StatusCode, Json<ErrorResponse>)> {
    state.pool.clone().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no database configured for this API instance — restart df-api with --db <url> --org-id <uuid>",
        )
    })
}

fn org_id(state: &ApiState) -> Result<Uuid, (StatusCode, Json<ErrorResponse>)> {
    state.org_id.ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no org configured for this API instance — restart df-api with --org-id <uuid>",
        )
    })
}

/// Store id from the query, falling back to the instance default.
fn store_id(
    query_store: Option<Uuid>,
    state: &ApiState,
) -> Result<Uuid, (StatusCode, Json<ErrorResponse>)> {
    query_store.or(state.store_id).ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "store_id is required (query ?store_id= or df-api --store-id)",
        )
    })
}

/// `?date=YYYY-MM-DD`, defaulting to today (UTC).
fn report_date(date: Option<String>) -> Result<NaiveDate, (StatusCode, Json<ErrorResponse>)> {
    match date {
        None => Ok(chrono::Utc::now().date_naive()),
        Some(text) => NaiveDate::from_str(&text).map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                format!("date must be YYYY-MM-DD, got {text:?}"),
            )
        }),
    }
}

fn db_err(e: df_db::DbError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match &e {
        df_db::DbError::NotFound(_) => StatusCode::NOT_FOUND,
        df_db::DbError::InvalidData(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    err(status, e.to_string())
}

fn edi_err(e: df_edi::EdiError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match &e {
        df_edi::EdiError::Parse(message) if message.ends_with("not found") => StatusCode::NOT_FOUND,
        df_edi::EdiError::Sqlx(_) | df_edi::EdiError::Db(_) | df_edi::EdiError::Io(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        _ => StatusCode::BAD_REQUEST,
    };
    err(status, e.to_string())
}

// --- handlers --------------------------------------------------------------

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

// --- OpenAPI document ------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        auth::login,
        auth::setup_status,
        auth::setup,
        auth::counter_switch,
        auth::logout,
        auth::passkey_verify,
        auth::oidc_callback,
        auth::override_verify,
        tinting::search_colors,
        tinting::list_formulas,
        tinting::list_colorants,
        reports::daily_usage,
        reports::low_stock
    ),
    components(schemas(
        HealthResponse,
        ErrorResponse,
        ErrorBody,
        LoginRequest,
        LoginResponse,
        LogoutResponse,
        SetupRequest,
        SetupStatusResponse,
        CounterSwitchRequest,
        PasskeyVerifyRequest,
        OverrideVerifyRequest,
        OverrideVerifyResponse
    )),
    tags(
        (name = "auth", description = "Authentication & counter switching"),
        (name = "tinting", description = "Paint & tinting formula tracking (Phase 2)"),
        (name = "reports", description = "Daily usage and low-stock reports (Phase 2)")
    )
)]
pub struct ApiDoc;

/// Serve the generated OpenAPI 3.1 document as JSON.
pub async fn openapi_json() -> Json<serde_json::Value> {
    let mut doc = ApiDoc::openapi();
    doc.info.title = "decor-pos API".to_string();
    doc.info.version = env!("CARGO_PKG_VERSION").to_string();
    Json(serde_json::to_value(doc).expect("OpenAPI document serialises"))
}

/// Build the router. Admin handlers read their pool/org/store from
/// the passed [`ApiState`]; with the default state they 503.
///
/// Phase 7 (v1.1): every route except `GET /health`,
/// `GET /api-docs/openapi.json` and the `/api/v1/auth/*` login
/// endpoints (plus the first-run setup endpoints
/// `GET /api/v1/auth/setup-status` and `POST /api/v1/auth/setup`,
/// public because no user exists yet to hold a session) requires a
/// `Bearer dfp_...` session token (see
/// `df_auth_http::gate` + `docs/auth.md` for the role → route
/// matrix). The gate answers 401 for a missing / invalid / expired
/// / revoked token and 403 for a valid token with the wrong role.
pub fn create_router(state: ApiState) -> Router {
    create_router_with_hardening(state, HttpHardeningConfig::default())
}

pub fn create_router_with_hardening(state: ApiState, hardening: HttpHardeningConfig) -> Router {
    let pool = state.pool.clone();
    let gate = |rule: AuthRule| {
        middleware::from_fn_with_state(Gate::new(pool.clone(), rule), df_auth_http::gate)
    };
    let authenticated_gate = gate(AuthRule::Authenticated);
    let inventory_view_gate = gate(AuthRule::AnyOf(vec![Permission::InventoryView]));
    let inventory_adjust_gate = gate(AuthRule::AnyOf(vec![Permission::InventoryAdjust]));
    let reports_gate = gate(AuthRule::AnyOf(vec![Permission::ReportView]));
    let ar_view_gate = gate(AuthRule::AnyOf(vec![Permission::ArView]));
    let customer_gate = gate(AuthRule::AnyOf(vec![Permission::CustomerManage]));
    let sync_ingest_gate = gate(AuthRule::AnyOfOrServiceScope {
        permissions: vec![Permission::SyncIngest],
        scope: df_auth::SCOPE_SYNC_INGEST,
    });
    let sync_conflicts_gate = gate(AuthRule::AnyOf(vec![Permission::SyncIngest]));
    let user_manage_gate = gate(AuthRule::AnyOf(vec![Permission::UserManage]));
    let rate_limiter = AuthRateLimiter::new(hardening.auth_rate_limit.clone());

    // Public by design: health, OpenAPI shapes, setup-status, and 501 stubs.
    let public = Router::new()
        .route("/health", get(health))
        .route("/api-docs/openapi.json", get(openapi_json))
        .route("/api/v1/auth/setup-status", get(setup_status))
        .route("/api/v1/auth/passkey/verify", post(passkey_verify))
        .route("/api/v1/auth/oidc/callback", get(oidc_callback));

    // Credential-checking endpoints (Argon2id / first-run setup):
    // rate-limited per client IP (closes F10).
    let rate_limited_auth = Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/setup", post(setup))
        .route("/api/v1/auth/counter-switch", post(counter_switch))
        .route("/api/v1/auth/override-verify", post(override_verify))
        .route_layer(middleware::from_fn_with_state(
            rate_limiter,
            auth_rate_limit,
        ));

    let logout_routes = Router::new()
        .route("/api/v1/auth/logout", post(logout))
        .route_layer(authenticated_gate);

    let service_token_routes = Router::new()
        .route(
            "/api/v1/auth/service-tokens",
            post(create_service_token).get(list_service_tokens),
        )
        .route(
            "/api/v1/auth/service-tokens/{id}/revoke",
            post(revoke_service_token),
        )
        .route_layer(user_manage_gate);

    let inventory_view_routes = Router::new()
        .route("/api/v1/colors/search", get(search_colors))
        .route("/api/v1/tinting/formulas", get(list_formulas))
        .route("/api/v1/tinting/colorants", get(list_colorants))
        .route("/api/v1/inventory/balances", get(inventory_balances))
        .route("/api/v1/deliveries/dispatch", get(delivery_dispatch))
        .route_layer(inventory_view_gate);

    let inventory_adjust_routes = Router::new()
        .route("/api/v1/tinting/formulas", post(create_formula))
        .route_layer(inventory_adjust_gate);

    let report_routes = Router::new()
        .route("/api/v1/reports/daily-usage", get(daily_usage))
        .route("/api/v1/reports/low-stock", get(low_stock))
        .route("/api/v1/edi/documents", get(edi_documents))
        .route_layer(reports_gate);

    let ar_routes = Router::new()
        .route(
            "/api/v1/ar/customers/{id}/summary",
            get(ar_customer_summary),
        )
        .route("/api/v1/ar/aging", get(ar_aging))
        .route_layer(ar_view_gate);

    let crm_routes = Router::new()
        .route("/api/v1/crm/activities", get(crm_activities))
        .route("/api/v1/crm/appointments", get(crm_appointments))
        .route_layer(customer_gate);

    let sync_routes = Router::new()
        .route("/api/v1/sync/ingest", post(sync_ingest))
        .route_layer(sync_ingest_gate);

    let sync_conflict_routes = Router::new()
        .route("/api/v1/sync/conflicts", get(sync_conflicts))
        .route(
            "/api/v1/sync/conflicts/{entry_id}/resolve",
            post(resolve_sync_conflict),
        )
        .route_layer(sync_conflicts_gate);

    let router = Router::new()
        .merge(public)
        .merge(rate_limited_auth)
        .merge(logout_routes)
        .merge(service_token_routes)
        .merge(inventory_view_routes)
        .merge(inventory_adjust_routes)
        .merge(report_routes)
        .merge(ar_routes)
        .merge(crm_routes)
        .merge(sync_routes)
        .merge(sync_conflict_routes)
        .with_state(state);

    apply_http_hardening(router, &hardening)
}
