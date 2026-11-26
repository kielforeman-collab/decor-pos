//! User + session repositories and DB-backed login services
//! (Phase 7, v1.1).
//!
//! Two layers in one module, kept visibly separate:
//!
//! * **Repositories** — plain sqlx over [`sqlx::AnyPool`]
//!   (`?` placeholders, ids/timestamps as text — see `repos.rs`):
//!   `get_user_by_id`, `create_user`, `set_password_hash`,
//!   `set_pin_hash`, `set_badge_hash`, `insert_session`,
//!   `get_session_by_token_hash`, `revoke_session`,
//!   `revoke_sessions_for_user`, `list_sessions_for_user`.
//!   (`get_user_by_email` already exists in [`crate::repos`].)
//! * **Login services** — one async function per login kind, each
//!   doing load → pure verify (in `df_auth::login`) → issue session
//!   → persist it: `login_with_password`, `login_with_pin`,
//!   `login_with_badge`, `login_portal`, `verify_manager_override`,
//!   plus `validate_session_token` (the per-request check the HTTP
//!   workers call) and `create_session` (issue + persist with an
//!   explicit TTL, for logins and tests).
//!
//! Error discipline: login failures are [`df_auth::AuthError`]
//! (generic "invalid credentials" — see `df_auth::login`); database
//! failures surface as `AuthError::Database` via the `From<DbError>`
//! impl below, so a service signature has one error type. The
//! repository functions themselves return [`DbError`] as usual.
//!
//! No COLORx code lives here or anywhere in Phase 7 (spec §9: the
//! Benjamin Moore COLORx integration is deferred post-v1.0).

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use df_auth::login::{
    verify_badge_login, verify_manager_override as verify_override_pure, verify_password_login,
    verify_pin_login, verify_portal_login,
};
use df_auth::{
    AuthError, AuthUser, OverrideApproval, OverrideCredential, Role, Session, hash_password,
    new_session, role_from_id, role_id, session_ttl,
    validate_session_token as validate_session_pure,
};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Executor, Row};
use uuid::Uuid;

use crate::error::DbError;
use crate::models::{Organization, UserRow};

impl From<DbError> for AuthError {
    fn from(err: DbError) -> Self {
        AuthError::Database(err.to_string())
    }
}

// --- row helpers (same text-crossing conventions as `repos.rs`) --------

fn uuid_str(id: Uuid) -> String {
    id.to_string()
}

fn opt_uuid_str(id: Option<Uuid>) -> Option<String> {
    id.map(|i| i.to_string())
}

fn dt_str(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn text_col(row: &AnyRow, name: &str) -> Result<String, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

fn opt_text_col(row: &AnyRow, name: &str) -> Result<Option<String>, DbError> {
    row.try_get(name)
        .map_err(|e| DbError::InvalidData(format!("column {name}: {e}")))
}

fn uuid_col(row: &AnyRow, name: &str) -> Result<Uuid, DbError> {
    Uuid::parse_str(&text_col(row, name)?)
        .map_err(|e| DbError::InvalidData(format!("column {name} is not a UUID: {e}")))
}

fn opt_uuid_col(row: &AnyRow, name: &str) -> Result<Option<Uuid>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => Uuid::parse_str(&text)
            .map(Some)
            .map_err(|e| DbError::InvalidData(format!("column {name} is not a UUID: {e}"))),
    }
}

fn dt_col(row: &AnyRow, name: &str) -> Result<DateTime<Utc>, DbError> {
    let text = text_col(row, name)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| DbError::InvalidData(format!("column {name} is not RFC 3339: {e}")))
}

fn opt_dt_col(row: &AnyRow, name: &str) -> Result<Option<DateTime<Utc>>, DbError> {
    match opt_text_col(row, name)? {
        None => Ok(None),
        Some(text) => DateTime::parse_from_rfc3339(&text)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|e| DbError::InvalidData(format!("column {name} is not RFC 3339: {e}"))),
    }
}

fn not_found(what: &str) -> impl Fn(sqlx::Error) -> DbError + '_ {
    move |e| match e {
        sqlx::Error::RowNotFound => DbError::NotFound(what.to_string()),
        other => DbError::Sqlx(other),
    }
}

/// Convert a stored user row to the [`AuthUser`] view the pure
/// login checks take. An unmapped `role_id` (hand-inserted before
/// Phase 7 seeded `roles`) is invalid data — never a default role.
pub fn user_to_auth_user(row: &UserRow) -> Result<AuthUser, DbError> {
    let role = role_from_id(row.role_id).ok_or_else(|| {
        DbError::InvalidData(format!(
            "user {} has unknown role_id {} (no seeded role owns it)",
            row.id, row.role_id
        ))
    })?;
    Ok(AuthUser {
        id: row.id,
        org_id: row.org_id,
        email: row.email.clone(),
        password_hash: row.password_hash.clone(),
        counter_pin_hash: row.counter_pin_hash.clone(),
        badge_token_hash: row.badge_token_hash.clone(),
        role,
        active: row.active,
        portal_customer_id: row.portal_customer_id,
    })
}

fn session_from_row(row: &AnyRow) -> Result<Session, DbError> {
    let role = Role::from_str(&text_col(row, "role")?)
        .map_err(|e| DbError::InvalidData(format!("session role: {e}")))?;
    Ok(Session {
        id: uuid_col(row, "id")?,
        user_id: uuid_col(row, "user_id")?,
        token_hash: text_col(row, "token_hash")?,
        role,
        customer_id: opt_uuid_col(row, "customer_id")?,
        created_at: dt_col(row, "created_at")?,
        expires_at: dt_col(row, "expires_at")?,
        revoked_at: opt_dt_col(row, "revoked_at")?,
        last_used_at: dt_col(row, "last_used_at")?,
    })
}

// --- users ---------------------------------------------------------------

/// Fields for creating a user. The id is minted here (UUIDv7);
/// credential fields already hold Argon2id hashes (see
/// `df_auth::{hash_password, hash_pin, hash_badge_token}`) — never
/// plaintext.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub org_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub password_hash: Option<String>,
    pub counter_pin_hash: Option<String>,
    pub badge_token_hash: Option<String>,
    pub role: Role,
    pub portal_customer_id: Option<Uuid>,
    pub active: bool,
}

/// Create a user. The canonical constructor — prefer this over the
/// raw [`crate::insert_user`] (which takes a full `UserRow` and
/// stays for compatibility with Phase 0 callers).
pub async fn create_user(pool: &AnyPool, new: &NewUser) -> Result<UserRow, DbError> {
    let user = UserRow {
        id: df_domain::new_id(),
        org_id: new.org_id,
        email: new.email.clone(),
        display_name: new.display_name.clone(),
        password_hash: new.password_hash.clone(),
        oidc_subject: None,
        counter_pin_hash: new.counter_pin_hash.clone(),
        badge_token_hash: new.badge_token_hash.clone(),
        role_id: role_id(new.role),
        portal_customer_id: new.portal_customer_id,
        active: new.active,
    };
    crate::insert_user(pool, &user).await?;
    Ok(user)
}

pub async fn get_user_by_id(pool: &AnyPool, id: Uuid) -> Result<UserRow, DbError> {
    let row = sqlx::query("SELECT * FROM users WHERE id = $1")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("user {id}")))?;
    crate::repos::user_from_row(&row)
}

/// Every user of an org, ordered by email — the raw material for
/// `df-server users list` and admin screens.
pub async fn list_users_for_org(pool: &AnyPool, org_id: Uuid) -> Result<Vec<UserRow>, DbError> {
    let rows = sqlx::query("SELECT * FROM users WHERE org_id = $1 ORDER BY email ASC")
        .bind(uuid_str(org_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(crate::repos::user_from_row).collect()
}

/// How many users exist in total, across all orgs. The first-run
/// setup gate reads this: setup is needed exactly when the table
/// is empty (not per-org — a fresh install has no users at all).
pub async fn count_users(pool: &AnyPool) -> Result<i64, DbError> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM users")
        .fetch_one(pool)
        .await?;
    row.try_get("n")
        .map_err(|e| DbError::InvalidData(format!("column n: {e}")))
}

/// Failure of a first-run setup attempt ([`setup_first_admin`]).
/// The HTTP layer maps these 1:1 — `AlreadySetup` → 409,
/// `InvalidInput` → 400, `Database` → 500 without the DB text.
#[derive(Debug)]
pub enum SetupError {
    /// A user already exists: setup ran before. Nothing was
    /// created by this attempt.
    AlreadySetup,
    /// Blank email / password, or a password
    /// [`df_auth::hash_password`] rejects (under 8 characters).
    InvalidInput(String),
    /// A database failure (including a unique-violation that is
    /// not the already-setup race — see `is_unique_violation`).
    Database(DbError),
}

impl From<DbError> for SetupError {
    fn from(err: DbError) -> Self {
        SetupError::Database(err)
    }
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupError::AlreadySetup => {
                write!(f, "setup already complete — a user already exists")
            }
            SetupError::InvalidInput(message) => write!(f, "{message}"),
            SetupError::Database(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SetupError {}

/// Whether a [`DbError`] is a unique-constraint violation
/// (`users.email` is `UNIQUE` on both dialects). Used to tell the
/// same-email setup race apart from other database failures: two
/// concurrent setups with the same email cannot both insert.
fn is_unique_violation(err: &DbError) -> bool {
    match err {
        DbError::Sqlx(sqlx::Error::Database(db_err)) => {
            let message = db_err.message();
            message.contains("UNIQUE constraint failed") || message.contains("duplicate key")
        }
        _ => false,
    }
}

/// Count the users visible inside a transaction (same statement as
/// [`count_users`], over the transaction's executor).
async fn count_users_exec<'e, E>(executor: E) -> Result<i64, DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    let row = sqlx::query("SELECT COUNT(*) AS n FROM users")
        .fetch_one(executor)
        .await?;
    row.try_get("n")
        .map_err(|e| DbError::InvalidData(format!("column n: {e}")))
}

/// First-run setup: create the first Admin user and log them
/// straight in (GUI first-run setup, spec §6.1 auth).
///
/// * The created user is **always** [`Role::Admin`] — there is no
///   role parameter anywhere in this path.
/// * `display_name` defaults to the email local part (before
///   `@`; the whole email when there is none) when missing or
///   blank.
/// * The password is hashed with Argon2id ([`hash_password`])
///   before any database write; plaintext is never stored or
///   logged.
/// * `org_id` is the caller's configured org (the edge's
///   `EdgeConfig.org_id`, the HQ's `ApiState.org_id`). The org
///   row is created inside the same transaction when it does not
///   exist yet (same defaults as `df-server users create-admin`'s
///   `ensure_org`: name `"Decor POS"`, currency `"USD"`) — a
///   fresh install has no org row, and `users.org_id` is a
///   foreign key on PostgreSQL.
///
/// # Race safety
///
/// The empty-check and the inserts run in **one transaction**:
/// count → (already-setup → roll back) → ensure org → insert
/// user → re-count → (more than one user → roll back) → insert
/// session → commit. The second count closes the window where a
/// concurrent setup committed between this attempt's first count
/// and its insert; the `UNIQUE` constraint on `users.email` is
/// the backstop for two setups racing with the same email (the
/// loser's insert fails and maps to `AlreadySetup`). On SQLite
/// the single-writer lock serializes concurrent setup attempts
/// outright, so the second attempt's first count already sees
/// the committed user. Residual limit: on PostgreSQL, two
/// concurrent setups with *different* emails could both commit
/// if the second commits in the sliver between its second count
/// and its commit — closing that needs a table lock or
/// serializable isolation, out of scope for a one-time operator
/// action (see the report).
pub async fn setup_first_admin(
    pool: &AnyPool,
    org_id: Uuid,
    email: &str,
    display_name: Option<&str>,
    password: &str,
) -> Result<LoginOutcome, SetupError> {
    let email = email.trim();
    if email.is_empty() {
        return Err(SetupError::InvalidInput("email is required".to_string()));
    }
    if password.trim().is_empty() {
        return Err(SetupError::InvalidInput("password is required".to_string()));
    }
    let display_name = match display_name.map(str::trim) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => email
            .split_once('@')
            .map(|(local, _)| local.to_string())
            .unwrap_or_else(|| email.to_string()),
    };
    // Hash before opening the transaction: Argon2id is slow, and
    // no database lock should be held while it runs.
    let password_hash =
        hash_password(password).map_err(|err| SetupError::InvalidInput(err.to_string()))?;

    let mut tx = pool.begin().await.map_err(DbError::Sqlx)?;
    if count_users_exec(&mut *tx).await? > 0 {
        return Err(SetupError::AlreadySetup);
    }
    // The org row may not exist on a fresh install (neither
    // `df-server edge` nor `df-server hq` creates it); the FK on
    // `users.org_id` requires it on PostgreSQL.
    let org_exists = sqlx::query("SELECT id FROM organizations WHERE id = $1")
        .bind(uuid_str(org_id))
        .fetch_optional(&mut *tx)
        .await
        .map_err(DbError::Sqlx)?;
    if org_exists.is_none() {
        crate::insert_organization_exec(
            &mut *tx,
            &Organization {
                id: org_id,
                name: "Decor POS".to_string(),
                default_currency: "USD".to_string(),
                enabled_modules: serde_json::json!({}),
                settings_json: serde_json::json!({}),
                created_at: Utc::now(),
            },
        )
        .await?;
    }
    let user = UserRow {
        id: df_domain::new_id(),
        org_id,
        email: email.to_string(),
        display_name,
        password_hash: Some(password_hash),
        oidc_subject: None,
        counter_pin_hash: None,
        badge_token_hash: None,
        role_id: role_id(Role::Admin),
        portal_customer_id: None,
        active: true,
    };
    if let Err(err) = crate::insert_user_exec(&mut *tx, &user).await {
        if is_unique_violation(&err) {
            return Err(SetupError::AlreadySetup);
        }
        return Err(SetupError::Database(err));
    }
    // Re-count after the insert: a concurrent setup that
    // committed between our first count and this insert shows up
    // here, and we roll back instead of leaving two admins.
    if count_users_exec(&mut *tx).await? > 1 {
        return Err(SetupError::AlreadySetup);
    }
    let now = Utc::now();
    let (token, session) = new_session(
        user.id,
        Role::Admin,
        None,
        session_ttl(Role::Admin, false),
        now,
    );
    insert_session_exec(&mut *tx, &session).await?;
    tx.commit().await.map_err(DbError::Sqlx)?;
    Ok(LoginOutcome {
        user,
        session,
        token,
    })
}
pub async fn count_users_with_role(
    pool: &AnyPool,
    org_id: Uuid,
    role: Role,
) -> Result<i64, DbError> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM users WHERE org_id = $1 AND role_id = $2")
        .bind(uuid_str(org_id))
        .bind(uuid_str(role_id(role)))
        .fetch_one(pool)
        .await?;
    row.try_get("n")
        .map_err(|e| DbError::InvalidData(format!("column n: {e}")))
}

/// Replace a user's password hash (`None` clears it — an OIDC-only
/// user). The caller hashes first; this never sees plaintext.
pub async fn set_password_hash(
    pool: &AnyPool,
    user_id: Uuid,
    password_hash: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(password_hash)
        .bind(uuid_str(user_id))
        .execute(pool)
        .await?;
    Ok(())
}

/// Replace a user's counter PIN hash (`None` clears it).
pub async fn set_pin_hash(
    pool: &AnyPool,
    user_id: Uuid,
    pin_hash: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET counter_pin_hash = $1 WHERE id = $2")
        .bind(pin_hash)
        .bind(uuid_str(user_id))
        .execute(pool)
        .await?;
    Ok(())
}

/// Replace a user's badge token hash (`None` clears it).
pub async fn set_badge_hash(
    pool: &AnyPool,
    user_id: Uuid,
    badge_hash: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET badge_token_hash = $1 WHERE id = $2")
        .bind(badge_hash)
        .bind(uuid_str(user_id))
        .execute(pool)
        .await?;
    Ok(())
}

/// Link (or unlink, with `None`) a user to a contractor customer.
/// Only meaningful for `PortalContractor` users; the caller
/// enforces that.
pub async fn set_portal_customer(
    pool: &AnyPool,
    user_id: Uuid,
    customer_id: Option<Uuid>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET portal_customer_id = $1 WHERE id = $2")
        .bind(opt_uuid_str(customer_id))
        .bind(uuid_str(user_id))
        .execute(pool)
        .await?;
    Ok(())
}

/// Activate / deactivate a user. Deactivation takes effect on the
/// next login and on the next session validation — existing
/// sessions are rejected by `validate_session_token` once the flag
/// flips, so no separate session sweep is needed (revoke as well
/// when the operator wants the sessions gone from the list).
pub async fn set_user_active(pool: &AnyPool, user_id: Uuid, active: bool) -> Result<(), DbError> {
    sqlx::query("UPDATE users SET active = $1 WHERE id = $2")
        .bind(active)
        .bind(uuid_str(user_id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- sessions --------------------------------------------------------------

/// Persist a session row over any executor, so a caller holding
/// a transaction (e.g. first-run setup) can join the insert to
/// it. [`insert_session`] is the pool shorthand.
pub async fn insert_session_exec<'e, E>(executor: E, session: &Session) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO sessions (id, user_id, token_hash, role, customer_id, created_at, expires_at, revoked_at, last_used_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(uuid_str(session.id))
    .bind(uuid_str(session.user_id))
    .bind(&session.token_hash)
    .bind(session.role.as_str())
    .bind(opt_uuid_str(session.customer_id))
    .bind(dt_str(session.created_at))
    .bind(dt_str(session.expires_at))
    .bind(session.revoked_at.map(dt_str))
    .bind(dt_str(session.last_used_at))
    .execute(executor)
    .await?;
    Ok(())
}

/// Persist a session row issued by [`df_auth::new_session`] (or by
/// [`create_session` below). Stores the token hash only.
pub async fn insert_session(pool: &AnyPool, session: &Session) -> Result<(), DbError> {
    insert_session_exec(pool, session).await
}

/// Load a session by the SHA-256 hex of its token. Callers hash the
/// presented token first (`df_auth::hash_token`) — the raw token
/// never reaches SQL.
pub async fn get_session_by_token_hash(
    pool: &AnyPool,
    token_hash: &str,
) -> Result<Session, DbError> {
    let row = sqlx::query("SELECT * FROM sessions WHERE token_hash = $1")
        .bind(token_hash)
        .fetch_one(pool)
        .await
        .map_err(not_found("session for token hash"))?;
    session_from_row(&row)
}

/// Revoke one session by id (sets `revoked_at` to now). Revoked
/// sessions stay in the table as an audit trail.
pub async fn revoke_session(pool: &AnyPool, session_id: Uuid) -> Result<(), DbError> {
    sqlx::query("UPDATE sessions SET revoked_at = $1 WHERE id = $2")
        .bind(dt_str(Utc::now()))
        .bind(uuid_str(session_id))
        .execute(pool)
        .await?;
    Ok(())
}

/// Revoke one session by the SHA-256 hex of its token.
pub async fn revoke_session_by_token_hash(pool: &AnyPool, token_hash: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE sessions SET revoked_at = $1 WHERE token_hash = $2")
        .bind(dt_str(Utc::now()))
        .bind(token_hash)
        .execute(pool)
        .await?;
    Ok(())
}

/// Revoke every live session of a user (logout-everywhere, role
/// change, deactivation sweep). Returns the number of rows revoked.
/// Already-revoked sessions are left untouched.
pub async fn revoke_sessions_for_user(pool: &AnyPool, user_id: Uuid) -> Result<u64, DbError> {
    let result = sqlx::query(
        "UPDATE sessions SET revoked_at = $1 WHERE user_id = $2 AND revoked_at IS NULL",
    )
    .bind(dt_str(Utc::now()))
    .bind(uuid_str(user_id))
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Every session of a user, newest first — live and revoked alike
/// (the caller filters on `is_valid` / `revoked_at`).
pub async fn list_sessions_for_user(
    pool: &AnyPool,
    user_id: Uuid,
) -> Result<Vec<Session>, DbError> {
    let rows = sqlx::query("SELECT * FROM sessions WHERE user_id = $1 ORDER BY created_at DESC")
        .bind(uuid_str(user_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(session_from_row).collect()
}

/// Refresh a session's `last_used_at` after a successful validation.
async fn touch_session(
    pool: &AnyPool,
    session_id: Uuid,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE sessions SET last_used_at = $1 WHERE id = $2")
        .bind(dt_str(now))
        .bind(uuid_str(session_id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- login services ----------------------------------------------------------

/// A successful login: the user row, the persisted session row, and
/// the presented token — returned to the caller exactly once. The
/// token is **not** stored anywhere; log it and the user cannot log
/// in with it again after it expires, but anyone holding the log
/// holds the session until then. Hand it to the HTTP layer, which
/// puts it in an `Authorization: Bearer` response / secure cookie
/// and never writes it to a log line.
#[derive(Debug, Clone)]
pub struct LoginOutcome {
    pub user: UserRow,
    pub session: Session,
    pub token: String,
}

/// Issue + persist a session for a user with an explicit TTL, and
/// return `(token, session)`. Logins use this through
/// `issue_login_session` below; tests use it directly to mint
/// short-lived or already-expired sessions.
pub async fn create_session(
    pool: &AnyPool,
    user: &UserRow,
    ttl: Duration,
    customer_id: Option<Uuid>,
) -> Result<(String, Session), AuthError> {
    let auth_user = user_to_auth_user(user)?;
    let now = Utc::now();
    let (token, session) = new_session(user.id, auth_user.role, customer_id, ttl, now);
    insert_session(pool, &session).await?;
    Ok((token, session))
}

/// Shared tail of every login: TTL from role + remember-me, portal
/// scope from the user's linked customer, persist, return.
async fn issue_login_session(
    pool: &AnyPool,
    user: &UserRow,
    remember_me: bool,
) -> Result<LoginOutcome, AuthError> {
    let auth_user = user_to_auth_user(user)?;
    let ttl = session_ttl(auth_user.role, remember_me);
    let customer_id = if auth_user.role == Role::PortalContractor {
        auth_user.portal_customer_id
    } else {
        None
    };
    let (token, session) = create_session(pool, user, ttl, customer_id).await?;
    Ok(LoginOutcome {
        user: user.clone(),
        session,
        token,
    })
}

/// Load a user by email for a login attempt. A missing user is
/// `InvalidCredentials` — never `NotFound` — so the login boundary
/// cannot be used to enumerate registered emails.
async fn load_user_for_login(pool: &AnyPool, email: &str) -> Result<UserRow, AuthError> {
    match crate::get_user_by_email(pool, email).await {
        Ok(user) => Ok(user),
        Err(DbError::NotFound(_)) => Err(AuthError::InvalidCredentials),
        Err(err) => Err(AuthError::from(err)),
    }
}

/// Password login (spec §6.1 `POST /auth/login`): email + password
/// → session. `remember_me` only changes the TTL for a
/// `PortalContractor` (30 days); staff sessions are always 8 hours.
pub async fn login_with_password(
    pool: &AnyPool,
    email: &str,
    password: &str,
    remember_me: bool,
) -> Result<LoginOutcome, AuthError> {
    let user = load_user_for_login(pool, email).await?;
    let auth_user = user_to_auth_user(&user)?;
    verify_password_login(&auth_user, password)?;
    issue_login_session(pool, &user, remember_me).await
}

/// Counter PIN login (spec §6.1 `POST /auth/counter-switch`):
/// email + PIN → session with the user's own role, 8-hour TTL.
/// The user is identified by email — an Argon2id hash cannot be
/// searched, so a bare PIN cannot name its user (see
/// `df_auth::login` docs).
pub async fn login_with_pin(
    pool: &AnyPool,
    email: &str,
    pin: &str,
) -> Result<LoginOutcome, AuthError> {
    let user = load_user_for_login(pool, email).await?;
    let auth_user = user_to_auth_user(&user)?;
    verify_pin_login(&auth_user, pin)?;
    issue_login_session(pool, &user, false).await
}

/// Counter badge login (spec §6.1 `POST /auth/counter-switch`):
/// email + badge token → session with the user's own role, 8-hour
/// TTL. Same identification note as [`login_with_pin`].
pub async fn login_with_badge(
    pool: &AnyPool,
    email: &str,
    badge_token: &str,
) -> Result<LoginOutcome, AuthError> {
    let user = load_user_for_login(pool, email).await?;
    let auth_user = user_to_auth_user(&user)?;
    verify_badge_login(&auth_user, badge_token)?;
    issue_login_session(pool, &user, false).await
}

/// Portal login (spec §6.1 `/portal/v1`, closes F4): email +
/// password for a `PortalContractor` user with a linked customer.
/// The issued session is scoped to that customer
/// (`session.customer_id`); 30-day TTL only when `remember_me` is
/// explicitly requested, 8 hours otherwise.
pub async fn login_portal(
    pool: &AnyPool,
    email: &str,
    password: &str,
    remember_me: bool,
) -> Result<LoginOutcome, AuthError> {
    let user = load_user_for_login(pool, email).await?;
    let auth_user = user_to_auth_user(&user)?;
    verify_portal_login(&auth_user, password)?;
    issue_login_session(pool, &user, remember_me).await
}

/// Manager override verification (spec §6.1
/// `POST /auth/override-verify`): the approver's email + password
/// or PIN. Returns the audit fields on success; no session is
/// issued — an override is a one-shot approval, not a login.
pub async fn verify_manager_override(
    pool: &AnyPool,
    email: &str,
    credential: &OverrideCredential,
) -> Result<OverrideApproval, AuthError> {
    let user = load_user_for_login(pool, email).await?;
    let auth_user = user_to_auth_user(&user)?;
    verify_override_pure(&auth_user, credential, Utc::now())
}

/// Per-request session validation — the check the HTTP workers run
/// on every authenticated request (spec §6.1, closes F1/F2's
/// missing check once the middleware calls it):
///
/// 1. hash the presented token, load the session row (unknown hash
///    → `InvalidSession`),
/// 2. pure-check hash / expiry / revocation / owning user active,
/// 3. refresh `last_used_at` and return the session (role +
///    portal `customer_id` included, so the handler can authorize
///    from the row alone).
pub async fn validate_session_token(pool: &AnyPool, token: &str) -> Result<Session, AuthError> {
    let token_hash = df_auth::hash_token(token);
    let session = match get_session_by_token_hash(pool, &token_hash).await {
        Ok(session) => session,
        Err(DbError::NotFound(_)) => return Err(AuthError::InvalidSession),
        Err(err) => return Err(AuthError::from(err)),
    };
    let user = match get_user_by_id(pool, session.user_id).await {
        Ok(user) => user,
        Err(DbError::NotFound(_)) => return Err(AuthError::InvalidSession),
        Err(err) => return Err(AuthError::from(err)),
    };
    let now = Utc::now();
    validate_session_pure(token, &session, user.active, now)?;
    touch_session(pool, session.id, now).await?;
    Ok(Session {
        last_used_at: now,
        ..session
    })
}
