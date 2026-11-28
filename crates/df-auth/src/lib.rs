//! `df-auth` — authentication & authorization (Phase 0, v0.1).
//!
//! Working in Phase 0:
//!
//! * [`password`] — Argon2id password hashing / verification.
//! * [`pin`] — counter PIN and RFID/barcode badge hashing (Argon2id).
//! * [`rbac`] — roles and permissions, including manager overrides.
//!
//! Working in Phase 7 (v1.1):
//!
//! * [`session`] — opaque `dfp_` session tokens (SHA-256 hash stored,
//!   8-hour staff TTL, 30-day portal remember-me), role/customer
//!   denormalized from the user at login.
//! * [`login`] — pure credential verification (password, PIN, badge,
//!   portal, manager override) against an [`AuthUser`] value. The
//!   DB-backed orchestration lives in `df_db::auth`.
//!
//! Stubbed (trait surface only, always `NotImplemented`) in Phase 0:
//!
//! * [`webauthn`] — Passkeys / WebAuthn.
//! * [`oidc`] — OIDC / SSO (Authentik, Keycloak, Entra ID).

pub mod error;
pub mod login;
pub mod oidc;
pub mod password;
pub mod pin;
pub mod rbac;
pub mod service_token;
pub mod session;
pub mod webauthn;

pub use error::AuthError;
pub use login::{AuthUser, OverrideApproval, OverrideCredential, OverrideMethod};
pub use oidc::{OidcConfig, OidcService, UnimplementedOidcService};
pub use password::{hash_password, verify_password};
pub use pin::{generate_badge_token, hash_badge_token, hash_pin, verify_badge_token, verify_pin};
pub use rbac::{Permission, Role, role_from_id, role_id};
pub use service_token::{
    SCOPE_SYNC_INGEST, SERVICE_TOKEN_PREFIX, ServiceToken, generate_service_token,
    new_service_token, validate_service_token_row,
};
pub use session::{
    PORTAL_REMEMBER_ME_TTL, PORTAL_SESSION_TTL, STAFF_SESSION_TTL, Session, TOKEN_PREFIX,
    generate_token, hash_token, hashes_equal, new_session, session_ttl, validate_session_token,
};
pub use webauthn::{PasskeyService, UnimplementedPasskeyService};
