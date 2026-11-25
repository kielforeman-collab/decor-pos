//! Passkeys / WebAuthn — **Phase 0 stub**.
//!
//! The trait surface exists so `df-api` can wire the routes from
//! spec §6.1 (`POST /auth/passkey/verify`), but there is no WebAuthn
//! implementation yet: every method returns
//! [`AuthError::NotImplemented`](crate::error::AuthError::NotImplemented).
//! Do not treat this as working passkey support. Real WebAuthn lands
//! in a later phase; Argon2id passwords + PINs are the working auth in
//! Phase 0.

use uuid::Uuid;

use crate::error::AuthError;

/// Passkey registration / authentication service.
pub trait PasskeyService {
    /// Begin a passkey registration ceremony for a user.
    fn registration_begin(&self, user_id: Uuid) -> Result<String, AuthError>;
    /// Verify a passkey authentication response.
    fn authentication_verify(&self, user_id: Uuid, response_json: &str) -> Result<bool, AuthError>;
}

/// The Phase 0 implementation: always unimplemented.
pub struct UnimplementedPasskeyService;

impl PasskeyService for UnimplementedPasskeyService {
    fn registration_begin(&self, _user_id: Uuid) -> Result<String, AuthError> {
        Err(AuthError::NotImplemented(
            "WebAuthn/Passkeys are not implemented in Phase 0",
        ))
    }

    fn authentication_verify(
        &self,
        _user_id: Uuid,
        _response_json: &str,
    ) -> Result<bool, AuthError> {
        Err(AuthError::NotImplemented(
            "WebAuthn/Passkeys are not implemented in Phase 0",
        ))
    }
}
