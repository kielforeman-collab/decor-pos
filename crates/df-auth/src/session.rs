//! Opaque session tokens (Phase 7, v1.1).
//!
//! A session is a random 32-byte token the caller presents as a
//! bearer credential. The database stores **only the SHA-256 hex of
//! the token** (`sessions.token_hash`); the raw token is returned to
//! the caller exactly once, at login, and is never persisted, logged,
//! or derivable from the stored hash. Losing the token means logging
//! in again — there is no recovery path, by design.
//!
//! Token shape: `dfp_<base64url>` — the `dfp_` prefix makes tokens
//! greppable in logs and distinguishes them from UUIDs, badge tokens
//! and idempotency keys at a glance. The 32 random bytes come from
//! `rand` (`OsRng` via `rand::rng`); the hash is plain SHA-256, not
//! Argon2id: a session token already has 256 bits of entropy, and
//! login validation must stay cheap enough to run on every request
//! (Argon2id verification on every API call would be a self-inflicted
//! denial of service). Hash comparison is constant-time over the hex
//! strings ([`hashes_equal`]).
//!
//! TTLs (see [`session_ttl`]):
//!
//! * Staff sessions (every role except a remembered portal login):
//!   **8 hours** — one till shift. A cashier who is still working
//!   after 8 hours logs in again; there is no silent refresh.
//! * Portal remember-me: **30 days**, only when the caller
//!   explicitly asks for it at login. A portal login without
//!   remember-me is 8 hours like staff.
//!
//! The `role` and `customer_id` on a session are denormalized from
//! the user at login (spec §6.1 / task brief): the HTTP layer can
//! authorize a request from the session row alone, and a portal
//! session is scoped to exactly the contractor customer it was
//! issued for. If the user's role changes, existing sessions keep
//! the old role until they expire or are revoked — revoke on role
//! change (see `df_db::revoke_sessions_for_user`).
//!
//! No COLORx code lives here or anywhere in Phase 7 (spec §9: the
//! Benjamin Moore COLORx integration is deferred post-v1.0).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AuthError;
use crate::rbac::Role;

/// Prefix for presented session tokens. Greppable, and distinct from
/// UUIDs / badge tokens / idempotency keys.
pub const TOKEN_PREFIX: &str = "dfp_";

/// Staff session lifetime: 8 hours (one till shift).
pub const STAFF_SESSION_TTL: Duration = Duration::hours(8);

/// Portal session lifetime without remember-me: 8 hours, like staff.
pub const PORTAL_SESSION_TTL: Duration = Duration::hours(8);

/// Portal remember-me lifetime: 30 days. Only issued when the caller
/// explicitly requests remember-me at login.
pub const PORTAL_REMEMBER_ME_TTL: Duration = Duration::days(30);

/// The TTL for a fresh session: 30 days only for a portal login with
/// remember-me explicitly requested; 8 hours for everything else.
pub fn session_ttl(role: Role, remember_me: bool) -> Duration {
    if role == Role::PortalContractor && remember_me {
        PORTAL_REMEMBER_ME_TTL
    } else {
        STAFF_SESSION_TTL
    }
}

/// Generate a presented token: `dfp_` + base64url (no padding) of 32
/// random bytes (256 bits of entropy).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// SHA-256 hex of a presented token. This — and only this — is what
/// the `sessions` table stores.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Constant-time equality for two hash strings (XOR fold, no early
/// exit on the first differing byte). Lengths differ only when the
/// inputs are not both SHA-256 hex, in which case the answer is
/// `false` — the length itself is not secret for fixed-size hashes.
pub fn hashes_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// One row of the `sessions` table (migration `0012_phase7`).
/// Carries the token **hash** only — there is no field that could
/// hold a raw token, so a `Session` can be logged and serialized
/// safely. Timestamps are UTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// UUIDv7 session id.
    pub id: Uuid,
    /// The user this session belongs to.
    pub user_id: Uuid,
    /// SHA-256 hex of the presented token. Never the token itself.
    pub token_hash: String,
    /// Role cached from the user at login (see module docs).
    pub role: Role,
    /// Contractor customer this session may access. `Some` only for
    /// `PortalContractor` sessions; `None` for staff.
    pub customer_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Set when the session was revoked; `None` while live.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Last successful validation. Updated by
    /// `df_db::validate_session_token` on every use.
    pub last_used_at: DateTime<Utc>,
}

impl Session {
    /// True when `now` is at or past `expires_at`.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }

    /// True when the session was revoked.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// True when the session is live: not expired and not revoked.
    /// (The owning user's `active` flag is checked separately by
    /// [`validate_session_token`], which loads the user.)
    pub fn is_valid(&self, now: DateTime<Utc>) -> bool {
        !self.is_expired(now) && !self.is_revoked()
    }
}

/// Issue a fresh session: mints the id, generates the presented
/// token, and returns `(token, session)`. The caller persists the
/// session row (see `df_db::insert_session` / `create_session`) and
/// hands the token to the caller exactly once — the token cannot be
/// recovered from the row afterwards.
pub fn new_session(
    user_id: Uuid,
    role: Role,
    customer_id: Option<Uuid>,
    ttl: Duration,
    now: DateTime<Utc>,
) -> (String, Session) {
    let token = generate_token();
    let session = Session {
        id: Uuid::now_v7(),
        user_id,
        token_hash: hash_token(&token),
        role,
        customer_id,
        created_at: now,
        expires_at: now + ttl,
        revoked_at: None,
        last_used_at: now,
    };
    (token, session)
}

/// Pure validation of a presented token against a stored session row
/// and the owning user's active flag. The caller loads the row by
/// token hash (`df_db::get_session_by_token_hash`) and the user
/// first; this function does the checks that need no database:
///
/// * the token's hash equals the stored hash (constant-time),
/// * the session is not expired and not revoked,
/// * the owning user is still active.
///
/// Every failure is [`AuthError::InvalidSession`] except an inactive
/// user, which is [`AuthError::AccountInactive`] — the HTTP layer
/// maps both to a generic 401.
pub fn validate_session_token(
    token: &str,
    session: &Session,
    user_active: bool,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    if !hashes_equal(&hash_token(token), &session.token_hash) {
        return Err(AuthError::InvalidSession);
    }
    if !session.is_valid(now) {
        return Err(AuthError::InvalidSession);
    }
    if !user_active {
        return Err(AuthError::AccountInactive);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn token_shape_and_hash_are_not_the_token() {
        let token = generate_token();
        assert!(token.starts_with(TOKEN_PREFIX));
        // 32 bytes -> 43 base64url chars, no padding.
        assert_eq!(token.len(), TOKEN_PREFIX.len() + 43);
        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert_ne!(hash, token, "stored hash must not be the raw token");
        assert_eq!(hash, hash_token(&token), "hashing is deterministic");
        assert!(hashes_equal(&hash, &hash_token(&token)));
        assert!(!hashes_equal(&hash, &hash_token("dfp_something-else")));
    }

    #[test]
    fn two_tokens_differ() {
        assert_ne!(generate_token(), generate_token());
    }

    #[test]
    fn session_ttl_choices() {
        assert_eq!(session_ttl(Role::Cashier, false), STAFF_SESSION_TTL);
        assert_eq!(session_ttl(Role::Cashier, true), STAFF_SESSION_TTL);
        assert_eq!(
            session_ttl(Role::PortalContractor, false),
            PORTAL_SESSION_TTL
        );
        assert_eq!(
            session_ttl(Role::PortalContractor, true),
            PORTAL_REMEMBER_ME_TTL
        );
        assert_eq!(STAFF_SESSION_TTL, Duration::hours(8));
        assert_eq!(PORTAL_REMEMBER_ME_TTL, Duration::days(30));
    }

    #[test]
    fn new_session_round_trips_and_validates() {
        let user_id = Uuid::now_v7();
        let (token, session) = new_session(user_id, Role::Cashier, None, STAFF_SESSION_TTL, now());
        assert_eq!(session.user_id, user_id);
        assert_eq!(session.role, Role::Cashier);
        assert_eq!(session.token_hash, hash_token(&token));
        assert!(session.is_valid(now()));
        assert!(validate_session_token(&token, &session, true, now()).is_ok());
        // Wrong token, inactive user.
        assert!(validate_session_token("dfp_wrong", &session, true, now()).is_err());
        assert!(matches!(
            validate_session_token(&token, &session, false, now()),
            Err(AuthError::AccountInactive)
        ));
    }

    #[test]
    fn expired_and_revoked_sessions_are_invalid() {
        let (token, mut session) = new_session(
            Uuid::now_v7(),
            Role::Manager,
            None,
            STAFF_SESSION_TTL,
            now(),
        );
        // Expired: now past expires_at.
        assert!(matches!(
            validate_session_token(&token, &session, true, session.expires_at),
            Err(AuthError::InvalidSession)
        ));
        assert!(session.is_expired(session.expires_at));
        // Revoked: live time, revoked_at set.
        session.revoked_at = Some(now());
        assert!(session.is_revoked());
        assert!(!session.is_valid(now()));
        assert!(validate_session_token(&token, &session, true, now()).is_err());
    }
}
