//! Live-PostgreSQL round-trip over the HQ-served repos (Phase 8).
//!
//! Env-gated: reads the URL from `DECOR_POS_TEST_PG_URL` first,
//! then `/tmp/decor-pos-pg-url.txt` (Worker 3's provisioning).
//! Prints `SKIP ...` and passes vacuously when no URL is present;
//! the URL itself is never printed, only its redacted form.
//! Fresh UUIDv7 ids per run, so the test can run twice against the
//! same server without PK collisions.

use std::str::FromStr;

use chrono::Utc;
use df_db::models::{
    ArLedgerEntry, Colorant, Customer, InventoryLedgerEntry, Organization, Product, Store,
    Transaction,
};
use df_db::{Dialect, connect, migrate};
use rust_decimal::Decimal;
use sqlx::AnyPool;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn pg_url() -> Option<String> {
    let url = std::env::var("DECOR_POS_TEST_PG_URL").ok().or_else(|| {
        std::fs::read_to_string("/tmp/decor-pos-pg-url.txt")
            .ok()
            .map(|s| s.trim().to_string())
    })?;
    if url.is_empty() { None } else { Some(url) }
}

async fn pg_pool() -> Option<AnyPool> {
    let url = pg_url()?;
    eprintln!("PG test using {}", df_db::redact_db_url(&url));
    let pool = connect(&url).await.expect("connect pg");
    migrate(&pool, Dialect::Postgres).await.expect("migrate pg");
    Some(pool)
}

#[tokio::test]
async fn pg_repos_round_trip() {
    let Some(pool) = pg_pool().await else {
        eprintln!("SKIP pg_repos test: no live Postgres URL");
        return;
    };
    let org_id = Uuid::now_v7();
    let store_id = Uuid::now_v7();
    let product_id = Uuid::now_v7();
    let customer_id = Uuid::now_v7();
    let now = Utc::now();

    // Reference data: org / store / product / customer.
    df_db::insert_organization(
        &pool,
        &Organization {
            id: org_id,
            name: "PG Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({}),
            settings_json: serde_json::json!({}),
            created_at: now,
        },
    )
    .await
    .unwrap();
    df_db::insert_store(
        &pool,
        &Store {
            id: store_id,
            org_id,
            // `stores.code` is globally unique: suffix it with the
            // full store id (a UUIDv7 timestamp prefix alone is not
            // enough — two runs within a minute share it) so the PG
            // test is repeatable against a shared DB.
            code: format!("PG-MAIN-{store_id}"),
            name: "PG Main Store".to_string(),
            country_code: "US".to_string(),
            region_code: "CA".to_string(),
            postal_code: "90210".to_string(),
            currency: "USD".to_string(),
            tax_zone_id: Uuid::now_v7(),
            timezone: "America/Los_Angeles".to_string(),
            is_warehouse: false,
        },
    )
    .await
    .unwrap();
    let store = df_db::get_store(&pool, store_id).await.unwrap();
    assert_eq!(store.currency, "USD");
    assert_eq!(store.code, format!("PG-MAIN-{store_id}"));
    df_db::insert_product(
        &pool,
        &Product {
            id: product_id,
            org_id,
            sku: "PG-REGAL-1G".to_string(),
            manufacturer_code: Some("BM".to_string()),
            name: "Regal Select Interior Gallon".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: dec("100.0000"),
            standard_cost: dec("50.0000"),
            avg_cost: dec("50.0000"),
            min_margin_pct: None,
            sheen: Some("Eggshell".to_string()),
            paint_base_code: Some("1X".to_string()),
            container_volume_ml: Some(3785),
            eco_fee_category: Some("architectural_paint".to_string()),
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            is_active: true,
            map: None,
            msrp: None,
        },
    )
    .await
    .unwrap();
    let product = df_db::get_product(&pool, product_id).await.unwrap();
    // Money survives the TEXT round-trip exactly, scale included.
    assert_eq!(product.list_price, dec("100.0000"));
    assert_eq!(product.standard_cost, dec("50.0000"));
    df_db::insert_customer(
        &pool,
        &Customer {
            id: customer_id,
            org_id,
            account_number: "PG-C-100".to_string(),
            customer_type: "contractor".to_string(),
            company_name: Some("Acme Painting".to_string()),
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: true,
            credit_limit: dec("1000.0000"),
            payment_terms: "NET30".to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
        },
    )
    .await
    .unwrap();
    let customer = df_db::get_customer(&pool, customer_id).await.unwrap();
    assert_eq!(customer.credit_limit, dec("1000.0000"));

    // Inventory: a receive then a sale; balances derive in Rust.
    let tx_id = Uuid::now_v7();
    df_db::insert_inventory_delta(
        &pool,
        &InventoryLedgerEntry {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            product_id,
            lot_number: None,
            delta_on_hand: dec("10"),
            delta_committed: Decimal::ZERO,
            delta_on_order: Decimal::ZERO,
            delta_in_transit: Decimal::ZERO,
            unit_cost: dec("50.0000"),
            reason_type: "po_receive".to_string(),
            reference_id: Uuid::now_v7(),
            created_at: now,
        },
    )
    .await
    .unwrap();
    df_db::insert_inventory_delta(
        &pool,
        &InventoryLedgerEntry {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            product_id,
            lot_number: None,
            delta_on_hand: dec("-2"),
            delta_committed: Decimal::ZERO,
            delta_on_order: Decimal::ZERO,
            delta_in_transit: Decimal::ZERO,
            unit_cost: dec("50.0000"),
            reason_type: "sale".to_string(),
            reference_id: tx_id,
            created_at: now,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        df_db::inventory_on_hand(&pool, store_id, product_id)
            .await
            .unwrap(),
        dec("8")
    );

    // Transaction + AR charge, then balances.
    df_db::insert_transaction(
        &pool,
        &Transaction {
            id: tx_id,
            org_id,
            store_id,
            till_session_id: None,
            doc_number: "PG-TX-000001".to_string(),
            doc_type: "sale".to_string(),
            status: "completed".to_string(),
            customer_id: Some(customer_id),
            job_id: None,
            po_number: None,
            salesperson_id: None,
            cashier_id: None,
            currency: "USD".to_string(),
            subtotal: dec("200.0000"),
            tax_total: dec("16.1088"),
            eco_fee_total: Decimal::ZERO,
            pos_fee_total: Decimal::ZERO,
            grand_total: dec("216.1088"),
            deposit_paid: Decimal::ZERO,
            fulfillment_mode: "counter".to_string(),
            scheduled_at: None,
            completed_at: Some(now),
            created_at: now,
            job_name: None,
        },
    )
    .await
    .unwrap();
    let tx = df_db::get_transaction(&pool, tx_id).await.unwrap();
    assert_eq!(tx.grand_total, dec("216.1088"));
    assert_eq!(tx.completed_at, Some(now));
    df_db::insert_ar_entry_exec(
        &pool,
        &ArLedgerEntry {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            customer_id,
            job_id: None,
            entry_type: "charge".to_string(),
            reference_type: "transaction".to_string(),
            reference_id: tx_id,
            amount: dec("216.1088"),
            due_date: None,
            statement_id: None,
            created_at: now,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        df_db::ar_balance_for_customer(&pool, customer_id)
            .await
            .unwrap(),
        dec("216.1088")
    );

    // Tint / colorant catalog insert + read.
    let colorant_id = Uuid::now_v7();
    df_db::insert_colorant(
        &pool,
        &Colorant {
            id: colorant_id,
            org_id,
            system_name: "Gennex".to_string(),
            code: "Y3".to_string(),
            name: "Yellow Oxide".to_string(),
            cost_per_fl_oz: dec("0.4200"),
            shots_per_fl_oz: 48,
        },
    )
    .await
    .unwrap();
    let back = df_db::get_colorant(&pool, colorant_id).await.unwrap();
    assert_eq!(back.code, "Y3");
    assert_eq!(back.cost_per_fl_oz, dec("0.4200"));

    // Auth: create a user, log in with the password, validate the
    // session token — the full Phase 7 session path on PG.
    let password_hash = df_auth::hash_password("password-123").unwrap();
    // `users.email` is globally unique: suffix it with the full
    // org id (a UUIDv7 timestamp prefix alone is not enough — two
    // runs within a minute share it) so the PG test is repeatable
    // against a shared DB.
    let email = format!("pg-manager-{org_id}@example.invalid");
    let user = df_db::auth::create_user(
        &pool,
        &df_db::auth::NewUser {
            org_id,
            email: email.clone(),
            display_name: "PG Manager".to_string(),
            password_hash: Some(password_hash),
            counter_pin_hash: None,
            badge_token_hash: None,
            role: df_auth::Role::Manager,
            portal_customer_id: None,
            active: true,
        },
    )
    .await
    .unwrap();
    let outcome = df_db::auth::login_with_password(&pool, &user.email, "password-123", false)
        .await
        .unwrap();
    assert_eq!(outcome.user.id, user.id);
    let session = df_db::auth::validate_session_token(&pool, &outcome.token)
        .await
        .unwrap();
    assert_eq!(session.user_id, user.id);
}
