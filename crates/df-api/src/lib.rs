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
//! `--db` flag), the admin endpoints return `503` with a clear
//! message instead of failing obscurely. Start the binary with
//! `--db <url> --org-id <uuid>` to enable them, or use
//! `df-server reports` / `df-server jobs run` for the CLI path.

use std::str::FromStr;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use df_auth::{OverrideCredential, Permission};
use df_auth_http::{
    AuthRule, AuthSession, ErrorBody, Gate, LoginResponse, LogoutResponse, OverrideVerifyResponse,
    SetupRequest, SetupStatusResponse, login_error, setup_admin_response, setup_status_response,
};
use df_db::AnyPool;
use df_db::InventoryBalance;
use df_db::models::{ColorCatalogEntry, ColorantLevelRow, TintFormula};
use df_domain::Currency;
use df_domain::ar::{ArEntry, aging_totals};
use df_sync::{ApplyReport, DeltaBatch, HqSink};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::OpenApi;
use utoipa::ToSchema;
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
            version: "1.2.1",
            pool: None,
            org_id: None,
            store_id: None,
        }
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
    /// Portal remember-me (30-day session) — only meaningful for a
    /// `PortalContractor`; staff sessions are always 8 hours.
    #[serde(default)]
    pub remember_me: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CounterSwitchRequest {
    /// Identifies the user: an Argon2id hash cannot be searched,
    /// so a bare PIN/badge cannot name its user (see
    /// `df_auth::login` docs).
    pub email: String,
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

/// One colorant level with its catalog identity (admin list view).
/// UUIDs and decimals are strings here, matching their JSON form —
/// `Uuid` / `Decimal` do not implement utoipa's `ToSchema`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ColorantLevelResponse {
    pub colorant_id: String,
    pub code: String,
    pub name: String,
    pub on_hand_fl_oz: String,
    pub low_threshold_fl_oz: String,
    pub is_low: bool,
}

/// Body for `POST /api/v1/tinting/colorants/levels`. UUIDs and
/// decimals are strings, matching their JSON form.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetColorantLevelRequest {
    pub colorant_id: String,
    pub store_id: String,
    pub on_hand_fl_oz: String,
    pub low_threshold_fl_oz: String,
}

#[derive(Debug, Deserialize)]
pub struct ColorSearchQuery {
    pub manufacturer: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FormulaQuery {
    pub query: Option<String>,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct StoreQuery {
    pub store_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ReportQuery {
    pub store_id: Option<Uuid>,
    pub date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ArAgingQuery {
    pub store_id: Option<Uuid>,
    pub as_of: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AppointmentListQuery {
    pub store_id: Option<Uuid>,
    pub date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DispatchQuery {
    pub store_id: Option<Uuid>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EdiDocumentsQuery {
    pub po_id: Uuid,
}

/// One customer's AR position (HQ read of the edge's summary shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArCustomerSummaryResponse {
    pub customer_id: String,
    pub account_number: String,
    pub company_name: Option<String>,
    pub customer_type: String,
    pub charge_enabled: bool,
    pub credit_limit: String,
    pub credit_hold: bool,
    pub balance: String,
    pub entry_count: usize,
    pub oldest_due_date: Option<NaiveDate>,
}

/// Per-customer aging row: buckets are
/// [current, 31-60, 61-90, 91-120, 120+], exact decimal strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgingCustomerResponse {
    pub customer_id: String,
    pub account_number: String,
    pub balance: String,
    pub buckets: [String; 5],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgingResponse {
    pub as_of: NaiveDate,
    pub totals: [String; 5],
    pub customers: Vec<AgingCustomerResponse>,
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

// --- Phase 8 sync ingest (spec §5.5/§7) -----------------------------------
// `POST /api/v1/sync/ingest`: an edge pushes one `DeltaBatch` of
// outbox entries; the DB-backed sink (`df_sync::DbHq`) applies them
// to this HQ pool in one transaction and answers with the same
// `ApplyReport` the in-process sinks return (applied / duplicates /
// conflicts / applied_ids), so the edge marks exactly the ids HQ
// resolved as synced.
//
// AUTH DECISION (Phase 8, recorded here — the coordinator carries it
// into `docs/auth.md` §6): a staff session `Bearer dfp_...` token
// whose role holds `Permission::SyncIngest` (Manager and Admin
// only). NOT a separate store service token. Rationale:
//   * No new credential type, storage, issuance CLI or rotation
//     story: sessions already exist (Phase 7), are SHA-256-hashed
//     at rest, expire after 8h, and revoke via logout /
//     `df-server users revoke-sessions`.
//   * The existing gate middleware enforces it (`AuthRule::AnyOf`),
//     answering 401 for a missing/invalid/expired token and 403
//     for a valid token with the wrong role — no bespoke auth code.
//   * The blast radius is bounded: only Manager/Admin can push
//     deltas, and the sink itself is idempotent, so a replayed or
//     intercepted batch cannot double-post money.
//   * Operational cost: the edge daemon needs a Manager/Admin
//     session token (8h TTL — re-login / re-issue on expiry). A
//     long-lived service token would remove that chore but add a
//     new secret to manage and rotate; if the edge fleet grows,
//     revisit with a dedicated service-token table then.
// A PortalContractor session is never accepted here (portal users
// hold no staff permissions).
//
// IDEMPOTENCY: enforced at the DB layer, not by trusting the
// caller — `sync_applied_entries.entry_id` is the PRIMARY KEY, so
// replaying the same entry UUIDs is a no-op (duplicates, nothing
// re-inserted, no money double-counted); see `df_sync::db::DbHq`.
// A batch whose store does not exist on HQ fails whole with 400
// (the store must be seeded on HQ before the first sync); unknown
// aggregate types are recorded applied and ignored. A conflicting
// transaction re-send is counted in `conflicts`, recorded in
// `sync_conflicts`, and NOT in `applied_ids`, so the edge leaves it
// pending for an operator.
//
// Not in the utoipa path list: `DeltaBatch` / `ApplyReport` carry
// `Uuid` / `DateTime` fields, which do not implement `ToSchema`.
pub async fn sync_ingest(
    State(state): State<ApiState>,
    Extension(_auth): Extension<AuthSession>,
    Json(batch): Json<DeltaBatch>,
) -> Result<Json<ApplyReport>, (StatusCode, Json<ErrorResponse>)> {
    let pool = pool(&state)?;
    df_sync::DbHq::new(pool)
        .apply_batch(&batch)
        .await
        .map(Json)
        .map_err(|e| match e {
            df_sync::SyncError::InvalidPayload(message) => err(StatusCode::BAD_REQUEST, message),
            other => err(StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
        })
}

/// The configured pool for an auth handler, or a 503 in the auth
/// error shape (`ErrorBody`, not the admin `ErrorResponse`).
fn api_pool(state: &ApiState) -> Result<AnyPool, (StatusCode, Json<ErrorBody>)> {
    state.pool.clone().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorBody {
                error: "no database configured for this API instance — restart df-api with --db <url> --org-id <uuid>".to_string(),
            }),
        )
    })
}

type AuthResult<T> = Result<Json<T>, (StatusCode, Json<ErrorBody>)>;

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Authenticated — token + role + expiry", body = LoginResponse),
        (status = 401, description = "Invalid credentials", body = ErrorBody),
        (status = 503, description = "No database configured", body = ErrorBody)
    )
)]
pub async fn login(
    State(state): State<ApiState>,
    Json(body): Json<LoginRequest>,
) -> AuthResult<LoginResponse> {
    let pool = api_pool(&state)?;
    df_db::login_with_password(
        &pool,
        &body.email,
        &body.password,
        body.remember_me.unwrap_or(false),
    )
    .await
    .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
    .map_err(login_error)
}

/// The configured org for a first-run setup handler, or a 503 in
/// the auth error shape. Unlike login (which reads the org off the
/// user row), setup creates the first user, so it needs the
/// instance's configured org up front.
fn api_org_id(state: &ApiState) -> Result<Uuid, (StatusCode, Json<ErrorBody>)> {
    state.org_id.ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorBody {
                error: "no org configured for this API instance — restart df-api with --db <url> --org-id <uuid>".to_string(),
            }),
        )
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/setup-status",
    responses(
        (status = 200, description = "Whether first-run setup is still needed", body = SetupStatusResponse),
        (status = 503, description = "No database configured", body = ErrorBody)
    )
)]
pub async fn setup_status(State(state): State<ApiState>) -> AuthResult<SetupStatusResponse> {
    let pool = api_pool(&state)?;
    setup_status_response(&pool).await
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/setup",
    request_body = SetupRequest,
    responses(
        (status = 201, description = "First admin created — token + role + expiry", body = LoginResponse),
        (status = 400, description = "Blank email / password, or a password under 8 characters", body = ErrorBody),
        (status = 409, description = "Setup already complete — a user already exists", body = ErrorBody),
        (status = 503, description = "No database / org configured", body = ErrorBody)
    )
)]
pub async fn setup(
    State(state): State<ApiState>,
    Json(body): Json<SetupRequest>,
) -> Result<(StatusCode, Json<LoginResponse>), (StatusCode, Json<ErrorBody>)> {
    let pool = api_pool(&state)?;
    let org_id = api_org_id(&state)?;
    setup_admin_response(&pool, org_id, body).await
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/counter-switch",
    request_body = CounterSwitchRequest,
    responses(
        (status = 200, description = "Authenticated — token + role + expiry", body = LoginResponse),
        (status = 400, description = "Neither a PIN nor a badge token was given", body = ErrorBody),
        (status = 401, description = "Invalid credentials", body = ErrorBody),
        (status = 503, description = "No database configured", body = ErrorBody)
    )
)]
pub async fn counter_switch(
    State(state): State<ApiState>,
    Json(body): Json<CounterSwitchRequest>,
) -> AuthResult<LoginResponse> {
    let pool = api_pool(&state)?;
    if let Some(pin) = &body.pin {
        return df_db::login_with_pin(&pool, &body.email, pin)
            .await
            .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
            .map_err(login_error);
    }
    if let Some(badge) = &body.badge_token {
        return df_db::login_with_badge(&pool, &body.email, badge)
            .await
            .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
            .map_err(login_error);
    }
    Err((
        StatusCode::BAD_REQUEST,
        Json(ErrorBody {
            error: "counter-switch needs a pin or a badge_token".to_string(),
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    responses(
        (status = 200, description = "Session revoked", body = LogoutResponse),
        (status = 401, description = "Invalid or expired session", body = ErrorBody)
    )
)]
pub async fn logout(
    State(state): State<ApiState>,
    Extension(auth): Extension<AuthSession>,
) -> AuthResult<LogoutResponse> {
    let pool = api_pool(&state)?;
    match df_db::revoke_session(&pool, auth.session_id).await {
        Ok(()) => Ok(Json(LogoutResponse { revoked: true })),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: "logout failed".to_string(),
            }),
        )),
    }
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
    responses(
        (status = 200, description = "Override approved — audit fields", body = OverrideVerifyResponse),
        (status = 401, description = "Invalid credentials", body = ErrorBody),
        (status = 403, description = "Approver is not a Manager or Admin", body = ErrorBody),
        (status = 503, description = "No database configured", body = ErrorBody)
    )
)]
pub async fn override_verify(
    State(state): State<ApiState>,
    Json(body): Json<OverrideVerifyRequest>,
) -> AuthResult<OverrideVerifyResponse> {
    let pool = api_pool(&state)?;
    if body.reason_code.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: "reason_code is required".to_string(),
            }),
        ));
    }
    let user_id = Uuid::parse_str(&body.user_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody {
                error: format!("user_id is not a UUID: {:?}", body.user_id),
            }),
        )
    })?;
    // Unknown approver id is a generic 401, like an unknown email.
    let user = df_db::get_user_by_id(&pool, user_id)
        .await
        .map_err(|_| login_error(df_auth::AuthError::InvalidCredentials))?;
    df_db::verify_manager_override(&pool, &user.email, &OverrideCredential::Pin(body.pin))
        .await
        .map(|approval| Json(OverrideVerifyResponse::from_approval(&approval)))
        .map_err(login_error)
}

// --- Phase 2 tinting admin (spec §6.1) --------------------------------------

#[utoipa::path(
    get,
    path = "/api/v1/colors/search",
    responses(
        (status = 200, description = "Matching colors"),
        (status = 400, description = "manufacturer is required"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn search_colors(
    State(state): State<ApiState>,
    Query(query): Query<ColorSearchQuery>,
) -> ApiResult<Vec<ColorCatalogEntry>> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let manufacturer = query.manufacturer.as_deref().unwrap_or("").trim();
    if manufacturer.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "manufacturer is required (?manufacturer=)",
        ));
    }
    df_db::search_colors(
        &pool,
        org,
        manufacturer,
        query.query.as_deref().unwrap_or(""),
    )
    .await
    .map(Json)
    .map_err(db_err)
}

#[utoipa::path(
    get,
    path = "/api/v1/tinting/formulas",
    responses(
        (status = 200, description = "Matching formulas, newest first"),
        (status = 400, description = "A color query or customer_id is required"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn list_formulas(
    State(state): State<ApiState>,
    Query(query): Query<FormulaQuery>,
) -> ApiResult<Vec<TintFormula>> {
    let pool = pool(&state)?;
    if let Some(customer_id) = query.customer_id {
        return df_db::list_tint_formulas_for_customer_job(&pool, customer_id, query.job_id)
            .await
            .map(Json)
            .map_err(db_err);
    }
    match query.query {
        Some(q) => {
            let org = org_id(&state)?;
            df_db::search_tint_formulas_by_color(&pool, org, &q)
                .await
                .map(Json)
                .map_err(db_err)
        }
        None => Err(err(
            StatusCode::BAD_REQUEST,
            "formula search needs ?query= (color) or ?customer_id=",
        )),
    }
}

/// Create a tint formula (admin). Not in the utoipa path list: the
/// `TintFormula` row type carries `Uuid`/`Decimal` fields, which do
/// not implement utoipa's `ToSchema`, so the endpoint is documented
/// in the module docs instead of the generated OpenAPI document.
pub async fn create_formula(
    State(state): State<ApiState>,
    Json(formula): Json<TintFormula>,
) -> Result<(StatusCode, Json<TintFormula>), (StatusCode, Json<ErrorResponse>)> {
    let pool = pool(&state)?;
    if !formula.colorants_json.is_array() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "colorants_json must be an array of {code, oz, shots} lines",
        ));
    }
    df_db::insert_tint_formula(&pool, &formula)
        .await
        .map(|()| (StatusCode::CREATED, Json(formula)))
        .map_err(db_err)
}

#[utoipa::path(
    get,
    path = "/api/v1/tinting/colorants",
    responses(
        (status = 200, description = "Levels plus shortage alerts"),
        (status = 400, description = "store_id is required"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn list_colorants(
    State(state): State<ApiState>,
    Query(query): Query<StoreQuery>,
) -> ApiResult<serde_json::Value> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    let rows = df_db::list_colorant_levels_for_store(&pool, store)
        .await
        .map_err(db_err)?;
    let mut levels = Vec::with_capacity(rows.len());
    for row in rows {
        let colorant = df_db::get_colorant(&pool, row.colorant_id)
            .await
            .map_err(db_err)?;
        levels.push(ColorantLevelResponse {
            colorant_id: row.colorant_id.to_string(),
            code: colorant.code,
            name: colorant.name,
            on_hand_fl_oz: row.on_hand_fl_oz.to_string(),
            low_threshold_fl_oz: row.low_threshold_fl_oz.to_string(),
            is_low: row.on_hand_fl_oz <= row.low_threshold_fl_oz,
        });
    }
    levels.sort_by(|a, b| a.code.cmp(&b.code));
    let alerts = df_reports::colorant_shortage_alerts(&pool, store)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        serde_json::json!({ "levels": levels, "alerts": alerts }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/tinting/colorants/levels",
    request_body = SetColorantLevelRequest,
    responses(
        (status = 200, description = "Level stored"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn set_colorant_level(
    State(state): State<ApiState>,
    Json(body): Json<SetColorantLevelRequest>,
) -> ApiResult<ColorantLevelRow> {
    let pool = pool(&state)?;
    let parse_uuid = |text: &str, what: &str| {
        Uuid::parse_str(text).map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                format!("{what} is not a UUID: {text:?}"),
            )
        })
    };
    let parse_dec = |text: &str, what: &str| {
        Decimal::from_str(text).map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                format!("{what} is not a decimal: {text:?}"),
            )
        })
    };
    let level = ColorantLevelRow {
        colorant_id: parse_uuid(&body.colorant_id, "colorant_id")?,
        store_id: parse_uuid(&body.store_id, "store_id")?,
        on_hand_fl_oz: parse_dec(&body.on_hand_fl_oz, "on_hand_fl_oz")?,
        low_threshold_fl_oz: parse_dec(&body.low_threshold_fl_oz, "low_threshold_fl_oz")?,
        updated_at: chrono::Utc::now(),
    };
    df_db::upsert_colorant_level(&pool, &level)
        .await
        .map(|()| Json(level))
        .map_err(db_err)
}

// --- Phase 2 reports (spec §6.1) ---------------------------------------------

#[utoipa::path(
    get,
    path = "/api/v1/reports/daily-usage",
    responses(
        (status = 200, description = "Daily usage report"),
        (status = 400, description = "store_id is required or date is malformed"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn daily_usage(
    State(state): State<ApiState>,
    Query(query): Query<ReportQuery>,
) -> ApiResult<serde_json::Value> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let store = store_id(query.store_id, &state)?;
    let date = report_date(query.date)?;
    let store_row = df_db::get_store(&pool, store).await.map_err(db_err)?;
    let currency = Currency::from_str(&store_row.currency)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    df_reports::daily_usage_report(&pool, org, store, date, currency)
        .await
        .map(|report| Json(serde_json::to_value(report).expect("report serialises")))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[utoipa::path(
    get,
    path = "/api/v1/reports/low-stock",
    responses(
        (status = 200, description = "Low-stock report (colorants; products need thresholds — see docs)"),
        (status = 400, description = "store_id is required or date is malformed"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn low_stock(
    State(state): State<ApiState>,
    Query(query): Query<ReportQuery>,
) -> ApiResult<serde_json::Value> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    let date = report_date(query.date)?;
    // No product thresholds are supplied here: there is no
    // min-level table yet (see `df-reports` docs), so this endpoint
    // reports colorants and leaves `products` empty. The edge
    // low-stock endpoint takes operator-configured thresholds.
    df_reports::low_stock_report(&pool, store, date, &[])
        .await
        .map(|report| Json(serde_json::to_value(report).expect("report serialises")))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[utoipa::path(
    get,
    path = "/api/v1/reports/shortage-alerts",
    responses(
        (status = 200, description = "Colorant shortage alerts"),
        (status = 400, description = "store_id is required"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn shortage_alerts(
    State(state): State<ApiState>,
    Query(query): Query<StoreQuery>,
) -> ApiResult<serde_json::Value> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    df_reports::colorant_shortage_alerts(&pool, store)
        .await
        .map(|alerts| Json(serde_json::json!({ "alerts": alerts })))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

// --- Phase 3 inventory + AR reads (spec §6.1) -------------------------------
// Read-only HQ mirror of the edge back-office surface: balances are
// derived from the append-only ledger by `df-db`, aging buckets by
// `df-domain::ar` — the same helpers the edge endpoints use, so HQ
// and edge agree. Mutating purchasing / transfer / count endpoints
// stay edge-only (they need the edge's store context and ledger
// transactions); the HQ API deliberately does not duplicate them.

/// Stock positions for every product at a store.
pub async fn inventory_balances(
    State(state): State<ApiState>,
    Query(query): Query<StoreQuery>,
) -> ApiResult<Vec<InventoryBalance>> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let store = store_id(query.store_id, &state)?;
    df_db::inventory_balances(&pool, org, store)
        .await
        .map(Json)
        .map_err(db_err)
}

/// One customer's AR position: ledger balance, credit controls and
/// entry count.
pub async fn ar_customer_summary(
    State(state): State<ApiState>,
    Path(customer_id): Path<Uuid>,
) -> ApiResult<ArCustomerSummaryResponse> {
    let pool = pool(&state)?;
    let customer = df_db::get_customer(&pool, customer_id)
        .await
        .map_err(db_err)?;
    let entries = df_db::list_ar_entries_for_customer(&pool, customer_id)
        .await
        .map_err(db_err)?;
    let balance = entries
        .iter()
        .map(|entry| entry.amount)
        .fold(Decimal::ZERO, |acc, amount| acc + amount);
    let oldest_due_date = entries.iter().filter_map(|entry| entry.due_date).min();
    Ok(Json(ArCustomerSummaryResponse {
        customer_id: customer.id.to_string(),
        account_number: customer.account_number,
        company_name: customer.company_name,
        customer_type: customer.customer_type,
        charge_enabled: customer.charge_enabled,
        credit_limit: customer.credit_limit.to_string(),
        credit_hold: customer.credit_hold,
        balance: balance.to_string(),
        entry_count: entries.len(),
        oldest_due_date,
    }))
}

/// 30/60/90/120 aging for every customer with AR entries at a
/// store. Same bucketing rule as the edge endpoint: positive entries
/// bucket by due date (no due date = current); payments and credit
/// memos (negative amounts) are unallocated and reduce the Current
/// bucket, so the bucket sum always equals the balance.
pub async fn ar_aging(
    State(state): State<ApiState>,
    Query(query): Query<ArAgingQuery>,
) -> ApiResult<AgingResponse> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    let as_of = report_date(query.as_of)?;
    let store_row = df_db::get_store(&pool, store).await.map_err(db_err)?;
    let currency = Currency::from_str(&store_row.currency)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let entries = df_db::list_ar_entries_for_store(&pool, store)
        .await
        .map_err(db_err)?;
    let mut by_customer: std::collections::HashMap<Uuid, Vec<df_db::models::ArLedgerEntry>> =
        std::collections::HashMap::new();
    for entry in entries {
        by_customer
            .entry(entry.customer_id)
            .or_default()
            .push(entry);
    }
    let mut customers: Vec<AgingCustomerResponse> = Vec::new();
    let mut totals = [
        df_domain::Money::zero(currency),
        df_domain::Money::zero(currency),
        df_domain::Money::zero(currency),
        df_domain::Money::zero(currency),
        df_domain::Money::zero(currency),
    ];
    for (customer_id, customer_entries) in &by_customer {
        let customer = df_db::get_customer(&pool, *customer_id)
            .await
            .map_err(db_err)?;
        let mut open: Vec<ArEntry> = Vec::new();
        let mut credits = df_domain::Money::zero(currency);
        for entry in customer_entries {
            if entry.amount >= Decimal::ZERO {
                open.push(ArEntry {
                    id: entry.id,
                    customer_id: entry.customer_id,
                    job_id: entry.job_id,
                    balance: df_domain::Money::new(entry.amount, currency),
                    due_date: entry.due_date.unwrap_or(entry.created_at.date_naive()),
                });
            } else {
                credits = credits
                    .checked_add(df_domain::Money::new(entry.amount, currency))
                    .unwrap_or(credits);
            }
        }
        let mut buckets = aging_totals(&open, as_of, currency);
        if !credits.is_zero() {
            buckets[0] = buckets[0].checked_add(credits).unwrap_or(buckets[0]);
        }
        let balance = customer_entries
            .iter()
            .map(|entry| entry.amount)
            .fold(Decimal::ZERO, |acc, amount| acc + amount);
        for (index, bucket) in buckets.iter().enumerate() {
            totals[index] = totals[index].checked_add(*bucket).unwrap_or(totals[index]);
        }
        customers.push(AgingCustomerResponse {
            customer_id: customer_id.to_string(),
            account_number: customer.account_number,
            balance: balance.to_string(),
            buckets: buckets
                .iter()
                .map(|bucket| bucket.amount().to_string())
                .collect::<Vec<_>>()
                .try_into()
                .expect("aging_totals returns 5 buckets"),
        });
    }
    customers.sort_by(|a, b| a.account_number.cmp(&b.account_number));
    Ok(Json(AgingResponse {
        as_of,
        totals: totals
            .iter()
            .map(|total| total.amount().to_string())
            .collect::<Vec<_>>()
            .try_into()
            .expect("aging_totals returns 5 buckets"),
        customers,
    }))
}

// --- Phase 4 HQ reads -------------------------------------------------------
// Read-only HQ views of the edge's CRM, delivery and EDI rows
// (spec §6.1/§7). Writes stay on the edge; the lapsed-contractor
// report is edge-only (see the module docs).

/// CRM activity log for a store, newest first (same rows as the
/// edge's `GET /crm/activities`, no filters on the HQ read).
pub async fn crm_activities(
    State(state): State<ApiState>,
    Query(query): Query<StoreQuery>,
) -> ApiResult<Vec<df_db::models::CrmActivity>> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    df_db::list_crm_activities_for_store(&pool, store)
        .await
        .map(Json)
        .map_err(db_err)
}

/// Appointments for a store, earliest first, optionally on one
/// UTC calendar day (`?date=YYYY-MM-DD`).
pub async fn crm_appointments(
    State(state): State<ApiState>,
    Query(query): Query<AppointmentListQuery>,
) -> ApiResult<Vec<df_db::models::Appointment>> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    let day = match query.date {
        None => None,
        Some(text) => Some(NaiveDate::from_str(&text).map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                format!("date must be YYYY-MM-DD, got {text:?}"),
            )
        })?),
    };
    let mut appointments = df_db::list_appointments_for_store(&pool, store)
        .await
        .map_err(db_err)?;
    if let Some(day) = day {
        appointments.retain(|a| a.starts_at.date_naive() == day);
    }
    appointments.sort_by_key(|a| a.starts_at);
    Ok(Json(appointments))
}

/// The dispatch board for a store: open deliveries, highest
/// priority first, then oldest first. An optional `?status=`
/// filters the board (storage strings: `draft`, `assigned`,
/// `out_for_delivery`, `delivered`, `cancelled` — an unknown
/// status is a 400).
pub async fn delivery_dispatch(
    State(state): State<ApiState>,
    Query(query): Query<DispatchQuery>,
) -> ApiResult<Vec<df_db::models::Delivery>> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    if let Some(status) = &query.status
        && !matches!(
            status.as_str(),
            "draft" | "assigned" | "out_for_delivery" | "delivered" | "cancelled"
        )
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            format!(
                "unknown delivery status {status:?} (draft, assigned, out_for_delivery, delivered, cancelled)"
            ),
        ));
    }
    let mut deliveries = df_db::list_dispatch_board(&pool, store)
        .await
        .map_err(db_err)?;
    if let Some(status) = query.status {
        deliveries.retain(|d| d.status == status);
    }
    Ok(Json(deliveries))
}

/// EDI documents linked to a PO, newest first. The PO must exist
/// (unknown ids are a 404).
pub async fn edi_documents(
    State(state): State<ApiState>,
    Query(query): Query<EdiDocumentsQuery>,
) -> ApiResult<Vec<df_edi::EdiDocument>> {
    let pool = pool(&state)?;
    df_edi::repos::get_purchase_order(&pool, query.po_id)
        .await
        .map_err(edi_err)?;
    df_edi::repos::list_edi_documents_for_po(&pool, query.po_id)
        .await
        .map(Json)
        .map_err(edi_err)
}

// --- OpenAPI document ------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        login,
        setup_status,
        setup,
        counter_switch,
        logout,
        passkey_verify,
        oidc_callback,
        override_verify,
        search_colors,
        list_formulas,
        list_colorants,
        set_colorant_level,
        daily_usage,
        low_stock,
        shortage_alerts
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
        OverrideVerifyResponse,
        ColorantLevelResponse,
        SetColorantLevelRequest
    )),
    tags(
        (name = "auth", description = "Authentication & counter switching"),
        (name = "tinting", description = "Paint & tinting admin (Phase 2)"),
        (name = "reports", description = "Daily usage, low-stock and shortage reports (Phase 2)")
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
    let sync_ingest_gate = gate(AuthRule::AnyOf(vec![Permission::SyncIngest]));

    // Public by design: health, the OpenAPI document (shapes only —
    // no data rows), and the login endpoints (how a session is
    // obtained) plus the first-run setup endpoints (how the first
    // admin is created, before any user exists). Passkey/OIDC stay
    // 501 stubs.
    let public = Router::new()
        .route("/health", get(health))
        .route("/api-docs/openapi.json", get(openapi_json))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/setup-status", get(setup_status))
        .route("/api/v1/auth/setup", post(setup))
        .route("/api/v1/auth/counter-switch", post(counter_switch))
        .route("/api/v1/auth/passkey/verify", post(passkey_verify))
        .route("/api/v1/auth/oidc/callback", get(oidc_callback))
        .route("/api/v1/auth/override-verify", post(override_verify));

    let logout_routes = Router::new()
        .route("/api/v1/auth/logout", post(logout))
        .route_layer(authenticated_gate);

    let inventory_view_routes = Router::new()
        .route("/api/v1/colors/search", get(search_colors))
        .route("/api/v1/tinting/formulas", get(list_formulas))
        .route("/api/v1/tinting/colorants", get(list_colorants))
        .route("/api/v1/inventory/balances", get(inventory_balances))
        .route("/api/v1/deliveries/dispatch", get(delivery_dispatch))
        .route_layer(inventory_view_gate);

    let inventory_adjust_routes = Router::new()
        .route("/api/v1/tinting/formulas", post(create_formula))
        .route("/api/v1/tinting/colorants/levels", post(set_colorant_level))
        .route_layer(inventory_adjust_gate);

    let report_routes = Router::new()
        .route("/api/v1/reports/daily-usage", get(daily_usage))
        .route("/api/v1/reports/low-stock", get(low_stock))
        .route("/api/v1/reports/shortage-alerts", get(shortage_alerts))
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

    Router::new()
        .merge(public)
        .merge(logout_routes)
        .merge(inventory_view_routes)
        .merge(inventory_adjust_routes)
        .merge(report_routes)
        .merge(ar_routes)
        .merge(crm_routes)
        .merge(sync_routes)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use chrono::Utc;
    use df_db::models::{ColorCatalogEntry, Colorant, Organization, Store};
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use tower::ServiceExt;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    async fn seeded_state() -> (ApiState, df_auth_http::test_support::SeededUsers) {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
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
        df_db::insert_color(
            &pool,
            &ColorCatalogEntry {
                id: Uuid::from_u128(0x6000),
                org_id,
                manufacturer: "Benjamin Moore".to_string(),
                color_code: "OC-17".to_string(),
                color_name: "White Dove".to_string(),
                collection: None,
                rgb_hex: None,
                recommended_base: None,
            },
        )
        .await
        .unwrap();
        // The formula's base product must exist (SQLite enforces
        // the `tint_formulas.base_product_id -> products(id)` FK).
        df_db::insert_product(
            &pool,
            &df_db::models::Product {
                id: Uuid::from_u128(0x3000),
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
                list_price: dec("100.0000"),
                standard_cost: dec("50.0000"),
                avg_cost: dec("50.0000"),
                min_margin_pct: None,
                sheen: Some("Eggshell".to_string()),
                paint_base_code: Some("1X".to_string()),
                container_volume_ml: Some(3785),
                eco_fee_category: Some("architectural_paint".to_string()),
                tax_class_id: Uuid::now_v7(),
                prompt_template_id: None,
                is_active: true,
                map: None,
                msrp: None,
            },
        )
        .await
        .unwrap();
        let y3 = Uuid::from_u128(0x5001);
        df_db::insert_colorant(
            &pool,
            &Colorant {
                id: y3,
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
        df_db::upsert_colorant_level(
            &pool,
            &ColorantLevelRow {
                colorant_id: y3,
                store_id,
                on_hand_fl_oz: dec("1"),
                low_threshold_fl_oz: dec("2"),
                updated_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        let users = df_auth_http::test_support::seed_test_users(&pool, org_id).await;
        (
            ApiState {
                version: "1.2.1",
                pool: Some(pool),
                org_id: Some(org_id),
                store_id: Some(store_id),
            },
            users,
        )
    }

    async fn get_json(
        router: Router,
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

    async fn post_json(
        router: Router,
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
        assert_eq!(value["version"], "1.2.1");
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
        assert!(value["paths"]["/api/v1/tinting/formulas"].is_object());
        assert!(value["paths"]["/api/v1/reports/daily-usage"].is_object());
        // The public document describes shapes only: it must not
        // contain any data rows (no product SKU, no user email, no
        // token, no seeded account number).
        let text = serde_json::to_string(&value).unwrap();
        for leak in [
            "BM-REGAL-1G",
            "example.invalid",
            "dfp_",
            "C-PORTAL-A",
            "C-100",
        ] {
            assert!(!text.contains(leak), "openapi doc leaks {leak}");
        }
    }

    #[tokio::test]
    async fn inventory_and_ar_reads() {
        use df_db::models::{ArLedgerEntry, Customer, InventoryLedgerEntry};

        let (state, users) = seeded_state().await;
        let pool = state.pool.clone().unwrap();
        let store_id = state.store_id.unwrap();
        let org_id = state.org_id.unwrap();
        let router = create_router(state);
        let admin = Some(users.admin.token.as_str());

        // 10 units received against the seeded Regal product.
        df_db::insert_inventory_delta(
            &pool,
            &InventoryLedgerEntry {
                id: Uuid::now_v7(),
                org_id,
                store_id,
                product_id: Uuid::from_u128(0x3000),
                lot_number: None,
                delta_on_hand: dec("10"),
                delta_committed: Decimal::ZERO,
                delta_on_order: Decimal::ZERO,
                delta_in_transit: Decimal::ZERO,
                unit_cost: dec("50.0000"),
                reason_type: "po_receive".to_string(),
                reference_id: Uuid::now_v7(),
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/inventory/balances?store_id={store_id}"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["sku"], "BM-REGAL-1G");
        assert_eq!(body[0]["on_hand"], "10");

        // One charge customer: $100 due Nov 1 (current on Nov 25),
        // $50 due Oct 1 (31-60), $20 payment (reduces current).
        let customer_id = Uuid::from_u128(0x4000);
        df_db::insert_customer(
            &pool,
            &Customer {
                id: customer_id,
                org_id,
                account_number: "C-100".to_string(),
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
                credit_limit: dec("1000.0000"),
                payment_terms: "NET30".to_string(),
                terms_days: None,
                finance_rate_pct_monthly: None,
                credit_hold: false,
                require_po_number: false,
                require_job_name: false,
                portal_enabled: false,
            },
        )
        .await
        .unwrap();
        for (amount, due) in [
            ("100.0000", Some("2026-11-01")),
            ("50.0000", Some("2026-10-01")),
            ("-20.0000", None),
        ] {
            df_db::insert_ar_entry(
                &pool,
                &ArLedgerEntry {
                    id: Uuid::now_v7(),
                    org_id,
                    store_id,
                    customer_id,
                    job_id: None,
                    entry_type: if amount.starts_with('-') {
                        "payment".to_string()
                    } else {
                        "charge".to_string()
                    },
                    reference_type: "transaction".to_string(),
                    reference_id: Uuid::now_v7(),
                    amount: dec(amount),
                    due_date: due
                        .map(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap()),
                    statement_id: None,
                    created_at: Utc::now(),
                },
            )
            .await
            .unwrap();
        }
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/ar/customers/{customer_id}/summary"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["balance"], "130.0000");
        assert_eq!(body["entry_count"], 3);
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/ar/aging?store_id={store_id}&as_of=2026-11-25"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["totals"],
            serde_json::json!(["80.0000", "50.0000", "0.0000", "0.0000", "0.0000"])
        );
        assert_eq!(body["customers"][0]["account_number"], "C-100");
    }

    #[tokio::test]
    async fn hq_login_logout_flow() {
        use df_auth_http::test_support::{TEST_PASSWORD, TEST_PIN};

        let (state, users) = seeded_state().await;
        let store_id = state.store_id.unwrap();
        let router = create_router(state);

        // Wrong password: generic 401.
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/auth/login",
            serde_json::json!({"email": "admin@example.invalid", "password": "wrong password"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "invalid credentials");

        // Right password: a dfp_ token, role + expiry in the body.
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
        assert_eq!(body["role"], "Admin");
        assert!(body["expires_at"].is_string());

        // The token works on a protected route…
        let (status, _) = get_json(
            router.clone(),
            &format!("/api/v1/inventory/balances?store_id={store_id}"),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // …logout revokes it…
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/auth/logout",
            serde_json::json!({}),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["revoked"], true);

        // …and the revoked token is a 401 from then on.
        let (status, _) = get_json(
            router.clone(),
            &format!("/api/v1/inventory/balances?store_id={store_id}"),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Counter-switch by PIN for the cashier, and a manager
        // override approval by the manager's PIN.
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/auth/counter-switch",
            serde_json::json!({"email": "cashier@example.invalid", "pin": TEST_PIN}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["role"], "Cashier");

        let (status, body) = post_json(
            router.clone(),
            "/api/v1/auth/override-verify",
            serde_json::json!({"user_id": users.manager.id().to_string(), "pin": TEST_PIN, "reason_code": "price_below_floor"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["approved"], true);
        assert_eq!(body["approver_role"], "Manager");
    }

    #[tokio::test]
    async fn hq_unauthenticated_is_401() {
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
        // Health and the OpenAPI document stay public.
        let (status, _) = get_json(router.clone(), "/health", None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = get_json(router, "/api-docs/openapi.json", None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn hq_cashier_cannot_set_colorant_level_but_manager_can() {
        let (state, users) = seeded_state().await;
        let store_id = state.store_id.unwrap();
        let router = create_router(state);
        let body = serde_json::json!({
            "colorant_id": Uuid::from_u128(0x5001).to_string(),
            "store_id": store_id.to_string(),
            "on_hand_fl_oz": "10",
            "low_threshold_fl_oz": "2",
        });
        // Setting a colorant level needs InventoryAdjust: the
        // cashier gets 403, the manager succeeds.
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
    async fn admin_endpoints_503_without_a_database() {
        let router = create_router(ApiState::default());
        let (status, body) = get_json(
            router,
            "/api/v1/reports/shortage-alerts?store_id=00000000-0000-7000-8000-000000000001",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["error"].as_str().unwrap().contains("--db"));
    }

    #[tokio::test]
    async fn tinting_admin_round_trip() {
        let (state, users) = seeded_state().await;
        let store_id = state.store_id.unwrap();
        let org_id = state.org_id.unwrap();
        let router = create_router(state);
        let _ = org_id;
        let admin = Some(users.admin.token.as_str());

        // Color search finds White Dove.
        let (status, body) = get_json(
            router.clone(),
            "/api/v1/colors/search?manufacturer=Benjamin%20Moore&query=dove",
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["color_code"], "OC-17");

        // Create a formula, then find it by customer + by color.
        let customer_id = Uuid::from_u128(0x4000);
        let formula_id = Uuid::from_u128(0x7000);
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/tinting/formulas",
            serde_json::json!({
                "id": formula_id.to_string(),
                "org_id": Uuid::from_u128(0x2000).to_string(),
                "store_id": store_id.to_string(),
                "customer_id": customer_id.to_string(),
                "job_id": null,
                "color_id": Uuid::from_u128(0x6000).to_string(),
                "color_code_snapshot": "OC-17",
                "color_name_snapshot": "White Dove",
                "base_product_id": Uuid::from_u128(0x3000).to_string(),
                "container_volume_ml": 3785,
                "sheen": "Eggshell",
                "room_or_usage": null,
                "is_custom_match": false,
                "colorants_json": [{"code": "Y3", "oz": 0, "shots": 14}],
                "total_tint_cost": "0.1225",
                "tint_surcharge": "0.0000",
                "dispenser_name": "virtual-simulator",
                "external_formula_ref": null,
                "dispensed_at": Utc::now().to_rfc3339(),
            }),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["id"], formula_id.to_string());
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/tinting/formulas?customer_id={customer_id}"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);
        let (status, body) = get_json(
            router.clone(),
            "/api/v1/tinting/formulas?query=white%20dove",
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);

        // Colorant levels: Y3 is low (1 on hand, threshold 2).
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/tinting/colorants?store_id={store_id}"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["levels"].as_array().unwrap().len(), 1);
        assert_eq!(body["levels"][0]["code"], "Y3");
        assert_eq!(body["levels"][0]["is_low"], true);
        assert_eq!(body["alerts"].as_array().unwrap().len(), 1);

        // Raise the level above threshold; the alert clears.
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/tinting/colorants/levels",
            serde_json::json!({
                "colorant_id": Uuid::from_u128(0x5001).to_string(),
                "store_id": store_id.to_string(),
                "on_hand_fl_oz": "10",
                "low_threshold_fl_oz": "2",
            }),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["on_hand_fl_oz"], "10");
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/reports/shortage-alerts?store_id={store_id}"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["alerts"].as_array().unwrap().is_empty());

        // Reports work (empty day, no tints yet).
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/reports/daily-usage?store_id={store_id}"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["tints_dispensed"], 0);
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/reports/low-stock?store_id={store_id}"),
            admin,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["colorants"].as_array().unwrap().is_empty());
    }

    fn sync_sale_delta_batch(
        store_id: Uuid,
        customer_id: Uuid,
        tx_id: Uuid,
    ) -> df_sync::DeltaBatch {
        use df_sync::{DeltaBatch, OutboxEntry};
        DeltaBatch::new(
            store_id,
            vec![
                OutboxEntry::new(
                    store_id,
                    "transaction",
                    tx_id,
                    serde_json::json!({
                        "id": tx_id.to_string(),
                        "doc_number": "TX-000001",
                        "doc_type": "sale",
                        "status": "completed",
                        "customer_id": customer_id.to_string(),
                        "subtotal": "200.0000",
                        "tax_total": "16.1088",
                        "eco_fee_total": "0.0000",
                        "pos_fee_total": "0.0000",
                        "grand_total": "216.1088"
                    }),
                ),
                OutboxEntry::new(
                    store_id,
                    "inventory_ledger",
                    Uuid::now_v7(),
                    serde_json::json!({
                        "product_id": Uuid::from_u128(0x3000).to_string(),
                        "delta_on_hand": "-2",
                        "delta_committed": "0",
                        "delta_on_order": "0",
                        "delta_in_transit": "0",
                        "unit_cost": "50.0000",
                        "lot_number": null,
                        "reason_type": "sale",
                        "reference_id": tx_id.to_string(),
                        "created_at": Utc::now().to_rfc3339()
                    }),
                ),
                OutboxEntry::new(
                    store_id,
                    "ar_ledger",
                    Uuid::now_v7(),
                    serde_json::json!({
                        "customer_id": customer_id.to_string(),
                        "amount": "216.1088",
                        "entry_type": "charge",
                        "reference_type": "transaction",
                        "reference_id": tx_id.to_string(),
                        "due_date": "2026-12-26",
                        "created_at": Utc::now().to_rfc3339()
                    }),
                ),
            ],
        )
    }

    fn sync_sale_batch(store_id: Uuid, customer_id: Uuid, tx_id: Uuid) -> serde_json::Value {
        serde_json::to_value(sync_sale_delta_batch(store_id, customer_id, tx_id)).unwrap()
    }

    #[tokio::test]
    async fn sync_ingest_applies_sale_and_replay_is_noop() {
        let (state, users) = seeded_state().await;
        let store_id = Uuid::from_u128(0x1000);
        let router = create_router(state);
        let tx_id = Uuid::now_v7();
        let batch = sync_sale_batch(store_id, users.customer_a, tx_id);
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/sync/ingest",
            batch.clone(),
            Some(&users.manager.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["applied"], 3);
        assert_eq!(body["duplicates"], 0);
        assert_eq!(body["conflicts"], 0);
        assert_eq!(body["applied_ids"].as_array().unwrap().len(), 3);

        // Replay: the same entry UUIDs are a no-op.
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/sync/ingest",
            batch,
            Some(&users.manager.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["applied"], 0);
        assert_eq!(body["duplicates"], 3);
        assert_eq!(body["conflicts"], 0);
    }

    #[tokio::test]
    async fn sync_ingest_auth() {
        let (state, users) = seeded_state().await;
        let router = create_router(state);
        let empty = serde_json::json!({
            "batch_id": Uuid::now_v7().to_string(),
            "store_id": Uuid::from_u128(0x1000).to_string(),
            "entries": []
        });
        // No token -> 401.
        let (status, _) =
            post_json(router.clone(), "/api/v1/sync/ingest", empty.clone(), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // Cashier -> 403 (SyncIngest is Manager/Admin only).
        let (status, _) = post_json(
            router.clone(),
            "/api/v1/sync/ingest",
            empty.clone(),
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // Portal contractor -> 403 (no staff permissions at all).
        let (status, _) = post_json(
            router.clone(),
            "/api/v1/sync/ingest",
            empty.clone(),
            Some(&users.contractor_a.token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        // Manager -> 200 on an empty batch.
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/sync/ingest",
            empty.clone(),
            Some(&users.manager.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["applied"], 0);
        // Admin -> 200 too.
        let (status, body) = post_json(
            router,
            "/api/v1/sync/ingest",
            empty,
            Some(&users.admin.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    #[tokio::test]
    async fn sync_ingest_remote_hq_end_to_end() {
        use df_sync::{HqSink, RemoteHq};
        let (state, users) = seeded_state().await;
        let store_id = Uuid::from_u128(0x1000);
        let router = create_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let remote = RemoteHq::new(format!("http://{addr}"), users.manager.token.clone());
        let batch = sync_sale_delta_batch(store_id, users.customer_a, Uuid::now_v7());
        let report = remote.apply_batch(&batch).await.unwrap();
        assert_eq!(report.applied, 3);
        assert_eq!(report.duplicates, 0);
        assert_eq!(report.conflicts, 0);
        // Replay through the remote client is a duplicate, not a
        // double post.
        let replay = remote.apply_batch(&batch).await.unwrap();
        assert_eq!(replay.applied, 0);
        assert_eq!(replay.duplicates, 3);
    }
}
