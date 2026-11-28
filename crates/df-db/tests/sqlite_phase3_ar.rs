//! Phase 3 `df-db` repositories (migration `0005_phase3`):
//! customer credit columns, customer jobs, AR allocations,
//! statements, inventory transfers, physical counts and reorder
//! points — all against the SQLite edge schema.

use chrono::{NaiveDate, Utc};
use df_db::models::{
    ArAllocation, ArLedgerEntry, CountLine, Customer, CustomerJob, InventoryTransfer, Organization,
    PhysicalCount, ReorderPoint, StatementRow, TransferLine,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn test_pool() -> df_db::AnyPool {
    let pool = df_db::connect("sqlite::memory:").await.unwrap();
    df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
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

fn customer(org_id: Uuid, number: &str) -> Customer {
    Customer {
        id: Uuid::now_v7(),
        org_id,
        account_number: number.to_string(),
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
        credit_limit: dec("5000.0000"),
        payment_terms: "NET30".to_string(),
        terms_days: Some(30),
        finance_rate_pct_monthly: Some(dec("1.500")),
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
    }
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

#[tokio::test]
async fn credit_columns_round_trip_and_update() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let cust = customer(org.id, "C-1001");
    df_db::insert_customer(&pool, &cust).await.unwrap();
    let back = df_db::get_customer(&pool, cust.id).await.unwrap();
    assert_eq!(back.terms_days, Some(30));
    assert_eq!(back.finance_rate_pct_monthly, Some(dec("1.500")));
    assert_eq!(back.credit_limit, dec("5000.0000"));

    // Update: new limit, clear the terms override, set a hold.
    df_db::update_customer_credit(
        &pool,
        cust.id,
        dec("2500.0000"),
        None,
        Some(dec("2.000")),
        true,
    )
    .await
    .unwrap();
    let back = df_db::get_customer(&pool, cust.id).await.unwrap();
    assert_eq!(back.credit_limit, dec("2500.0000"));
    assert_eq!(back.terms_days, None);
    assert_eq!(back.finance_rate_pct_monthly, Some(dec("2.000")));
    assert!(back.credit_hold);

    // Charge-enabled listing finds the account.
    let charge = df_db::list_charge_customers(&pool, org.id).await.unwrap();
    assert_eq!(charge.len(), 1);
    assert_eq!(charge[0].account_number, "C-1001");
}

#[tokio::test]
async fn customer_jobs_round_trip() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let cust = customer(org.id, "C-1001");
    df_db::insert_customer(&pool, &cust).await.unwrap();
    let job = CustomerJob {
        id: Uuid::now_v7(),
        customer_id: cust.id,
        job_code: "JOB-1".to_string(),
        job_name: "Downtown Suites repaint".to_string(),
        site_address_json: serde_json::json!({"street": "1174 W Pender"}),
        tax_zone_override: None,
        is_active: true,
    };
    df_db::insert_customer_job(&pool, &job).await.unwrap();
    let back = df_db::get_customer_job(&pool, job.id).await.unwrap();
    assert_eq!(back.job_name, "Downtown Suites repaint");
    assert_eq!(
        df_db::list_jobs_for_customer(&pool, cust.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

fn ar_entry(
    org_id: Uuid,
    store_id: Uuid,
    customer_id: Uuid,
    kind: &str,
    amount: &str,
    due: Option<NaiveDate>,
) -> ArLedgerEntry {
    ArLedgerEntry {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        customer_id,
        job_id: None,
        entry_type: kind.to_string(),
        reference_type: "transaction".to_string(),
        reference_id: Uuid::now_v7(),
        amount: dec(amount),
        due_date: due,
        statement_id: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn allocations_and_reference_lookup_round_trip() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let cust = customer(org.id, "C-1001");
    df_db::insert_customer(&pool, &cust).await.unwrap();
    let store_id = Uuid::now_v7();
    let charge = ar_entry(
        org.id,
        store_id,
        cust.id,
        "charge",
        "400.0000",
        Some(date(2026, 9, 14)),
    );
    df_db::insert_ar_entry(&pool, &charge).await.unwrap();
    let payment = ar_entry(org.id, store_id, cust.id, "payment", "-150.0000", None);
    df_db::insert_ar_entry(&pool, &payment).await.unwrap();
    let alloc = ArAllocation {
        id: Uuid::now_v7(),
        payment_entry_id: payment.id,
        charge_entry_id: charge.id,
        allocated_amount: dec("150.0000"),
        created_at: Utc::now(),
    };
    df_db::insert_ar_allocation(&pool, &alloc).await.unwrap();
    assert_eq!(
        df_db::list_allocations_for_customer(&pool, cust.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        df_db::list_allocations_for_payment(&pool, payment.id)
            .await
            .unwrap()[0]
            .allocated_amount,
        dec("150.0000")
    );

    // Reference lookup: the finance-charge idempotency check.
    let key = Uuid::now_v7();
    let finance = ArLedgerEntry {
        reference_type: "statement_run".to_string(),
        reference_id: key,
        ..ar_entry(
            org.id,
            store_id,
            cust.id,
            "finance_charge",
            "3.7500",
            Some(date(2026, 10, 31)),
        )
    };
    df_db::insert_ar_entry(&pool, &finance).await.unwrap();
    let found =
        df_db::find_ar_entry_by_reference(&pool, cust.id, "finance_charge", "statement_run", key)
            .await
            .unwrap();
    assert_eq!(found.map(|e| e.id), Some(finance.id));
    assert!(
        df_db::find_ar_entry_by_reference(
            &pool,
            cust.id,
            "finance_charge",
            "statement_run",
            Uuid::now_v7(),
        )
        .await
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn statements_round_trip_and_pdf_hash() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let cust = customer(org.id, "C-1001");
    df_db::insert_customer(&pool, &cust).await.unwrap();
    let row = StatementRow {
        id: Uuid::now_v7(),
        customer_id: cust.id,
        period_start: date(2026, 10, 1),
        period_end: date(2026, 10, 31),
        opening_balance: dec("400.0000"),
        charges_total: dec("300.0000"),
        payments_total: dec("150.0000"),
        finance_charge_total: dec("3.7500"),
        closing_balance: dec("553.7500"),
        aging_json: serde_json::json!({
            "current": "303.7500",
            "31-60": "250.0000",
            "61-90": "0.0000",
            "91-120": "0.0000",
            "120+": "0.0000",
        }),
        pdf_sha256: None,
        created_at: Utc::now(),
    };
    df_db::insert_statement(&pool, &row).await.unwrap();
    let back = df_db::get_statement(&pool, cust.id, date(2026, 10, 1), date(2026, 10, 31))
        .await
        .unwrap()
        .expect("statement row");
    assert_eq!(back.closing_balance, dec("553.7500"));
    assert_eq!(back.aging_json["31-60"], "250.0000");
    assert!(
        df_db::get_statement(&pool, cust.id, date(2026, 11, 1), date(2026, 11, 30))
            .await
            .unwrap()
            .is_none()
    );
    df_db::set_statement_pdf_sha256(&pool, row.id, "abc123")
        .await
        .unwrap();
    let back = df_db::get_statement(&pool, cust.id, date(2026, 10, 1), date(2026, 10, 31))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.pdf_sha256.as_deref(), Some("abc123"));
    assert_eq!(
        df_db::list_statements_for_customer(&pool, cust.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn transfers_counts_and_reorder_points_round_trip() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_a = Uuid::now_v7();
    let store_b = Uuid::now_v7();
    let product_id = Uuid::now_v7();

    let transfer = InventoryTransfer {
        id: Uuid::now_v7(),
        org_id: org.id,
        from_store_id: store_a,
        to_store_id: store_b,
        status: "draft".to_string(),
        reference: Some("TR-1".to_string()),
        created_at: Utc::now(),
    };
    df_db::insert_inventory_transfer(&pool, &transfer)
        .await
        .unwrap();
    df_db::insert_transfer_line(
        &pool,
        &TransferLine {
            id: Uuid::now_v7(),
            transfer_id: transfer.id,
            product_id,
            qty: dec("12.0000"),
            unit_cost: dec("50.0000"),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        df_db::list_transfer_lines(&pool, transfer.id)
            .await
            .unwrap()
            .len(),
        1
    );
    df_db::set_transfer_status(&pool, transfer.id, "shipped")
        .await
        .unwrap();
    assert_eq!(
        df_db::get_inventory_transfer(&pool, transfer.id)
            .await
            .unwrap()
            .status,
        "shipped"
    );
    assert_eq!(
        df_db::list_transfers_for_store(&pool, store_b)
            .await
            .unwrap()
            .len(),
        1
    );

    let count = PhysicalCount {
        id: Uuid::now_v7(),
        store_id: store_a,
        status: "open".to_string(),
        counted_at: None,
        created_at: Utc::now(),
    };
    df_db::insert_physical_count(&pool, &count).await.unwrap();
    df_db::insert_count_line(
        &pool,
        &CountLine {
            id: Uuid::now_v7(),
            count_id: count.id,
            product_id,
            counted_qty: dec("9.0000"),
            system_qty: dec("12.0000"),
            variance_qty: dec("-3.0000"),
            unit_cost: dec("50.0000"),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        df_db::list_count_lines(&pool, count.id).await.unwrap()[0].variance_qty,
        dec("-3.0000")
    );
    df_db::reconcile_physical_count(&pool, count.id, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        df_db::get_physical_count(&pool, count.id)
            .await
            .unwrap()
            .status,
        "reconciled"
    );

    let point = ReorderPoint {
        id: Uuid::now_v7(),
        store_id: store_a,
        product_id,
        min_qty: dec("4.0000"),
        max_qty: dec("24.0000"),
        reorder_qty: dec("12.0000"),
    };
    df_db::upsert_reorder_point(&pool, &point).await.unwrap();
    // Upsert again with new levels: still one row, new values.
    let point2 = ReorderPoint {
        id: Uuid::now_v7(),
        min_qty: dec("6.0000"),
        ..point.clone()
    };
    df_db::upsert_reorder_point(&pool, &point2).await.unwrap();
    let points = df_db::list_reorder_points_for_store(&pool, store_a)
        .await
        .unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].min_qty, dec("6.0000"));
    assert!(
        df_db::get_reorder_point(&pool, store_b, product_id)
            .await
            .unwrap()
            .is_none()
    );
}
