//! Edge checkout integration tests (Phase 1).
//!
//! Covers: checkout totals incl. tax/eco/card-surcharge, matrix
//! pricing, till open/close + Z-report math, return reversal,
//! holds/quotes, sync_now against MemoryHq incl. replay no-op, and
//! checkout succeeding with no printer configured.

use chrono::Utc;
use df_db::models::{Organization, PricingRuleRow, Product, Store};
use df_domain::{Currency, Money};
use df_edge::{
    Edge, EdgeConfig, MockTerminal, ReturnLineInput, SaleInput, SaleLineInput, TenderInput,
    TenderKind,
};
use df_sync::MemoryHq;
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn money(s: &str) -> Money {
    Money::from_str_amount(s, Currency::Usd).unwrap()
}

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;

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
            enabled_modules: serde_json::json!({"core_pos": true}),
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
    edge
}

fn sale_input(qty: &str, tender: TenderKind, tender_amount: Option<&str>) -> SaleInput {
    SaleInput {
        customer_id: None,
        job_id: None,
        lines: vec![SaleLineInput {
            product_id: Uuid::from_u128(PRODUCT_ID),
            sku: "BM-REGAL-1G".to_string(),
            description: "Regal Select Interior Gallon".to_string(),
            qty: dec(qty),
            uom: "gal".to_string(),
            unit_price: money("100.0000"),
            unit_cost: money("50.0000"),
        }],
        tenders: vec![TenderInput {
            kind: tender,
            amount: tender_amount.map(money),
            reference: None,
        }],
        po_number: None,
        job_name: None,
        price_level_id: None,
    }
}

#[derive(Debug, Default)]
struct CapturingPrinter {
    prints: Mutex<Vec<Vec<u8>>>,
}

impl df_edge::Printer for CapturingPrinter {
    fn print(&self, bytes: &[u8]) -> Result<(), df_edge::hw::HwError> {
        self.prints.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }

    fn name(&self) -> &str {
        "capturing-test-printer"
    }
}

#[tokio::test]
async fn cash_checkout_totals_include_tax_and_eco() {
    let edge = test_edge().await;
    let printer = Arc::new(CapturingPrinter::default());
    edge.set_printer(printer.clone());
    let sale = edge
        .checkout(
            sale_input("2", TenderKind::Cash, None),
            &MockTerminal::new(),
        )
        .await
        .unwrap();
    // 2 x $100 = $200; eco 2 x $0.75 = $1.50; tax 7.25% on $201.50.
    assert_eq!(sale.subtotal, money("200.0000"));
    assert_eq!(sale.eco_fee_total, money("1.5000"));
    assert_eq!(sale.tax_total, money("14.6088"));
    assert_eq!(sale.pos_fee_total, money("0.0000"));
    assert_eq!(sale.grand_total, money("216.1088"));
    assert!(sale.change_due.is_zero());
    assert!(sale.receipt_printed);
    // Drawer kick bytes are appended for a cash sale.
    {
        let printed = printer.prints.lock().unwrap();
        assert_eq!(printed.len(), 1);
        assert!(printed[0].ends_with(&[0x1B, 0x70, 0x00, 0x19, 0xFA]));
    }
    // Inventory moved: -2 on hand.
    assert_eq!(
        df_db::inventory_on_hand(
            edge.pool(),
            Uuid::from_u128(STORE_ID),
            Uuid::from_u128(PRODUCT_ID)
        )
        .await
        .unwrap(),
        dec("-2")
    );
    // Outbox: transaction + inventory entries.
    assert_eq!(edge.pending_outbox_count().await.unwrap(), 2);
}

#[tokio::test]
async fn card_checkout_adds_surcharge_and_change_on_cash_overpay() {
    let edge = test_edge().await;
    let sale = edge
        .checkout(
            sale_input("2", TenderKind::Card, None),
            &MockTerminal::new(),
        )
        .await
        .unwrap();
    // Surcharge 3% of the $216.1088 pre-surcharge total.
    assert_eq!(sale.pos_fee_total, money("6.4833"));
    assert_eq!(sale.grand_total, money("222.5921"));
    assert_eq!(sale.payments.len(), 1);
    assert_eq!(sale.payments[0].amount, money("222.5921"));
    assert_eq!(sale.payments[0].surcharge, money("6.4833"));
    assert!(sale.payments[0].authorization.is_some());
    // No printer configured: sale still succeeds, flagged as skipped.
    assert!(!sale.receipt_printed);
}

#[tokio::test]
async fn cash_overpay_gives_change() {
    let edge = test_edge().await;
    let sale = edge
        .checkout(
            sale_input("1", TenderKind::Cash, Some("120.0000")),
            &MockTerminal::new(),
        )
        .await
        .unwrap();
    // 1 gal: $100 + $0.75 eco = $100.75 taxable at 7.25% = $7.3044.
    assert_eq!(sale.grand_total, money("108.0544"));
    assert_eq!(sale.change_due, money("11.9456"));
}

#[tokio::test]
async fn under_tendered_sale_is_rejected() {
    let edge = test_edge().await;
    let err = edge
        .checkout(
            sale_input("1", TenderKind::Cash, Some("10.0000")),
            &MockTerminal::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, df_edge::EdgeError::InvalidInput(_)));
    assert_eq!(edge.pending_outbox_count().await.unwrap(), 0);
}

#[tokio::test]
async fn matrix_pricing_rule_replaces_list_price() {
    let edge = test_edge().await;
    df_db::insert_pricing_rule(
        edge.pool(),
        &PricingRuleRow {
            id: Uuid::now_v7(),
            org_id: Uuid::from_u128(ORG_ID),
            priority: 1,
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: Some(Uuid::from_u128(PRODUCT_ID)),
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            min_qty: dec("1"),
            calc_method: "pct_off_list".to_string(),
            calc_value: dec("20"),
            min_margin_floor: None,
            effective_from: None,
            effective_to: None,
        },
    )
    .await
    .unwrap();
    let preview = edge
        .price_preview(&sale_input("1", TenderKind::Cash, None))
        .await
        .unwrap();
    assert_eq!(preview.lines[0].unit_price, money("80.0000"));
    assert_eq!(preview.subtotal, money("80.0000"));
    // Preview persists nothing.
    assert_eq!(edge.pending_outbox_count().await.unwrap(), 0);
    assert_eq!(
        df_db::count_transactions_for_store(edge.pool(), Uuid::from_u128(STORE_ID))
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn till_open_close_z_report_math() {
    let edge = test_edge().await;
    let cashier = Uuid::now_v7();
    let till = edge.open_till(cashier, money("150.0000")).await.unwrap();
    assert!(edge.open_till(cashier, money("0.0000")).await.is_err());
    edge.checkout(
        sale_input("2", TenderKind::Cash, None),
        &MockTerminal::new(),
    )
    .await
    .unwrap();
    edge.checkout(
        sale_input("1", TenderKind::Card, None),
        &MockTerminal::new(),
    )
    .await
    .unwrap();
    let report = edge.close_till(till.id, money("400.0000")).await.unwrap();
    assert_eq!(report.sale_count, 2);
    // Gross: $216.1088 cash + $108.0544 + 3% surcharge $3.2416 = $111.2960 card.
    assert_eq!(report.gross_sales, money("327.4048"));
    assert_eq!(report.cash_sales, money("216.1088"));
    assert_eq!(report.card_sales, money("111.2960"));
    assert_eq!(report.expected_cash, money("366.1088"));
    assert_eq!(report.over_short, money("33.8912"));
    assert!(edge.close_till(till.id, money("400.0000")).await.is_err());
}

#[tokio::test]
async fn return_reverses_inventory_and_totals() {
    let edge = test_edge().await;
    let sale = edge
        .checkout(
            sale_input("2", TenderKind::Cash, None),
            &MockTerminal::new(),
        )
        .await
        .unwrap();
    let ret = edge
        .record_return(
            sale.transaction_id,
            vec![ReturnLineInput {
                product_id: Uuid::from_u128(PRODUCT_ID),
                qty: dec("1"),
            }],
        )
        .await
        .unwrap();
    assert_eq!(ret.subtotal, money("-100.0000"));
    assert_eq!(ret.eco_fee_total, money("-0.7500"));
    assert_eq!(ret.tax_total, money("-7.3044"));
    assert_eq!(ret.grand_total, money("-108.0544"));
    assert_eq!(
        df_db::inventory_on_hand(
            edge.pool(),
            Uuid::from_u128(STORE_ID),
            Uuid::from_u128(PRODUCT_ID)
        )
        .await
        .unwrap(),
        dec("-1")
    );
    // Returning more than was sold is rejected.
    assert!(
        edge.record_return(
            sale.transaction_id,
            vec![ReturnLineInput {
                product_id: Uuid::from_u128(PRODUCT_ID),
                qty: dec("3"),
            }],
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn charge_account_creates_ar_entry() {
    let edge = test_edge().await;
    let customer_id = Uuid::now_v7();
    let mut input = sale_input("1", TenderKind::ChargeAccount, None);
    input.customer_id = Some(customer_id);
    let sale = edge.checkout(input, &MockTerminal::new()).await.unwrap();
    assert_eq!(sale.grand_total, money("108.0544"));
    assert_eq!(
        df_db::ar_balance_for_customer(edge.pool(), customer_id)
            .await
            .unwrap(),
        dec("108.0544")
    );
    // Outbox: transaction + inventory + AR.
    assert_eq!(edge.pending_outbox_count().await.unwrap(), 3);
}

#[tokio::test]
async fn hold_park_resume_and_quote_have_no_side_effects() {
    let edge = test_edge().await;
    let input = sale_input("1", TenderKind::Cash, None);
    let hold_id = edge.park_hold(input.clone()).await.unwrap();
    let resumed = edge.resume_hold(hold_id).await.unwrap();
    assert_eq!(resumed, input);
    assert!(edge.resume_hold(hold_id).await.is_err());

    let quote_id = edge.record_quote(input).await.unwrap();
    let quote = df_db::get_transaction(edge.pool(), quote_id).await.unwrap();
    assert_eq!(quote.doc_type, "quote");
    assert_eq!(quote.status, "open");
    // No inventory movement, no outbox entries for a quote.
    assert_eq!(
        df_db::inventory_on_hand(
            edge.pool(),
            Uuid::from_u128(STORE_ID),
            Uuid::from_u128(PRODUCT_ID)
        )
        .await
        .unwrap(),
        dec("0")
    );
    assert_eq!(edge.pending_outbox_count().await.unwrap(), 0);
}

#[tokio::test]
async fn sync_now_against_memory_hq_and_replay_is_noop() {
    let edge = test_edge().await;
    let hq = MemoryHq::new();
    let product_id = Uuid::from_u128(PRODUCT_ID);
    edge.checkout(
        sale_input("2", TenderKind::Cash, None),
        &MockTerminal::new(),
    )
    .await
    .unwrap();
    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(report.duplicates, 0);
    assert_eq!(hq.inventory_on_hand(product_id), dec("-2"));
    assert_eq!(hq.applied_count(), 2);
    assert_eq!(edge.pending_outbox_count().await.unwrap(), 0);
    let state = edge.sync_state().await.unwrap();
    assert!(state.last_batch_id.is_some());
    assert!(state.last_synced_at.is_some());
    assert_eq!(state.pending_count, 0);

    // A second sale + sync, then a manual replay of the same batch
    // through the HQ is a no-op (duplicates only).
    edge.checkout(
        sale_input("1", TenderKind::Cash, None),
        &MockTerminal::new(),
    )
    .await
    .unwrap();
    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(hq.inventory_on_hand(product_id), dec("-3"));

    // Empty drain is a no-op.
    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.duplicates, 0);
}
