//! Argon2id password hashing (spec §1: self-contained local auth).
//!
//! Hashes are PHC strings (`$argon2id$...`) produced with Argon2's
//! default parameters (m=19 MiB, t=2, p=1). Only the hash is stored;
//! the plaintext password never leaves this module's callers.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::RngCore;

use crate::error::AuthError;

/// Generate a random 16-byte salt. Built from `rand` 0.9 bytes rather
/// than argon2's re-exported `rand_core` 0.6 `OsRng`, whose availability
/// depends on feature unification elsewhere in the workspace.
pub(crate) fn new_salt() -> Result<SaltString, AuthError> {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    SaltString::encode_b64(&bytes)
        .map_err(|e| AuthError::Hashing(format!("salt generation failed: {e}")))
}

/// Hash a password. Rejects passwords shorter than 8 characters.
pub fn hash_password(password: &str) -> Result<String, AuthError> {
    if password.chars().count() < 8 {
        return Err(AuthError::Hashing(
            "password must be at least 8 characters".to_string(),
        ));
    }
    let salt = new_salt()?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AuthError::Hashing(e.to_string()))
}

/// Verify a password against a stored PHC hash. Returns `Ok(false)`
/// for a wrong password and `Err` only for a malformed stored hash.
pub fn verify_password(hash: &str, password: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| AuthError::Hashing(format!("malformed password hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_round_trips() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password(&hash, "correct horse battery staple").unwrap());
        assert!(!verify_password(&hash, "wrong password").unwrap());
    }

    #[test]
    fn short_passwords_rejected() {
        assert!(hash_password("short").is_err());
    }

    #[test]
    fn same_password_hashes_differ() {
        let a = hash_password("same-password-123").unwrap();
        let b = hash_password("same-password-123").unwrap();
        assert_ne!(a, b, "random salt must make hashes differ");
        assert!(verify_password(&a, "same-password-123").unwrap());
        assert!(verify_password(&b, "same-password-123").unwrap());
    }

    #[test]
    fn malformed_hash_is_an_error() {
        assert!(verify_password("not-a-hash", "whatever").is_err());
    }
}
