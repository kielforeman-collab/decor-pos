//! Phase 3 exit test (AR core): the month-end statement batch.
//!
//! Seeds two charge accounts — A with an overdue balance, a
//! partial payment and a current charge; B current-only — runs
//! `generate_statements` for October 2026 and asserts, per
//! statement, that opening + charges − payments + finance charges
//! == closing exactly, that the aging buckets match the ledger,
//! and that re-running the batch for the same period changes
//! nothing and double-posts no finance charge. Also asserts that
//! a charge sale over the credit limit (and one on credit hold)
//! is rejected and posts nothing to `ar_ledger`, and that an AR
//! payment allocates FIFO.

use chrono::{NaiveDate, TimeZone, Utc};
use df_db::models::{ArAllocation, ArLedgerEntry, Customer, Organization, Product, Store};
use df_domain::{Currency, Money};
use df_edge::{Edge, EdgeConfig, MockTerminal, SaleInput, SaleLineInput, TenderInput, TenderKind};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn money(s: &str) -> Money {
    Money::from_str_amount(s, Currency::Usd).unwrap()
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;
const CUSTOMER_A: u128 = 0x4001;
const CUSTOMER_B: u128 = 0x4002;
const CUSTOMER_C: u128 = 0x4003;
const CUSTOMER_D: u128 = 0x4004;

fn customer(id: Uuid, org_id: Uuid, number: &str, limit: &str, hold: bool) -> Customer {
    Customer {
        id,
        org_id,
        account_number: number.to_string(),
        customer_type: "contractor".to_string(),
        company_name: Some(format!("{number} Painting")),
        first_name: None,
        last_name: None,
        email: None,
        phone: None,
        price_level_id: None,
        salesperson_id: None,
        tax_exempt_cert: None,
        tax_exempt_expires: None,
        charge_enabled: true,
        credit_limit: dec(limit),
        payment_terms: "NET30".to_string(),
        terms_days: None,
        finance_rate_pct_monthly: Some(dec("1.500")),
        credit_hold: hold,
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

fn ar_entry(
    org_id: Uuid,
    customer_id: Uuid,
    kind: &str,
    amount: &str,
    due: Option<NaiveDate>,
    created: chrono::DateTime<Utc>,
) -> ArLedgerEntry {
    ArLedgerEntry {
        id: Uuid::now_v7(),
        org_id,
        store_id: Uuid::from_u128(STORE_ID),
        customer_id,
        job_id: None,
        entry_type: kind.to_string(),
        reference_type: "transaction".to_string(),
        reference_id: Uuid::now_v7(),
        amount: dec(amount),
        due_date: due,
        statement_id: None,
        created_at: created,
    }
}

async fn test_edge() -> (Edge, Uuid) {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    let edge = Edge::open(config).await.unwrap();
    let pool = edge.pool();
    df_db::insert_organization(
        pool,
        &Organization {
            id: Uuid::from_u128(ORG_ID),
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
            id: Uuid::from_u128(STORE_ID),
            org_id: Uuid::from_u128(ORG_ID),
            code: "MAIN".to_string(),
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
            id: Uuid::from_u128(PRODUCT_ID),
            org_id: Uuid::from_u128(ORG_ID),
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
    (edge, Uuid::from_u128(ORG_ID))
}

async fn seed_ledger(edge: &Edge, org_id: Uuid) {
    let pool = edge.pool();
    // Account A: overdue + partial payment + current charge.
    df_db::insert_customer(
        pool,
        &customer(Uuid::from_u128(CUSTOMER_A), org_id, "C-A", "5000", false),
    )
    .await
    .unwrap();
    // Account B: one current charge only.
    df_db::insert_customer(
        pool,
        &customer(Uuid::from_u128(CUSTOMER_B), org_id, "C-B", "5000", false),
    )
    .await
    .unwrap();
    // Account C: $100 limit — the over-limit checkout target.
    df_db::insert_customer(
        pool,
        &customer(Uuid::from_u128(CUSTOMER_C), org_id, "C-C", "100", false),
    )
    .await
    .unwrap();
    // Account D: on credit hold.
    df_db::insert_customer(
        pool,
        &customer(Uuid::from_u128(CUSTOMER_D), org_id, "C-D", "5000", true),
    )
    .await
    .unwrap();

    let a = Uuid::from_u128(CUSTOMER_A);
    let old_charge = ar_entry(
        org_id,
        a,
        "charge",
        "400.0000",
        Some(date(2026, 9, 14)),
        Utc.with_ymd_and_hms(2026, 8, 15, 0, 0, 0).unwrap(),
    );
    df_db::insert_ar_entry(pool, &old_charge).await.unwrap();
    let payment = ar_entry(
        org_id,
        a,
        "payment",
        "-150.0000",
        None,
        Utc.with_ymd_and_hms(2026, 10, 20, 0, 0, 0).unwrap(),
    );
    df_db::insert_ar_entry(pool, &payment).await.unwrap();
    df_db::insert_ar_allocation(
        pool,
        &ArAllocation {
            id: Uuid::now_v7(),
            payment_entry_id: payment.id,
            charge_entry_id: old_charge.id,
            allocated_amount: dec("150.0000"),
            created_at: Utc.with_ymd_and_hms(2026, 10, 20, 0, 0, 0).unwrap(),
        },
    )
    .await
    .unwrap();
    df_db::insert_ar_entry(
        pool,
        &ar_entry(
            org_id,
            a,
            "charge",
            "300.0000",
            Some(date(2026, 11, 4)),
            Utc.with_ymd_and_hms(2026, 10, 5, 0, 0, 0).unwrap(),
        ),
    )
    .await
    .unwrap();

    let b = Uuid::from_u128(CUSTOMER_B);
    df_db::insert_ar_entry(
        pool,
        &ar_entry(
            org_id,
            b,
            "charge",
            "100.0000",
            Some(date(2026, 11, 9)),
            Utc.with_ymd_and_hms(2026, 10, 10, 0, 0, 0).unwrap(),
        ),
    )
    .await
    .unwrap();
}

fn charge_sale_input(customer_id: Uuid) -> SaleInput {
    SaleInput {
        customer_id: Some(customer_id),
        job_id: None,
        lines: vec![SaleLineInput {
            product_id: Uuid::from_u128(PRODUCT_ID),
            sku: "BM-REGAL-1G".to_string(),
            description: "Regal Select Interior Gallon".to_string(),
            qty: dec("1"),
            uom: "gal".to_string(),
            unit_price: money("100.0000"),
            unit_cost: money("50.0000"),
        }],
        tenders: vec![TenderInput {
            kind: TenderKind::ChargeAccount,
            amount: None,
            reference: None,
        }],
        po_number: None,
        job_name: None,
        price_level_id: None,
    }
}

#[tokio::test]
async fn statement_batch_balances_age_and_is_idempotent() {
    let (edge, org_id) = test_edge().await;
    seed_ledger(&edge, org_id).await;
    let pool = edge.pool();
    let a = Uuid::from_u128(CUSTOMER_A);
    let b = Uuid::from_u128(CUSTOMER_B);

    let start = date(2026, 10, 1);
    let end = date(2026, 10, 31);
    let statements = edge.generate_statements(start, end).await.unwrap();
    assert_eq!(statements.len(), 4, "one statement per charge account");
    let stmt_a = statements.iter().find(|s| s.customer_id == a).unwrap();
    let stmt_b = statements.iter().find(|s| s.customer_id == b).unwrap();

    // Account A: opening 400 (Aug charge) + charges 300 (Oct 5) −
    // payments 150 (Oct 20) + finance 3.75 (1.5% of the $250
    // overdue balance) = closing 553.75.
    assert!(stmt_a.check_identity().is_ok());
    assert_eq!(stmt_a.opening_balance, money("400"));
    assert_eq!(stmt_a.charges_total, money("300"));
    assert_eq!(stmt_a.payments_total, money("150"));
    assert_eq!(stmt_a.finance_charge_total, money("3.75"));
    assert_eq!(stmt_a.closing_balance, money("553.75"));
    // Aging as of Oct 31: current = Oct charge 300 + finance
    // charge 3.75 (due Oct 31); 31-60 = the $250 still open on the
    // Aug charge (due Sep 14, 47 days past due).
    assert_eq!(stmt_a.aging.current, money("303.75"));
    assert_eq!(stmt_a.aging.days_31_60, money("250"));
    assert_eq!(stmt_a.aging.days_61_90, money("0"));
    assert_eq!(stmt_a.aging.days_91_120, money("0"));
    assert_eq!(stmt_a.aging.over_120, money("0"));
    assert_eq!(stmt_a.aging.total().unwrap(), money("553.75"));
    // Three lines in the period: Oct charge, Oct payment, finance
    // charge.
    assert_eq!(stmt_a.lines.len(), 3);

    // Account B: current only, no finance charge.
    assert!(stmt_b.check_identity().is_ok());
    assert_eq!(stmt_b.opening_balance, money("0"));
    assert_eq!(stmt_b.charges_total, money("100"));
    assert_eq!(stmt_b.payments_total, money("0"));
    assert_eq!(stmt_b.finance_charge_total, money("0"));
    assert_eq!(stmt_b.closing_balance, money("100"));
    assert_eq!(stmt_b.aging.current, money("100"));
    assert_eq!(stmt_b.aging.total().unwrap(), money("100"));

    // Ledger agrees with the statements.
    assert_eq!(
        df_db::ar_balance_for_customer(pool, a).await.unwrap(),
        dec("553.7500")
    );
    assert_eq!(
        df_db::ar_balance_for_customer(pool, b).await.unwrap(),
        dec("100.0000")
    );

    // Re-run the batch for the same period: frozen statements,
    // same ids, same balances, no second finance charge.
    let rerun = edge.generate_statements(start, end).await.unwrap();
    let stmt_a2 = rerun.iter().find(|s| s.customer_id == a).unwrap();
    let stmt_b2 = rerun.iter().find(|s| s.customer_id == b).unwrap();
    assert_eq!(stmt_a2.id, stmt_a.id);
    assert_eq!(stmt_b2.id, stmt_b.id);
    assert_eq!(stmt_a2.closing_balance, stmt_a.closing_balance);
    assert_eq!(stmt_b2.closing_balance, stmt_b.closing_balance);
    let finance_entries = df_db::list_ar_entries_for_customer(pool, a)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.entry_type == "finance_charge")
        .collect::<Vec<_>>();
    assert_eq!(finance_entries.len(), 1, "no double-posted finance charge");
    assert_eq!(finance_entries[0].amount, dec("3.7500"));
    assert_eq!(
        df_db::ar_balance_for_customer(pool, a).await.unwrap(),
        dec("553.7500"),
        "re-running the batch must not change balances"
    );

    // A standalone finance-charge run for the same period is also
    // a no-op.
    let posted = edge
        .assess_finance_charges(end, dec("1.500"))
        .await
        .unwrap();
    assert!(posted.is_empty());
}

#[tokio::test]
async fn charge_sale_over_limit_and_on_hold_are_rejected() {
    let (edge, org_id) = test_edge().await;
    seed_ledger(&edge, org_id).await;
    let pool = edge.pool();

    // Account C: $100 limit; a $100 gallon rings up at $108.0544
    // with tax/eco, so the charge would reach past the limit.
    let err = edge
        .checkout(
            charge_sale_input(Uuid::from_u128(CUSTOMER_C)),
            &MockTerminal::new(),
        )
        .await
        .expect_err("over-limit charge must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("credit limit"),
        "error must name the limit: {message}"
    );
    assert!(
        df_db::list_ar_entries_for_customer(pool, Uuid::from_u128(CUSTOMER_C))
            .await
            .unwrap()
            .is_empty(),
        "rejected charge posts nothing to ar_ledger"
    );
    assert_eq!(
        df_db::ar_balance_for_customer(pool, Uuid::from_u128(CUSTOMER_C))
            .await
            .unwrap(),
        dec("0")
    );

    // Account D is on credit hold: any charge is rejected.
    let err = edge
        .checkout(
            charge_sale_input(Uuid::from_u128(CUSTOMER_D)),
            &MockTerminal::new(),
        )
        .await
        .expect_err("on-hold charge must be rejected");
    assert!(
        err.to_string().contains("credit hold"),
        "error must name the hold: {err}"
    );
    assert!(
        df_db::list_ar_entries_for_customer(pool, Uuid::from_u128(CUSTOMER_D))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn ar_payment_allocates_fifo() {
    let (edge, org_id) = test_edge().await;
    seed_ledger(&edge, org_id).await;
    let a = Uuid::from_u128(CUSTOMER_A);
    edge.generate_statements(date(2026, 10, 1), date(2026, 10, 31))
        .await
        .unwrap();

    // Open charges for A: Aug charge $250 (due Sep 14), finance
    // charge $3.75 (due Oct 31), Oct charge $300 (due Nov 4).
    // A $100 payment goes entirely to the oldest charge.
    let posted = edge
        .post_ar_payment(a, money("100"), "check")
        .await
        .unwrap();
    assert_eq!(posted.allocations.len(), 1);
    assert_eq!(posted.allocations[0].1, dec("100.0000"));
    let open = edge.open_charges_for(a).await.unwrap();
    assert_eq!(
        open[0].open_amount,
        dec("150.0000"),
        "Aug charge reduced FIFO"
    );
    assert_eq!(
        df_db::ar_balance_for_customer(edge.pool(), a)
            .await
            .unwrap(),
        dec("453.7500")
    );
    // The payment entry and its allocations are in the ledger.
    let entries = df_db::list_ar_entries_for_customer(edge.pool(), a)
        .await
        .unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.id == posted.payment_entry_id && e.entry_type == "payment")
    );
    assert_eq!(
        df_db::list_allocations_for_payment(edge.pool(), posted.payment_entry_id)
            .await
            .unwrap()
            .len(),
        1
    );
}
