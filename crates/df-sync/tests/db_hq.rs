//! `DbHq` tests: SQLite always, live PostgreSQL when Worker 3's URL
//! is available (env `DECOR_POS_TEST_PG_URL` first, then
//! `/tmp/decor-pos-pg-url.txt`). The PG tests print `SKIP ...` when
//! no URL is present; the URL itself is never printed.

use std::str::FromStr;

use chrono::Utc;
use df_db::models::{Customer, Organization, Product, Store};
use df_db::{Dialect, connect, migrate};
use df_sync::{ApplyReport, DbHq, DeltaBatch, HqSink, OutboxEntry};
use rust_decimal::Decimal;
use sqlx::AnyPool;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Seeded reference rows. Fresh UUIDv7s per run, so the PG test can
/// run twice against the same server without PK collisions.
struct Seed {
    org_id: Uuid,
    store_id: Uuid,
    product_id: Uuid,
    customer_id: Uuid,
}

async fn seed(pool: &AnyPool) -> Seed {
    let seed = Seed {
        org_id: Uuid::now_v7(),
        store_id: Uuid::now_v7(),
        product_id: Uuid::now_v7(),
        customer_id: Uuid::now_v7(),
    };
    df_db::insert_organization(
        pool,
        &Organization {
            id: seed.org_id,
            name: "Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({}),
            settings_json: serde_json::json!({}),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    df_db::insert_store(
        pool,
        &Store {
            id: seed.store_id,
            org_id: seed.org_id,
            // `stores.code` is globally unique: suffix it with the
            // full store id (a UUIDv7 timestamp prefix alone is not
            // enough — two runs within a minute share it) so the PG
            // test is repeatable against a shared DB.
            code: format!("MAIN-{}", seed.store_id),
            name: "Main Store".to_string(),
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
    df_db::insert_product(
        pool,
        &Product {
            id: seed.product_id,
            org_id: seed.org_id,
            sku: "BM-REGAL-1G".to_string(),
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
            upc: None,
            category: None,
            line_code: None,
            color_name: None,
            size_code: None,
            source_metadata: None,
            is_active: true,
            map: None,
            msrp: None,
        },
    )
    .await
    .unwrap();
    df_db::insert_customer(
        pool,
        &Customer {
            id: seed.customer_id,
            org_id: seed.org_id,
            account_number: "C-100".to_string(),
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
            address_line1: None,
            address_line2: None,
            city: None,
            province: None,
            postal_code: None,
            notes: None,
        },
    )
    .await
    .unwrap();
    seed
}

async fn sqlite_pool() -> (AnyPool, Seed) {
    let pool = connect("sqlite::memory:").await.unwrap();
    migrate(&pool, Dialect::Sqlite).await.unwrap();
    let seed = seed(&pool).await;
    (pool, seed)
}

/// Live-Postgres pool when Worker 3's URL is available; `None` skips
/// the PG tests. The URL is never printed — only its redacted form.
async fn pg_pool() -> Option<(AnyPool, Seed)> {
    let url = std::env::var("DECOR_POS_TEST_PG_URL").ok().or_else(|| {
        std::fs::read_to_string("/tmp/decor-pos-pg-url.txt")
            .ok()
            .map(|s| s.trim().to_string())
    })?;
    if url.is_empty() {
        return None;
    }
    eprintln!("PG test using {}", df_db::redact_db_url(&url));
    let pool = connect(&url).await.expect("connect pg");
    migrate(&pool, Dialect::Postgres).await.expect("migrate pg");
    let seed = seed(&pool).await;
    Some((pool, seed))
}

/// One cash sale: transaction + inventory delta + AR charge, as the
/// edge enqueues them (enriched payloads).
fn sale_batch(seed: &Seed, tx_id: Uuid) -> DeltaBatch {
    DeltaBatch::new(
        seed.store_id,
        vec![
            OutboxEntry::new(
                seed.store_id,
                "transaction",
                tx_id,
                serde_json::json!({
                    "id": tx_id.to_string(),
                    "doc_number": "TX-000001",
                    "doc_type": "sale",
                    "status": "completed",
                    "customer_id": seed.customer_id.to_string(),
                    "subtotal": "200.0000",
                    "tax_total": "16.1088",
                    "eco_fee_total": "0.0000",
                    "pos_fee_total": "0.0000",
                    "grand_total": "216.1088",
                }),
            ),
            OutboxEntry::new(
                seed.store_id,
                "inventory_ledger",
                Uuid::now_v7(),
                serde_json::json!({
                    "product_id": seed.product_id.to_string(),
                    "delta_on_hand": "-2",
                    "delta_committed": "0",
                    "delta_on_order": "0",
                    "delta_in_transit": "0",
                    "unit_cost": "50.0000",
                    "lot_number": null,
                    "reason_type": "sale",
                    "reference_id": tx_id.to_string(),
                }),
            ),
            OutboxEntry::new(
                seed.store_id,
                "ar_ledger",
                Uuid::now_v7(),
                serde_json::json!({
                    "customer_id": seed.customer_id.to_string(),
                    "amount": "216.1088",
                    "entry_type": "charge",
                    "reference_type": "transaction",
                    "reference_id": tx_id.to_string(),
                    "due_date": "2026-12-26",
                }),
            ),
        ],
    )
}

async fn assert_sale_applied(pool: &AnyPool, seed: &Seed, tx_id: Uuid) {
    let tx = df_db::get_transaction(pool, tx_id).await.unwrap();
    assert_eq!(tx.doc_number, "TX-000001");
    assert_eq!(tx.doc_type, "sale");
    assert_eq!(tx.status, "completed");
    assert_eq!(tx.grand_total, dec("216.1088"));
    assert_eq!(tx.subtotal, dec("200.0000"));
    assert_eq!(tx.currency, "USD");
    assert_eq!(
        df_db::inventory_on_hand(pool, seed.store_id, seed.product_id)
            .await
            .unwrap(),
        dec("-2")
    );
    assert_eq!(
        df_db::ar_balance_for_customer(pool, seed.customer_id)
            .await
            .unwrap(),
        dec("216.1088")
    );
}

async fn apply_and_assert_sale(pool: &AnyPool, seed: &Seed) {
    let hq = DbHq::new(pool.clone());
    let tx_id = Uuid::now_v7();
    let batch = sale_batch(seed, tx_id);
    let report = hq.apply_batch(&batch).await.unwrap();
    assert_eq!(report.applied, 3);
    assert_eq!(report.duplicates, 0);
    assert_eq!(report.conflicts, 0);
    assert_eq!(report.applied_ids.len(), 3);
    assert_sale_applied(pool, seed, tx_id).await;

    // Replaying the identical batch is a no-op: no duplicate rows,
    // no double-counted money.
    let replay = hq.apply_batch(&batch).await.unwrap();
    assert_eq!(replay.applied, 0);
    assert_eq!(replay.duplicates, 3);
    assert_eq!(replay.conflicts, 0);
    assert_eq!(replay.applied_ids.len(), 3);
    assert_sale_applied(pool, seed, tx_id).await;

    // A conflicting re-send (same transaction id, different total,
    // new entry id) is skipped, recorded, and never overwrites the
    // stored row.
    let bad = DeltaBatch::new(
        seed.store_id,
        vec![OutboxEntry::new(
            seed.store_id,
            "transaction",
            tx_id,
            serde_json::json!({
                "id": tx_id.to_string(),
                "doc_number": "TX-000001",
                "doc_type": "sale",
                "status": "completed",
                "customer_id": seed.customer_id.to_string(),
                "subtotal": "200.0000",
                "tax_total": "16.1088",
                "eco_fee_total": "0.0000",
                "pos_fee_total": "0.0000",
                "grand_total": "999.0000",
            }),
        )],
    );
    let report = hq.apply_batch(&bad).await.unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.duplicates, 0);
    assert_eq!(report.conflicts, 1);
    assert!(report.applied_ids.is_empty());
    assert_sale_applied(pool, seed, tx_id).await;
}

#[tokio::test]
async fn db_hq_applies_sale_replay_is_noop_conflict_skipped_sqlite() {
    let (pool, seed) = sqlite_pool().await;
    apply_and_assert_sale(&pool, &seed).await;
}

#[tokio::test]
async fn db_hq_unknown_store_fails_whole_batch() {
    let (pool, seed) = sqlite_pool().await;
    let hq = DbHq::new(pool);
    let mut batch = sale_batch(&seed, Uuid::now_v7());
    batch.store_id = Uuid::now_v7();
    assert!(hq.apply_batch(&batch).await.is_err());
}

#[tokio::test]
async fn db_hq_unknown_aggregate_type_is_applied_and_ignored() {
    let (pool, seed) = sqlite_pool().await;
    let hq = DbHq::new(pool);
    let batch = DeltaBatch::new(
        seed.store_id,
        vec![OutboxEntry::new(
            seed.store_id,
            "tint_formula",
            Uuid::now_v7(),
            serde_json::json!({"id": "x"}),
        )],
    );
    let report: ApplyReport = hq.apply_batch(&batch).await.unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.duplicates, 0);
    // Not retried forever: the replay is a duplicate.
    let replay = hq.apply_batch(&batch).await.unwrap();
    assert_eq!(replay.applied, 0);
    assert_eq!(replay.duplicates, 1);
}

#[tokio::test]
async fn db_hq_applies_sale_replay_is_noop_conflict_skipped_pg() {
    let Some((pool, seed)) = pg_pool().await else {
        eprintln!("SKIP db_hq PG test: no live Postgres URL");
        return;
    };
    apply_and_assert_sale(&pool, &seed).await;
}
