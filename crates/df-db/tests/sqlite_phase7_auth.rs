//! Phase 7 (v1.1) auth-core SQLite round-trip tests.
//!
//! Covers: migration `0012_phase7` applies (roles seeded, sessions
//! table present), user creation + role-id mapping, password / PIN /
//! badge logins, portal login scoping + TTLs, manager override
//! rules, token issue/validate round-trip, expired + revoked
//! rejection, generic errors for wrong password / unknown email /
//! inactive user, token uniqueness, and the stored hash never being
//! the raw token.

use chrono::{Duration, Utc};
use df_auth::{
    AuthError, OverrideCredential, Role, generate_badge_token, hash_badge_token, hash_password,
    hash_pin, hash_token, role_id,
};
use df_db::auth::{
    NewUser, create_session, create_user, get_session_by_token_hash, list_sessions_for_user,
    login_portal, login_with_badge, login_with_password, login_with_pin, revoke_sessions_for_user,
    validate_session_token, verify_manager_override,
};
use df_db::models::{Customer, Organization};
use df_db::{Dialect, connect, get_customer, insert_customer, insert_organization, migrate};
use df_db::{count_users_with_role, get_user_by_id};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

async fn test_pool() -> df_db::AnyPool {
    let pool = connect("sqlite::memory:").await.unwrap();
    migrate(&pool, Dialect::Sqlite).await.unwrap();
    pool
}

fn org() -> Organization {
    Organization {
        id: Uuid::now_v7(),
        name: "Test Paint Co".to_string(),
        default_currency: "USD".to_string(),
        enabled_modules: serde_json::json!({}),
        settings_json: serde_json::json!({}),
        created_at: Utc::now(),
    }
}

fn customer(org_id: Uuid) -> Customer {
    Customer {
        id: Uuid::now_v7(),
        org_id,
        account_number: "C-PORTAL".to_string(),
        customer_type: "contractor".to_string(),
        company_name: Some("Portal Builders".to_string()),
        first_name: None,
        last_name: None,
        email: Some("portal@example.invalid".to_string()),
        phone: None,
        price_level_id: None,
        salesperson_id: None,
        tax_exempt_cert: None,
        tax_exempt_expires: None,
        charge_enabled: true,
        credit_limit: Decimal::from_str("1000").unwrap(),
        payment_terms: "NET30".to_string(),
        terms_days: None,
        finance_rate_pct_monthly: None,
        credit_hold: false,
        require_po_number: false,
        require_job_name: false,
        portal_enabled: true,
    }
}

/// Test-user spec (builder, so call sites stay readable without
/// an 8-argument helper — clippy's `too_many_arguments`).
struct UserSpec<'a> {
    org_id: Uuid,
    email: &'a str,
    role: Role,
    password: Option<&'a str>,
    pin: Option<&'a str>,
    badge: Option<&'a str>,
    portal_customer_id: Option<Uuid>,
}

impl<'a> UserSpec<'a> {
    fn new(org_id: Uuid, email: &'a str, role: Role) -> Self {
        Self {
            org_id,
            email,
            role,
            password: None,
            pin: None,
            badge: None,
            portal_customer_id: None,
        }
    }

    fn password(mut self, password: &'a str) -> Self {
        self.password = Some(password);
        self
    }

    fn pin(mut self, pin: &'a str) -> Self {
        self.pin = Some(pin);
        self
    }

    fn badge(mut self, badge: &'a str) -> Self {
        self.badge = Some(badge);
        self
    }

    fn portal(mut self, customer_id: Uuid) -> Self {
        self.portal_customer_id = Some(customer_id);
        self
    }
}

async fn make_user(pool: &df_db::AnyPool, spec: UserSpec<'_>) -> df_db::models::UserRow {
    create_user(
        pool,
        &NewUser {
            org_id: spec.org_id,
            email: spec.email.to_string(),
            display_name: spec.email.to_string(),
            password_hash: spec.password.map(|p| hash_password(p).unwrap()),
            counter_pin_hash: spec.pin.map(|p| hash_pin(p).unwrap()),
            badge_token_hash: spec.badge.map(|b| hash_badge_token(b).unwrap()),
            role: spec.role,
            portal_customer_id: spec.portal_customer_id,
            active: true,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn migration_seeds_roles_and_user_role_maps() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    // All seven roles are seeded with the well-known ids.
    for role in [
        Role::Cashier,
        Role::SalesAssociate,
        Role::Warehouse,
        Role::Driver,
        Role::Manager,
        Role::Admin,
        Role::PortalContractor,
    ] {
        let row = sqlx::query("SELECT name FROM roles WHERE id = $1")
            .bind(role_id(role).to_string())
            .fetch_one(&pool)
            .await
            .unwrap();
        let name: String = sqlx::Row::try_get(&row, "name").unwrap();
        assert_eq!(name, role.as_str());
    }
    let user = make_user(
        &pool,
        UserSpec::new(org.id, "admin@example.invalid", Role::Admin)
            .password("correct horse battery staple"),
    )
    .await;
    assert_eq!(user.role_id, role_id(Role::Admin));
    assert_eq!(user.portal_customer_id, None);
    let back = get_user_by_id(&pool, user.id).await.unwrap();
    assert_eq!(back.email, "admin@example.invalid");
    assert_eq!(
        count_users_with_role(&pool, org.id, Role::Admin)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        count_users_with_role(&pool, org.id, Role::Cashier)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn password_login_round_trip_and_token_validates() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    let user = make_user(
        &pool,
        UserSpec::new(org.id, "cashier@example.invalid", Role::Cashier)
            .password("correct horse battery staple")
            .pin("4821"),
    )
    .await;
    let outcome = login_with_password(
        &pool,
        "cashier@example.invalid",
        "correct horse battery staple",
        false,
    )
    .await
    .unwrap();
    assert_eq!(outcome.user.id, user.id);
    assert_eq!(outcome.session.user_id, user.id);
    assert_eq!(outcome.session.role, Role::Cashier);
    assert_eq!(outcome.session.customer_id, None);
    assert!(outcome.token.starts_with("dfp_"));
    // Staff TTL is 8 hours.
    assert_eq!(
        outcome.session.expires_at - outcome.session.created_at,
        Duration::hours(8)
    );
    // The issued token validates, and validation refreshes last_used_at.
    let validated = validate_session_token(&pool, &outcome.token).await.unwrap();
    assert_eq!(validated.id, outcome.session.id);
    assert!(validated.last_used_at >= outcome.session.last_used_at);
    // The stored hash is the SHA-256 of the token — never the token.
    let stored = get_session_by_token_hash(&pool, &hash_token(&outcome.token))
        .await
        .unwrap();
    assert_eq!(stored.id, outcome.session.id);
    assert_ne!(stored.token_hash, outcome.token);
    assert_eq!(stored.token_hash, hash_token(&outcome.token));
    // Raw SQL confirms the sessions row holds no raw token anywhere.
    let row = sqlx::query("SELECT token_hash, role FROM sessions WHERE id = $1")
        .bind(outcome.session.id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    let token_hash: String = sqlx::Row::try_get(&row, "token_hash").unwrap();
    let role_text: String = sqlx::Row::try_get(&row, "role").unwrap();
    assert!(!token_hash.contains(&outcome.token));
    assert_eq!(role_text, "Cashier");
}

#[tokio::test]
async fn wrong_password_and_unknown_email_are_the_same_generic_error() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    make_user(
        &pool,
        UserSpec::new(org.id, "cashier@example.invalid", Role::Cashier)
            .password("correct horse battery staple"),
    )
    .await;
    let wrong = login_with_password(&pool, "cashier@example.invalid", "wrong password", false)
        .await
        .unwrap_err();
    assert!(matches!(wrong, AuthError::InvalidCredentials));
    assert_eq!(wrong.to_string(), "invalid credentials");
    // Unknown email: identical error, no enumeration oracle.
    let unknown = login_with_password(&pool, "nobody@example.invalid", "wrong password", false)
        .await
        .unwrap_err();
    assert!(matches!(unknown, AuthError::InvalidCredentials));
    assert_eq!(unknown.to_string(), wrong.to_string());
}

#[tokio::test]
async fn inactive_user_cannot_log_in() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    let user = make_user(
        &pool,
        UserSpec::new(org.id, "cashier@example.invalid", Role::Cashier)
            .password("correct horse battery staple"),
    )
    .await;
    df_db::set_user_active(&pool, user.id, false).await.unwrap();
    assert!(matches!(
        login_with_password(
            &pool,
            "cashier@example.invalid",
            "correct horse battery staple",
            false
        )
        .await
        .unwrap_err(),
        AuthError::AccountInactive
    ));
    // Deactivation also invalidates an already-issued session.
    df_db::set_user_active(&pool, user.id, true).await.unwrap();
    let outcome = login_with_password(
        &pool,
        "cashier@example.invalid",
        "correct horse battery staple",
        false,
    )
    .await
    .unwrap();
    df_db::set_user_active(&pool, user.id, false).await.unwrap();
    assert!(matches!(
        validate_session_token(&pool, &outcome.token)
            .await
            .unwrap_err(),
        AuthError::AccountInactive
    ));
}

#[tokio::test]
async fn pin_and_badge_logins_work() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    let badge = generate_badge_token();
    make_user(
        &pool,
        UserSpec::new(org.id, "cashier@example.invalid", Role::Cashier)
            .password("correct horse battery staple")
            .pin("4821")
            .badge(&badge),
    )
    .await;
    let pin_outcome = login_with_pin(&pool, "cashier@example.invalid", "4821")
        .await
        .unwrap();
    assert_eq!(pin_outcome.session.role, Role::Cashier);
    assert!(matches!(
        login_with_pin(&pool, "cashier@example.invalid", "9999")
            .await
            .unwrap_err(),
        AuthError::InvalidCredentials
    ));
    let badge_outcome = login_with_badge(&pool, "cashier@example.invalid", &badge)
        .await
        .unwrap();
    assert_eq!(badge_outcome.session.role, Role::Cashier);
    assert_ne!(pin_outcome.token, badge_outcome.token);
    assert!(matches!(
        login_with_badge(
            &pool,
            "cashier@example.invalid",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        )
        .await
        .unwrap_err(),
        AuthError::InvalidCredentials
    ));
}

#[tokio::test]
async fn portal_login_scopes_to_linked_customer_and_remember_me_ttl() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    let cust = customer(org.id);
    insert_customer(&pool, &cust).await.unwrap();
    let back = get_customer(&pool, cust.id).await.unwrap();
    assert!(back.portal_enabled);
    // No linked customer yet: config error, not a credential error.
    make_user(
        &pool,
        UserSpec::new(org.id, "unlinked@example.invalid", Role::PortalContractor)
            .password("correct horse battery staple"),
    )
    .await;
    assert!(matches!(
        login_portal(
            &pool,
            "unlinked@example.invalid",
            "correct horse battery staple",
            false
        )
        .await
        .unwrap_err(),
        AuthError::PortalNotLinked
    ));
    // Linked: session is scoped to the customer.
    make_user(
        &pool,
        UserSpec::new(org.id, "portal@example.invalid", Role::PortalContractor)
            .password("correct horse battery staple")
            .portal(cust.id),
    )
    .await;
    let short = login_portal(
        &pool,
        "portal@example.invalid",
        "correct horse battery staple",
        false,
    )
    .await
    .unwrap();
    assert_eq!(short.session.customer_id, Some(cust.id));
    assert_eq!(short.session.role, Role::PortalContractor);
    assert_eq!(
        short.session.expires_at - short.session.created_at,
        Duration::hours(8)
    );
    let remembered = login_portal(
        &pool,
        "portal@example.invalid",
        "correct horse battery staple",
        true,
    )
    .await
    .unwrap();
    assert_eq!(remembered.session.customer_id, Some(cust.id));
    assert_eq!(
        remembered.session.expires_at - remembered.session.created_at,
        Duration::days(30)
    );
    // Staff cannot use the portal login, even with a linked customer.
    make_user(
        &pool,
        UserSpec::new(org.id, "staff@example.invalid", Role::Cashier)
            .password("correct horse battery staple")
            .portal(cust.id),
    )
    .await;
    assert!(matches!(
        login_portal(
            &pool,
            "staff@example.invalid",
            "correct horse battery staple",
            false
        )
        .await
        .unwrap_err(),
        AuthError::InvalidCredentials
    ));
}

#[tokio::test]
async fn manager_override_requires_manager_or_admin() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    make_user(
        &pool,
        UserSpec::new(org.id, "manager@example.invalid", Role::Manager)
            .password("correct horse battery staple")
            .pin("4821"),
    )
    .await;
    make_user(
        &pool,
        UserSpec::new(org.id, "cashier@example.invalid", Role::Cashier)
            .password("correct horse battery staple")
            .pin("4821"),
    )
    .await;
    // Manager by password and by PIN.
    let by_password = verify_manager_override(
        &pool,
        "manager@example.invalid",
        &OverrideCredential::Password("correct horse battery staple".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(by_password.approver_role, Role::Manager);
    let by_pin = verify_manager_override(
        &pool,
        "manager@example.invalid",
        &OverrideCredential::Pin("4821".to_string()),
    )
    .await
    .unwrap();
    assert_eq!(by_pin.approver_role, Role::Manager);
    // Cashier cannot approve even with a correct credential.
    assert!(matches!(
        verify_manager_override(
            &pool,
            "cashier@example.invalid",
            &OverrideCredential::Password("correct horse battery staple".to_string()),
        )
        .await
        .unwrap_err(),
        AuthError::PermissionDenied(_)
    ));
    // Manager with a wrong password: generic error.
    assert!(matches!(
        verify_manager_override(
            &pool,
            "manager@example.invalid",
            &OverrideCredential::Password("wrong password".to_string()),
        )
        .await
        .unwrap_err(),
        AuthError::InvalidCredentials
    ));
}

#[tokio::test]
async fn expired_token_rejected() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    let user = make_user(
        &pool,
        UserSpec::new(org.id, "cashier@example.invalid", Role::Cashier)
            .password("correct horse battery staple"),
    )
    .await;
    // A session minted with a negative TTL is already expired.
    let (token, _) = create_session(&pool, &user, Duration::seconds(-1), None)
        .await
        .unwrap();
    assert!(matches!(
        validate_session_token(&pool, &token).await.unwrap_err(),
        AuthError::InvalidSession
    ));
    // An unknown token is the same generic error.
    assert!(matches!(
        validate_session_token(&pool, "dfp_does-not-exist")
            .await
            .unwrap_err(),
        AuthError::InvalidSession
    ));
}

#[tokio::test]
async fn revoked_token_rejected_and_revoke_all_logs_out_everywhere() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    let user = make_user(
        &pool,
        UserSpec::new(org.id, "cashier@example.invalid", Role::Cashier)
            .password("correct horse battery staple"),
    )
    .await;
    let first = login_with_password(
        &pool,
        "cashier@example.invalid",
        "correct horse battery staple",
        false,
    )
    .await
    .unwrap();
    let second = login_with_password(
        &pool,
        "cashier@example.invalid",
        "correct horse battery staple",
        false,
    )
    .await
    .unwrap();
    // Two logins → two different tokens, both valid.
    assert_ne!(first.token, second.token);
    assert_ne!(first.session.id, second.session.id);
    assert!(validate_session_token(&pool, &first.token).await.is_ok());
    assert!(validate_session_token(&pool, &second.token).await.is_ok());
    assert_eq!(
        list_sessions_for_user(&pool, user.id).await.unwrap().len(),
        2
    );
    // Revoke one: the other keeps working.
    df_db::revoke_session(&pool, first.session.id)
        .await
        .unwrap();
    assert!(matches!(
        validate_session_token(&pool, &first.token)
            .await
            .unwrap_err(),
        AuthError::InvalidSession
    ));
    assert!(validate_session_token(&pool, &second.token).await.is_ok());
    // Revoke-all: nothing validates afterwards.
    let revoked = revoke_sessions_for_user(&pool, user.id).await.unwrap();
    assert_eq!(revoked, 1);
    assert!(matches!(
        validate_session_token(&pool, &second.token)
            .await
            .unwrap_err(),
        AuthError::InvalidSession
    ));
    // The rows stay as an audit trail (2 rows, both revoked).
    let listed = list_sessions_for_user(&pool, user.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|s| s.revoked_at.is_some()));
    // Revoke-all again is a no-op.
    assert_eq!(revoke_sessions_for_user(&pool, user.id).await.unwrap(), 0);
}

#[tokio::test]
async fn credential_setters_replace_hashes() {
    let pool = test_pool().await;
    let org = org();
    insert_organization(&pool, &org).await.unwrap();
    let user = make_user(
        &pool,
        UserSpec::new(org.id, "cashier@example.invalid", Role::Cashier)
            .password("correct horse battery staple")
            .pin("4821"),
    )
    .await;
    df_db::set_password_hash(
        &pool,
        user.id,
        Some(&hash_password("new password here").unwrap()),
    )
    .await
    .unwrap();
    assert!(
        login_with_password(
            &pool,
            "cashier@example.invalid",
            "correct horse battery staple",
            false
        )
        .await
        .is_err()
    );
    assert!(
        login_with_password(&pool, "cashier@example.invalid", "new password here", false)
            .await
            .is_ok()
    );
    df_db::set_pin_hash(&pool, user.id, Some(&hash_pin("9999").unwrap()))
        .await
        .unwrap();
    assert!(
        login_with_pin(&pool, "cashier@example.invalid", "4821")
            .await
            .is_err()
    );
    assert!(
        login_with_pin(&pool, "cashier@example.invalid", "9999")
            .await
            .is_ok()
    );
    // Clearing the password makes password login fail generically.
    df_db::set_password_hash(&pool, user.id, None)
        .await
        .unwrap();
    assert!(matches!(
        login_with_password(&pool, "cashier@example.invalid", "new password here", false)
            .await
            .unwrap_err(),
        AuthError::InvalidCredentials
    ));
}
