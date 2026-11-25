//! SQLite round-trip tests for the df-edi repos (migration 0004).
//!
//! Covers: migrations apply, PO insert/get/list with exact Decimal
//! money, and EDI document insert/get/list-by-PO with raw text +
//! parsed JSON. The PostgreSQL path is migration-embedded and
//! compile-checked only — no live Postgres runs here (same caveat as
//! df-db Phase 0/1).

use chrono::Utc;
use df_db::{Dialect, connect, migrate};
use df_edi::purchase_order::{LowStockItem, generate_purchase_orders};
use df_edi::repos::{
    EdiDirection, EdiDocType, EdiDocument, get_edi_document, get_purchase_order,
    insert_edi_document, insert_purchase_order, list_edi_documents_for_po,
    list_purchase_orders_for_vendor,
};
use df_edi::{Shipment, parse_856};
use rust_decimal::Decimal;
use serde_json::json;
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

#[tokio::test]
async fn purchase_order_round_trips_with_exact_money() {
    let pool = test_pool().await;
    let store = Uuid::now_v7();
    let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let items = vec![LowStockItem {
        product_id: Uuid::now_v7(),
        vendor_id: "VENDOR-A".to_string(),
        vendor_sku: "SKU-A".to_string(),
        description: "Interior Eggshell Gallon".to_string(),
        on_hand: dec("2"),
        min_level: dec("10"),
        reorder_qty: dec("12"),
        unit_cost: dec("24.5000"),
        uom: "GAL".to_string(),
    }];
    let pos = generate_purchase_orders(&items, store, date);
    assert_eq!(pos.len(), 1);
    let po = &pos[0];

    insert_purchase_order(&pool, po).await.unwrap();
    let back = get_purchase_order(&pool, po.id).await.unwrap();
    assert_eq!(back, *po);
    assert_eq!(back.total(), dec("294.0000"));

    let listed = list_purchase_orders_for_vendor(&pool, "VENDOR-A")
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, po.id);
    assert!(
        list_purchase_orders_for_vendor(&pool, "NOBODY")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn edi_document_round_trips_and_links_to_po() {
    let pool = test_pool().await;
    let store = Uuid::now_v7();
    let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let items = vec![LowStockItem {
        product_id: Uuid::now_v7(),
        vendor_id: "VENDOR-A".to_string(),
        vendor_sku: "SKU-A".to_string(),
        description: "Interior Eggshell Gallon".to_string(),
        on_hand: dec("0"),
        min_level: dec("5"),
        reorder_qty: dec("6"),
        unit_cost: dec("24.5000"),
        uom: "GAL".to_string(),
    }];
    let po = generate_purchase_orders(&items, store, date).pop().unwrap();
    insert_purchase_order(&pool, &po).await.unwrap();

    let raw = "ST*856*1~BSN*00*SHIP-77*20261125*0830~LIN*1*VP*SKU-A~SN1**6*GAL~SE*6*1~";
    let shipment: Shipment = parse_856(raw).unwrap();
    let doc = EdiDocument {
        id: Uuid::now_v7(),
        doc_type: EdiDocType::Asn856,
        direction: EdiDirection::Inbound,
        vendor_id: "VENDOR-A".to_string(),
        related_po_id: Some(po.id),
        raw_text: raw.to_string(),
        parsed_json: json!(&shipment),
        received_at: Utc::now(),
    };
    insert_edi_document(&pool, &doc).await.unwrap();
    let back = get_edi_document(&pool, doc.id).await.unwrap();
    assert_eq!(back, doc);
    assert_eq!(back.raw_text, raw);
    assert_eq!(back.parsed_json["shipment_id"], json!("SHIP-77"));

    let linked = list_edi_documents_for_po(&pool, po.id).await.unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].id, doc.id);
}
