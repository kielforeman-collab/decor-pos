//! Phase 1 counter completeness integration tests:
//!   1. DB-backed holds (`pos_holds`, migration 0017) survive Edge restart.
//!   2. Real signed-in cashier `user_id` is stamped on HTTP checkout, return,
//!      quote, layaway, and parked holds (even with no open till).
//!   3. Card auto-void on split-tender 2nd card decline AND on failed DB commit.
//!   4. Card provider refund on partial & full returns (persists negative refund
//!      `PaymentRow` and returns refund `PaymentResult`), plus card void on
//!      `void_transaction`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use df_auth_http::test_support::seed_test_users;
use df_db::models::Product;
use df_domain::{Currency, Money};
use df_edge::{Edge, EdgeConfig, ReturnLineInput, SaleInput, SaleLineInput, TenderInput};
use df_payments::{MockOutcome, MockTerminal, PaymentStatus, TenderKind};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;

fn dec(s: &str) -> Decimal {
    Decimal::from_str_exact(s).unwrap()
}

fn usd(s: &str) -> Money {
    Money::from_str_amount(s, Currency::Usd).unwrap()
}

fn edge_config(db_url: String) -> EdgeConfig {
    EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url,
        store_name: "Counter Completeness Store".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    }
}

async fn seed_product(edge: &Edge) {
    let _ = df_db::insert_product(
        edge.pool(),
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
    .await;
}

fn sale_input(qty: &str, tenders: Vec<TenderInput>) -> SaleInput {
    SaleInput {
        customer_id: None,
        job_id: None,
        price_level_id: None,
        job_name: None,
        lines: vec![SaleLineInput {
            product_id: Uuid::from_u128(PRODUCT_ID),
            sku: "BM-REGAL-1G".to_string(),
            description: "Regal Select Interior Gallon".to_string(),
            qty: dec(qty),
            uom: "gal".to_string(),
            unit_price: usd("100.0000"),
            unit_cost: usd("50.0000"),
        }],
        tenders,
        po_number: None,
    }
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn holds_persist_in_sqlite_across_edge_restart() {
    let path = std::env::temp_dir().join(format!(
        "decor-pos-db-holds-{}-{}.db",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let db_url = format!("sqlite:{}?mode=rwc", path.display());
    let _ = std::fs::remove_file(&path);

    let input = sale_input(
        "2",
        vec![TenderInput {
            kind: TenderKind::Cash,
            amount: None,
            reference: None,
        }],
    );

    let hold_id = {
        let edge1 = Edge::open(edge_config(db_url.clone())).await.unwrap();
        edge1
            .ensure_configured_store(Some("US"), Some("CA"))
            .await
            .unwrap();
        seed_product(&edge1).await;
        let id = edge1.park_hold(input.clone()).await.unwrap();
        assert_eq!(edge1.list_holds().await, vec![id]);
        id
    };

    // Re-open a brand-new Edge instance against the same SQLite file:
    // the parked hold must still be there and resume cleanly once.
    let edge2 = Edge::open(edge_config(db_url)).await.unwrap();
    assert_eq!(edge2.list_holds().await, vec![hold_id]);
    let resumed = edge2.resume_hold(hold_id).await.unwrap();
    assert_eq!(resumed, input);
    assert!(edge2.list_holds().await.is_empty());
    // Second resume fails (atomically removed).
    assert!(edge2.resume_hold(hold_id).await.is_err());

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn http_pos_routes_stamp_real_signed_in_cashier_id() {
    let edge = Edge::open(edge_config("sqlite::memory:".to_string()))
        .await
        .unwrap();
    edge.ensure_configured_store(Some("US"), Some("CA"))
        .await
        .unwrap();
    seed_product(&edge).await;
    edge.set_payment_provider(Arc::new(MockTerminal::new()));

    let users = seed_test_users(edge.pool(), Uuid::from_u128(ORG_ID)).await;
    let cashier_id = users.cashier.user.id;
    let cashier_tok = users.cashier.token.clone();
    let app = edge.router();

    // 1. Checkout with NO till open -> stamps signed-in cashier's user_id.
    let sale = sale_input(
        "2",
        vec![TenderInput {
            kind: TenderKind::Cash,
            amount: None,
            reference: None,
        }],
    );
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/pos/transactions")
                .header("authorization", format!("Bearer {cashier_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&sale).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let sale_json = body_json(res).await;
    let sale_tx_id = Uuid::parse_str(sale_json["transaction_id"].as_str().unwrap()).unwrap();
    let stored_sale = df_db::get_transaction(edge.pool(), sale_tx_id)
        .await
        .unwrap();
    assert_eq!(stored_sale.cashier_id, Some(cashier_id));

    // 2. Return processed by Manager -> stamps Manager's user_id (not original cashier).
    let manager_id = users.manager.user.id;
    let manager_tok = users.manager.token.clone();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/pos/transactions/{sale_tx_id}/return"))
                .header("authorization", format!("Bearer {manager_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!([{ "product_id": Uuid::from_u128(PRODUCT_ID), "qty": "1" }]).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ret_json = body_json(res).await;
    let ret_tx_id = Uuid::parse_str(ret_json["transaction_id"].as_str().unwrap()).unwrap();
    let stored_ret = df_db::get_transaction(edge.pool(), ret_tx_id)
        .await
        .unwrap();
    assert_eq!(stored_ret.cashier_id, Some(manager_id));

    // 3. Quote -> stamps signed-in cashier's user_id.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/pos/quotes")
                .header("authorization", format!("Bearer {cashier_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&sale).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let quote_id =
        Uuid::parse_str(body_json(res).await["transaction_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        df_db::get_transaction(edge.pool(), quote_id)
            .await
            .unwrap()
            .cashier_id,
        Some(cashier_id)
    );

    // 4. Layaway -> stamps signed-in cashier's user_id.
    let layaway_body = json!({
        "sale": sale,
        "deposit": TenderInput {
            kind: TenderKind::Cash,
            amount: Some(usd("50.0000")),
            reference: None,
        }
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/pos/layaways")
                .header("authorization", format!("Bearer {cashier_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(layaway_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let layaway_id =
        Uuid::parse_str(body_json(res).await["transaction_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        df_db::get_transaction(edge.pool(), layaway_id)
            .await
            .unwrap()
            .cashier_id,
        Some(cashier_id)
    );

    // 5. Hold -> persists cashier_id in `pos_holds`.
    let res = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/pos/holds")
                .header("authorization", format!("Bearer {cashier_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&sale).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let holds = df_db::list_pos_holds(edge.pool(), Uuid::from_u128(STORE_ID))
        .await
        .unwrap();
    assert_eq!(holds.len(), 1);
    assert_eq!(holds[0].cashier_id, Some(cashier_id));
}

#[tokio::test]
async fn card_auto_voids_on_split_tender_decline_and_on_failed_db_commit() {
    // (a) Split-tender: card #1 approves, card #2 declines -> card #1 is auto-voided!
    let edge = Edge::open(edge_config("sqlite::memory:".to_string()))
        .await
        .unwrap();
    edge.ensure_configured_store(Some("US"), Some("CA"))
        .await
        .unwrap();
    seed_product(&edge).await;

    let terminal = MockTerminal::with_script(vec![
        MockOutcome::Approve,
        MockOutcome::Decline("insufficient funds".to_string()),
    ]);
    let split = sale_input(
        "1",
        vec![
            TenderInput {
                kind: TenderKind::Visa,
                amount: Some(usd("50.0000")),
                reference: None,
            },
            TenderInput {
                kind: TenderKind::Mastercard,
                amount: None,
                reference: None,
            },
        ],
    );
    let err = edge.checkout(split, &terminal).await.unwrap_err();
    assert!(
        err.to_string().contains("insufficient funds"),
        "unexpected error: {err}"
    );
    // Card #1 (`mock_1`) must have been automatically voided!
    assert_eq!(terminal.voided_refs(), vec!["mock_1".to_string()]);
    assert_eq!(
        df_db::count_transactions_for_store(edge.pool(), Uuid::from_u128(STORE_ID))
            .await
            .unwrap(),
        0
    );

    // (b) Forced DB failure after card authorization -> authorized card is auto-voided!
    // Drop `payments` table to force a DB error mid-transaction after authorization succeeds.
    sqlx::query("DROP TABLE payments")
        .execute(edge.pool())
        .await
        .unwrap();
    let terminal2 = MockTerminal::new();
    let single_card = sale_input(
        "1",
        vec![TenderInput {
            kind: TenderKind::Card,
            amount: None,
            reference: None,
        }],
    );
    assert!(edge.checkout(single_card, &terminal2).await.is_err());
    assert_eq!(terminal2.voided_refs(), vec!["mock_1".to_string()]);
}

#[tokio::test]
async fn card_return_refunds_provider_and_void_voids_card_payment() {
    let edge = Edge::open(edge_config("sqlite::memory:".to_string()))
        .await
        .unwrap();
    edge.ensure_configured_store(Some("US"), Some("CA"))
        .await
        .unwrap();
    seed_product(&edge).await;

    let terminal = Arc::new(MockTerminal::new());
    edge.set_payment_provider(terminal.clone());

    // Sale of 2 gallons paid by Visa.
    let sale = edge
        .checkout(
            sale_input(
                "2",
                vec![TenderInput {
                    kind: TenderKind::Visa,
                    amount: None,
                    reference: None,
                }],
            ),
            terminal.as_ref(),
        )
        .await
        .unwrap();
    let orig_card_ref = sale.payments[0].reference.clone();
    assert_eq!(sale.payments[0].tender, TenderKind::Visa);

    // Partial return of 1 gallon -> issues provider refund against `orig_card_ref`
    // and persists a negative refund `PaymentRow` on the return transaction.
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
    let expected_refund = Money::new(-ret.grand_total.amount(), Currency::Usd);
    assert_eq!(
        terminal.refunds(),
        vec![(orig_card_ref.clone(), expected_refund)]
    );
    assert_eq!(ret.payments.len(), 1);
    assert_eq!(ret.payments[0].status, PaymentStatus::Approved);
    assert_eq!(ret.payments[0].tender, TenderKind::Visa);
    assert_eq!(ret.payments[0].amount, expected_refund);

    let stored_ret_payments = df_db::list_payments_for_transaction(edge.pool(), ret.transaction_id)
        .await
        .unwrap();
    assert_eq!(stored_ret_payments.len(), 1);
    assert_eq!(stored_ret_payments[0].tender, "visa");
    assert_eq!(stored_ret_payments[0].amount, ret.grand_total.amount());
    assert!(stored_ret_payments[0].amount < Decimal::ZERO);

    // Voiding a card sale voids its original card reference via the provider.
    let sale2 = edge
        .checkout(
            sale_input(
                "1",
                vec![TenderInput {
                    kind: TenderKind::Mastercard,
                    amount: None,
                    reference: None,
                }],
            ),
            terminal.as_ref(),
        )
        .await
        .unwrap();
    let card2_ref = sale2.payments[0].reference.clone();
    let voided = edge.void_transaction(sale2.transaction_id).await.unwrap();
    assert_eq!(voided.status, "voided");
    assert_eq!(terminal.voided_refs(), vec![card2_ref]);
}
