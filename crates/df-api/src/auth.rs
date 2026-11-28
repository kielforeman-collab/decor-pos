//! HQ authentication, first-run admin setup, manager override, and
//! edge sync ingest handlers.

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use df_auth::OverrideCredential;
use df_auth_http::{
    AuthSession, ErrorBody, LoginResponse, LogoutResponse, OverrideVerifyResponse, SetupRequest,
    SetupStatusResponse, login_error, setup_admin_response, setup_status_response,
};
use df_db::AnyPool;
use df_sync::{ApplyReport, DeltaBatch, HqSink};
use uuid::Uuid;

use crate::schemas::{
    CounterSwitchRequest, ErrorResponse, LoginRequest, OverrideVerifyRequest, PasskeyVerifyRequest,
};
use crate::{ApiState, err, not_implemented, pool};

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
