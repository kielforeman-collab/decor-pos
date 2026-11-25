//! Counter PIN and RFID/barcode badge hashing (spec §1).
//!
//! PINs are 4–6 ASCII digits, used for instant counter switching and
//! manager overrides. Like passwords, a PIN is stored only as an
//! Argon2id PHC hash (`counter_pin_hash` / `badge_token_hash` in
//! spec §5.1) — never plaintext, never a reversible encoding.

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier};
use rand::{Rng, distr::Alphanumeric};

use crate::error::AuthError;
use crate::password::new_salt;

/// Validate PIN format: 4–6 ASCII digits.
pub fn validate_pin(pin: &str) -> Result<(), AuthError> {
    if !(4..=6).contains(&pin.chars().count()) || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(AuthError::InvalidPin("PIN must be 4-6 digits".to_string()));
    }
    Ok(())
}

/// Hash a counter PIN with Argon2id.
pub fn hash_pin(pin: &str) -> Result<String, AuthError> {
    validate_pin(pin)?;
    let salt = new_salt()?;
    Argon2::default()
        .hash_password(pin.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AuthError::Hashing(e.to_string()))
}

/// Verify a PIN against its stored hash.
pub fn verify_pin(hash: &str, pin: &str) -> Result<bool, AuthError> {
    validate_pin(pin)?;
    let parsed = PasswordHash::new(hash)
        .map_err(|e| AuthError::Hashing(format!("malformed PIN hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(pin.as_bytes(), &parsed)
        .is_ok())
}

/// Generate a random 32-character badge token (RFID/barcode badge).
/// The token itself is shown to the operator once at enrolment; only
/// its Argon2id hash is stored.
pub fn generate_badge_token() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect()
}

/// Hash a badge token with Argon2id.
pub fn hash_badge_token(token: &str) -> Result<String, AuthError> {
    if token.len() < 16 {
        return Err(AuthError::Hashing(
            "badge token must be at least 16 characters".to_string(),
        ));
    }
    let salt = new_salt()?;
    Argon2::default()
        .hash_password(token.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AuthError::Hashing(e.to_string()))
}

/// Verify a badge token against its stored hash.
pub fn verify_badge_token(hash: &str, token: &str) -> Result<bool, AuthError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| AuthError::Hashing(format!("malformed badge hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(token.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_format_rules() {
        assert!(validate_pin("1234").is_ok());
        assert!(validate_pin("123456").is_ok());
        assert!(validate_pin("123").is_err());
        assert!(validate_pin("1234567").is_err());
        assert!(validate_pin("12a4").is_err());
        assert!(validate_pin("").is_err());
    }

    #[test]
    fn pin_hash_and_verify_round_trips() {
        let hash = hash_pin("4821").unwrap();
        assert!(verify_pin(&hash, "4821").unwrap());
        assert!(!verify_pin(&hash, "4822").unwrap());
        assert!(!verify_pin(&hash, "9999").unwrap());
        // A malformed PIN (wrong shape) is an error, not a mismatch.
        assert!(verify_pin(&hash, "12").is_err());
    }

    #[test]
    fn badge_token_round_trips() {
        let token = generate_badge_token();
        assert_eq!(token.len(), 32);
        let hash = hash_badge_token(&token).unwrap();
        assert!(verify_badge_token(&hash, &token).unwrap());
        assert!(!verify_badge_token(&hash, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap());
    }
}
