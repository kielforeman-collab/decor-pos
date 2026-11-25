//! Phase 1 exit test: full offline checkout → print → reconnect →
//! cloud ledger parity (spec §7, Phase 1 exit criterion).
//!
//! Uses the REAL crates end to end — `df-payments`' `MockTerminal`,
//! `df-hw`'s `MockPrinter`, `df-sync`'s `MemoryHq` — against an
//! in-memory SQLite edge. Deterministic: fixed UUIDs/amounts, no
//! sleeps, no network. Prices come from a matrix pricing rule (20%
//! off list), so the $100 list price sells at $80.
//!
//! Expected math (US/CA store, 7.25% tax, $0.75 PaintCare eco fee
//! per gallon, 3% card surcharge — no card tenders here):
//! * 2 gal: subtotal $160.0000, eco $1.5000, tax $11.7088,
//!   grand $173.2088.
//! * 1 gal: subtotal $80.0000, eco $0.7500, tax $5.8544,
//!   grand $86.6044.

use chrono::Utc;
use df_db::models::{Customer, Organization, PricingRuleRow, Product, Store};
use df_domain::{Currency, Money};
use df_edge::{
    Edge, EdgeConfig, MockTerminal, ReturnLineInput, SaleInput, SaleLineInput, TenderInput,
    TenderKind,
};
use df_hw::MockPrinter;
use df_sync::{DeltaBatch, HqSink, MemoryHq, Outbox};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;
const CUSTOMER_ID: u128 = 0x4000;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn money(s: &str) -> Money {
    Money::from_str_amount(s, Currency::Usd).unwrap()
}

fn store_id() -> Uuid {
    Uuid::from_u128(STORE_ID)
}

fn product_id() -> Uuid {
    Uuid::from_u128(PRODUCT_ID)
}

fn customer_id() -> Uuid {
    Uuid::from_u128(CUSTOMER_ID)
}

/// Edge seeded with org + store + one $100 paint product + a 20%-off
/// matrix pricing rule for that product + a charge-enabled customer.
async fn seeded_edge() -> Edge {
    let config = EdgeConfig {
        store_id: store_id(),
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
            id: store_id(),
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
            id: product_id(),
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
    // Matrix rule: 20% off list for this product, any customer.
    df_db::insert_pricing_rule(
        pool,
        &PricingRuleRow {
            id: Uuid::from_u128(0x5000),
            org_id: Uuid::from_u128(ORG_ID),
            priority: 1,
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: Some(product_id()),
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
    df_db::insert_customer(
        pool,
        &Customer {
            id: customer_id(),
            org_id: Uuid::from_u128(ORG_ID),
            account_number: "C-1001".to_string(),
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
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
        },
    )
    .await
    .unwrap();
    edge
}

fn sale_input(qty: &str, tender: TenderKind, customer: Option<Uuid>) -> SaleInput {
    SaleInput {
        customer_id: customer,
        job_id: None,
        lines: vec![SaleLineInput {
            product_id: product_id(),
            sku: "BM-REGAL-1G".to_string(),
            description: "Regal Select Interior Gallon".to_string(),
            qty: dec(qty),
            uom: "gal".to_string(),
            unit_price: money("100.0000"),
            unit_cost: money("50.0000"),
        }],
        tenders: vec![TenderInput {
            kind: tender,
            amount: None,
        }],
        po_number: None,
    }
}

#[tokio::test]
async fn offline_checkout_print_reconnect_parity() {
    let edge = seeded_edge().await;
    let terminal = MockTerminal::new();
    let printer = Arc::new(MockPrinter::new());
    edge.set_printer(printer.clone());
    let cashier = Uuid::from_u128(0x6000);
    edge.open_till(cashier, money("150.0000")).await.unwrap();

    // --- OFFLINE: cash sale, no HQ contact. ---
    let sale = edge
        .checkout(sale_input("2", TenderKind::Cash, None), &terminal)
        .await
        .unwrap();
    // Matrix pricing replaced the $100 list price with $80.
    assert_eq!(sale.subtotal, money("160.0000"));
    assert_eq!(sale.eco_fee_total, money("1.5000"));
    assert_eq!(sale.tax_total, money("11.7088"));
    assert_eq!(sale.pos_fee_total, money("0.0000"));
    assert_eq!(sale.grand_total, money("173.2088"));
    assert!(sale.receipt_printed);
    assert_eq!(sale.doc_number, "TX-000001");

    // Receipt bytes captured on the mock printer: doc number, grand
    // total (2-decimal display), and the drawer kick for cash.
    let printed = printer.captured();
    assert_eq!(printed.len(), 1);
    assert!(printed[0].ends_with(&df_hw::drawer_kick_bytes()));
    let receipt_text = printer.captured_text();
    assert!(receipt_text.contains("TX-000001"));
    assert!(receipt_text.contains("$173.21"));
    assert!(receipt_text.contains("Cash"));

    // Sale is pending sync: outbox holds transaction + inventory.
    assert!(edge.pending_outbox_count().await.unwrap() > 0);
    // Local edge inventory moved: -2 on hand.
    assert_eq!(
        df_db::inventory_on_hand(edge.pool(), store_id(), product_id())
            .await
            .unwrap(),
        dec("-2")
    );

    // --- RECONNECT: sync into a fresh HQ. ---
    // Snapshot the batch first so the same batch can be replayed
    // against the HQ afterwards.
    let pending = Outbox::pending(edge.pool(), store_id(), 1000)
        .await
        .unwrap();
    assert_eq!(pending.len(), 2);
    let batch = DeltaBatch::new(store_id(), pending);

    let hq = MemoryHq::new();
    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(report.duplicates, 0);
    assert_eq!(edge.pending_outbox_count().await.unwrap(), 0);

    // HQ inventory delta matches the edge exactly.
    assert_eq!(hq.inventory_on_hand(product_id()), dec("-2"));
    // HQ transaction totals match the edge sale exactly.
    let hq_tx = hq.transaction(sale.transaction_id).unwrap();
    assert_eq!(hq_tx["subtotal"], "160.0000");
    assert_eq!(hq_tx["tax_total"], "11.7088");
    assert_eq!(hq_tx["eco_fee_total"], "1.5000");
    assert_eq!(hq_tx["pos_fee_total"], "0.0000");
    assert_eq!(hq_tx["grand_total"], "173.2088");
    assert_eq!(hq_tx["doc_number"], "TX-000001");

    // --- Second sale, charge account: HQ AR must match the edge. ---
    let charge_sale = edge
        .checkout(
            sale_input("1", TenderKind::ChargeAccount, Some(customer_id())),
            &terminal,
        )
        .await
        .unwrap();
    assert_eq!(charge_sale.grand_total, money("86.6044"));
    assert_eq!(charge_sale.subtotal, money("80.0000"));
    assert_eq!(charge_sale.tax_total, money("5.8544"));
    assert_eq!(charge_sale.eco_fee_total, money("0.7500"));
    let edge_ar = df_db::ar_balance_for_customer(edge.pool(), customer_id())
        .await
        .unwrap();
    assert_eq!(edge_ar, dec("86.6044"));

    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 3); // transaction + inventory + AR
    assert_eq!(report.duplicates, 0);
    assert_eq!(hq.inventory_on_hand(product_id()), dec("-3"));
    assert_eq!(hq.ar_balance(customer_id()), dec("86.6044"));
    assert_eq!(hq.ar_balance(customer_id()), edge_ar);
    let hq_charge_tx = hq.transaction(charge_sale.transaction_id).unwrap();
    assert_eq!(hq_charge_tx["grand_total"], "86.6044");

    // --- Replay: re-applying the first batch changes NOTHING. ---
    let applied_before = hq.applied_count();
    let replay = hq.apply_batch(&batch).await.unwrap();
    assert_eq!(replay.applied, 0);
    assert_eq!(replay.duplicates, 2);
    assert_eq!(hq.applied_count(), applied_before);
    assert_eq!(hq.inventory_on_hand(product_id()), dec("-3"));
    assert_eq!(hq.ar_balance(customer_id()), dec("86.6044"));
    assert_eq!(
        hq.transaction(sale.transaction_id).unwrap()["grand_total"],
        "173.2088"
    );

    // A second drain with an empty outbox is also a no-op.
    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.duplicates, 0);
    assert_eq!(hq.inventory_on_hand(product_id()), dec("-3"));
    assert_eq!(hq.ar_balance(customer_id()), dec("86.6044"));
}

#[tokio::test]
async fn offline_return_reverses_ledger() {
    let edge = seeded_edge().await;
    let terminal = MockTerminal::new();
    let printer = Arc::new(MockPrinter::new());
    edge.set_printer(printer.clone());

    // Charge-account sale of 2 gal, synced.
    let sale = edge
        .checkout(
            sale_input("2", TenderKind::ChargeAccount, Some(customer_id())),
            &terminal,
        )
        .await
        .unwrap();
    assert_eq!(sale.grand_total, money("173.2088"));
    let hq = MemoryHq::new();
    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 3);
    assert_eq!(hq.inventory_on_hand(product_id()), dec("-2"));
    assert_eq!(hq.ar_balance(customer_id()), dec("173.2088"));

    // Return 1 gal after sync.
    let ret = edge
        .record_return(
            sale.transaction_id,
            vec![ReturnLineInput {
                product_id: product_id(),
                qty: dec("1"),
            }],
        )
        .await
        .unwrap();
    assert_eq!(ret.grand_total, money("-86.6044"));
    assert_eq!(ret.subtotal, money("-80.0000"));
    assert!(edge.pending_outbox_count().await.unwrap() > 0);
    // Edge-local ledgers already reversed before sync.
    assert_eq!(
        df_db::inventory_on_hand(edge.pool(), store_id(), product_id())
            .await
            .unwrap(),
        dec("-1")
    );
    assert_eq!(
        df_db::ar_balance_for_customer(edge.pool(), customer_id())
            .await
            .unwrap(),
        dec("86.6044")
    );

    // Next sync reverses the HQ deltas too.
    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 3); // return tx + inventory + AR credit
    assert_eq!(report.duplicates, 0);
    assert_eq!(hq.inventory_on_hand(product_id()), dec("-1"));
    assert_eq!(hq.ar_balance(customer_id()), dec("86.6044"));
    let hq_return = hq.transaction(ret.transaction_id).unwrap();
    assert_eq!(hq_return["doc_type"], "return");
    assert_eq!(hq_return["grand_total"], "-86.6044");

    // Replaying the return sync changes nothing.
    let report = edge.sync_now(&hq).await.unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(hq.inventory_on_hand(product_id()), dec("-1"));
    assert_eq!(hq.ar_balance(customer_id()), dec("86.6044"));
}
