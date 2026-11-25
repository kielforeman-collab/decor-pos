//! `df-auth` — authentication & authorization (Phase 0, v0.1).
//!
//! Working in Phase 0:
//!
//! * [`password`] — Argon2id password hashing / verification.
//! * [`pin`] — counter PIN and RFID/barcode badge hashing (Argon2id).
//! * [`rbac`] — roles and permissions, including manager overrides.
//!
//! Stubbed (trait surface only, always `NotImplemented`) in Phase 0:
//!
//! * [`webauthn`] — Passkeys / WebAuthn.
//! * [`oidc`] — OIDC / SSO (Authentik, Keycloak, Entra ID).

pub mod error;
pub mod oidc;
pub mod password;
pub mod pin;
pub mod rbac;
pub mod webauthn;

pub use error::AuthError;
pub use oidc::{OidcConfig, OidcService, UnimplementedOidcService};
pub use password::{hash_password, verify_password};
pub use pin::{generate_badge_token, hash_badge_token, hash_pin, verify_badge_token, verify_pin};
pub use rbac::{Permission, Role};
pub use webauthn::{PasskeyService, UnimplementedPasskeyService};
