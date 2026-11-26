//! HTTP auth enforcement shared by `df-edge` and `df-api`
//! (Phase 7, v1.1; closes security-review findings F1–F4's HTTP
//! enforcement half — the session/token primitives live in
//! `df-auth` + `df-db::auth`, this crate only enforces them on
//! HTTP).
//!
//! Two pieces:
//!
//! * [`gate`] — an axum middleware (used with
//!   `middleware::from_fn_with_state`) that validates
//!   `Authorization: Bearer dfp_...` via
//!   [`df_db::validate_session_token`], rejects missing / invalid /
//!   expired / revoked tokens with **401 JSON**, enforces the
//!   route group's [`AuthRule`] with **403 JSON** (never 403 for a
//!   missing token), and inserts the validated [`AuthSession`] into
//!   request extensions for handlers that need the actor or the
//!   portal scope. WebSocket upgrades (`/ws/...`) also accept the
//!   token as a `?token=` query parameter, because a browser
//!   `WebSocket` cannot set an `Authorization` header.
//! * Login response DTOs ([`LoginResponse`], [`LogoutResponse`],
//!   [`OverrideVerifyResponse`]) — one shape for the edge and HQ
//!   login endpoints. Ids/timestamps are strings and `role` is the
//!   role name: `Uuid` / `DateTime` do not implement utoipa's
//!   `ToSchema`, and the OpenAPI document must stay free of data
//!   rows (it lists shapes only).
//!
//! [`test_support`] seeds staff + contractor users and mints real
//! tokens for the HTTP integration tests. Test code only.
//!
//! No COLORx code lives here or anywhere in Phase 7 (spec §9: the
//! Benjamin Moore COLORx integration is deferred post-v1.0).

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, body::Body};
use chrono::{DateTime, Utc};
use df_auth::{AuthError, Permission, Role, Session};
use df_db::AnyPool;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub mod test_support;

/// The authenticated identity a request carries. Built by
/// [`gate`] from the validated [`Session`] and placed in request
/// extensions; handlers read it with
/// `Extension<AuthSession>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub role: Role,
    /// Contractor customer this session may access. `Some` only
    /// for `PortalContractor` sessions; `None` for staff.
    pub customer_id: Option<Uuid>,
}

impl AuthSession {
    pub fn from_session(session: &Session) -> Self {
        Self {
            user_id: session.user_id,
            session_id: session.id,
            role: session.role,
            customer_id: session.customer_id,
        }
    }

    pub fn has_permission(&self, permission: Permission) -> bool {
        self.role.has_permission(permission)
    }

    pub fn allows_any(&self, permissions: &[Permission]) -> bool {
        permissions
            .iter()
            .any(|permission| self.has_permission(*permission))
    }

    /// Portal scope (F4): a portal session may access exactly the
    /// contractor customer it was issued for — never another
    /// contractor's id, and never a staff session.
    pub fn may_access_customer(&self, customer_id: Uuid) -> bool {
        self.role == Role::PortalContractor && self.customer_id == Some(customer_id)
    }
}

/// What a gated route group requires, beyond a valid session.
#[derive(Debug, Clone)]
pub enum AuthRule {
    /// Any live session, any role (e.g. scheduling + timeclock:
    /// there is no scheduling permission in `df-auth` yet, so any
    /// authenticated staff member may use these routes — see
    /// `docs/auth.md`).
    Authenticated,
    /// A live session whose role holds at least one of these
    /// permissions (any-of, not all-of).
    AnyOf(Vec<Permission>),
    /// A live `PortalContractor` session. Handlers additionally
    /// check [`AuthSession::may_access_customer`] against the
    /// requested customer id — the gate alone cannot know which
    /// customer a route is about.
    Portal,
}

/// Axum state for [`gate`] (via `middleware::from_fn_with_state`):
/// the pool tokens validate against, plus the rule for the route
/// group. `pool` is `None` on an HQ instance started without
/// `--db` — protected routes then answer 503, like the admin
/// handlers did before Phase 7.
#[derive(Debug, Clone)]
pub struct Gate {
    pub pool: Option<AnyPool>,
    pub rule: AuthRule,
}

impl Gate {
    pub fn new(pool: Option<AnyPool>, rule: AuthRule) -> Self {
        Self { pool, rule }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
}

fn error_json(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
        .into_response()
}

fn unauthenticated() -> Response {
    error_json(
        StatusCode::UNAUTHORIZED,
        "invalid or expired session — log in again",
    )
}

/// `Authorization: Bearer <token>`, strict: anything else is "no
/// token", never a best-effort parse.
fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    value
        .strip_prefix("Bearer ")
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

/// `?token=` fallback, for WebSocket upgrades only
/// (`/ws/...`): a browser `WebSocket` cannot set an
/// `Authorization` header. Tokens are base64url, so no
/// percent-decoding is needed. Deliberately not offered on other
/// routes — tokens in URLs end up in logs.
fn ws_query_token(req: &Request<Body>) -> Option<String> {
    if !req.uri().path().starts_with("/ws/") {
        return None;
    }
    let query = req.uri().query()?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key == "token" && !value.is_empty() {
            Some(value.to_string())
        } else {
            None
        }
    })
}

/// Validate the bearer token and enforce the route group's rule.
///
/// * No pool → 503 (HQ started without `--db`).
/// * No token, or a token `df_db::validate_session_token`
///   rejects (unknown / expired / revoked / inactive user) →
///   **401**. The three token failures are deliberately one
///   response: callers must not distinguish them.
/// * Valid token, wrong role → **403**.
/// * Otherwise the [`AuthSession`] is inserted into request
///   extensions and the request proceeds.
pub async fn gate(State(gate): State<Gate>, mut req: Request<Body>, next: Next) -> Response {
    let Some(pool) = &gate.pool else {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "no database configured for this API instance — restart with --db <url> --org-id <uuid>",
        );
    };
    let token = bearer_token(req.headers()).or_else(|| ws_query_token(&req));
    let Some(token) = token else {
        return unauthenticated();
    };
    let session = match df_db::validate_session_token(pool, &token).await {
        Ok(session) => session,
        Err(AuthError::InvalidSession | AuthError::AccountInactive) => {
            return unauthenticated();
        }
        Err(err) => {
            // A database failure validating the session: 500, and
            // never the DB error text (it can name tables).
            let _ = err;
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "session validation failed",
            );
        }
    };
    let auth = AuthSession::from_session(&session);
    let allowed = match &gate.rule {
        AuthRule::Authenticated => true,
        AuthRule::AnyOf(permissions) => auth.allows_any(permissions),
        AuthRule::Portal => auth.role == Role::PortalContractor,
    };
    if !allowed {
        return error_json(
            StatusCode::FORBIDDEN,
            format!("role {:?} may not use this route", auth.role),
        );
    }
    req.extensions_mut().insert(auth);
    next.run(req).await
}

// --- login DTOs -----------------------------------------------------------

/// A successful login: the presented token (returned exactly once
/// — the client stores it as a bearer credential) plus the identity
/// the session carries. Built from [`df_db::LoginOutcome`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LoginResponse {
    pub token: String,
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub customer_id: Option<String>,
    pub expires_at: String,
}

impl LoginResponse {
    pub fn from_outcome(outcome: &df_db::LoginOutcome) -> Self {
        Self {
            token: outcome.token.clone(),
            user_id: outcome.user.id.to_string(),
            email: outcome.user.email.clone(),
            display_name: outcome.user.display_name.clone(),
            role: outcome.session.role.as_str().to_string(),
            customer_id: outcome.session.customer_id.map(|id| id.to_string()),
            expires_at: outcome.session.expires_at.to_rfc3339(),
        }
    }
}

/// `POST /auth/logout`: the presented session was revoked.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LogoutResponse {
    pub revoked: bool,
}

/// A successful manager-override verification: audit fields only —
/// no credential material is ever carried back.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OverrideVerifyResponse {
    pub approved: bool,
    pub approver_user_id: String,
    pub approver_role: String,
    pub method: String,
    pub approved_at: String,
}

impl OverrideVerifyResponse {
    pub fn from_approval(approval: &df_auth::OverrideApproval) -> Self {
        Self {
            approved: true,
            approver_user_id: approval.approver_user_id.to_string(),
            approver_role: approval.approver_role.as_str().to_string(),
            method: match approval.method {
                df_auth::OverrideMethod::Password => "password".to_string(),
                df_auth::OverrideMethod::Pin => "pin".to_string(),
            },
            approved_at: approval.approved_at.to_rfc3339(),
        }
    }
}

/// Map a login-service [`AuthError`] to an HTTP status + message.
/// Login failures are a generic 401 (the boundary must not reveal
/// which emails exist or which accounts are active); a portal user
/// with no linked customer is a 403 config error, a non-manager
/// approver is a 403, and a database failure is a 500 without the
/// DB text.
pub fn login_error(err: AuthError) -> (StatusCode, Json<ErrorBody>) {
    match err {
        AuthError::InvalidCredentials | AuthError::AccountInactive => (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "invalid credentials".to_string(),
            }),
        ),
        AuthError::PortalNotLinked => (
            StatusCode::FORBIDDEN,
            Json(ErrorBody {
                error: "portal user has no linked customer".to_string(),
            }),
        ),
        AuthError::PermissionDenied(message) => {
            (StatusCode::FORBIDDEN, Json(ErrorBody { error: message }))
        }
        AuthError::InvalidSession => (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "invalid or expired session".to_string(),
            }),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: "login failed".to_string(),
            }),
        ),
    }
}

/// Map a first-run setup [`df_db::SetupError`] to an HTTP status
/// and message. Already-setup is **409** (and creates nothing),
/// bad input is **400**, and a database failure is a **500**
/// without the DB text (it can name tables).
pub fn setup_error(err: df_db::SetupError) -> (StatusCode, Json<ErrorBody>) {
    match err {
        df_db::SetupError::AlreadySetup => (
            StatusCode::CONFLICT,
            Json(ErrorBody {
                error: "setup already complete — a user already exists".to_string(),
            }),
        ),
        df_db::SetupError::InvalidInput(message) => {
            (StatusCode::BAD_REQUEST, Json(ErrorBody { error: message }))
        }
        df_db::SetupError::Database(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: "setup failed".to_string(),
            }),
        ),
    }
}

/// `GET /auth/setup-status`: whether first-run setup is still
/// needed — exactly when the users table is empty. Shared by the
/// edge (`GET /pos/auth/setup-status`) and HQ (`GET
/// /api/v1/auth/setup-status`) handlers; the caller only supplies
/// the pool.
pub async fn setup_status_response(
    pool: &AnyPool,
) -> Result<Json<SetupStatusResponse>, (StatusCode, Json<ErrorBody>)> {
    match df_db::count_users(pool).await {
        Ok(count) => Ok(Json(SetupStatusResponse {
            needs_setup: count == 0,
        })),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorBody {
                error: "setup status check failed".to_string(),
            }),
        )),
    }
}

/// `POST /auth/setup`: create the first Admin and return a
/// [`LoginResponse`] for them, so the GUI logs the new admin
/// straight in without a second call. Shared by the edge (`POST
/// /pos/auth/setup`) and HQ (`POST /api/v1/auth/setup`) handlers;
/// the caller only supplies the pool and its configured org id.
/// Success is **201**; a second setup is **409** and creates
/// nothing (see [`df_db::setup_first_admin`]).
pub async fn setup_admin_response(
    pool: &AnyPool,
    org_id: Uuid,
    body: SetupRequest,
) -> Result<(StatusCode, Json<LoginResponse>), (StatusCode, Json<ErrorBody>)> {
    df_db::setup_first_admin(
        pool,
        org_id,
        &body.email,
        body.display_name.as_deref(),
        &body.password,
    )
    .await
    .map(|outcome| {
        (
            StatusCode::CREATED,
            Json(LoginResponse::from_outcome(&outcome)),
        )
    })
    .map_err(setup_error)
}

/// `POST /auth/setup` request: the first admin's credentials.
/// There is deliberately **no role field** — the created user is
/// always Admin — and unknown fields are rejected, so a `role`
/// parameter cannot slip through silently. `display_name` is
/// optional: when missing or blank it defaults to the email local
/// part (see [`df_db::setup_first_admin`]).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupRequest {
    pub email: String,
    pub display_name: Option<String>,
    pub password: String,
}

/// `GET /auth/setup-status` response: `needs_setup` is true
/// exactly when the users table is empty.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetupStatusResponse {
    pub needs_setup: bool,
}

/// The current UTC time, for `expires_at` comparisons in tests.
/// (A one-line re-export so callers do not need a chrono import.)
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}
