//! SQLite round-trip tests for the Phase 1 tables (spec §5.5).
//!
//! Covers: transaction + lines round-trip, inventory ledger deltas
//! and derived on-hand balance, till session open/close, payment
//! rows, sync-state upsert, and transactional (all-or-nothing) writes.

use chrono::Utc;
use df_db::models::{
    InventoryLedgerEntry, Organization, PaymentRow, Product, Store, SyncStateRow, TillSession,
    Transaction, TransactionLine,
};
use df_db::{Dialect, connect, migrate};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

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
        enabled_modules: serde_json::json!({"core_pos": true}),
        settings_json: serde_json::json!({}),
        created_at: Utc::now(),
    }
}

fn store(org_id: Uuid) -> Store {
    Store {
        id: Uuid::now_v7(),
        org_id,
        code: "MAIN".to_string(),
        name: "Main Store".to_string(),
        country_code: "US".to_string(),
        region_code: "CA".to_string(),
        postal_code: "90210".to_string(),
        currency: "USD".to_string(),
        tax_zone_id: Uuid::now_v7(),
        timezone: "America/Los_Angeles".to_string(),
        is_warehouse: false,
    }
}

fn product(org_id: Uuid) -> Product {
    Product {
        id: Uuid::now_v7(),
        org_id,
        sku: "BM-REGAL-1G".to_string(),
        manufacturer_code: Some("BM".to_string()),
        name: "Regal Select Interior Gallon".to_string(),
        department_id: Uuid::now_v7(),
        class_id: None,
        product_line_id: None,
        primary_vendor_id: None,
        product_type: "paint_base".to_string(),
        base_uom: "gal".to_string(),
        list_price: dec("64.9900"),
        standard_cost: dec("38.5000"),
        avg_cost: dec("38.5000"),
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
    }
}

fn transaction(org_id: Uuid, store_id: Uuid, till_id: Option<Uuid>) -> Transaction {
    Transaction {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        till_session_id: till_id,
        doc_number: "TX-000001".to_string(),
        doc_type: "sale".to_string(),
        status: "completed".to_string(),
        customer_id: None,
        job_id: None,
        po_number: None,
        salesperson_id: None,
        cashier_id: None,
        currency: "USD".to_string(),
        subtotal: dec("129.9800"),
        tax_total: dec("9.4236"),
        eco_fee_total: dec("1.5000"),
        pos_fee_total: dec("0.0000"),
        grand_total: dec("140.9036"),
        deposit_paid: dec("0.0000"),
        fulfillment_mode: "counter".to_string(),
        scheduled_at: None,
        completed_at: Some(Utc::now()),
        created_at: Utc::now(),
        job_name: None,
    }
}

#[tokio::test]
async fn transaction_and_lines_round_trip() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();
    let product = product(org.id);
    df_db::insert_product(&pool, &product).await.unwrap();

    let tx = transaction(org.id, store.id, None);
    df_db::insert_transaction(&pool, &tx).await.unwrap();
    let line = TransactionLine {
        id: Uuid::now_v7(),
        transaction_id: tx.id,
        line_no: 1,
        product_id: product.id,
        description: "Regal Select Interior Gallon".to_string(),
        qty: dec("2"),
        uom_code: "gal".to_string(),
        unit_price: dec("64.9900"),
        base_unit_cost: dec("38.5000"),
        tint_unit_cost: dec("0.0000"),
        discount_amount: dec("0.0000"),
        tax_amount: dec("9.4236"),
        eco_fee_amount: dec("1.5000"),
        line_total: dec("129.9800"),
        tint_formula_id: None,
        prompts_json: serde_json::json!({}),
        vertical_spec_json: serde_json::json!({}),
        linked_po_line_id: None,
        fulfilled_qty: dec("2"),
    };
    df_db::insert_transaction_line(&pool, &line).await.unwrap();

    let back = df_db::get_transaction(&pool, tx.id).await.unwrap();
    assert_eq!(back.doc_number, "TX-000001");
    assert_eq!(back.grand_total, dec("140.9036"));
    let lines = df_db::list_transaction_lines(&pool, tx.id).await.unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].qty, dec("2"));
    assert_eq!(lines[0].line_total, dec("129.9800"));
    assert_eq!(
        df_db::count_transactions_for_store(&pool, store.id)
            .await
            .unwrap(),
        1
    );

    df_db::set_transaction_status(&pool, tx.id, "voided")
        .await
        .unwrap();
    assert_eq!(
        df_db::get_transaction(&pool, tx.id).await.unwrap().status,
        "voided"
    );
}

#[tokio::test]
async fn inventory_ledger_derives_on_hand() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();
    let product = product(org.id);
    df_db::insert_product(&pool, &product).await.unwrap();

    let entry = |delta: &str| InventoryLedgerEntry {
        id: Uuid::now_v7(),
        org_id: org.id,
        store_id: store.id,
        product_id: product.id,
        lot_number: None,
        delta_on_hand: dec(delta),
        delta_committed: dec("0"),
        delta_on_order: dec("0"),
        delta_in_transit: dec("0"),
        unit_cost: dec("38.5000"),
        reason_type: "sale".to_string(),
        reference_id: Uuid::now_v7(),
        created_at: Utc::now(),
    };
    df_db::insert_inventory_delta(&pool, &entry("10"))
        .await
        .unwrap();
    df_db::insert_inventory_delta(&pool, &entry("-2"))
        .await
        .unwrap();
    df_db::insert_inventory_delta(&pool, &entry("-3"))
        .await
        .unwrap();
    assert_eq!(
        df_db::inventory_on_hand(&pool, store.id, product.id)
            .await
            .unwrap(),
        dec("5")
    );
    // Unknown product: zero, not an error.
    assert_eq!(
        df_db::inventory_on_hand(&pool, store.id, Uuid::now_v7())
            .await
            .unwrap(),
        dec("0")
    );
}

#[tokio::test]
async fn till_session_open_and_close() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();

    let till = TillSession {
        id: Uuid::now_v7(),
        org_id: org.id,
        store_id: store.id,
        cashier_id: Uuid::now_v7(),
        opened_at: Utc::now(),
        closed_at: None,
        opening_float: dec("150.0000"),
        counted_cash: None,
        expected_cash: None,
        status: "open".to_string(),
    };
    df_db::insert_till_session(&pool, &till).await.unwrap();
    let back = df_db::get_till_session(&pool, till.id).await.unwrap();
    assert_eq!(back.status, "open");
    assert_eq!(back.opening_float, dec("150.0000"));

    df_db::close_till_session(&pool, till.id, Utc::now(), dec("162.5000"), dec("160.0000"))
        .await
        .unwrap();
    let back = df_db::get_till_session(&pool, till.id).await.unwrap();
    assert_eq!(back.status, "closed");
    assert_eq!(back.counted_cash, Some(dec("162.5000")));
    assert_eq!(back.expected_cash, Some(dec("160.0000")));
    assert!(back.closed_at.is_some());
}

#[tokio::test]
async fn payments_and_sync_state_round_trip() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();
    let tx = transaction(org.id, store.id, None);
    df_db::insert_transaction(&pool, &tx).await.unwrap();

    let payment = PaymentRow {
        id: Uuid::now_v7(),
        transaction_id: tx.id,
        tender: "card".to_string(),
        amount: dec("140.9036"),
        reference: "MOCK-abc123".to_string(),
        authorization: Some("AUTH-1".to_string()),
        created_at: Utc::now(),
    };
    df_db::insert_payment(&pool, &payment).await.unwrap();
    let payments = df_db::list_payments_for_transaction(&pool, tx.id)
        .await
        .unwrap();
    assert_eq!(payments.len(), 1);
    assert_eq!(payments[0].tender, "card");
    assert_eq!(payments[0].amount, dec("140.9036"));

    assert!(
        df_db::get_sync_state(&pool, store.id)
            .await
            .unwrap()
            .is_none()
    );
    let batch_id = Uuid::now_v7();
    df_db::upsert_sync_state(
        &pool,
        &SyncStateRow {
            store_id: store.id,
            last_batch_id: Some(batch_id),
            last_synced_at: Some(Utc::now()),
            pending_count: 0,
        },
    )
    .await
    .unwrap();
    let back = df_db::get_sync_state(&pool, store.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.last_batch_id, Some(batch_id));
    assert_eq!(back.pending_count, 0);
    // Upsert replaces.
    df_db::upsert_sync_state(
        &pool,
        &SyncStateRow {
            store_id: store.id,
            last_batch_id: None,
            last_synced_at: None,
            pending_count: 3,
        },
    )
    .await
    .unwrap();
    let back = df_db::get_sync_state(&pool, store.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.pending_count, 3);
    assert!(back.last_batch_id.is_none());
}

#[tokio::test]
async fn transactional_writes_are_atomic() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store = store(org.id);
    df_db::insert_store(&pool, &store).await.unwrap();

    // Commit path: tx + line + inventory delta land together.
    let tx = transaction(org.id, store.id, None);
    let mut sql_tx = pool.begin().await.unwrap();
    df_db::insert_transaction(&mut *sql_tx, &tx).await.unwrap();
    df_db::insert_inventory_delta(
        &mut *sql_tx,
        &InventoryLedgerEntry {
            id: Uuid::now_v7(),
            org_id: org.id,
            store_id: store.id,
            product_id: Uuid::now_v7(),
            lot_number: None,
            delta_on_hand: dec("-1"),
            delta_committed: dec("0"),
            delta_on_order: dec("0"),
            delta_in_transit: dec("0"),
            unit_cost: dec("38.5000"),
            reason_type: "sale".to_string(),
            reference_id: tx.id,
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    sql_tx.commit().await.unwrap();
    assert!(df_db::get_transaction(&pool, tx.id).await.is_ok());

    // Rollback path: nothing lands.
    let tx2 = transaction(org.id, store.id, None);
    let mut sql_tx = pool.begin().await.unwrap();
    df_db::insert_transaction(&mut *sql_tx, &tx2).await.unwrap();
    sql_tx.rollback().await.unwrap();
    assert!(df_db::get_transaction(&pool, tx2.id).await.is_err());
}
