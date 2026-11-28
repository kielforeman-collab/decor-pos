//! HQ authentication, first-run admin setup, manager override, and
//! edge sync ingest handlers.

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use chrono::Duration;
use df_auth::{OverrideCredential, SCOPE_SYNC_INGEST, ServiceToken};
use df_auth_http::{
    AuthSession, ErrorBody, LoginResponse, LogoutResponse, OverrideVerifyResponse, SetupRequest,
    SetupStatusResponse, SyncIdentity, login_error, setup_admin_response, setup_status_response,
};
use df_db::AnyPool;
use df_sync::{ApplyReport, DeltaBatch, HqSink, SyncConflictRow};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::schemas::{
    CounterSwitchRequest, ErrorResponse, LoginRequest, OverrideVerifyRequest, PasskeyVerifyRequest,
};
use crate::{ApiState, err, not_implemented, org_id, pool};

// --- Phase 8 + Security Hardening sync ingest (spec §5.5/§7) ---------------
// `POST /api/v1/sync/ingest`: accepts EITHER:
//   * a staff `Bearer dfp_...` session whose role holds `Permission::SyncIngest`
//     (Manager / Admin), OR
//   * a revocable store service token `Bearer dfs_...` with scope `sync:ingest`
//     (migration 0016), strictly bound to its single `store_id` (403 if the
//     batch names a different store).
//
// `DbHq` applies the batch in one transaction and answers with `ApplyReport`
// (including `conflict_ids` so the edge quarantines conflicts into its
// dead-letter queue — closes F8).
pub async fn sync_ingest(
    State(state): State<ApiState>,
    Extension(actor): Extension<SyncIdentity>,
    Json(batch): Json<DeltaBatch>,
) -> Result<Json<ApplyReport>, (StatusCode, Json<ErrorResponse>)> {
    if let SyncIdentity::ServiceToken { store_id, .. } = actor
        && batch.store_id != store_id
    {
        return Err(err(
            StatusCode::FORBIDDEN,
            format!(
                "service token is scoped to store {store_id}, cannot ingest batch for store {}",
                batch.store_id
            ),
        ));
    }
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

#[derive(Debug, Deserialize)]
pub struct SyncConflictsQuery {
    pub store_id: Option<Uuid>,
    pub include_resolved: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveConflictRequest {
    pub resolution_note: String,
}

#[derive(Debug, Serialize)]
pub struct ResolveConflictResponse {
    pub entry_id: Uuid,
    pub resolved: bool,
}

/// `GET /api/v1/sync/conflicts` — list HQ `sync_conflicts` rows (closes F8).
/// Manager / Admin staff sessions only.
pub async fn sync_conflicts(
    State(state): State<ApiState>,
    Query(query): Query<SyncConflictsQuery>,
) -> Result<Json<Vec<SyncConflictRow>>, (StatusCode, Json<ErrorResponse>)> {
    let pool = pool(&state)?;
    df_sync::DbHq::new(pool)
        .list_conflicts(query.store_id, query.include_resolved.unwrap_or(false))
        .await
        .map(Json)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// `POST /api/v1/sync/conflicts/{entry_id}/resolve` — mark an HQ conflict
/// resolved with an operator note (e.g. reversing transaction reference).
pub async fn resolve_sync_conflict(
    State(state): State<ApiState>,
    Path(entry_id): Path<Uuid>,
    Json(body): Json<ResolveConflictRequest>,
) -> Result<Json<ResolveConflictResponse>, (StatusCode, Json<ErrorResponse>)> {
    let pool = pool(&state)?;
    let resolved = df_sync::DbHq::new(pool)
        .resolve_conflict(entry_id, &body.resolution_note)
        .await
        .map_err(|e| match e {
            df_sync::SyncError::InvalidPayload(msg) => err(StatusCode::BAD_REQUEST, msg),
            other => err(StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
        })?;
    if !resolved {
        return Err(err(
            StatusCode::NOT_FOUND,
            format!("open sync conflict {entry_id} not found"),
        ));
    }
    Ok(Json(ResolveConflictResponse { entry_id, resolved }))
}

// --- Service tokens (Admin `UserManage` only) ------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateServiceTokenRequest {
    pub store_id: Uuid,
    pub label: String,
    /// Optional lifetime in days (`None` = non-expiring until revoked).
    pub ttl_days: Option<i64>,
}

/// Metadata summary for a service token — NEVER includes `token_hash` or
/// raw secret material.
#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceTokenSummary {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub label: String,
    pub scopes: Vec<String>,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub last_used_at: Option<String>,
}

impl ServiceTokenSummary {
    fn from_row(row: ServiceToken) -> Self {
        Self {
            id: row.id,
            org_id: row.org_id,
            store_id: row.store_id,
            label: row.label,
            scopes: row.scopes,
            created_at: row.created_at.to_rfc3339(),
            expires_at: row.expires_at.map(|d| d.to_rfc3339()),
            revoked_at: row.revoked_at.map(|d| d.to_rfc3339()),
            last_used_at: row.last_used_at.map(|d| d.to_rfc3339()),
        }
    }
}

/// Returned exactly once at service-token creation (`201 Created`).
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateServiceTokenResponse {
    pub token: String,
    #[serde(flatten)]
    pub metadata: ServiceTokenSummary,
}

#[derive(Debug, Deserialize)]
pub struct ServiceTokenListQuery {
    pub store_id: Option<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RevokeServiceTokenResponse {
    pub id: Uuid,
    pub revoked: bool,
}

/// `POST /api/v1/auth/service-tokens` — issue a store-scoped `dfs_` sync
/// token (Admin only). The raw token is returned once and never stored.
pub async fn create_service_token(
    State(state): State<ApiState>,
    Json(body): Json<CreateServiceTokenRequest>,
) -> Result<(StatusCode, Json<CreateServiceTokenResponse>), (StatusCode, Json<ErrorResponse>)> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let ttl = match body.ttl_days {
        Some(days) if days <= 0 => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "ttl_days must be a positive integer when provided",
            ));
        }
        Some(days) => Some(Duration::days(days)),
        None => None,
    };
    let (raw, row) = df_db::create_service_token(
        &pool,
        org,
        body.store_id,
        &body.label,
        vec![SCOPE_SYNC_INGEST.to_string()],
        ttl,
    )
    .await
    .map_err(|e| match e {
        df_db::DbError::InvalidData(msg) => err(StatusCode::BAD_REQUEST, msg),
        other => err(StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
    })?;
    Ok((
        StatusCode::CREATED,
        Json(CreateServiceTokenResponse {
            token: raw,
            metadata: ServiceTokenSummary::from_row(row),
        }),
    ))
}

/// `GET /api/v1/auth/service-tokens` — list service tokens for the org
/// (metadata only; hashes never returned).
pub async fn list_service_tokens(
    State(state): State<ApiState>,
    Query(query): Query<ServiceTokenListQuery>,
) -> Result<Json<Vec<ServiceTokenSummary>>, (StatusCode, Json<ErrorResponse>)> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let rows = df_db::list_service_tokens(&pool, org, query.store_id)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        rows.into_iter()
            .map(ServiceTokenSummary::from_row)
            .collect(),
    ))
}

/// `POST /api/v1/auth/service-tokens/{id}/revoke` — revoke a service token.
pub async fn revoke_service_token(
    State(state): State<ApiState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RevokeServiceTokenResponse>, (StatusCode, Json<ErrorResponse>)> {
    let pool = pool(&state)?;
    let revoked = df_db::revoke_service_token(&pool, id)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !revoked {
        return Err(err(
            StatusCode::NOT_FOUND,
            format!("active service token {id} not found"),
        ));
    }
    Ok(Json(RevokeServiceTokenResponse { id, revoked }))
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
