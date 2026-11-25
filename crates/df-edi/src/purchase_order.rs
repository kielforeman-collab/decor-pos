//! Purchase orders: low-stock generation, X12 850 builder, CSV
//! serialization, and 850 parsing (round-trip).
//!
//! Money is `rust_decimal::Decimal` throughout — no floats.

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::csv::render_csv_row;
use crate::error::{EdiError, Result};
use crate::x12::{
    Segment, envelope_close, envelope_open, find, parse_document, parse_x12_date, render_document,
    require, wrap_transaction,
};

/// Purchase-order lifecycle status (stored as text in the repos).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PoStatus {
    Draft,
    Sent,
    Acknowledged,
    Received,
    Cancelled,
}

impl PoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Sent => "sent",
            Self::Acknowledged => "acknowledged",
            Self::Received => "received",
            Self::Cancelled => "cancelled",
        }
    }
}

impl std::str::FromStr for PoStatus {
    type Err = EdiError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "draft" => Ok(Self::Draft),
            "sent" => Ok(Self::Sent),
            "acknowledged" => Ok(Self::Acknowledged),
            "received" => Ok(Self::Received),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(EdiError::Parse(format!("unknown PO status {other:?}"))),
        }
    }
}

/// One line of a purchase order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurchaseOrderLine {
    pub line_no: u32,
    pub product_id: Uuid,
    pub vendor_sku: String,
    pub description: String,
    pub qty: Decimal,
    pub uom: String,
    pub unit_cost: Decimal,
}

impl PurchaseOrderLine {
    /// Exact line total (`qty * unit_cost`, Decimal — no float).
    pub fn line_total(&self) -> Decimal {
        self.qty * self.unit_cost
    }
}

/// A purchase order document: header + lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PurchaseOrder {
    /// UUIDv7 id.
    pub id: Uuid,
    /// Human PO number, e.g. `PO-20261125-0001`.
    pub po_number: String,
    /// Vendor identifier (the vendor's EDI id / account code).
    pub vendor_id: String,
    pub store_id: Uuid,
    pub status: PoStatus,
    pub lines: Vec<PurchaseOrderLine>,
    pub created_at: DateTime<Utc>,
}

impl PurchaseOrder {
    /// Exact order total: the sum of the line totals.
    pub fn total(&self) -> Decimal {
        self.lines
            .iter()
            .map(PurchaseOrderLine::line_total)
            .fold(Decimal::ZERO, |acc, amount| acc + amount)
    }
}

/// One low-stock input to replenishment: a product's on-hand quantity
/// against its minimum level, with the vendor that supplies it.
#[derive(Debug, Clone, PartialEq)]
pub struct LowStockItem {
    pub product_id: Uuid,
    pub vendor_id: String,
    pub vendor_sku: String,
    pub description: String,
    pub on_hand: Decimal,
    pub min_level: Decimal,
    /// Quantity to order when below minimum (before the shortfall is
    /// considered — the order quantity is `max(reorder_qty, shortfall)`).
    pub reorder_qty: Decimal,
    pub unit_cost: Decimal,
    pub uom: String,
}

/// Generate purchase orders from low-stock items.
///
/// * Items at or above their minimum level are skipped.
/// * Items below minimum order `max(reorder_qty, min_level - on_hand)`.
/// * Items are grouped by `vendor_id`, one PO per vendor, in first-seen
///   vendor order. PO numbers are `PO-<YYYYMMDD>-<seq>` with a global
///   sequence across vendors.
/// * An empty (or fully in-stock) input yields no POs.
pub fn generate_purchase_orders(
    items: &[LowStockItem],
    store_id: Uuid,
    date: NaiveDate,
) -> Vec<PurchaseOrder> {
    let mut vendors: Vec<String> = Vec::new();
    let mut by_vendor: Vec<Vec<&LowStockItem>> = Vec::new();
    for item in items {
        if item.on_hand >= item.min_level {
            continue;
        }
        let shortfall = item.min_level - item.on_hand;
        if shortfall <= Decimal::ZERO && item.reorder_qty <= Decimal::ZERO {
            continue;
        }
        let position = match vendors.iter().position(|v| v == &item.vendor_id) {
            Some(position) => position,
            None => {
                vendors.push(item.vendor_id.clone());
                by_vendor.push(Vec::new());
                vendors.len() - 1
            }
        };
        by_vendor[position].push(item);
    }

    let now = Utc::now();
    vendors
        .into_iter()
        .enumerate()
        .map(|(index, vendor_id)| {
            let lines = by_vendor[index]
                .iter()
                .enumerate()
                .map(|(line_index, item)| {
                    let shortfall = item.min_level - item.on_hand;
                    let qty = if item.reorder_qty > shortfall {
                        item.reorder_qty
                    } else {
                        shortfall
                    };
                    PurchaseOrderLine {
                        line_no: (line_index + 1) as u32,
                        product_id: item.product_id,
                        vendor_sku: item.vendor_sku.clone(),
                        description: item.description.clone(),
                        qty,
                        uom: item.uom.clone(),
                        unit_cost: item.unit_cost,
                    }
                })
                .collect();
            PurchaseOrder {
                id: Uuid::now_v7(),
                po_number: format!("PO-{}-{:04}", date.format("%Y%m%d"), index + 1),
                vendor_id,
                store_id,
                status: PoStatus::Draft,
                lines,
                created_at: now,
            }
        })
        .collect()
}

/// Serialize a purchase order as an X12 850 (basic segment builder).
///
/// Envelope: `ISA/GS/ST/BEG/PO1…/SE/GE/IEA` with real control numbers
/// (`control` is used for ISA13, GS06, ST02, SE02) and a correct `SE`
/// segment count. `sender_id` is our EDI id; `receiver_id` is the
/// vendor's EDI id (defaults to the PO's `vendor_id` when empty).
/// The PO date is taken from `po.created_at`.
pub fn purchase_order_to_x12_850(
    po: &PurchaseOrder,
    sender_id: &str,
    receiver_id: &str,
    control: u32,
) -> String {
    let receiver = if receiver_id.is_empty() {
        po.vendor_id.as_str()
    } else {
        receiver_id
    };
    let date = po.created_at.date_naive();
    let time = po.created_at.time();
    let mut segments = envelope_open("PO", sender_id, receiver, date, time, control);
    let mut body = vec![Segment::new(
        "BEG",
        &[
            "00",                        // transaction set purpose: original
            "NE",                        // type: new order
            &po.po_number,               // BEG03 purchase order number
            "",                          // BEG04 release number (unused)
            &crate::x12::x12_date(date), // BEG05 date
        ],
    )];
    for line in &po.lines {
        body.push(Segment::new(
            "PO1",
            &[
                &line.line_no.to_string(),
                &line.qty.to_string(),
                &line.uom,
                &line.unit_cost.to_string(),
                "",   // PO1-05 basis of unit price code (unused)
                "VP", // PO1-06 product id qualifier: vendor's part number
                &line.vendor_sku,
            ],
        ));
    }
    segments.extend(wrap_transaction("850", control, &body));
    segments.extend(envelope_close(control, control));
    render_document(&segments)
}

/// Serialize a purchase order as CSV (header + one row per line).
pub fn purchase_order_to_csv(po: &PurchaseOrder) -> String {
    let mut out = String::new();
    out.push_str(
        "po_number,vendor_id,line_no,vendor_sku,description,qty,uom,unit_cost,line_total\n",
    );
    for line in &po.lines {
        out.push_str(&render_csv_row(&[
            &po.po_number,
            &po.vendor_id,
            &line.line_no.to_string(),
            &line.vendor_sku,
            &line.description,
            &line.qty.to_string(),
            &line.uom,
            &line.unit_cost.to_string(),
            &line.line_total().to_string(),
        ]));
        out.push('\n');
    }
    out
}

fn parse_decimal_field(value: &str, what: &str) -> Result<Decimal> {
    Decimal::from_str(value.trim())
        .map_err(|e| EdiError::InvalidDecimal(format!("{what}: {value:?}"), e))
}

/// Parse an X12 850 document back into a [`PurchaseOrder`].
///
/// Round-trips the meaningful fields: PO number, vendor (from the GS
/// receiver / ISA08), date, and every PO1 line (line number, quantity,
/// UOM, unit cost, vendor SKU). Fields X12 cannot carry — `product_id`
/// and `store_id` — come back as nil UUIDs, and `status` comes back as
/// `Draft`; callers that need them must keep the original PO (or the
/// `edi_documents.related_po_id` link). The `BEG` date becomes
/// `created_at` at midnight UTC.
pub fn parse_850(text: &str) -> Result<PurchaseOrder> {
    let segments = parse_document(text)?;
    let st = require(&segments, "ST")?;
    if st.get(1) != Some("850") {
        return Err(EdiError::Parse(format!(
            "expected transaction set 850, found {:?}",
            st.get(1)
        )));
    }
    let beg = require(&segments, "BEG")?;
    let po_number = beg
        .get_trimmed(3)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| EdiError::Parse("BEG03 purchase order number is missing".to_string()))?
        .to_string();
    let date = match beg.get_trimmed(5) {
        Some(value) if !value.is_empty() => parse_x12_date(value)?,
        _ => {
            return Err(EdiError::Parse(
                "BEG05 purchase order date is missing".to_string(),
            ));
        }
    };
    // Vendor = the envelope receiver: GS03, falling back to ISA08.
    let vendor_id = find(&segments, "GS")
        .and_then(|gs| gs.get_trimmed(3))
        .filter(|s| !s.is_empty())
        .or_else(|| find(&segments, "ISA").and_then(|isa| isa.get_trimmed(8)))
        .unwrap_or("")
        .to_string();
    let mut lines = Vec::new();
    for segment in &segments {
        if segment.tag != "PO1" {
            continue;
        }
        let line_no: u32 = segment.get_trimmed(1).unwrap_or("0").parse().map_err(|_| {
            EdiError::Parse(format!("PO1 line number is not an integer: {segment:?}"))
        })?;
        let qty = parse_decimal_field(segment.get(2).unwrap_or(""), "PO1 quantity")?;
        let uom = segment.get_trimmed(3).unwrap_or("").to_string();
        let unit_cost = parse_decimal_field(segment.get(4).unwrap_or(""), "PO1 unit price")?;
        // PO1-06 qualifier should be VP; the SKU is PO1-07. Accept a
        // bare 6-element PO1 (no qualifier) as the SKU too.
        let vendor_sku = match segment.get_trimmed(6) {
            Some("VP") => segment.get_trimmed(7).unwrap_or("").to_string(),
            Some(other) if !other.is_empty() => other.to_string(),
            _ => segment.get_trimmed(7).unwrap_or("").to_string(),
        };
        lines.push(PurchaseOrderLine {
            line_no,
            product_id: Uuid::nil(),
            vendor_sku,
            description: String::new(),
            qty,
            uom,
            unit_cost,
        });
    }
    if lines.is_empty() {
        return Err(EdiError::MissingSegment("PO1"));
    }
    Ok(PurchaseOrder {
        id: Uuid::now_v7(),
        po_number,
        vendor_id,
        store_id: Uuid::nil(),
        status: PoStatus::Draft,
        lines,
        created_at: date.and_hms_opt(0, 0, 0).unwrap().and_utc(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn low_stock(vendor: &str, on_hand: &str, min: &str) -> LowStockItem {
        LowStockItem {
            product_id: Uuid::now_v7(),
            vendor_id: vendor.to_string(),
            vendor_sku: format!("SKU-{vendor}"),
            description: "Test paint".to_string(),
            on_hand: dec(on_hand),
            min_level: dec(min),
            reorder_qty: dec("12"),
            unit_cost: dec("24.5000"),
            uom: "EA".to_string(),
        }
    }

    #[test]
    fn low_stock_generation_groups_by_vendor_and_skips_in_stock() {
        let store = Uuid::now_v7();
        let date = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let items = vec![
            low_stock("VENDOR-A", "2", "10"),  // shortfall 8 < reorder 12 -> 12
            low_stock("VENDOR-A", "20", "10"), // in stock -> skipped
            low_stock("VENDOR-B", "0", "5"),   // shortfall 5 < reorder 12 -> 12
        ];
        let pos = generate_purchase_orders(&items, store, date);
        assert_eq!(pos.len(), 2);
        assert_eq!(pos[0].vendor_id, "VENDOR-A");
        assert_eq!(pos[0].lines.len(), 1);
        assert_eq!(pos[0].lines[0].qty, dec("12"));
        assert_eq!(pos[0].po_number, "PO-20261125-0001");
        assert_eq!(pos[1].po_number, "PO-20261125-0002");
        assert_eq!(pos[0].total(), dec("294.0000"));
    }

    #[test]
    fn reorder_qty_below_shortfall_orders_the_shortfall() {
        let store = Uuid::now_v7();
        let date = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let mut item = low_stock("VENDOR-A", "1", "30");
        item.reorder_qty = dec("4");
        let pos = generate_purchase_orders(&[item], store, date);
        assert_eq!(pos[0].lines[0].qty, dec("29"));
    }

    #[test]
    fn empty_input_yields_no_pos() {
        let pos = generate_purchase_orders(
            &[],
            Uuid::now_v7(),
            NaiveDate::from_ymd_opt(2026, 11, 25).unwrap(),
        );
        assert!(pos.is_empty());
    }

    #[test]
    fn x12_850_round_trip() {
        let store = Uuid::now_v7();
        let date = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let pos = generate_purchase_orders(
            &[
                low_stock("VENDOR-A", "2", "10"),
                low_stock("VENDOR-A", "0", "6"),
            ],
            store,
            date,
        );
        assert_eq!(pos.len(), 1);
        let po = &pos[0];
        let text = purchase_order_to_x12_850(po, "DECORPOS", "VENDOR-A", 42);
        assert!(text.contains("ISA*00*"));
        assert!(text.contains("ST*850*42~"));
        assert!(text.contains("BEG*00*NE*PO-20261125-0001**20261125~"));
        // SE01 counts ST + BEG + 2x PO1 + SE = 5.
        assert!(text.contains("SE*5*42~"));
        assert!(text.contains("GE*1*42~"));
        assert!(text.contains("IEA*1*000000042~"));

        let back = parse_850(&text).unwrap();
        assert_eq!(back.po_number, po.po_number);
        assert_eq!(back.vendor_id, "VENDOR-A");
        assert_eq!(back.lines.len(), 2);
        assert_eq!(back.lines[0].qty, dec("12"));
        assert_eq!(back.lines[0].unit_cost, dec("24.5000"));
        assert_eq!(back.lines[0].vendor_sku, "SKU-VENDOR-A");
        assert_eq!(back.lines[0].uom, "EA");
        assert_eq!(back.total(), po.total());
    }

    #[test]
    fn parse_850_rejects_wrong_set_and_missing_beg() {
        assert!(parse_850("ST*856*1~BSN*00*S1*20261125~").is_err());
        assert!(parse_850("ST*850*1~SE*2*1~").is_err());
    }

    #[test]
    fn csv_serialization_has_header_and_line_totals() {
        let store = Uuid::now_v7();
        let date = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let pos = generate_purchase_orders(&[low_stock("VENDOR-A", "2", "10")], store, date);
        let csv = purchase_order_to_csv(&pos[0]);
        assert!(csv.starts_with("po_number,vendor_id,line_no,"));
        assert!(csv.contains("294.0000"));
    }
}
