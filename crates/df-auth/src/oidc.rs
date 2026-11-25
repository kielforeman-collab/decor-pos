//! OIDC / SSO — **Phase 0 stub**.
//!
//! The config type and trait surface exist so `df-api` can wire the
//! route from spec §6.1 (`GET /auth/oidc/callback`), but there is no
//! OIDC implementation yet: every method returns
//! [`AuthError::NotImplemented`](crate::error::AuthError::NotImplemented).
//! Do not treat this as working SSO. Real OIDC (Authentik / Keycloak /
//! Entra ID) lands in a later phase; Argon2id local auth is the working
//! auth in Phase 0.

use serde::{Deserialize, Serialize};

use crate::error::AuthError;

/// OIDC provider configuration (no secrets in this struct — the client
/// secret lives in the deployment's secret store, never in code).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub redirect_uri: String,
}

/// OIDC login flow.
pub trait OidcService {
    /// Build the authorization URL to redirect the user to.
    fn authorization_url(&self, state: &str) -> Result<String, AuthError>;
    /// Exchange an authorization code for an identity.
    fn exchange_code(&self, code: &str) -> Result<String, AuthError>;
}

/// The Phase 0 implementation: always unimplemented.
pub struct UnimplementedOidcService {
    pub config: OidcConfig,
}

impl OidcService for UnimplementedOidcService {
    fn authorization_url(&self, _state: &str) -> Result<String, AuthError> {
        Err(AuthError::NotImplemented(
            "OIDC/SSO is not implemented in Phase 0",
        ))
    }

    fn exchange_code(&self, _code: &str) -> Result<String, AuthError> {
        Err(AuthError::NotImplemented(
            "OIDC/SSO is not implemented in Phase 0",
        ))
    }
}
