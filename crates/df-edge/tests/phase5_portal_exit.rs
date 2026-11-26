//! Phase 5 exit test (portal + BOPIS + mobile KPIs).
//!
//! Seeds one portal-enabled contractor with two open charges
//! (400.00 + 300.00) and one stored formula, then:
//!   (a) the contractor reorders the formula online — an open
//!       order is created, priced through the matrix path, with
//!       its line linked to the formula;
//!   (b) the contractor pays 150.00 of the statement online — the
//!       mock provider approves, the payment auto-applies FIFO
//!       (oldest charge first), the balance drops 700.00 → 550.00
//!       exactly, a replay of the same idempotency key changes
//!       nothing, and the month-end statement shows the payment
//!       (closing 550.00);
//!   (c) a guest places a BOPIS pickup order (card authorized,
//!       stock committed) and the counter completes the pickup
//!       (on-hand down, committed released, order completed);
//!   (d) the mobile KPIs reflect the day (1 open order, AR open
//!       550.00, no deliveries out).
//! Negative cases: portal-disabled customer is refused, another
//! customer's formula cannot be reordered, a zero payment is
//! refused, and a pickup short on stock is refused.

use chrono::{NaiveDate, TimeZone, Utc};
use df_db::models::{
    ArLedgerEntry, Customer, InventoryLedgerEntry, Organization, Product, Store, TintFormula,
};
use df_domain::{Currency, Money};
use df_edge::{Edge, EdgeConfig};
use df_portal::{BillPayIntent, BopisLine, BopisOrderRequest, Fulfillment, ReorderRequest};
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
const CUSTOMER_ID: u128 = 0x4001;
const OTHER_CUSTOMER_ID: u128 = 0x4002;
const FORMULA_ID: u128 = 0x5001;

fn customer(id: Uuid, org_id: Uuid, number: &str, portal: bool) -> Customer {
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
        credit_limit: dec("5000"),
        payment_terms: "NET30".to_string(),
        terms_days: None,
        finance_rate_pct_monthly: Some(dec("1.500")),
        credit_hold: false,
        require_po_number: false,
        require_job_name: false,
        portal_enabled: portal,
    }
}

async fn test_edge() -> Edge {
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
            is_active: true,
        },
    )
    .await
    .unwrap();
    edge
}

fn ar_charge(org_id: Uuid, customer_id: Uuid, amount: &str, due: NaiveDate) -> ArLedgerEntry {
    ArLedgerEntry {
        id: Uuid::now_v7(),
        org_id,
        store_id: Uuid::from_u128(STORE_ID),
        customer_id,
        job_id: None,
        entry_type: "charge".to_string(),
        reference_type: "transaction".to_string(),
        reference_id: Uuid::now_v7(),
        amount: dec(amount),
        due_date: Some(due),
        statement_id: None,
        created_at: Utc.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap(),
    }
}

#[tokio::test]
async fn portal_reorder_pay_and_bopis_exit() {
    let edge = test_edge().await;
    let pool = edge.pool();
    let org_id = Uuid::from_u128(ORG_ID);
    let customer_id = Uuid::from_u128(CUSTOMER_ID);
    let other_id = Uuid::from_u128(OTHER_CUSTOMER_ID);

    df_db::insert_customer(pool, &customer(customer_id, org_id, "C-PORTAL", true))
        .await
        .unwrap();
    df_db::insert_customer(pool, &customer(other_id, org_id, "C-NOPORTAL", false))
        .await
        .unwrap();
    df_db::insert_ar_entry(
        pool,
        &ar_charge(org_id, customer_id, "400.0000", date(2026, 11, 20)),
    )
    .await
    .unwrap();
    df_db::insert_ar_entry(
        pool,
        &ar_charge(org_id, customer_id, "300.0000", date(2026, 12, 5)),
    )
    .await
    .unwrap();
    let formula_id = Uuid::from_u128(FORMULA_ID);
    df_db::insert_tint_formula(
        pool,
        &TintFormula {
            id: formula_id,
            org_id,
            store_id: Uuid::from_u128(STORE_ID),
            customer_id: Some(customer_id),
            job_id: None,
            color_id: None,
            color_code_snapshot: "OC-17".to_string(),
            color_name_snapshot: "White Dove".to_string(),
            base_product_id: Uuid::from_u128(PRODUCT_ID),
            container_volume_ml: 3785,
            sheen: Some("Eggshell".to_string()),
            room_or_usage: Some("Living Room".to_string()),
            is_custom_match: false,
            colorants_json: serde_json::json!([{"code": "Y3", "oz": 0, "shots": 14}]),
            total_tint_cost: dec("0.6225"),
            tint_surcharge: dec("0"),
            dispenser_name: None,
            external_formula_ref: None,
            dispensed_at: Utc.with_ymd_and_hms(2026, 11, 9, 0, 0, 0).unwrap(),
        },
    )
    .await
    .unwrap();
    // 10 gallons on hand for the BOPIS leg.
    df_db::insert_inventory_delta(
        pool,
        &InventoryLedgerEntry {
            id: Uuid::now_v7(),
            org_id,
            store_id: Uuid::from_u128(STORE_ID),
            product_id: Uuid::from_u128(PRODUCT_ID),
            lot_number: None,
            delta_on_hand: dec("10"),
            delta_committed: Decimal::ZERO,
            delta_on_order: Decimal::ZERO,
            delta_in_transit: Decimal::ZERO,
            unit_cost: dec("50.0000"),
            reason_type: "po_receive".to_string(),
            reference_id: Uuid::now_v7(),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();

    // Negative: portal-disabled customer is refused everywhere.
    assert!(
        edge.portal_customer(other_id).await.is_err(),
        "portal access must require portal_enabled"
    );
    // Negative: another customer's formula cannot be reordered.
    assert!(
        edge.portal_reorder(ReorderRequest {
            customer_id: other_id,
            formula_id,
            qty: dec("1"),
            job_id: None,
            fulfillment: Fulfillment::Pickup,
        })
        .await
        .is_err(),
        "reorder must refuse a formula that is not the customer's"
    );

    // (a) Formula reorder: 2 gallons of White Dove, pickup.
    let reorder = edge
        .portal_reorder(ReorderRequest {
            customer_id,
            formula_id,
            qty: dec("2"),
            job_id: None,
            fulfillment: Fulfillment::Pickup,
        })
        .await
        .unwrap();
    let order = df_db::get_transaction(pool, reorder.order_id)
        .await
        .unwrap();
    assert_eq!(order.doc_type, "order");
    assert_eq!(order.status, "open");
    assert_eq!(order.fulfillment_mode, "pickup");
    let lines = df_db::list_transaction_lines(pool, reorder.order_id)
        .await
        .unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].qty, dec("2"));
    assert_eq!(lines[0].tint_formula_id, Some(formula_id));
    assert_eq!(order.subtotal, dec("200.0000"));
    assert_eq!(
        order.grand_total,
        order.subtotal + order.tax_total + order.eco_fee_total,
        "grand total must equal subtotal + tax + eco exactly"
    );

    // (b) Bill pay: 150.00 against a 700.00 balance, FIFO.
    assert_eq!(
        edge.open_balance_for(customer_id).await.unwrap(),
        money("700.0000"),
        "opening balance is the two charges"
    );
    let paid = edge
        .portal_pay_bill(BillPayIntent {
            customer_id,
            amount: money("150.0000"),
            method: "card".to_string(),
            idempotency_key: "billpay-1".to_string(),
        })
        .await
        .unwrap();
    assert!(!paid.replayed);
    assert_eq!(paid.allocations.len(), 1, "FIFO: oldest charge first");
    assert_eq!(paid.allocations[0].1, dec("150.0000"));
    assert_eq!(
        edge.open_balance_for(customer_id).await.unwrap(),
        money("550.0000"),
        "balance drops by exactly the payment amount"
    );
    // Replay: same key returns the original payment, posts nothing.
    let replay = edge
        .portal_pay_bill(BillPayIntent {
            customer_id,
            amount: money("150.0000"),
            method: "card".to_string(),
            idempotency_key: "billpay-1".to_string(),
        })
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.payment_entry_id, paid.payment_entry_id);
    assert_eq!(
        edge.open_balance_for(customer_id).await.unwrap(),
        money("550.0000"),
        "replay must not double-post"
    );
    // Negative: a zero payment is refused.
    assert!(
        edge.portal_pay_bill(BillPayIntent {
            customer_id,
            amount: money("0.0000"),
            method: "card".to_string(),
            idempotency_key: "billpay-zero".to_string(),
        })
        .await
        .is_err()
    );
    // Statement shows the payment: charges 700 − payments 150 = 550.
    let statements = edge
        .generate_statements(date(2026, 11, 1), date(2026, 11, 30))
        .await
        .unwrap();
    let stmt = statements
        .iter()
        .find(|s| s.customer_id == customer_id)
        .unwrap();
    assert_eq!(stmt.payments_total, money("150.0000"));
    assert_eq!(stmt.closing_balance, money("550.0000"));

    // (c) BOPIS: guest orders 2 gallons for pickup, pays by card.
    let bopis = edge
        .record_bopis_order(BopisOrderRequest {
            lines: vec![BopisLine {
                product_id: Uuid::from_u128(PRODUCT_ID),
                qty: dec("2"),
            }],
            pickup_name: "Dana Driver".to_string(),
            contact: "dana@example.invalid".to_string(),
            scheduled_at: Utc::now() + chrono::Duration::hours(4),
        })
        .await
        .unwrap();
    assert!(
        !bopis.payment_reference.is_empty(),
        "card authorization reference is recorded"
    );
    assert_eq!(
        df_db::inventory_on_hand(pool, Uuid::from_u128(STORE_ID), Uuid::from_u128(PRODUCT_ID))
            .await
            .unwrap(),
        dec("10"),
        "on-hand is untouched until pickup completes"
    );
    let committed: Decimal = df_db::list_inventory_entries_for_product(
        pool,
        Uuid::from_u128(STORE_ID),
        Uuid::from_u128(PRODUCT_ID),
    )
    .await
    .unwrap()
    .iter()
    .map(|e| e.delta_committed)
    .sum();
    assert_eq!(committed, dec("2"), "stock is committed by the order");
    let picked_up = edge.complete_bopis_pickup(bopis.order_id).await.unwrap();
    assert_eq!(picked_up.order.status, "completed");
    assert_eq!(picked_up.lines[0].fulfilled_qty, dec("2"));
    assert_eq!(
        df_db::inventory_on_hand(pool, Uuid::from_u128(STORE_ID), Uuid::from_u128(PRODUCT_ID))
            .await
            .unwrap(),
        dec("8"),
        "pickup moves stock to sold"
    );
    // Negative: a pickup short on stock is refused, order stays open.
    let short = edge
        .record_bopis_order(BopisOrderRequest {
            lines: vec![BopisLine {
                product_id: Uuid::from_u128(PRODUCT_ID),
                qty: dec("9"),
            }],
            pickup_name: "Short Buyer".to_string(),
            contact: "short@example.invalid".to_string(),
            scheduled_at: Utc::now() + chrono::Duration::hours(4),
        })
        .await
        .unwrap();
    assert!(
        edge.complete_bopis_pickup(short.order_id).await.is_err(),
        "pickup must refuse when on-hand is short"
    );
    let still_open = df_db::get_transaction(pool, short.order_id).await.unwrap();
    assert_eq!(still_open.status, "open");

    // (d) Mobile KPIs reflect the day.
    let kpis = edge.mobile_kpis(Uuid::from_u128(STORE_ID)).await.unwrap();
    assert_eq!(
        kpis.open_orders, 2,
        "reorder + short BOPIS order still open"
    );
    assert_eq!(kpis.ar_open_total, money("550.0000"));
    assert_eq!(
        kpis.sales_today_count, 0,
        "no completed counter sales today"
    );
    assert_eq!(kpis.deliveries_out_today, 0);
}
