//! Login services — pure credential verification (Phase 7, v1.1).
//!
//! These functions verify credentials against an [`AuthUser`] value
//! the caller loaded from the database; they never touch a database
//! themselves. The DB-backed orchestration (load user → verify →
//! issue session → persist it) lives in `df_db::auth`
//! (`login_with_password`, `login_with_pin`, `login_with_badge`,
//! `login_portal`, `verify_manager_override`) so the HTTP workers in
//! `df-edge` / `df-api` call one function per login kind.
//!
//! Error discipline (deliberate, do not "improve" by distinguishing
//! cases): an unknown email, a wrong password, a missing password
//! hash, and a wrong-role portal attempt all return
//! [`AuthError::InvalidCredentials`] ("invalid credentials"). Only an
//! inactive account is a different variant
//! ([`AuthError::AccountInactive`]), and the HTTP layer maps that to
//! the same generic 401 — a login oracle must not reveal which
//! emails exist or which accounts are active.
//!
//! Counter switching (PIN / badge) identifies the user by email:
//! Argon2id hashes cannot be searched, so a PIN or badge token alone
//! cannot name its user. The counter UI selects the cashier (or
//! re-uses the previous session's user) and the PIN/badge proves it
//! is them. A badge-only lookup would need an extra non-secret badge
//! identifier column; that is an open Phase 7 follow-up, not
//! something this module fakes by scanning every user's hash.
//!
//! No COLORx code lives here or anywhere in Phase 7 (spec §9: the
//! Benjamin Moore COLORx integration is deferred post-v1.0).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AuthError;
use crate::password::verify_password;
use crate::pin::{verify_badge_token, verify_pin};
use crate::rbac::Role;

/// The user fields a login check needs, loaded from the `users` row
/// by the caller. No database handle, no SQL — this is the seam
/// between `df-db` rows and the pure verification below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthUser {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub password_hash: Option<String>,
    pub counter_pin_hash: Option<String>,
    pub badge_token_hash: Option<String>,
    pub role: Role,
    pub active: bool,
    /// Contractor customer this user may access in the portal.
    /// `Some` only for `PortalContractor` users
    /// (`users.portal_customer_id`, migration `0012_phase7`).
    pub portal_customer_id: Option<Uuid>,
}

/// Which credential a manager override was approved with. Recorded
/// on the approval so the audit log can say how the override was
/// authorized without storing the credential itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverrideMethod {
    Password,
    Pin,
}

/// The credential presented for a manager override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideCredential {
    Password(String),
    Pin(String),
}

/// A successful manager-override verification: the audit fields the
/// caller records on the overridden action (see `audit_logs`,
/// spec §5.1). No credential material is carried here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideApproval {
    pub approver_user_id: Uuid,
    pub approver_role: Role,
    pub method: OverrideMethod,
    pub approved_at: DateTime<Utc>,
}

/// Shared gate for every login kind: inactive users never log in,
/// whatever credential they present.
fn require_active(user: &AuthUser) -> Result<(), AuthError> {
    if !user.active {
        return Err(AuthError::AccountInactive);
    }
    Ok(())
}

/// Password login (spec §6.1 `POST /auth/login`): email + password.
/// Unknown email is indistinguishable from a wrong password — the
/// caller maps a missing user to this same error before calling
/// here, and a user with no password hash (OIDC-only) fails here.
pub fn verify_password_login(user: &AuthUser, password: &str) -> Result<(), AuthError> {
    require_active(user)?;
    let Some(hash) = user.password_hash.as_deref() else {
        return Err(AuthError::InvalidCredentials);
    };
    match verify_password(hash, password) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AuthError::InvalidCredentials),
        Err(_) => Err(AuthError::InvalidCredentials),
    }
}

/// Counter PIN login (spec §6.1 `POST /auth/counter-switch`): the
/// same user, authenticated by their 4–6 digit PIN. Sessions issued
/// from a PIN login carry the user's own role and the 8-hour staff
/// TTL — a PIN proves who is at the till, it does not escalate
/// privilege.
pub fn verify_pin_login(user: &AuthUser, pin: &str) -> Result<(), AuthError> {
    require_active(user)?;
    let Some(hash) = user.counter_pin_hash.as_deref() else {
        return Err(AuthError::InvalidCredentials);
    };
    match verify_pin(hash, pin) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AuthError::InvalidCredentials),
        // A malformed PIN shape is a caller error, but at the login
        // boundary it is still just "invalid credentials".
        Err(_) => Err(AuthError::InvalidCredentials),
    }
}

/// Counter badge login (spec §6.1 `POST /auth/counter-switch`): the
/// same user, authenticated by their RFID/barcode badge token.
pub fn verify_badge_login(user: &AuthUser, badge_token: &str) -> Result<(), AuthError> {
    require_active(user)?;
    let Some(hash) = user.badge_token_hash.as_deref() else {
        return Err(AuthError::InvalidCredentials);
    };
    match verify_badge_token(hash, badge_token) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AuthError::InvalidCredentials),
        Err(_) => Err(AuthError::InvalidCredentials),
    }
}

/// Portal login (spec §6.1 `/portal/v1`, closes finding F4): email +
/// password for a user whose role is `PortalContractor` **and** who
/// has a linked customer. Anything else — wrong role, no linked
/// customer, wrong password — is `InvalidCredentials`, except an
/// inactive account (`AccountInactive`) and a contractor user with
/// no link, which is the distinct [`AuthError::PortalNotLinked`]
/// (a config error the operator must fix, not a retryable login).
pub fn verify_portal_login(user: &AuthUser, password: &str) -> Result<(), AuthError> {
    require_active(user)?;
    if user.role != Role::PortalContractor {
        return Err(AuthError::InvalidCredentials);
    }
    if user.portal_customer_id.is_none() {
        return Err(AuthError::PortalNotLinked);
    }
    verify_password_login(user, password)
}

/// Manager override verification (spec §6.1
/// `POST /auth/override-verify`): a Manager/Admin password or PIN.
/// Returns the audit fields on success. A non-manager is
/// [`AuthError::PermissionDenied`] (not "invalid credentials": the
/// caller already authenticated the requesting cashier, so the
/// approver's role is the question, and the UI tells them plainly
/// they need a manager).
pub fn verify_manager_override(
    approver: &AuthUser,
    credential: &OverrideCredential,
    now: DateTime<Utc>,
) -> Result<OverrideApproval, AuthError> {
    require_active(approver)?;
    if !approver.role.can_approve_override() {
        return Err(AuthError::PermissionDenied(
            "manager override requires a Manager or Admin approver".to_string(),
        ));
    }
    let (method, ok) = match credential {
        OverrideCredential::Password(password) => {
            let ok = approver
                .password_hash
                .as_deref()
                .map(|hash| verify_password(hash, password).unwrap_or(false))
                .unwrap_or(false);
            (OverrideMethod::Password, ok)
        }
        OverrideCredential::Pin(pin) => {
            let ok = approver
                .counter_pin_hash
                .as_deref()
                .map(|hash| verify_pin(hash, pin).unwrap_or(false))
                .unwrap_or(false);
            (OverrideMethod::Pin, ok)
        }
    };
    if !ok {
        return Err(AuthError::InvalidCredentials);
    }
    Ok(OverrideApproval {
        approver_user_id: approver.id,
        approver_role: approver.role,
        method,
        approved_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::hash_password;
    use crate::pin::{generate_badge_token, hash_badge_token, hash_pin};

    fn user(role: Role) -> AuthUser {
        AuthUser {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            email: "cashier@example.invalid".to_string(),
            password_hash: Some(hash_password("correct horse battery staple").unwrap()),
            counter_pin_hash: Some(hash_pin("4821").unwrap()),
            badge_token_hash: None,
            role,
            active: true,
            portal_customer_id: None,
        }
    }

    #[test]
    fn password_login_ok_and_generic_on_wrong() {
        let user = user(Role::Cashier);
        assert!(verify_password_login(&user, "correct horse battery staple").is_ok());
        let err = verify_password_login(&user, "wrong password").unwrap_err();
        assert_eq!(err.to_string(), "invalid credentials");
        // No password hash (OIDC-only user): same generic error.
        let mut oidc = user.clone();
        oidc.password_hash = None;
        assert!(matches!(
            verify_password_login(&oidc, "anything"),
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[test]
    fn inactive_user_cannot_log_in_by_any_credential() {
        let mut user = user(Role::Cashier);
        user.active = false;
        assert!(matches!(
            verify_password_login(&user, "correct horse battery staple"),
            Err(AuthError::AccountInactive)
        ));
        assert!(matches!(
            verify_pin_login(&user, "4821"),
            Err(AuthError::AccountInactive)
        ));
    }

    #[test]
    fn pin_and_badge_logins_work() {
        let user = user(Role::Cashier);
        assert!(verify_pin_login(&user, "4821").is_ok());
        assert!(matches!(
            verify_pin_login(&user, "9999"),
            Err(AuthError::InvalidCredentials)
        ));
        // No PIN set: generic error, not a panic or a distinct code.
        let mut no_pin = user.clone();
        no_pin.counter_pin_hash = None;
        assert!(matches!(
            verify_pin_login(&no_pin, "4821"),
            Err(AuthError::InvalidCredentials)
        ));

        let badge = generate_badge_token();
        let mut badged = user.clone();
        badged.badge_token_hash = Some(hash_badge_token(&badge).unwrap());
        assert!(verify_badge_login(&badged, &badge).is_ok());
        assert!(matches!(
            verify_badge_login(&badged, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[test]
    fn portal_login_requires_contractor_role_and_link() {
        let mut contractor = user(Role::PortalContractor);
        // No linked customer yet: distinct config error.
        assert!(matches!(
            verify_portal_login(&contractor, "correct horse battery staple"),
            Err(AuthError::PortalNotLinked)
        ));
        contractor.portal_customer_id = Some(Uuid::now_v7());
        assert!(verify_portal_login(&contractor, "correct horse battery staple").is_ok());
        assert!(matches!(
            verify_portal_login(&contractor, "wrong password"),
            Err(AuthError::InvalidCredentials)
        ));
        // Staff role with a link still cannot use the portal.
        let mut staff = user(Role::Cashier);
        staff.portal_customer_id = Some(Uuid::now_v7());
        assert!(matches!(
            verify_portal_login(&staff, "correct horse battery staple"),
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[test]
    fn manager_override_requires_manager_or_admin() {
        let manager = user(Role::Manager);
        let approval = verify_manager_override(
            &manager,
            &OverrideCredential::Password("correct horse battery staple".to_string()),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(approval.approver_user_id, manager.id);
        assert_eq!(approval.approver_role, Role::Manager);
        assert_eq!(approval.method, OverrideMethod::Password);
        let pin_approval = verify_manager_override(
            &manager,
            &OverrideCredential::Pin("4821".to_string()),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(pin_approval.method, OverrideMethod::Pin);

        // Cashier cannot approve, even with a correct password.
        let cashier = user(Role::Cashier);
        assert!(matches!(
            verify_manager_override(
                &cashier,
                &OverrideCredential::Password("correct horse battery staple".to_string()),
                Utc::now(),
            ),
            Err(AuthError::PermissionDenied(_))
        ));
        // Manager with a wrong password: generic error.
        assert!(matches!(
            verify_manager_override(
                &manager,
                &OverrideCredential::Password("wrong password".to_string()),
                Utc::now(),
            ),
            Err(AuthError::InvalidCredentials)
        ));
    }
}
