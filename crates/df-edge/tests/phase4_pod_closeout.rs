//! Phase 4 exit test (POD close-out): cash sale → delivery →
//! assign → out for delivery → POD without a photo is refused up
//! front (nothing written) → POD with signature + photo closes
//! the delivery out, writes the audit event, and auto-closes the
//! linked open order. Also covers the packing slip and the
//! refusal of a direct jump to `delivered`.
//!
//! Uses the REAL crates end to end — the edge's checkout,
//! dispatch state machine (`df-delivery` via `df-db::delivery`)
//! and POD gate — against an in-memory SQLite edge. The photo is
//! a reference + SHA-256, never bytes in the database; no camera
//! hardware is involved.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::Utc;
use df_db::models::{Organization, Product, Store, Transaction};
use df_domain::{Currency, Money};
use df_edge::{
    AssignDeliveryRequest, CreateDeliveryRequest, Edge, EdgeConfig, MockTerminal, SaleInput,
    SaleLineInput, SetDeliveryStatusRequest, SubmitPodRequest, TenderInput, TenderKind,
};
use rust_decimal::Decimal;
use std::str::FromStr;
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
            map: None,
            msrp: None,
        },
    )
    .await
    .unwrap();
    edge
}

fn cash_sale_input() -> SaleInput {
    SaleInput {
        customer_id: None,
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
            kind: TenderKind::Cash,
            amount: None,
            reference: None,
        }],
        po_number: None,
        job_name: None,
        price_level_id: None,
    }
}

#[tokio::test]
async fn pod_closeout_gates_on_signature_and_photo() {
    let edge = test_edge().await;
    let terminal = MockTerminal::new();
    let cashier = Uuid::from_u128(0x6000);
    edge.open_till(cashier, money("150.0000")).await.unwrap();

    // A completed cash sale to deliver.
    let sale = edge.checkout(cash_sale_input(), &terminal).await.unwrap();
    let transaction = df_db::get_transaction(edge.pool(), sale.transaction_id)
        .await
        .unwrap();
    assert_eq!(transaction.status, "completed");
    assert_eq!(transaction.doc_type, "sale");

    // Draft delivery on the board, priority 5.
    let delivery = edge
        .create_delivery(CreateDeliveryRequest {
            transaction_id: Some(sale.transaction_id),
            customer_id: None,
            address_json: serde_json::json!({
                "street": "1 Main St",
                "city": "Vancouver",
                "postal_code": "V6B 1A1",
            }),
            scheduled_date: None,
            priority: Some(5),
            vehicle: None,
        })
        .await
        .unwrap();
    assert_eq!(delivery.status, "draft");
    assert_eq!(delivery.priority, 5);
    let board = edge.list_deliveries(None, None, None).await.unwrap();
    assert_eq!(board.len(), 1);
    assert_eq!(board[0].id, delivery.id);

    // A direct jump to delivered is refused — only a POD closes out.
    assert!(
        edge.set_delivery_status(
            delivery.id,
            SetDeliveryStatusRequest {
                status: "delivered".to_string(),
                actor_user_id: None,
                note: None,
            },
        )
        .await
        .is_err()
    );

    // Assign, then out for delivery.
    let driver = Uuid::now_v7();
    let assigned = edge
        .assign_delivery(
            delivery.id,
            AssignDeliveryRequest {
                driver_id: driver,
                vehicle: Some("Van 2".to_string()),
                actor_user_id: None,
                note: Some("morning run".to_string()),
            },
        )
        .await
        .unwrap();
    assert_eq!(assigned.status, "assigned");
    assert_eq!(assigned.driver_id, Some(driver));
    assert_eq!(assigned.vehicle.as_deref(), Some("Van 2"));
    assert_eq!(assigned.events.len(), 1);
    assert_eq!(assigned.events[0].to_status, "assigned");
    let out = edge
        .set_delivery_status(
            delivery.id,
            SetDeliveryStatusRequest {
                status: "out_for_delivery".to_string(),
                actor_user_id: None,
                note: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(out.status, "out_for_delivery");

    // Packing slip: the sale's line, rendered and stored.
    let slip = edge.packing_slip(delivery.id).await.unwrap();
    assert_eq!(slip.lines.len(), 1);
    assert_eq!(slip.lines[0].description, "Regal Select Interior Gallon");
    assert!(slip.text.contains(&sale.doc_number));
    assert!(slip.zpl.contains("^XA"));

    // POD without a photo: refused up front, nothing written.
    let signature_b64 = BASE64_STANDARD.encode(b"fake-png-signature-bytes");
    assert!(
        edge.submit_pod(
            delivery.id,
            SubmitPodRequest {
                signature_b64: signature_b64.clone(),
                photo_ref: None,
                photo_sha256: None,
                received_by: "Dana Driver".to_string(),
                signed_at: None,
                actor_user_id: None,
            },
        )
        .await
        .is_err(),
        "a signature-only POD must be refused"
    );
    let still_out = edge.get_delivery_view(delivery.id).await.unwrap();
    assert_eq!(still_out.status, "out_for_delivery");
    assert_eq!(still_out.events.len(), 2, "no audit event was written");
    assert!(edge.get_pod(delivery.id).await.unwrap().is_none());

    // A malformed photo hash is refused too.
    assert!(
        edge.submit_pod(
            delivery.id,
            SubmitPodRequest {
                signature_b64: signature_b64.clone(),
                photo_ref: Some("pod/2026-11-26/delivery-1.jpg".to_string()),
                photo_sha256: Some("not-hex".to_string()),
                received_by: "Dana Driver".to_string(),
                signed_at: None,
                actor_user_id: None,
            },
        )
        .await
        .is_err()
    );

    // Complete POD: signature + photo closes the delivery out.
    let photo_sha256 = "ab".repeat(32);
    let delivered = edge
        .submit_pod(
            delivery.id,
            SubmitPodRequest {
                signature_b64,
                photo_ref: Some("pod/2026-11-26/delivery-1.jpg".to_string()),
                photo_sha256: Some(photo_sha256.clone()),
                received_by: "Dana Driver".to_string(),
                signed_at: None,
                actor_user_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(delivered.status, "delivered");
    let pod = delivered.pod.expect("POD stored on close-out");
    assert_eq!(
        pod.photo_ref.as_deref(),
        Some("pod/2026-11-26/delivery-1.jpg")
    );
    assert_eq!(pod.photo_sha256.as_deref(), Some(photo_sha256.as_str()));
    assert_eq!(pod.received_by, "Dana Driver");
    assert_eq!(pod.photo_at, Some(pod.signed_at));
    assert_eq!(delivered.events.len(), 3);
    assert_eq!(delivered.events[2].to_status, "delivered");
    // Delivered is off the board.
    assert!(
        edge.list_deliveries(None, None, None)
            .await
            .unwrap()
            .is_empty()
    );
    // A second POD on a delivered delivery is refused.
    assert!(
        edge.submit_pod(
            delivery.id,
            SubmitPodRequest {
                signature_b64: BASE64_STANDARD.encode(b"again"),
                photo_ref: Some("pod/2026-11-26/delivery-1.jpg".to_string()),
                photo_sha256: Some(photo_sha256),
                received_by: "Dana Driver".to_string(),
                signed_at: None,
                actor_user_id: None,
            },
        )
        .await
        .is_err()
    );

    // --- Auto-close: a delivery linked to an OPEN order closes it. ---
    let open_tx = Transaction {
        id: Uuid::now_v7(),
        org_id: Uuid::from_u128(ORG_ID),
        store_id: Uuid::from_u128(STORE_ID),
        till_session_id: None,
        doc_number: "TX-OPEN-1".to_string(),
        doc_type: "sale".to_string(),
        status: "open".to_string(),
        customer_id: None,
        job_id: None,
        po_number: None,
        salesperson_id: None,
        cashier_id: None,
        currency: "USD".to_string(),
        subtotal: dec("100.0000"),
        tax_total: Decimal::ZERO,
        eco_fee_total: Decimal::ZERO,
        pos_fee_total: Decimal::ZERO,
        grand_total: dec("100.0000"),
        deposit_paid: Decimal::ZERO,
        fulfillment_mode: "delivery".to_string(),
        scheduled_at: None,
        completed_at: None,
        created_at: Utc::now(),
        job_name: None,
    };
    df_db::insert_transaction(edge.pool(), &open_tx)
        .await
        .unwrap();
    let second = edge
        .create_delivery(CreateDeliveryRequest {
            transaction_id: Some(open_tx.id),
            customer_id: None,
            address_json: serde_json::json!({"street": "2 Main St"}),
            scheduled_date: None,
            priority: None,
            vehicle: None,
        })
        .await
        .unwrap();
    edge.assign_delivery(
        second.id,
        AssignDeliveryRequest {
            driver_id: driver,
            vehicle: None,
            actor_user_id: None,
            note: None,
        },
    )
    .await
    .unwrap();
    edge.set_delivery_status(
        second.id,
        SetDeliveryStatusRequest {
            status: "out_for_delivery".to_string(),
            actor_user_id: None,
            note: None,
        },
    )
    .await
    .unwrap();
    edge.submit_pod(
        second.id,
        SubmitPodRequest {
            signature_b64: BASE64_STANDARD.encode(b"sig-2"),
            photo_ref: Some("pod/2026-11-26/delivery-2.jpg".to_string()),
            photo_sha256: Some("cd".repeat(32)),
            received_by: "Dana Driver".to_string(),
            signed_at: None,
            actor_user_id: None,
        },
    )
    .await
    .unwrap();
    let closed = df_db::get_transaction(edge.pool(), open_tx.id)
        .await
        .unwrap();
    assert_eq!(closed.status, "completed");
}
