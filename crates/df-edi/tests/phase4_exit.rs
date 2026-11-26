//! Phase 4 (v0.5) exit test for the df-edi half.
//!
//! Full round-trip, file-based, no vendor and no network:
//!
//! 1. Generate an 850 from a PO and parse it back.
//! 2. Parse a vendor 855 acknowledgment and apply it to the PO.
//! 3. Receive stock against a vendor 856 ASN.
//! 4. Match the vendor 810 invoice three-way (PO vs received vs
//!    invoiced) — a quantity variance in one case, a price
//!    variance in another, and a clean match in a third.
//! 5. Move the documents through the drop-folder transport and
//!    the file-based `EdiVendorPlugin`, with an AS2 message + MDN.
//! 6. Apply an 832 price catalog through the reviewed report.
//! 7. Persist the ack, the match reports, the catalog
//!    application and a transport record in SQLite (migration
//!    0007) and read them back with exact Decimal money.
//!
//! Nothing here posts a variance, writes a live price, or
//! contacts a vendor: the match reports and the catalog
//! application are the outputs.

use std::str::FromStr;

use chrono::{NaiveDate, Utc};
use df_db::{Dialect, connect, migrate};
use df_edi::acknowledgment::{AckCode, AckLine, PoAcknowledgment};
use df_edi::as2::{
    MdnDisposition, build_as2_message, build_mdn, parse_as2_message, parse_mdn, render_as2_message,
    render_mdn,
};
use df_edi::file_vendor::FileEdiVendorPlugin;
use df_edi::intake::{Invoice, InvoiceLine, Shipment, ShipmentLine};
use df_edi::matching::{MatchStatus, VarianceKind, match_three_way};
use df_edi::price_catalog::{
    CatalogAction, CurrentPrice, PriceApplyResult, PriceCatalog, PriceCatalogItem,
    build_price_apply_report,
};
use df_edi::purchase_order::{
    LowStockItem, generate_purchase_orders, parse_850, purchase_order_to_x12_850,
};
use df_edi::repos::{
    EdiDirection, EdiDocType, TransportRecord, get_invoice_match_report, get_po_acknowledgment,
    get_price_catalog_application, insert_invoice_match_report, insert_po_acknowledgment,
    insert_price_catalog_application, insert_purchase_order, insert_transport_record,
    list_transport_records_for_vendor,
};
use df_edi::transport::{LocalSftpStandIn, VendorTransport};
use df_edi::{
    apply_acknowledgment_to_po, invoice_to_x12_810, parse_810, parse_832, parse_855, parse_856,
    po_acknowledgment_to_x12_855, price_catalog_to_x12_832, shipment_to_x12_856,
};
use df_plugin_sdk::EdiVendorPlugin;
use rust_decimal::Decimal;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn sample_po() -> df_edi::PurchaseOrder {
    let date = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let items = vec![
        LowStockItem {
            product_id: Uuid::now_v7(),
            vendor_id: "VENDOR-A".to_string(),
            vendor_sku: "SKU-A".to_string(),
            description: "Interior Eggshell Gallon".to_string(),
            on_hand: dec("2"),
            min_level: dec("10"),
            reorder_qty: dec("12"),
            unit_cost: dec("24.5000"),
            uom: "GAL".to_string(),
        },
        LowStockItem {
            product_id: Uuid::now_v7(),
            vendor_id: "VENDOR-A".to_string(),
            vendor_sku: "SKU-B".to_string(),
            description: "Primer Gallon".to_string(),
            on_hand: dec("0"),
            min_level: dec("5"),
            reorder_qty: dec("6"),
            unit_cost: dec("15.0000"),
            uom: "GAL".to_string(),
        },
    ];
    generate_purchase_orders(&items, Uuid::now_v7(), date)
        .into_iter()
        .next()
        .unwrap()
}

fn shipment(qty_a: &str, qty_b: &str) -> Shipment {
    Shipment {
        shipment_id: "SHIP-77".to_string(),
        po_number: Some("PO-20261125-0001".to_string()),
        ship_date: NaiveDate::from_ymd_opt(2026, 11, 26),
        lines: vec![
            ShipmentLine {
                line_no: 1,
                vendor_sku: "SKU-A".to_string(),
                qty_shipped: dec(qty_a),
                uom: "GAL".to_string(),
            },
            ShipmentLine {
                line_no: 2,
                vendor_sku: "SKU-B".to_string(),
                qty_shipped: dec(qty_b),
                uom: "GAL".to_string(),
            },
        ],
    }
}

fn invoice(qty_a: &str, price_a: &str, qty_b: &str, price_b: &str) -> Invoice {
    Invoice {
        invoice_number: "INV-99".to_string(),
        po_number: Some("PO-20261125-0001".to_string()),
        invoice_date: NaiveDate::from_ymd_opt(2026, 11, 27),
        lines: vec![
            InvoiceLine {
                line_no: 1,
                vendor_sku: "SKU-A".to_string(),
                qty: dec(qty_a),
                uom: "GAL".to_string(),
                unit_price: dec(price_a),
            },
            InvoiceLine {
                line_no: 2,
                vendor_sku: "SKU-B".to_string(),
                qty: dec(qty_b),
                uom: "GAL".to_string(),
                unit_price: dec(price_b),
            },
        ],
        total: None,
    }
}

#[test]
fn exit_850_855_856_810_round_trip_with_both_variance_kinds() {
    let po = sample_po();
    assert_eq!(po.total(), dec("384.0000"));

    // 1. Generate the 850 and parse it back.
    let x12_850 = purchase_order_to_x12_850(&po, "DECORPOS", "VENDOR-A", 1);
    let po_back = parse_850(&x12_850).unwrap();
    assert_eq!(po_back.po_number, po.po_number);
    assert_eq!(po_back.total(), po.total());

    // 2. Vendor 855: SKU-A accepted in full, SKU-B changed
    // (4 of 6 at 16.0000 instead of 15.0000).
    let ack = PoAcknowledgment {
        po_number: po.po_number.clone(),
        ack_date: NaiveDate::from_ymd_opt(2026, 11, 26),
        vendor_id: Some("VENDOR-A".to_string()),
        lines: vec![
            AckLine {
                line_no: 1,
                vendor_sku: "SKU-A".to_string(),
                ack_code: AckCode::Accepted,
                qty_acknowledged: dec("12"),
                uom: "GAL".to_string(),
                unit_cost: None,
                promised_date: NaiveDate::from_ymd_opt(2026, 11, 28),
            },
            AckLine {
                line_no: 2,
                vendor_sku: "SKU-B".to_string(),
                ack_code: AckCode::Changed,
                qty_acknowledged: dec("4"),
                uom: "GAL".to_string(),
                unit_cost: Some(dec("16.0000")),
                promised_date: None,
            },
        ],
    };
    let ack_text = po_acknowledgment_to_x12_855(&ack, "VENDOR-A", "DECORPOS", 2);
    let ack_back = parse_855(&ack_text).unwrap();
    assert_eq!(ack_back, ack);
    let application = apply_acknowledgment_to_po(&po, &ack_back).unwrap();
    assert_eq!(application.accepted_lines, 1);
    assert_eq!(application.changed_lines, 1);
    assert_eq!(application.line_results[1].qty_difference, dec("-2"));
    assert!(application.line_results[1].unit_cost_changed);

    // 3. Vendor 856 ASN: everything received (12 + 6).
    let asn = shipment("12", "6");
    let asn_text = shipment_to_x12_856(&asn, "VENDOR-A", "DECORPOS", 3);
    let asn_back = parse_856(&asn_text).unwrap();
    assert_eq!(asn_back, asn);
    assert_eq!(asn_back.po_number.as_deref(), Some("PO-20261125-0001"));

    // 4a. Quantity variance: billed 14 of SKU-A, received 12.
    let qty_invoice = invoice("14", "24.5000", "6", "15.0000");
    let qty_invoice_text = invoice_to_x12_810(&qty_invoice, "VENDOR-A", "DECORPOS", 4);
    let qty_invoice_back = parse_810(&qty_invoice_text).unwrap();
    assert_eq!(qty_invoice_back.lines_total(), dec("433.0000"));
    let qty_report = match_three_way(&po, &asn_back, &qty_invoice_back);
    assert_eq!(qty_report.status, MatchStatus::Variance);
    assert_eq!(qty_report.lines[0].variance_kind, VarianceKind::Quantity);
    assert_eq!(qty_report.lines[0].qty_variance, dec("2"));
    assert_eq!(qty_report.lines[0].qty_variance_amount, dec("49.0000"));
    assert_eq!(qty_report.total_variance_amount, dec("49.0000"));

    // 4b. Price variance: billed SKU-B at 16.0000, PO cost 15.0000.
    let price_invoice = invoice("12", "24.5000", "6", "16.0000");
    let price_report = match_three_way(&po, &asn_back, &price_invoice);
    assert_eq!(price_report.status, MatchStatus::Variance);
    assert_eq!(price_report.lines[1].variance_kind, VarianceKind::Price);
    assert_eq!(price_report.lines[1].price_variance_per_unit, dec("1.0000"));
    assert_eq!(price_report.lines[1].price_variance_amount, dec("6.0000"));
    assert_eq!(price_report.total_variance_amount, dec("6.0000"));

    // 4c. Clean match: nothing to flag.
    let clean_report = match_three_way(&po, &asn_back, &invoice("12", "24.5000", "6", "15.0000"));
    assert_eq!(clean_report.status, MatchStatus::Matched);
    assert_eq!(clean_report.total_variance_amount, Decimal::ZERO);
}

#[test]
fn exit_832_catalog_apply_report_has_before_and_after() {
    let catalog = PriceCatalog {
        vendor_id: "VENDOR-A".to_string(),
        catalog_id: "CAT-2026-11".to_string(),
        catalog_date: NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
        items: vec![
            PriceCatalogItem {
                vendor_sku: "SKU-A".to_string(),
                description: "Interior Eggshell Gallon".to_string(),
                cost: Some(dec("26.0000")),
                list_price: Some(dec("52.9900")),
                uom: "GAL".to_string(),
                action: CatalogAction::Change,
                effective_date: NaiveDate::from_ymd_opt(2026, 12, 1),
            },
            PriceCatalogItem {
                vendor_sku: "SKU-NEW".to_string(),
                description: "New Primer Quart".to_string(),
                cost: Some(dec("9.2500")),
                list_price: Some(dec("18.4900")),
                uom: "QT".to_string(),
                action: CatalogAction::Add,
                effective_date: None,
            },
        ],
    };
    let text = price_catalog_to_x12_832(&catalog, "VENDOR-A", "DECORPOS", 5);
    let back = parse_832(&text).unwrap();
    assert_eq!(back, catalog);

    let current = vec![CurrentPrice {
        vendor_sku: "SKU-A".to_string(),
        cost: dec("24.5000"),
        list_price: dec("49.9900"),
    }];
    let report = build_price_apply_report(&back, &current);
    assert_eq!(report.lines[0].result, PriceApplyResult::Apply);
    assert_eq!(report.lines[0].old_cost, Some(dec("24.5000")));
    assert_eq!(report.lines[0].new_cost, Some(dec("26.0000")));
    assert_eq!(report.lines[0].old_list_price, Some(dec("49.9900")));
    assert_eq!(report.lines[0].new_list_price, Some(dec("52.9900")));
    assert_eq!(report.lines[1].result, PriceApplyResult::Add);
    // Still pending review — nothing was applied.
    assert_eq!(
        report.status,
        df_edi::price_catalog::ApplicationStatus::PendingReview
    );
}

#[tokio::test]
async fn exit_transport_as2_and_file_vendor_flow() {
    let root = std::env::temp_dir().join(format!(
        "df-edi-exit-{}-{}",
        "transport",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);

    // The 850 goes out through the drop folder (the SFTP stand-in).
    let transport = LocalSftpStandIn::new(&root);
    let po = sample_po();
    let x12_850 = purchase_order_to_x12_850(&po, "DECORPOS", "VENDOR-A", 1);
    let sent = transport
        .send("850-PO-20261125-0001.x12", &x12_850)
        .unwrap();
    assert!(sent.exists());

    // The same 850 wrapped as an AS2 message, with an MDN back.
    let as2 = build_as2_message("<msg-1@decor-pos>", "DECORPOS", "VENDOR-A", &x12_850);
    let rendered = render_as2_message(&as2);
    let parsed = parse_as2_message(&rendered).unwrap();
    assert_eq!(parsed.payload, x12_850);
    let mdn = build_mdn(&parsed.message_id, MdnDisposition::Processed);
    let mdn_back = parse_mdn(&render_mdn(&mdn)).unwrap();
    assert_eq!(mdn_back.disposition, MdnDisposition::Processed);
    assert_eq!(mdn_back.original_message_id, "<msg-1@decor-pos>");

    // The file vendor plugin: send the PO, then poll a vendor 855
    // dropped into the inbox; the second poll finds nothing.
    let plugin = FileEdiVendorPlugin::new("VENDOR-A", &root);
    let file_name = plugin.send_po(&x12_850).await.unwrap();
    assert_eq!(file_name, "850-PO-20261125-0001.x12");
    std::fs::create_dir_all(root.join("inbox")).unwrap();
    let ack = PoAcknowledgment {
        po_number: po.po_number.clone(),
        ack_date: NaiveDate::from_ymd_opt(2026, 11, 26),
        vendor_id: Some("VENDOR-A".to_string()),
        lines: vec![AckLine {
            line_no: 1,
            vendor_sku: "SKU-A".to_string(),
            ack_code: AckCode::Accepted,
            qty_acknowledged: dec("12"),
            uom: "GAL".to_string(),
            unit_cost: None,
            promised_date: None,
        }],
    };
    std::fs::write(
        root.join("inbox").join("855-PO-1.x12"),
        po_acknowledgment_to_x12_855(&ack, "VENDOR-A", "DECORPOS", 2),
    )
    .unwrap();
    let acks = plugin.poll_acknowledgments().await.unwrap();
    assert_eq!(acks.len(), 1);
    assert_eq!(parse_855(&acks[0]).unwrap(), ack);
    assert!(plugin.poll_acknowledgments().await.unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn exit_phase4_repos_round_trip_sqlite() {
    let pool = connect("sqlite::memory:").await.unwrap();
    migrate(&pool, Dialect::Sqlite).await.unwrap();

    let po = sample_po();
    insert_purchase_order(&pool, &po).await.unwrap();

    // 855 ack stored and read back.
    let ack = PoAcknowledgment {
        po_number: po.po_number.clone(),
        ack_date: NaiveDate::from_ymd_opt(2026, 11, 26),
        vendor_id: Some("VENDOR-A".to_string()),
        lines: vec![
            AckLine {
                line_no: 1,
                vendor_sku: "SKU-A".to_string(),
                ack_code: AckCode::Accepted,
                qty_acknowledged: dec("12"),
                uom: "GAL".to_string(),
                unit_cost: None,
                promised_date: NaiveDate::from_ymd_opt(2026, 11, 28),
            },
            AckLine {
                line_no: 2,
                vendor_sku: "SKU-B".to_string(),
                ack_code: AckCode::Changed,
                qty_acknowledged: dec("4"),
                uom: "GAL".to_string(),
                unit_cost: Some(dec("16.0000")),
                promised_date: None,
            },
        ],
    };
    let ack_id = insert_po_acknowledgment(&pool, po.id, &ack).await.unwrap();
    let ack_back = get_po_acknowledgment(&pool, ack_id).await.unwrap();
    assert_eq!(ack_back.po_id, po.id);
    assert_eq!(ack_back.ack.po_number, ack.po_number);
    assert_eq!(ack_back.ack.lines.len(), 2);
    assert_eq!(ack_back.ack.lines[1].unit_cost, Some(dec("16.0000")));
    assert_eq!(
        ack_back.ack.lines[0].promised_date,
        NaiveDate::from_ymd_opt(2026, 11, 28)
    );

    // Three-way match report (price-variance case) stored and read back.
    let report = match_three_way(
        &po,
        &shipment("12", "6"),
        &invoice("12", "24.5000", "6", "16.0000"),
    );
    insert_invoice_match_report(&pool, po.id, &report)
        .await
        .unwrap();
    let report_back = get_invoice_match_report(&pool, report.id).await.unwrap();
    assert_eq!(report_back.report.status, MatchStatus::Variance);
    assert_eq!(report_back.report.total_variance_amount, dec("6.0000"));
    assert_eq!(report_back.report.lines.len(), 2);
    assert_eq!(
        report_back.report.lines[1].price_variance_amount,
        dec("6.0000")
    );

    // 832 catalog application stored and read back.
    let catalog = PriceCatalog {
        vendor_id: "VENDOR-A".to_string(),
        catalog_id: "CAT-2026-11".to_string(),
        catalog_date: NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
        items: vec![PriceCatalogItem {
            vendor_sku: "SKU-A".to_string(),
            description: "Interior Eggshell Gallon".to_string(),
            cost: Some(dec("26.0000")),
            list_price: Some(dec("52.9900")),
            uom: "GAL".to_string(),
            action: CatalogAction::Change,
            effective_date: None,
        }],
    };
    let current = vec![CurrentPrice {
        vendor_sku: "SKU-A".to_string(),
        cost: dec("24.5000"),
        list_price: dec("49.9900"),
    }];
    let application = build_price_apply_report(&catalog, &current);
    insert_price_catalog_application(&pool, &application, None)
        .await
        .unwrap();
    let application_back = get_price_catalog_application(&pool, application.id)
        .await
        .unwrap();
    assert_eq!(application_back.application.lines.len(), 1);
    assert_eq!(
        application_back.application.lines[0].result,
        PriceApplyResult::Apply
    );
    assert_eq!(
        application_back.application.lines[0].new_cost,
        Some(dec("26.0000"))
    );
    assert_eq!(application_back.applied_at, None);

    // Transport record stored and listed.
    let record = TransportRecord {
        id: Uuid::now_v7(),
        vendor_id: "VENDOR-A".to_string(),
        direction: EdiDirection::Outbound,
        transport: "local-sftp-stand-in".to_string(),
        file_name: "850-PO-20261125-0001.x12".to_string(),
        doc_type: Some(EdiDocType::Po850),
        status: "sent".to_string(),
        error: None,
        created_at: Utc::now(),
    };
    insert_transport_record(&pool, &record).await.unwrap();
    let records = list_transport_records_for_vendor(&pool, "VENDOR-A")
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0], record);
}
