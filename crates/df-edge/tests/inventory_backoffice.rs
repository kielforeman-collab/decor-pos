//! Phase 3 back-office exit tests + endpoint coverage (v0.4).
//!
//! Exit tests (spec §7 Phase 3):
//!
//! * (b) multi-store transfer — ship/receive moves stock A→B with
//!   the in-transit leg in both stores' ledgers.
//! * (c) count reconciliation — counted 8 vs 10 on hand posts a
//!   `count_adj` of -2 (and 12 posts +2); reconciled on-hand equals
//!   the count.
//! * PO receive — receiving a PO line posts stock and moves the PO
//!   to `partially_received` / `received`; over-receiving is
//!   rejected and posts nothing.

use std::str::FromStr;

use chrono::{NaiveDate, Utc};
use df_db::models::{
    ArLedgerEntry, Customer, InventoryLedgerEntry, Organization, PricingRuleRow, Product, Store,
};
use df_domain::{Currency, Money};
use df_edge::{
    BulkPricingRequest, CreatePurchaseOrderRequest, CreateTransferRequest, Edge, EdgeConfig,
    PricingRuleUpdate, ReceivePoLineRequest, ReceivePurchaseOrderRequest,
    ReplenishmentSuggestRequest, ScanCountRequest, SetReorderPointRequest, StartCountRequest,
    TransferLineView,
};
use rust_decimal::Decimal;
use uuid::Uuid;

const ORG_ID: u128 = 0x2000;
const STORE_A: u128 = 0x1000;
const STORE_B: u128 = 0x1001;
const PRODUCT_A: u128 = 0x3000;
const PRODUCT_B: u128 = 0x3001;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn test_config() -> EdgeConfig {
    EdgeConfig {
        store_id: Uuid::from_u128(STORE_A),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    }
}

async fn test_edge() -> Edge {
    let edge = Edge::open(test_config()).await.unwrap();
    df_db::insert_organization(
        edge.pool(),
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
        edge.pool(),
        &Store {
            id: Uuid::from_u128(STORE_A),
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
    edge
}

async fn seed_product(
    edge: &Edge,
    id: u128,
    sku: &str,
    name: &str,
    list: &str,
    cost: &str,
) -> Uuid {
    let product_id = Uuid::from_u128(id);
    df_db::insert_product(
        edge.pool(),
        &Product {
            id: product_id,
            org_id: Uuid::from_u128(ORG_ID),
            sku: sku.to_string(),
            manufacturer_code: None,
            name: name.to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "stock".to_string(),
            base_uom: "ea".to_string(),
            list_price: dec(list),
            standard_cost: dec(cost),
            avg_cost: dec(cost),
            min_margin_pct: None,
            sheen: None,
            paint_base_code: None,
            container_volume_ml: None,
            eco_fee_category: None,
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            is_active: true,
        },
    )
    .await
    .unwrap();
    product_id
}

async fn seed_paint(edge: &Edge) -> Uuid {
    seed_product(
        edge,
        PRODUCT_A,
        "BM-REGAL-1G",
        "Regal Select Interior Gallon",
        "100.0000",
        "50.0000",
    )
    .await
}

/// Post one raw ledger entry (test seeding — the edge's own methods
/// are what the tests exercise).
async fn seed_ledger(
    edge: &Edge,
    store_id: Uuid,
    product_id: Uuid,
    deltas: (Decimal, Decimal, Decimal, Decimal),
    reason: &str,
) {
    let (on_hand, committed, on_order, in_transit) = deltas;
    df_db::insert_inventory_delta(
        edge.pool(),
        &InventoryLedgerEntry {
            id: Uuid::now_v7(),
            org_id: Uuid::from_u128(ORG_ID),
            store_id,
            product_id,
            lot_number: None,
            delta_on_hand: on_hand,
            delta_committed: committed,
            delta_on_order: on_order,
            delta_in_transit: in_transit,
            unit_cost: dec("50.0000"),
            reason_type: reason.to_string(),
            reference_id: Uuid::now_v7(),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
}

async fn seed_on_hand(edge: &Edge, store_id: Uuid, product_id: Uuid, qty: &str) {
    seed_ledger(
        edge,
        store_id,
        product_id,
        (dec(qty), Decimal::ZERO, Decimal::ZERO, Decimal::ZERO),
        "po_receive",
    )
    .await;
}

fn on_hand(balances: &[df_db::InventoryBalance], product_id: Uuid) -> Decimal {
    balances
        .iter()
        .find(|b| b.product_id == product_id)
        .map(|b| b.on_hand)
        .unwrap_or(Decimal::ZERO)
}

// --- exit test (b): multi-store transfer ------------------------------------

#[tokio::test]
async fn transfer_ship_receive_moves_stock_between_stores() {
    let edge = test_edge().await;
    let product = seed_paint(&edge).await;
    let store_a = Uuid::from_u128(STORE_A);
    let store_b = Uuid::from_u128(STORE_B);
    seed_on_hand(&edge, store_a, product, "10").await;

    let transfer = edge
        .create_transfer(CreateTransferRequest {
            from_store_id: store_a,
            to_store_id: store_b,
            lines: vec![TransferLineView {
                product_id: product,
                qty: dec("5"),
                unit_cost: dec("50.0000"),
            }],
        })
        .await
        .unwrap();
    assert_eq!(transfer.status, "draft");

    // Ship: A loses 5 on-hand, gains 5 in-transit.
    let shipped = edge.ship_transfer(transfer.id).await.unwrap();
    assert_eq!(shipped.status, "shipped");
    let balances_a = edge.inventory_balances(store_a).await.unwrap();
    assert_eq!(on_hand(&balances_a, product), dec("5"));
    assert_eq!(
        balances_a
            .iter()
            .find(|b| b.product_id == product)
            .unwrap()
            .in_transit,
        dec("5")
    );
    let entries_a = df_db::list_inventory_entries_for_store(edge.pool(), store_a)
        .await
        .unwrap();
    let ship_leg = entries_a
        .iter()
        .find(|e| e.reason_type == "transfer" && e.reference_id == transfer.id)
        .expect("ship leg in A's ledger");
    assert_eq!(ship_leg.delta_on_hand, dec("-5"));
    assert_eq!(ship_leg.delta_in_transit, dec("5"));

    // Receive: in-transit closes at A, B gains 5 on-hand.
    let received = edge.receive_transfer(transfer.id).await.unwrap();
    assert_eq!(received.status, "received");
    let balances_a = edge.inventory_balances(store_a).await.unwrap();
    assert_eq!(on_hand(&balances_a, product), dec("5"));
    assert_eq!(
        balances_a
            .iter()
            .find(|b| b.product_id == product)
            .unwrap()
            .in_transit,
        dec("0")
    );
    let balances_b = edge.inventory_balances(store_b).await.unwrap();
    assert_eq!(on_hand(&balances_b, product), dec("5"));
    let entries_b = df_db::list_inventory_entries_for_store(edge.pool(), store_b)
        .await
        .unwrap();
    let arrive_leg = entries_b
        .iter()
        .find(|e| e.reason_type == "transfer" && e.reference_id == transfer.id)
        .expect("arrival leg in B's ledger");
    assert_eq!(arrive_leg.delta_on_hand, dec("5"));
    let entries_a = df_db::list_inventory_entries_for_store(edge.pool(), store_a)
        .await
        .unwrap();
    let close_leg = entries_a
        .iter()
        .find(|e| e.reason_type == "transfer" && e.delta_in_transit == dec("-5"))
        .expect("in-transit close leg in A's ledger");
    assert_eq!(close_leg.reference_id, transfer.id);

    // Shipping twice / receiving twice are rejected.
    assert!(edge.ship_transfer(transfer.id).await.is_err());
    assert!(edge.receive_transfer(transfer.id).await.is_err());
}

// --- exit test (c): count reconciliation -------------------------------------

#[tokio::test]
async fn count_reconcile_posts_adjustments() {
    let edge = test_edge().await;
    let product = seed_paint(&edge).await;
    let store_a = Uuid::from_u128(STORE_A);

    // Counted 8 vs 10 on hand: -2 adjustment, on-hand becomes 8.
    seed_on_hand(&edge, store_a, product, "10").await;
    let count = edge
        .start_count(StartCountRequest {
            store_id: Some(store_a),
        })
        .await
        .unwrap();
    assert_eq!(count.status, "open");
    let scanned = edge
        .scan_count(
            count.id,
            ScanCountRequest {
                product_id: product,
                counted_qty: dec("8"),
            },
        )
        .await
        .unwrap();
    assert_eq!(scanned.lines.len(), 1);
    assert_eq!(scanned.lines[0].counted_qty, dec("8"));
    let report = edge.reconcile_count(count.id).await.unwrap();
    assert_eq!(report.adjustments.len(), 1);
    assert_eq!(report.adjustments[0].on_hand_before, dec("10"));
    assert_eq!(report.adjustments[0].adjustment, dec("-2"));
    let balances = edge.inventory_balances(store_a).await.unwrap();
    assert_eq!(on_hand(&balances, product), dec("8"));
    let entries = df_db::list_inventory_entries_for_store(edge.pool(), store_a)
        .await
        .unwrap();
    let adj = entries
        .iter()
        .find(|e| e.reason_type == "count_adj" && e.reference_id == count.id)
        .expect("count_adj entry in the ledger");
    assert_eq!(adj.delta_on_hand, dec("-2"));

    // Counted 12 vs 10 on hand: +2 adjustment, on-hand becomes 12.
    let product_b = seed_product(
        &edge,
        PRODUCT_B,
        "BRUSH-2IN",
        "2 in Angle Brush",
        "12.5000",
        "6.0000",
    )
    .await;
    seed_on_hand(&edge, store_a, product_b, "10").await;
    let count_b = edge
        .start_count(StartCountRequest {
            store_id: Some(store_a),
        })
        .await
        .unwrap();
    edge.scan_count(
        count_b.id,
        ScanCountRequest {
            product_id: product_b,
            counted_qty: dec("12"),
        },
    )
    .await
    .unwrap();
    let report_b = edge.reconcile_count(count_b.id).await.unwrap();
    assert_eq!(report_b.adjustments[0].adjustment, dec("2"));
    let balances = edge.inventory_balances(store_a).await.unwrap();
    assert_eq!(on_hand(&balances, product_b), dec("12"));

    // A reconciled count cannot be scanned or reconciled again.
    assert!(
        edge.scan_count(
            count.id,
            ScanCountRequest {
                product_id: product,
                counted_qty: dec("8"),
            },
        )
        .await
        .is_err()
    );
    assert!(edge.reconcile_count(count.id).await.is_err());
}

// --- PO receive ---------------------------------------------------------------

#[tokio::test]
async fn po_receive_posts_stock_and_updates_status() {
    let edge = test_edge().await;
    let product = seed_paint(&edge).await;
    let store_a = Uuid::from_u128(STORE_A);

    let po = edge
        .create_purchase_order(CreatePurchaseOrderRequest {
            vendor_id: "VENDOR-A".to_string(),
            store_id: Some(store_a),
            lines: vec![df_edge::CreatePoLineRequest {
                product_id: product,
                qty: dec("10"),
                uom: None,
                unit_cost: None,
            }],
        })
        .await
        .unwrap();
    assert_eq!(po.status, "draft");
    assert!(po.po_number.starts_with("PO-"));
    assert_eq!(po.total, dec("500.0000")); // 10 x avg cost 50
    assert_eq!(po.lines[0].received_qty, dec("0"));

    // Partial receive: 4 of 10.
    let partial = edge
        .receive_purchase_order(
            po.id,
            ReceivePurchaseOrderRequest {
                lines: vec![ReceivePoLineRequest {
                    line_id: None,
                    product_id: product,
                    qty: dec("4"),
                }],
            },
        )
        .await
        .unwrap();
    assert_eq!(partial.status, "partially_received");
    assert_eq!(partial.lines[0].received_qty, dec("4"));
    let balances = edge.inventory_balances(store_a).await.unwrap();
    assert_eq!(on_hand(&balances, product), dec("4"));

    // Over-receiving the remainder (7 > 6 left) is rejected, posts nothing.
    assert!(
        edge.receive_purchase_order(
            po.id,
            ReceivePurchaseOrderRequest {
                lines: vec![ReceivePoLineRequest {
                    line_id: None,
                    product_id: product,
                    qty: dec("7"),
                }],
            },
        )
        .await
        .is_err()
    );
    let balances = edge.inventory_balances(store_a).await.unwrap();
    assert_eq!(on_hand(&balances, product), dec("4"));

    // Receive the rest: fully received, on-hand 10.
    let done = edge
        .receive_purchase_order(
            po.id,
            ReceivePurchaseOrderRequest {
                lines: vec![ReceivePoLineRequest {
                    line_id: None,
                    product_id: product,
                    qty: dec("6"),
                }],
            },
        )
        .await
        .unwrap();
    assert_eq!(done.status, "received");
    assert_eq!(done.lines[0].received_qty, dec("10"));
    let balances = edge.inventory_balances(store_a).await.unwrap();
    assert_eq!(on_hand(&balances, product), dec("10"));
    let entries = df_db::list_inventory_entries_for_store(edge.pool(), store_a)
        .await
        .unwrap();
    let received: Decimal = entries
        .iter()
        .filter(|e| e.reason_type == "po_receive" && e.reference_id == po.id)
        .map(|e| e.delta_on_hand)
        .fold(Decimal::ZERO, |acc, delta| acc + delta);
    assert_eq!(received, dec("10"));
}

// --- reorder points + replenishment ------------------------------------------

#[tokio::test]
async fn reorder_points_and_replenishment_suggest() {
    let edge = test_edge().await;
    let product = seed_paint(&edge).await;
    let store_a = Uuid::from_u128(STORE_A);
    // On-hand 2 after a sale of 30 today: velocity 30 / 30 days = 1/day.
    seed_on_hand(&edge, store_a, product, "32").await;
    seed_ledger(
        &edge,
        store_a,
        product,
        (dec("-30"), Decimal::ZERO, Decimal::ZERO, Decimal::ZERO),
        "sale",
    )
    .await;

    // No points yet: nothing suggested.
    let empty = edge
        .suggest_replenishment(ReplenishmentSuggestRequest {
            store_id: Some(store_a),
            days: Some(30),
            lead_time_days: Some(7),
        })
        .await
        .unwrap();
    assert!(empty.is_empty());

    let point = edge
        .set_reorder_point(SetReorderPointRequest {
            product_id: product,
            store_id: Some(store_a),
            min_qty: dec("10"),
            max_qty: dec("20"),
            reorder_qty: dec("12"),
        })
        .await
        .unwrap();
    assert_eq!(point.min_qty, dec("10"));
    let points = edge.list_reorder_points(store_a).await.unwrap();
    assert_eq!(points.len(), 1);

    // Supply 2 <= min 10: suggest max(12, 20-2=18, 1*7=7) = 18.
    let suggested = edge
        .suggest_replenishment(ReplenishmentSuggestRequest {
            store_id: Some(store_a),
            days: Some(30),
            lead_time_days: Some(7),
        })
        .await
        .unwrap();
    assert_eq!(suggested.len(), 1);
    assert_eq!(suggested[0].vendor_id, "UNKNOWN"); // no primary vendor
    assert_eq!(suggested[0].lines.len(), 1);
    let line = &suggested[0].lines[0];
    assert_eq!(line.qty, dec("18"));
    assert_eq!(line.on_hand, dec("2"));
    assert_eq!(line.velocity_per_day, dec("1"));

    // An in-stock product is not suggested.
    let product_b = seed_product(
        &edge,
        PRODUCT_B,
        "BRUSH-2IN",
        "2 in Angle Brush",
        "12.5000",
        "6.0000",
    )
    .await;
    seed_on_hand(&edge, store_a, product_b, "50").await;
    edge.set_reorder_point(SetReorderPointRequest {
        product_id: product_b,
        store_id: Some(store_a),
        min_qty: dec("10"),
        max_qty: dec("20"),
        reorder_qty: dec("12"),
    })
    .await
    .unwrap();
    let suggested = edge
        .suggest_replenishment(ReplenishmentSuggestRequest {
            store_id: Some(store_a),
            days: Some(30),
            lead_time_days: Some(7),
        })
        .await
        .unwrap();
    assert_eq!(suggested.len(), 1);
    assert_eq!(suggested[0].lines[0].product_id, product);

    // Invalid points are rejected.
    assert!(
        edge.set_reorder_point(SetReorderPointRequest {
            product_id: product,
            store_id: Some(store_a),
            min_qty: dec("20"),
            max_qty: dec("10"),
            reorder_qty: dec("12"),
        })
        .await
        .is_err()
    );
}

// --- balances / shelf tags / pricing / AR -------------------------------------

#[tokio::test]
async fn balances_report_on_hand_committed_on_order() {
    let edge = test_edge().await;
    let product = seed_paint(&edge).await;
    let store_a = Uuid::from_u128(STORE_A);
    seed_ledger(
        &edge,
        store_a,
        product,
        (dec("10"), dec("2"), dec("5"), Decimal::ZERO),
        "order",
    )
    .await;
    let balances = edge.inventory_balances(store_a).await.unwrap();
    assert_eq!(balances.len(), 1);
    let balance = &balances[0];
    assert_eq!(balance.sku, "BM-REGAL-1G");
    assert_eq!(balance.on_hand, dec("10"));
    assert_eq!(balance.committed, dec("2"));
    assert_eq!(balance.on_order, dec("5"));
    assert_eq!(balance.in_transit, dec("0"));
}

#[tokio::test]
async fn shelf_tag_returns_zpl() {
    let edge = test_edge().await;
    let product = seed_paint(&edge).await;
    let tag = edge.shelf_tag(product).await.unwrap();
    assert_eq!(tag.sku, "BM-REGAL-1G");
    assert_eq!(tag.price, Money::new(dec("100.0000"), Currency::Usd));
    assert!(tag.zpl.starts_with("^XA"));
    assert!(tag.zpl.contains("Regal Select Interior Gallon"));
    assert!(tag.zpl.contains("BM-REGAL-1G"));
    assert!(tag.zpl.contains("$100.00"));
    assert!(tag.zpl.trim_end().ends_with("^XZ"));
}

#[tokio::test]
async fn pricing_bulk_update_enforces_margin_floor() {
    let edge = test_edge().await;
    let product = seed_paint(&edge).await;
    let rule_id = Uuid::now_v7();
    df_db::insert_pricing_rule(
        edge.pool(),
        &PricingRuleRow {
            id: rule_id,
            org_id: Uuid::from_u128(ORG_ID),
            priority: 1,
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: Some(product),
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            min_qty: dec("1"),
            calc_method: "pct_off_list".to_string(),
            calc_value: dec("10"),
            min_margin_floor: Some(dec("30")),
            effective_from: None,
            effective_to: None,
        },
    )
    .await
    .unwrap();

    // 60% off list ($40 on $50 cost) is below the 30% floor: rejected.
    assert!(
        edge.bulk_update_pricing_rules(BulkPricingRequest {
            rules: vec![PricingRuleUpdate {
                id: rule_id,
                calc_method: "pct_off_list".to_string(),
                calc_value: dec("60"),
                min_margin_floor: Some(dec("30")),
            }],
        })
        .await
        .is_err()
    );
    // The rejected batch changed nothing.
    let rules = edge.list_pricing_rules().await.unwrap();
    assert_eq!(rules[0].calc_value, dec("10"));

    // Fixed $80 (37.5% margin) clears the floor: accepted.
    let updated = edge
        .bulk_update_pricing_rules(BulkPricingRequest {
            rules: vec![PricingRuleUpdate {
                id: rule_id,
                calc_method: "fixed".to_string(),
                calc_value: dec("80"),
                min_margin_floor: Some(dec("30")),
            }],
        })
        .await
        .unwrap();
    assert_eq!(updated[0].calc_method, "fixed");
    assert_eq!(updated[0].calc_value, dec("80"));
}

async fn seed_customer(
    edge: &Edge,
    id: u128,
    account: &str,
    credit_limit: &str,
    charge_enabled: bool,
    credit_hold: bool,
) -> Uuid {
    let customer_id = Uuid::from_u128(id);
    df_db::insert_customer(
        edge.pool(),
        &Customer {
            id: customer_id,
            org_id: Uuid::from_u128(ORG_ID),
            account_number: account.to_string(),
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
            charge_enabled,
            credit_limit: dec(credit_limit),
            payment_terms: "NET30".to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
        },
    )
    .await
    .unwrap();
    customer_id
}

async fn seed_ar_entry(
    edge: &Edge,
    customer_id: Uuid,
    amount: &str,
    due_date: Option<&str>,
    entry_type: &str,
) {
    df_db::insert_ar_entry(
        edge.pool(),
        &ArLedgerEntry {
            id: Uuid::now_v7(),
            org_id: Uuid::from_u128(ORG_ID),
            store_id: Uuid::from_u128(STORE_A),
            customer_id,
            job_id: None,
            entry_type: entry_type.to_string(),
            reference_type: "transaction".to_string(),
            reference_id: Uuid::now_v7(),
            amount: dec(amount),
            due_date: due_date.map(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap()),
            statement_id: None,
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn ar_summary_aging_and_collections() {
    let edge = test_edge().await;
    let customer = seed_customer(&edge, 0x4000, "C-100", "1000.0000", true, false).await;
    // $100 due Nov 1 (24 days past due on Nov 25 -> current),
    // $50 due Oct 1 (55 days -> 31-60), $20 payment (reduces current).
    seed_ar_entry(&edge, customer, "100.0000", Some("2026-11-01"), "charge").await;
    seed_ar_entry(&edge, customer, "50.0000", Some("2026-10-01"), "charge").await;
    seed_ar_entry(&edge, customer, "-20.0000", None, "payment").await;

    let summary = edge.ar_customer_summary(customer).await.unwrap();
    assert_eq!(summary.account_number, "C-100");
    assert_eq!(summary.balance, Money::new(dec("130.0000"), Currency::Usd));
    assert_eq!(summary.entry_count, 3);
    assert_eq!(
        summary.oldest_due_date,
        Some(NaiveDate::from_ymd_opt(2026, 10, 1).unwrap())
    );

    let as_of = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let aging = edge
        .ar_aging(Uuid::from_u128(STORE_A), as_of)
        .await
        .unwrap();
    assert_eq!(aging.customers.len(), 1);
    let buckets = &aging.customers[0].buckets;
    assert_eq!(buckets[0], Money::new(dec("80.0000"), Currency::Usd)); // 100 - 20
    assert_eq!(buckets[1], Money::new(dec("50.0000"), Currency::Usd)); // 31-60
    assert!(buckets[2].is_zero() && buckets[3].is_zero() && buckets[4].is_zero());
    assert_eq!(aging.totals[0], buckets[0]);
    assert_eq!(aging.totals[1], buckets[1]);

    let collections = edge
        .ar_collections(Uuid::from_u128(STORE_A), as_of)
        .await
        .unwrap();
    assert_eq!(collections.customers.len(), 1);
    assert_eq!(
        collections.customers[0].overdue,
        Money::new(dec("50.0000"), Currency::Usd)
    );
}

// --- HTTP wiring ----------------------------------------------------------------

#[tokio::test]
async fn balances_and_transfer_create_over_http() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    let edge = test_edge().await;
    let product = seed_paint(&edge).await;
    let store_a = Uuid::from_u128(STORE_A);
    let store_b = Uuid::from_u128(STORE_B);
    seed_on_hand(&edge, store_a, product, "10").await;
    let router = edge.router();

    async fn get_json(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn post_json(
        router: axum::Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let response = router
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    let (status, body) = get_json(
        router.clone(),
        &format!("/inventory/balances?store_id={store_a}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["on_hand"], "10");
    assert_eq!(body[0]["sku"], "BM-REGAL-1G");

    let (status, body) = post_json(
        router.clone(),
        "/inventory/transfers",
        serde_json::json!({
            "from_store_id": store_a.to_string(),
            "to_store_id": store_b.to_string(),
            "lines": [{"product_id": product.to_string(), "qty": "5", "unit_cost": "50.0000"}],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "draft");
    let transfer_id = body["id"].as_str().unwrap().to_string();

    let (status, body) = post_json(
        router.clone(),
        &format!("/inventory/transfers/{transfer_id}/ship"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "shipped");

    let (status, body) = get_json(
        router.clone(),
        &format!("/inventory/shelf-tags?product_id={product}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["zpl"].as_str().unwrap().starts_with("^XA"));
}
