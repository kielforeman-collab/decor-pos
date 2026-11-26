//! Shared HTTP-test seeding (Phase 7, v1.1).
//!
//! Test code only — never used by production handlers. Seeds one
//! user per role (plus two portal contractors, each linked to their
//! own `portal_enabled` customer) and logs each in through the real
//! login services, so tests exercise the same token path the HTTP
//! gate validates: Argon2id password/PIN hashes in the `users`
//! table, `dfp_` tokens whose SHA-256 hash is the only thing
//! stored server-side.
//!
//! Passwords/PINs are fixed test values, never production
//! credentials: `password-123` for every seeded user, PIN `4821`
//! for cashier + manager.

use df_auth::{Role, hash_password, hash_pin};
use df_db::AnyPool;
use df_db::auth::{NewUser, create_user, login_with_password};
use df_db::models::{Customer, UserRow};
use uuid::Uuid;

/// The test password for every seeded user.
pub const TEST_PASSWORD: &str = "password-123";

/// The test counter PIN for cashier + manager.
pub const TEST_PIN: &str = "4821";

/// One seeded user plus the live token their login returned.
#[derive(Debug, Clone)]
pub struct TestUser {
    pub user: UserRow,
    pub email: String,
    pub token: String,
}

impl TestUser {
    pub fn id(&self) -> Uuid {
        self.user.id
    }

    pub fn role(&self) -> Role {
        df_auth::role_from_id(self.user.role_id).expect("seeded role is mapped")
    }

    /// `Authorization: Bearer <token>` header value.
    pub fn bearer(&self) -> String {
        format!("Bearer {}", self.token)
    }
}

/// Every seeded user, plus the two portal customers (A and B) the
/// contractors are scoped to.
#[derive(Debug, Clone)]
pub struct SeededUsers {
    pub admin: TestUser,
    pub manager: TestUser,
    pub cashier: TestUser,
    pub sales: TestUser,
    pub warehouse: TestUser,
    pub driver: TestUser,
    pub contractor_a: TestUser,
    pub contractor_b: TestUser,
    pub customer_a: Uuid,
    pub customer_b: Uuid,
}

async fn seed_customer(pool: &AnyPool, org_id: Uuid, account: &str, company: &str) -> Uuid {
    let id = Uuid::now_v7();
    df_db::insert_customer(
        pool,
        &Customer {
            id,
            org_id,
            account_number: account.to_string(),
            customer_type: "contractor".to_string(),
            company_name: Some(company.to_string()),
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: true,
            credit_limit: "10000.0000".parse().unwrap(),
            payment_terms: "NET30".to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: true,
        },
    )
    .await
    .expect("seed portal customer");
    id
}

async fn seed_user(
    pool: &AnyPool,
    org_id: Uuid,
    email: &str,
    role: Role,
    with_pin: bool,
    portal_customer_id: Option<Uuid>,
) -> TestUser {
    let user = create_user(
        pool,
        &NewUser {
            org_id,
            email: email.to_string(),
            display_name: format!("Test {role:?}"),
            password_hash: Some(hash_password(TEST_PASSWORD).expect("hash test password")),
            counter_pin_hash: if with_pin {
                Some(hash_pin(TEST_PIN).expect("hash test PIN"))
            } else {
                None
            },
            badge_token_hash: None,
            role,
            portal_customer_id,
            active: true,
        },
    )
    .await
    .expect("seed test user");
    let outcome = login_with_password(pool, email, TEST_PASSWORD, false)
        .await
        .expect("seeded user logs in");
    TestUser {
        user,
        email: email.to_string(),
        token: outcome.token,
    }
}

/// Seed one user per role + two portal contractors (A and B, each
/// linked to their own portal-enabled customer) and return their
/// live tokens. The caller owns the pool and the org row.
pub async fn seed_test_users(pool: &AnyPool, org_id: Uuid) -> SeededUsers {
    let customer_a = seed_customer(pool, org_id, "C-PORTAL-A", "A Painting Co").await;
    let customer_b = seed_customer(pool, org_id, "C-PORTAL-B", "B Painting Co").await;
    SeededUsers {
        admin: seed_user(
            pool,
            org_id,
            "admin@example.invalid",
            Role::Admin,
            false,
            None,
        )
        .await,
        manager: seed_user(
            pool,
            org_id,
            "manager@example.invalid",
            Role::Manager,
            true,
            None,
        )
        .await,
        cashier: seed_user(
            pool,
            org_id,
            "cashier@example.invalid",
            Role::Cashier,
            true,
            None,
        )
        .await,
        sales: seed_user(
            pool,
            org_id,
            "sales@example.invalid",
            Role::SalesAssociate,
            false,
            None,
        )
        .await,
        warehouse: seed_user(
            pool,
            org_id,
            "warehouse@example.invalid",
            Role::Warehouse,
            false,
            None,
        )
        .await,
        driver: seed_user(
            pool,
            org_id,
            "driver@example.invalid",
            Role::Driver,
            false,
            None,
        )
        .await,
        contractor_a: seed_user(
            pool,
            org_id,
            "contractor-a@example.invalid",
            Role::PortalContractor,
            false,
            Some(customer_a),
        )
        .await,
        contractor_b: seed_user(
            pool,
            org_id,
            "contractor-b@example.invalid",
            Role::PortalContractor,
            false,
            Some(customer_b),
        )
        .await,
        customer_a,
        customer_b,
    }
}
