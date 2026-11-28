//! Edge authentication, first-run admin setup, and manager override
//! handlers (Phase 7 / v1.2.1).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use df_auth::OverrideCredential;
use df_auth_http::{
    AuthSession, LoginResponse, LogoutResponse, OverrideVerifyResponse, SetupRequest,
    SetupStatusResponse, login_error, setup_admin_response, setup_status_response,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{HandlerResult, err_response};
use crate::edge::Edge;

// --- auth endpoints (Phase 7, v1.1) ---------------------------------------
// Public by design: these are how a session is obtained. Login
// failures are a generic 401 (see `df_auth_http::login_error`) —
// the boundary must not reveal which emails exist or which
// accounts are active.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeLoginRequest {
    pub email: String,
    pub password: String,
    /// Portal remember-me (30-day session) — only meaningful for a
    /// `PortalContractor`; staff sessions are always 8 hours.
    pub remember_me: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeCounterSwitchRequest {
    /// Identifies the user: an Argon2id hash cannot be searched,
    /// so a bare PIN/badge cannot name its user (see
    /// `df_auth::login` docs).
    pub email: String,
    pub pin: Option<String>,
    pub badge_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeOverrideVerifyRequest {
    pub user_id: String,
    pub pin: String,
    pub reason_code: String,
}

type AuthResult<T> = Result<Json<T>, (StatusCode, Json<df_auth_http::ErrorBody>)>;

pub(super) async fn edge_login(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<EdgeLoginRequest>,
) -> AuthResult<LoginResponse> {
    df_db::login_with_password(
        edge.pool(),
        &body.email,
        &body.password,
        body.remember_me.unwrap_or(false),
    )
    .await
    .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
    .map_err(login_error)
}

/// First-run setup status: `{"needs_setup": true}` exactly while
/// the users table is empty. Public — the GUI asks this before
/// any user exists, so there is no session to gate on.
pub(super) async fn edge_setup_status(
    State(edge): State<Arc<Edge>>,
) -> AuthResult<SetupStatusResponse> {
    setup_status_response(edge.pool()).await
}

/// First-run setup: create the first Admin and return a
/// [`LoginResponse`] for them (201), so the GUI logs the new admin
/// straight in. The admin belongs to the edge's configured org
/// (`EdgeConfig.org_id`) — no org is invented and none is taken
/// from the request. A second setup is 409 and creates nothing.
/// Public — like login, this is how the first session is obtained.
pub(super) async fn edge_setup(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<SetupRequest>,
) -> Result<(StatusCode, Json<LoginResponse>), (StatusCode, Json<df_auth_http::ErrorBody>)> {
    setup_admin_response(edge.pool(), edge.config().org_id, body).await
}

pub(super) async fn edge_counter_switch(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<EdgeCounterSwitchRequest>,
) -> AuthResult<LoginResponse> {
    if let Some(pin) = &body.pin {
        return df_db::login_with_pin(edge.pool(), &body.email, pin)
            .await
            .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
            .map_err(login_error);
    }
    if let Some(badge) = &body.badge_token {
        return df_db::login_with_badge(edge.pool(), &body.email, badge)
            .await
            .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
            .map_err(login_error);
    }
    Err((
        StatusCode::BAD_REQUEST,
        Json(df_auth_http::ErrorBody {
            error: "counter-switch needs a pin or a badge_token".to_string(),
        }),
    ))
}

pub(super) async fn edge_portal_login(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<EdgeLoginRequest>,
) -> AuthResult<LoginResponse> {
    df_db::login_portal(
        edge.pool(),
        &body.email,
        &body.password,
        body.remember_me.unwrap_or(false),
    )
    .await
    .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
    .map_err(login_error)
}

pub(super) async fn edge_logout(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
) -> HandlerResult<LogoutResponse> {
    match df_db::revoke_session(edge.pool(), auth.session_id).await {
        Ok(()) => Ok(Json(LogoutResponse { revoked: true })),
        Err(err) => Err(err_response(err.into())),
    }
}

pub(super) async fn edge_override_verify(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<EdgeOverrideVerifyRequest>,
) -> AuthResult<OverrideVerifyResponse> {
    if body.reason_code.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(df_auth_http::ErrorBody {
                error: "reason_code is required".to_string(),
            }),
        ));
    }
    let user_id = Uuid::parse_str(&body.user_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(df_auth_http::ErrorBody {
                error: format!("user_id is not a UUID: {:?}", body.user_id),
            }),
        )
    })?;
    // Unknown approver id is a generic 401, like an unknown email.
    let user = df_db::get_user_by_id(edge.pool(), user_id)
        .await
        .map_err(|_| login_error(df_auth::AuthError::InvalidCredentials))?;
    df_db::verify_manager_override(edge.pool(), &user.email, &OverrideCredential::Pin(body.pin))
        .await
        .map(|approval| Json(OverrideVerifyResponse::from_approval(&approval)))
        .map_err(login_error)
}
