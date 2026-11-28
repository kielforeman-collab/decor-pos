//! Long-lived, revocable, store-scoped service tokens (`dfs_...`) for
//! unattended edge -> HQ sync daemons.
//!
//! Same cryptographic discipline as [`crate::session`]: 32 random bytes
//! (256-bit entropy via `rand::rng()`) encoded as base64url with the
//! greppable `dfs_` prefix; only the SHA-256 hex digest is persisted in
//! `service_tokens.token_hash`. The raw token is returned once at
//! issuance and cannot be recovered afterwards.
//!
//! Least privilege: every service token is bound to a single `store_id`
//! and an explicit scope list (e.g. `["sync:ingest"]`). The HTTP gate
//! rejects `dfs_` tokens on every non-service-token route (403), and
//! sync ingest refuses batches whose `store_id` does not match the
//! token's bound `store_id` (403).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AuthError;
use crate::session::{hash_token, hashes_equal};

/// Prefix for presented service tokens (distinct from `dfp_` user sessions).
pub const SERVICE_TOKEN_PREFIX: &str = "dfs_";

/// Scope allowing an edge daemon to push delta batches to HQ
/// (`POST /api/v1/sync/ingest`) for its bound `store_id`.
pub const SCOPE_SYNC_INGEST: &str = "sync:ingest";

/// Generate a raw service token: `dfs_` + base64url (no padding) of 32
/// random bytes (256 bits of entropy).
pub fn generate_service_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{SERVICE_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

/// One row of the `service_tokens` table (migration `0016`).
/// Holds the SHA-256 hash only — never the raw token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceToken {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub label: String,
    pub token_hash: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ServiceToken {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|exp| now >= exp)
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    pub fn is_valid(&self, now: DateTime<Utc>) -> bool {
        !self.is_revoked() && !self.is_expired(now)
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

/// Issue a fresh service token. Returns `(raw_token, row)` — the caller
/// persists `row` (which holds only the SHA-256 hash) and returns
/// `raw_token` to the operator exactly once.
pub fn new_service_token(
    org_id: Uuid,
    store_id: Uuid,
    label: impl Into<String>,
    scopes: Vec<String>,
    ttl: Option<Duration>,
    now: DateTime<Utc>,
) -> (String, ServiceToken) {
    let token = generate_service_token();
    let token_hash = hash_token(&token);
    let expires_at = ttl.map(|d| now + d);
    let row = ServiceToken {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        label: label.into(),
        token_hash,
        scopes,
        created_at: now,
        expires_at,
        revoked_at: None,
        last_used_at: None,
    };
    (token, row)
}

/// Pure validation of a presented service token against its stored row.
/// Rejects wrong prefix, hash mismatch, revoked, or expired tokens with
/// the uniform [`AuthError::InvalidSession`].
pub fn validate_service_token_row(
    presented_token: &str,
    stored: &ServiceToken,
    now: DateTime<Utc>,
) -> Result<(), AuthError> {
    if !presented_token.starts_with(SERVICE_TOKEN_PREFIX) {
        return Err(AuthError::InvalidSession);
    }
    let presented_hash = hash_token(presented_token);
    if !hashes_equal(&presented_hash, &stored.token_hash) {
        return Err(AuthError::InvalidSession);
    }
    if !stored.is_valid(now) {
        return Err(AuthError::InvalidSession);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_and_validate_service_token() {
        let now = Utc::now();
        let org = Uuid::now_v7();
        let store = Uuid::now_v7();
        let (raw, row) = new_service_token(
            org,
            store,
            "store-1 edge",
            vec![SCOPE_SYNC_INGEST.to_string()],
            Some(Duration::days(90)),
            now,
        );
        assert!(raw.starts_with(SERVICE_TOKEN_PREFIX));
        assert_ne!(raw, row.token_hash);
        assert!(row.has_scope(SCOPE_SYNC_INGEST));
        assert!(!row.has_scope("admin:all"));

        validate_service_token_row(&raw, &row, now).unwrap();
        assert!(matches!(
            validate_service_token_row("dfp_wrong_prefix", &row, now),
            Err(AuthError::InvalidSession)
        ));
        assert!(matches!(
            validate_service_token_row("dfs_wrong_secret", &row, now),
            Err(AuthError::InvalidSession)
        ));
        // Expired
        assert!(matches!(
            validate_service_token_row(&raw, &row, now + Duration::days(91)),
            Err(AuthError::InvalidSession)
        ));
        // Revoked
        let mut revoked = row.clone();
        revoked.revoked_at = Some(now);
        assert!(matches!(
            validate_service_token_row(&raw, &revoked, now),
            Err(AuthError::InvalidSession)
        ));
    }

    #[test]
    fn non_expiring_service_token_valid_indefinitely_until_revoked() {
        let now = Utc::now();
        let (raw, mut row) = new_service_token(
            Uuid::now_v7(),
            Uuid::now_v7(),
            "forever-edge",
            vec![SCOPE_SYNC_INGEST.to_string()],
            None,
            now,
        );
        assert!(row.expires_at.is_none());
        validate_service_token_row(&raw, &row, now + Duration::days(3650)).unwrap();
        row.revoked_at = Some(now + Duration::days(1));
        assert!(matches!(
            validate_service_token_row(&raw, &row, now + Duration::days(2)),
            Err(AuthError::InvalidSession)
        ));
    }
}
