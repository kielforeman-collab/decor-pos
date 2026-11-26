//! Phase 4 exit test (EDI round-trip): PO → 850 send via the
//! local transport stand-in → 855/856/810 intake built with the
//! `df-edi` builders → 3-way match `matched` with matching
//! prices/qtys → 832 catalog apply report pending review, then
//! marked applied.
//!
//! Uses the REAL crates end to end — `df-edi` X12 builders and
//! parsers, the `LocalSftpStandIn` transport, the edge's EDI
//! wiring — against an in-memory SQLite edge. No live vendor,
//! no network: the staged 850 only ever lands in a temp
//! directory's `outbox/`.

use chrono::{NaiveDate, Utc};
use df_db::models::{Organization, Product, Store};
use df_domain::{Currency, Money};
use df_edge::{Edge, EdgeConfig, IntakeRequest};
use df_edi::{
    AckCode, AckLine, CatalogAction, Invoice, InvoiceLine, PoAcknowledgment, PoStatus,
    PriceCatalog, PriceCatalogItem, PurchaseOrder, PurchaseOrderLine, Shipment, ShipmentLine,
    invoice_to_x12_810, po_acknowledgment_to_x12_855, price_catalog_to_x12_832,
    shipment_to_x12_856,
};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;
const VENDOR: &str = "VENDOR1";
const PO_NUMBER: &str = "PO-20261125-0001";

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
    // The product the 832 apply report compares against: cost $50,
    // list $100 — the catalog below reprices it to $55 / $110.
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

fn draft_po() -> PurchaseOrder {
    PurchaseOrder {
        id: Uuid::now_v7(),
        po_number: PO_NUMBER.to_string(),
        vendor_id: VENDOR.to_string(),
        store_id: Uuid::from_u128(STORE_ID),
        status: PoStatus::Draft,
        lines: vec![
            PurchaseOrderLine {
                line_no: 1,
                product_id: Uuid::now_v7(),
                vendor_sku: "SKU-A".to_string(),
                description: "Primer, 1 gal".to_string(),
                qty: dec("10"),
                uom: "EA".to_string(),
                unit_cost: dec("24.5000"),
            },
            PurchaseOrderLine {
                line_no: 2,
                product_id: Uuid::now_v7(),
                vendor_sku: "SKU-B".to_string(),
                description: "Topcoat, 1 gal".to_string(),
                qty: dec("4"),
                uom: "GAL".to_string(),
                unit_cost: dec("58.0000"),
            },
        ],
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn edi_round_trip_850_855_856_810_match_832() {
    let edge = test_edge().await;
    let pool = edge.pool();
    let po = draft_po();
    df_edi::repos::insert_purchase_order(pool, &po)
        .await
        .unwrap();

    // --- 850 send via the transport stand-in. ---
    let root = std::env::temp_dir().join(format!("decor-pos-edi-test-{}", Uuid::now_v7()));
    let sent = edge
        .send_purchase_order(po.id, Some(root.to_string_lossy().to_string()))
        .await
        .unwrap();
    assert_eq!(sent.po_number, PO_NUMBER);
    assert!(sent.x12_850.contains(PO_NUMBER));
    let staged = sent.written_to.expect("850 staged to a file");
    let staged_text = std::fs::read_to_string(&staged).unwrap();
    assert!(staged_text.contains("ISA"));
    assert!(staged_text.contains(PO_NUMBER));
    // The transport audit trail records the send.
    let records = edge.list_transport_records(VENDOR).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, "sent");
    assert_eq!(records[0].file_name, format!("{PO_NUMBER}.850.x12"));
    // The PO flipped to sent — and a sent PO cannot be sent again.
    let stored = df_edi::repos::get_purchase_order(pool, po.id)
        .await
        .unwrap();
    assert_eq!(stored.status.as_str(), "sent");
    assert!(
        edge.send_purchase_order(po.id, None).await.is_err(),
        "only a draft PO can be sent"
    );

    // --- 855 acknowledgment: 1 accepted, 1 changed (short). ---
    let ack = PoAcknowledgment {
        po_number: PO_NUMBER.to_string(),
        ack_date: Some(date(2026, 11, 25)),
        vendor_id: Some(VENDOR.to_string()),
        lines: vec![
            AckLine {
                line_no: 1,
                vendor_sku: "SKU-A".to_string(),
                ack_code: AckCode::Accepted,
                qty_acknowledged: dec("10"),
                uom: "EA".to_string(),
                unit_cost: None,
                promised_date: None,
            },
            AckLine {
                line_no: 2,
                vendor_sku: "SKU-B".to_string(),
                ack_code: AckCode::Changed,
                qty_acknowledged: dec("3"),
                uom: "GAL".to_string(),
                unit_cost: Some(dec("58.0000")),
                promised_date: None,
            },
        ],
    };
    let ack_x12 = po_acknowledgment_to_x12_855(&ack, VENDOR, "DECORPOS", 1);
    let intake = edge
        .intake_edi(IntakeRequest {
            doc_type: "855".to_string(),
            raw_text: ack_x12,
            vendor_id: VENDOR.to_string(),
            related_po_id: None,
        })
        .await
        .unwrap();
    assert!(intake.parsed);
    assert_eq!(intake.related_po_id, Some(po.id));
    let application = intake.ack_application.expect("ack application reported");
    assert_eq!(application.accepted_lines, 1);
    assert_eq!(application.changed_lines, 1);
    assert_eq!(application.rejected_lines, 0);
    assert_eq!(application.line_results.len(), 2);
    // The PO itself is untouched by the ack.
    let stored = df_edi::repos::get_purchase_order(pool, po.id)
        .await
        .unwrap();
    assert_eq!(stored.status.as_str(), "sent");
    assert_eq!(stored.lines[1].qty, dec("4"));
    let acks = edge.list_po_acknowledgments(po.id).await.unwrap();
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].ack.po_number, PO_NUMBER);

    // --- 856 ASN: the X12 form carries no PO number, so link it. ---
    let shipment = Shipment {
        shipment_id: "SHIP-77".to_string(),
        po_number: None,
        ship_date: Some(date(2026, 11, 25)),
        lines: vec![
            ShipmentLine {
                line_no: 1,
                vendor_sku: "SKU-A".to_string(),
                qty_shipped: dec("10"),
                uom: "EA".to_string(),
            },
            ShipmentLine {
                line_no: 2,
                vendor_sku: "SKU-B".to_string(),
                qty_shipped: dec("4"),
                uom: "GAL".to_string(),
            },
        ],
    };
    let asn_x12 = shipment_to_x12_856(&shipment, VENDOR, "DECORPOS", 2);
    let intake = edge
        .intake_edi(IntakeRequest {
            doc_type: "856".to_string(),
            raw_text: asn_x12,
            vendor_id: VENDOR.to_string(),
            related_po_id: Some(po.id),
        })
        .await
        .unwrap();
    assert!(intake.parsed);
    assert_eq!(intake.related_po_id, Some(po.id));

    // --- 810 invoice: the vendor echoes the PO number, auto-link. ---
    let invoice = Invoice {
        invoice_number: "INV-99".to_string(),
        po_number: Some(PO_NUMBER.to_string()),
        invoice_date: Some(date(2026, 11, 20)),
        lines: vec![
            InvoiceLine {
                line_no: 1,
                vendor_sku: "SKU-A".to_string(),
                qty: dec("10"),
                uom: "EA".to_string(),
                unit_price: dec("24.5000"),
            },
            InvoiceLine {
                line_no: 2,
                vendor_sku: "SKU-B".to_string(),
                qty: dec("4"),
                uom: "GAL".to_string(),
                unit_price: dec("58.0000"),
            },
        ],
        total: Some(dec("477.0000")),
    };
    let invoice_x12 = invoice_to_x12_810(&invoice, VENDOR, "DECORPOS", 3);
    let intake = edge
        .intake_edi(IntakeRequest {
            doc_type: "810".to_string(),
            raw_text: invoice_x12,
            vendor_id: VENDOR.to_string(),
            related_po_id: None,
        })
        .await
        .unwrap();
    assert!(intake.parsed);
    assert_eq!(intake.related_po_id, Some(po.id));

    // --- 3-way match: ordered == shipped == billed. ---
    let report = edge.three_way_match(po.id).await.unwrap();
    assert_eq!(report.status, df_edi::MatchStatus::Matched);
    assert_eq!(report.po_total, dec("477.0000"));
    assert_eq!(report.invoiced_total, dec("477.0000"));
    assert_eq!(report.total_variance_amount, Decimal::ZERO);
    assert_eq!(report.lines.len(), 2);
    assert!(
        report
            .lines
            .iter()
            .all(|line| line.status == df_edi::MatchStatus::Matched),
        "every line matched: {report:?}"
    );
    // Persisted for later review.
    let stored_reports = edge.list_match_reports(po.id).await.unwrap();
    assert_eq!(stored_reports.len(), 1);
    assert_eq!(stored_reports[0].report.id, report.id);

    // --- 832 catalog: reprice one SKU, add one, drop an unknown. ---
    let catalog = PriceCatalog {
        vendor_id: VENDOR.to_string(),
        catalog_id: "CAT-2026-11".to_string(),
        catalog_date: date(2026, 11, 25),
        items: vec![
            PriceCatalogItem {
                vendor_sku: "BM-REGAL-1G".to_string(),
                description: "Regal Select Interior Gallon".to_string(),
                cost: Some(dec("55.0000")),
                list_price: Some(dec("110.0000")),
                uom: "gal".to_string(),
                action: CatalogAction::Change,
                effective_date: None,
            },
            PriceCatalogItem {
                vendor_sku: "BM-NEW-1G".to_string(),
                description: "New product".to_string(),
                cost: Some(dec("40.0000")),
                list_price: Some(dec("80.0000")),
                uom: "gal".to_string(),
                action: CatalogAction::Add,
                effective_date: None,
            },
            PriceCatalogItem {
                vendor_sku: "BM-GONE-1G".to_string(),
                description: "Dropped product".to_string(),
                cost: None,
                list_price: None,
                uom: "gal".to_string(),
                action: CatalogAction::Drop,
                effective_date: None,
            },
        ],
    };
    let catalog_x12 = price_catalog_to_x12_832(&catalog, VENDOR, "DECORPOS", 4);
    let intake = edge
        .intake_edi(IntakeRequest {
            doc_type: "832".to_string(),
            raw_text: catalog_x12,
            vendor_id: VENDOR.to_string(),
            related_po_id: None,
        })
        .await
        .unwrap();
    assert!(intake.parsed);
    let application_id = intake
        .price_application_id
        .expect("832 intake stores a price-apply report");
    let row = edge
        .get_price_catalog_application(application_id)
        .await
        .unwrap();
    assert_eq!(
        row.application.status,
        df_edi::ApplicationStatus::PendingReview
    );
    let by_sku = |sku: &str| {
        row.application
            .lines
            .iter()
            .find(|line| line.vendor_sku == sku)
            .unwrap_or_else(|| panic!("no apply line for {sku}"))
    };
    assert_eq!(
        by_sku("BM-REGAL-1G").result,
        df_edi::PriceApplyResult::Apply
    );
    assert_eq!(by_sku("BM-NEW-1G").result, df_edi::PriceApplyResult::Add);
    assert_eq!(
        by_sku("BM-GONE-1G").result,
        df_edi::PriceApplyResult::Skipped
    );
    // Marking applied records the review decision — and the
    // product's own prices are untouched (no repricing here).
    let applied = edge
        .mark_price_application_applied(application_id, "reviewer-1")
        .await
        .unwrap();
    assert_eq!(
        applied.application.status,
        df_edi::ApplicationStatus::Applied
    );
    assert_eq!(
        applied.application.reviewed_by.as_deref(),
        Some("reviewer-1")
    );
    let product = df_db::get_product(pool, Uuid::from_u128(PRODUCT_ID))
        .await
        .unwrap();
    assert_eq!(product.standard_cost, dec("50.0000"));
    assert_eq!(product.list_price, dec("100.0000"));

    // An 855 for a PO we do not hold is a 400, not a silent drop.
    let stray = PoAcknowledgment {
        po_number: "PO-NOPE-1".to_string(),
        ack_date: None,
        vendor_id: Some(VENDOR.to_string()),
        lines: vec![],
    };
    assert!(
        edge.intake_edi(IntakeRequest {
            doc_type: "855".to_string(),
            raw_text: po_acknowledgment_to_x12_855(&stray, VENDOR, "DECORPOS", 5),
            vendor_id: VENDOR.to_string(),
            related_po_id: None,
        })
        .await
        .is_err()
    );

    std::fs::remove_dir_all(&root).ok();
    // `Money` is referenced so the import stays honest: the PO total
    // is $477.00.
    assert_eq!(
        Money::from_str_amount("477.0000", Currency::Usd)
            .unwrap()
            .amount(),
        dec("477.0000")
    );
}
