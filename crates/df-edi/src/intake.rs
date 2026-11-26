//! ASN (X12 856) and invoice (X12 810) intake parsing, plus the CSV
//! equivalents for file-based vendor drops.
//!
//! Parsers are strict about structure: a wrong transaction set, a
//! missing header segment, or a quantity line without its item line is
//! an error — never a silently partial document.

use std::str::FromStr;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::csv::{field_by_name, parse_csv};
use crate::error::{EdiError, Result};
use crate::x12::{
    Segment, envelope_close, envelope_open, find, parse_document, parse_x12_date, render_document,
    require, wrap_transaction, x12_date,
};

fn parse_decimal_field(value: &str, what: &str) -> Result<Decimal> {
    Decimal::from_str(value.trim())
        .map_err(|e| EdiError::InvalidDecimal(format!("{what}: {value:?}"), e))
}

fn parse_optional_x12_date(value: Option<&str>) -> Result<Option<NaiveDate>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(value) => parse_x12_date(value).map(Some),
    }
}

// --- ASN (856) -------------------------------------------------------------

/// One shipped line on an ASN.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShipmentLine {
    pub line_no: u32,
    pub vendor_sku: String,
    pub qty_shipped: Decimal,
    pub uom: String,
}

/// A shipment notice: header + shipped lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shipment {
    /// Shipment identification (BSN02).
    pub shipment_id: String,
    /// Our PO number, when the vendor echoes it (BSN / HL references
    /// are not standardized across vendors, so this is `None` unless
    /// the document carries it — see `parse_856_csv` for the CSV form
    /// that always carries it).
    pub po_number: Option<String>,
    pub ship_date: Option<NaiveDate>,
    pub lines: Vec<ShipmentLine>,
}

/// Parse an X12 856 (ship notice / manifest).
///
/// Expected segments: `ST*856`, `BSN` (purpose, shipment id, date,
/// time), optional `REF*PO*<po number>` (the PO this shipment
/// answers — when present it becomes [`Shipment::po_number`]),
/// then repeating `HL` / `LIN` / `SN1` groups. A line is a
/// `LIN` (item, `VP` + vendor SKU) followed by its `SN1` (quantity
/// shipped). An `SN1` with no preceding `LIN`, or a `LIN` with no
/// following `SN1`, is an error.
pub fn parse_856(text: &str) -> Result<Shipment> {
    let segments = parse_document(text)?;
    let st = require(&segments, "ST")?;
    if st.get(1) != Some("856") {
        return Err(EdiError::Parse(format!(
            "expected transaction set 856, found {:?}",
            st.get(1)
        )));
    }
    let bsn = require(&segments, "BSN")?;
    let shipment_id = bsn
        .get_trimmed(2)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| EdiError::Parse("BSN02 shipment identification is missing".to_string()))?
        .to_string();
    let ship_date = parse_optional_x12_date(bsn.get(3))?;
    let po_number = segments
        .iter()
        .find(|segment| segment.tag == "REF" && segment.get_trimmed(1) == Some("PO"))
        .and_then(|segment| segment.get_trimmed(2))
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut lines: Vec<ShipmentLine> = Vec::new();
    let mut pending: Option<ShipmentLine> = None;
    for segment in &segments {
        match segment.tag.as_str() {
            "LIN" => {
                if let Some(unfinished) = pending.take() {
                    return Err(EdiError::Parse(format!(
                        "LIN for SKU {:?} has no SN1 quantity",
                        unfinished.vendor_sku
                    )));
                }
                let line_no: u32 = segment.get_trimmed(1).unwrap_or("0").parse().map_err(|_| {
                    EdiError::Parse(format!("LIN line number is not an integer: {segment:?}"))
                })?;
                // LIN02 qualifier (usually VP), LIN03 the vendor SKU.
                let vendor_sku = segment.get_trimmed(3).unwrap_or("").to_string();
                pending = Some(ShipmentLine {
                    line_no,
                    vendor_sku,
                    qty_shipped: Decimal::ZERO,
                    uom: String::new(),
                });
            }
            "SN1" => {
                let Some(mut line) = pending.take() else {
                    return Err(EdiError::Parse(
                        "SN1 quantity with no preceding LIN segment".to_string(),
                    ));
                };
                line.qty_shipped =
                    parse_decimal_field(segment.get(2).unwrap_or(""), "SN1 quantity")?;
                line.uom = segment.get_trimmed(3).unwrap_or("").to_string();
                lines.push(line);
            }
            _ => {}
        }
    }
    if let Some(unfinished) = pending {
        return Err(EdiError::Parse(format!(
            "LIN for SKU {:?} has no SN1 quantity",
            unfinished.vendor_sku
        )));
    }
    if lines.is_empty() {
        return Err(EdiError::MissingSegment("LIN/SN1"));
    }
    Ok(Shipment {
        shipment_id,
        po_number,
        ship_date,
        lines,
    })
}

/// Serialize a shipment as an X12 856.
///
/// Envelope: `ISA/GS/ST/BSN/REF*PO/HL/LIN/SN1…/SE/GE/IEA`,
/// functional group `SH`, real control numbers and a correct `SE`
/// count. The `REF*PO` segment carries the shipment's PO number
/// (when known) so the receiver can tie it to the order; the ship
/// date comes from `shipment.ship_date` (midnight when absent).
pub fn shipment_to_x12_856(
    shipment: &Shipment,
    sender_id: &str,
    receiver_id: &str,
    control: u32,
) -> String {
    let date = shipment
        .ship_date
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 11, 25).unwrap());
    let time = chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    let mut segments = envelope_open("SH", sender_id, receiver_id, date, time, control);
    let mut body = vec![Segment::new(
        "BSN",
        &[
            "00",
            &shipment.shipment_id,
            &x12_date(date),
            &crate::x12::x12_time(time),
        ],
    )];
    if let Some(po_number) = &shipment.po_number {
        body.push(Segment::new("REF", &["PO", po_number]));
    }
    body.push(Segment::new("HL", &["1", "", "S"]));
    for line in &shipment.lines {
        body.push(Segment::new(
            "LIN",
            &[&line.line_no.to_string(), "VP", &line.vendor_sku],
        ));
        body.push(Segment::new(
            "SN1",
            &["", &line.qty_shipped.to_string(), &line.uom],
        ));
    }
    segments.extend(wrap_transaction("856", control, &body));
    segments.extend(envelope_close(control, control));
    render_document(&segments)
}

// --- Invoice (810) ---------------------------------------------------------

/// One billed line on an invoice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceLine {
    pub line_no: u32,
    pub vendor_sku: String,
    pub qty: Decimal,
    pub uom: String,
    pub unit_price: Decimal,
}

impl InvoiceLine {
    /// Exact line total (`qty * unit_price`).
    pub fn line_total(&self) -> Decimal {
        self.qty * self.unit_price
    }
}

/// A vendor invoice: header + billed lines + optional total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invoice {
    /// Invoice number (BIG02).
    pub invoice_number: String,
    /// Our PO number (BIG04), when the vendor echoes it.
    pub po_number: Option<String>,
    pub invoice_date: Option<NaiveDate>,
    pub lines: Vec<InvoiceLine>,
    /// Invoice total (TDS01), when present.
    pub total: Option<Decimal>,
}

impl Invoice {
    /// Exact total of the billed lines.
    pub fn lines_total(&self) -> Decimal {
        self.lines
            .iter()
            .map(InvoiceLine::line_total)
            .fold(Decimal::ZERO, |acc, amount| acc + amount)
    }
}

/// Parse an X12 810 (invoice).
///
/// Expected segments: `ST*810`, `BIG` (date, invoice number, PO
/// date, PO number), repeating `IT1` lines (`IT1*line*qty*uom*price**
/// VP*sku`), optional `TDS` total. An `IT1` with a bad quantity or
/// price is an error.
pub fn parse_810(text: &str) -> Result<Invoice> {
    let segments = parse_document(text)?;
    let st = require(&segments, "ST")?;
    if st.get(1) != Some("810") {
        return Err(EdiError::Parse(format!(
            "expected transaction set 810, found {:?}",
            st.get(1)
        )));
    }
    let big = require(&segments, "BIG")?;
    let invoice_date = parse_optional_x12_date(big.get(1))?;
    let invoice_number = big
        .get_trimmed(2)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| EdiError::Parse("BIG02 invoice number is missing".to_string()))?
        .to_string();
    let po_number = big
        .get_trimmed(4)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut lines = Vec::new();
    for segment in &segments {
        if segment.tag != "IT1" {
            continue;
        }
        let line_no: u32 = segment.get_trimmed(1).unwrap_or("0").parse().map_err(|_| {
            EdiError::Parse(format!("IT1 line number is not an integer: {segment:?}"))
        })?;
        let qty = parse_decimal_field(segment.get(2).unwrap_or(""), "IT1 quantity")?;
        let uom = segment.get_trimmed(3).unwrap_or("").to_string();
        let unit_price = parse_decimal_field(segment.get(4).unwrap_or(""), "IT1 unit price")?;
        let vendor_sku = match segment.get_trimmed(6) {
            Some("VP") => segment.get_trimmed(7).unwrap_or("").to_string(),
            Some(other) if !other.is_empty() => other.to_string(),
            _ => segment.get_trimmed(7).unwrap_or("").to_string(),
        };
        lines.push(InvoiceLine {
            line_no,
            vendor_sku,
            qty,
            uom,
            unit_price,
        });
    }
    if lines.is_empty() {
        return Err(EdiError::MissingSegment("IT1"));
    }
    let total = find(&segments, "TDS")
        .and_then(|tds| tds.get(1))
        .map(|value| parse_decimal_field(value, "TDS total"))
        .transpose()?;
    Ok(Invoice {
        invoice_number,
        po_number,
        invoice_date,
        lines,
        total,
    })
}

/// Serialize an invoice as an X12 810.
///
/// Envelope: `ISA/GS/ST/BIG/IT1…/TDS/SE/GE/IEA`, functional group
/// `IN`, real control numbers and a correct `SE` count. `BIG01`
/// is the invoice date, `BIG02` the invoice number, `BIG04` the PO
/// number (when known); `BIG03` repeats the invoice date as the PO
/// date when the PO date is not known separately. `TDS01` is the
/// sum of the invoice lines.
pub fn invoice_to_x12_810(
    invoice: &Invoice,
    sender_id: &str,
    receiver_id: &str,
    control: u32,
) -> String {
    let date = invoice
        .invoice_date
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 11, 25).unwrap());
    let time = chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    let mut segments = envelope_open("IN", sender_id, receiver_id, date, time, control);
    let mut body = vec![Segment::new(
        "BIG",
        &[
            &x12_date(date),
            &invoice.invoice_number,
            &x12_date(date),
            invoice.po_number.as_deref().unwrap_or(""),
        ],
    )];
    for line in &invoice.lines {
        body.push(Segment::new(
            "IT1",
            &[
                &line.line_no.to_string(),
                &line.qty.to_string(),
                &line.uom,
                &line.unit_price.to_string(),
                "",
                "VP",
                &line.vendor_sku,
            ],
        ));
    }
    body.push(Segment::new("TDS", &[&invoice.lines_total().to_string()]));
    segments.extend(wrap_transaction("810", control, &body));
    segments.extend(envelope_close(control, control));
    render_document(&segments)
}

// --- CSV intake ------------------------------------------------------------

/// A parsed CSV intake document: either a shipment or an invoice.
#[derive(Debug, Clone, PartialEq)]
pub enum CsvIntake {
    Shipment(Shipment),
    Invoice(Invoice),
}

/// Parse a simple CSV ASN/invoice file.
///
/// Expected header (case-insensitive):
/// `doc_type,doc_number,po_number,date,line_no,vendor_sku,qty,uom,unit_price`
///
/// * `doc_type` is `856` (shipment) or `810` (invoice); rows for one
///   document share a `doc_number`.
/// * `unit_price` may be blank on 856 rows; it is required on 810 rows.
/// * `date` is `YYYY-MM-DD`.
/// * All rows in one file must belong to a single document (same
///   `doc_type` and `doc_number`); mixing documents is an error.
pub fn parse_csv_intake(text: &str) -> Result<CsvIntake> {
    let rows = parse_csv(text)?;
    if rows.is_empty() {
        return Err(EdiError::Parse("CSV intake file is empty".to_string()));
    }
    let header = &rows[0];
    for required in [
        "doc_type",
        "doc_number",
        "po_number",
        "date",
        "line_no",
        "vendor_sku",
        "qty",
        "uom",
        "unit_price",
    ] {
        if crate::csv::header_index(header, required).is_none() {
            return Err(EdiError::Parse(format!(
                "CSV intake header is missing column {required:?}"
            )));
        }
    }
    if rows.len() < 2 {
        return Err(EdiError::Parse(
            "CSV intake file has a header but no data rows".to_string(),
        ));
    }
    let first = &rows[1];
    let doc_type = field_by_name(header, first, "doc_type")
        .unwrap_or("")
        .trim();
    if doc_type != "856" && doc_type != "810" {
        return Err(EdiError::Parse(format!(
            "unsupported CSV doc_type {doc_type:?} (expected 856 or 810)"
        )));
    }
    let doc_number = field_by_name(header, first, "doc_number")
        .unwrap_or("")
        .trim()
        .to_string();
    if doc_number.is_empty() {
        return Err(EdiError::Parse(
            "CSV intake doc_number is missing".to_string(),
        ));
    }
    let po_number = field_by_name(header, first, "po_number")
        .unwrap_or("")
        .trim()
        .to_string();
    let date_text = field_by_name(header, first, "date").unwrap_or("").trim();
    let date = if date_text.is_empty() {
        None
    } else {
        Some(
            NaiveDate::parse_from_str(date_text, "%Y-%m-%d")
                .map_err(|e| EdiError::InvalidDate(date_text.to_string(), e))?,
        )
    };

    let mut shipment_lines: Vec<ShipmentLine> = Vec::new();
    let mut invoice_lines: Vec<InvoiceLine> = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(1) {
        let row_no = index + 1; // 1-based, header is row 1
        let row_type = field_by_name(header, row, "doc_type").unwrap_or("").trim();
        let row_doc = field_by_name(header, row, "doc_number")
            .unwrap_or("")
            .trim();
        if row_type != doc_type || row_doc != doc_number {
            return Err(EdiError::Parse(format!(
                "CSV intake row {row_no} belongs to a different document ({row_type} {row_doc}); one file holds one document"
            )));
        }
        let line_no: u32 = field_by_name(header, row, "line_no")
            .unwrap_or("")
            .trim()
            .parse()
            .map_err(|_| {
                EdiError::Parse(format!(
                    "CSV intake row {row_no}: line_no is not an integer"
                ))
            })?;
        let vendor_sku = field_by_name(header, row, "vendor_sku")
            .unwrap_or("")
            .trim()
            .to_string();
        if vendor_sku.is_empty() {
            return Err(EdiError::Parse(format!(
                "CSV intake row {row_no}: vendor_sku is missing"
            )));
        }
        let qty = parse_decimal_field(
            field_by_name(header, row, "qty").unwrap_or(""),
            &format!("CSV intake row {row_no} qty"),
        )?;
        let uom = field_by_name(header, row, "uom")
            .unwrap_or("")
            .trim()
            .to_string();
        let price_text = field_by_name(header, row, "unit_price")
            .unwrap_or("")
            .trim();
        if doc_type == "856" {
            shipment_lines.push(ShipmentLine {
                line_no,
                vendor_sku,
                qty_shipped: qty,
                uom,
            });
        } else {
            if price_text.is_empty() {
                return Err(EdiError::Parse(format!(
                    "CSV intake row {row_no}: unit_price is required for 810 invoices"
                )));
            }
            let unit_price =
                parse_decimal_field(price_text, &format!("CSV intake row {row_no} unit_price"))?;
            invoice_lines.push(InvoiceLine {
                line_no,
                vendor_sku,
                qty,
                uom,
                unit_price,
            });
        }
    }
    let po_number = if po_number.is_empty() {
        None
    } else {
        Some(po_number)
    };
    if doc_type == "856" {
        Ok(CsvIntake::Shipment(Shipment {
            shipment_id: doc_number,
            po_number,
            ship_date: date,
            lines: shipment_lines,
        }))
    } else {
        let total = invoice_lines
            .iter()
            .map(InvoiceLine::line_total)
            .fold(Decimal::ZERO, |acc, amount| acc + amount);
        Ok(CsvIntake::Invoice(Invoice {
            invoice_number: doc_number,
            po_number,
            invoice_date: date,
            lines: invoice_lines,
            total: Some(total),
        }))
    }
}

/// Helper used by tests and the `edi_documents` repo: render a segment
/// list for fixtures.
#[doc(hidden)]
pub fn segments_for_test(segments: &[(&str, &[&str])]) -> Vec<Segment> {
    segments
        .iter()
        .map(|(tag, elements)| Segment::new(tag, elements))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::x12::render_document;

    fn asn_856() -> String {
        render_document(&segments_for_test(&[
            ("ST", &["856", "1"]),
            ("BSN", &["00", "SHIP-77", "20261125", "0830"]),
            ("HL", &["1", "", "S"]),
            ("LIN", &["1", "VP", "SKU-A"]),
            ("SN1", &["", "10", "EA"]),
            ("LIN", &["2", "VP", "SKU-B"]),
            ("SN1", &["", "4", "GAL"]),
            ("SE", &["8", "1"]),
        ]))
    }

    fn invoice_810() -> String {
        render_document(&segments_for_test(&[
            ("ST", &["810", "1"]),
            (
                "BIG",
                &["20261125", "INV-99", "20261120", "PO-20261125-0001"],
            ),
            ("IT1", &["1", "10", "EA", "24.5000", "", "VP", "SKU-A"]),
            ("IT1", &["2", "4", "GAL", "58.0000", "", "VP", "SKU-B"]),
            ("TDS", &["477.0000"]),
            ("SE", &["6", "1"]),
        ]))
    }

    #[test]
    fn parses_856_with_lines() {
        let shipment = parse_856(&asn_856()).unwrap();
        assert_eq!(shipment.shipment_id, "SHIP-77");
        assert_eq!(shipment.ship_date, NaiveDate::from_ymd_opt(2026, 11, 25));
        assert_eq!(shipment.lines.len(), 2);
        assert_eq!(shipment.lines[0].vendor_sku, "SKU-A");
        assert_eq!(
            shipment.lines[0].qty_shipped,
            Decimal::from_str("10").unwrap()
        );
        assert_eq!(shipment.lines[1].uom, "GAL");
    }

    #[test]
    fn parses_810_with_total() {
        let invoice = parse_810(&invoice_810()).unwrap();
        assert_eq!(invoice.invoice_number, "INV-99");
        assert_eq!(invoice.po_number.as_deref(), Some("PO-20261125-0001"));
        assert_eq!(invoice.lines.len(), 2);
        assert_eq!(invoice.total, Some(Decimal::from_str("477.0000").unwrap()));
        assert_eq!(
            invoice.lines_total(),
            Decimal::from_str("477.0000").unwrap()
        );
    }

    #[test]
    fn malformed_856_and_810_error() {
        // Wrong transaction set.
        assert!(parse_856(&invoice_810()).is_err());
        assert!(parse_810(&asn_856()).is_err());
        // Missing BSN.
        assert!(parse_856("ST*856*1~LIN*1*VP*SKU-A~SN1**10*EA~").is_err());
        // SN1 with no LIN.
        assert!(parse_856("ST*856*1~BSN*00*S1*20261125~SN1**10*EA~").is_err());
        // LIN with no SN1.
        assert!(parse_856("ST*856*1~BSN*00*S1*20261125~LIN*1*VP*SKU-A~").is_err());
        // Bad quantity.
        assert!(parse_856("ST*856*1~BSN*00*S1*20261125~LIN*1*VP*SKU-A~SN1**ten*EA~").is_err());
        // Bad date.
        assert!(parse_856("ST*856*1~BSN*00*S1*20261340~LIN*1*VP*SKU-A~SN1**1*EA~").is_err());
        // Invoice with no IT1 lines.
        assert!(parse_810("ST*810*1~BIG*20261125*INV-1**PO-1~").is_err());
    }

    #[test]
    fn parses_csv_asn_and_invoice() {
        let asn = parse_csv_intake(
            "doc_type,doc_number,po_number,date,line_no,vendor_sku,qty,uom,unit_price\n\
             856,SHIP-9,PO-20261125-0001,2026-11-25,1,SKU-A,10,EA,\n\
             856,SHIP-9,PO-20261125-0001,2026-11-25,2,SKU-B,4,GAL,\n",
        )
        .unwrap();
        match asn {
            CsvIntake::Shipment(shipment) => {
                assert_eq!(shipment.shipment_id, "SHIP-9");
                assert_eq!(shipment.po_number.as_deref(), Some("PO-20261125-0001"));
                assert_eq!(shipment.lines.len(), 2);
            }
            other => panic!("expected shipment, got {other:?}"),
        }
        let invoice = parse_csv_intake(
            "doc_type,doc_number,po_number,date,line_no,vendor_sku,qty,uom,unit_price\n\
             810,INV-5,PO-20261125-0001,2026-11-25,1,SKU-A,10,EA,24.5000\n",
        )
        .unwrap();
        match invoice {
            CsvIntake::Invoice(invoice) => {
                assert_eq!(invoice.invoice_number, "INV-5");
                assert_eq!(invoice.total, Some(Decimal::from_str("245.0000").unwrap()));
            }
            other => panic!("expected invoice, got {other:?}"),
        }
    }

    #[test]
    fn csv_intake_rejects_mixed_documents_and_bad_rows() {
        assert!(
            parse_csv_intake(
                "doc_type,doc_number,po_number,date,line_no,vendor_sku,qty,uom,unit_price\n\
             856,SHIP-1,,2026-11-25,1,SKU-A,10,EA,\n\
             856,SHIP-2,,2026-11-25,1,SKU-A,10,EA,\n"
            )
            .is_err()
        );
        assert!(parse_csv_intake("doc_type,doc_number\n856,SHIP-1\n").is_err());
        assert!(
            parse_csv_intake(
                "doc_type,doc_number,po_number,date,line_no,vendor_sku,qty,uom,unit_price\n\
             810,INV-1,,2026-11-25,1,SKU-A,10,EA,\n"
            )
            .is_err()
        ); // 810 row without unit_price
    }

    #[test]
    fn x12_856_and_810_round_trip_with_po_link() {
        let shipment = Shipment {
            shipment_id: "SHIP-77".to_string(),
            po_number: Some("PO-20261125-0001".to_string()),
            ship_date: NaiveDate::from_ymd_opt(2026, 11, 25),
            lines: vec![
                ShipmentLine {
                    line_no: 1,
                    vendor_sku: "SKU-A".to_string(),
                    qty_shipped: Decimal::from_str("10").unwrap(),
                    uom: "EA".to_string(),
                },
                ShipmentLine {
                    line_no: 2,
                    vendor_sku: "SKU-B".to_string(),
                    qty_shipped: Decimal::from_str("4").unwrap(),
                    uom: "GAL".to_string(),
                },
            ],
        };
        let text_856 = shipment_to_x12_856(&shipment, "VENDOR-A", "DECORPOS", 21);
        assert!(text_856.contains("ST*856*21~"));
        assert!(text_856.contains("REF*PO*PO-20261125-0001~"));
        assert!(text_856.contains("SN1**10*EA~"));
        let back_856 = parse_856(&text_856).unwrap();
        assert_eq!(back_856, shipment);

        let invoice = Invoice {
            invoice_number: "INV-99".to_string(),
            po_number: Some("PO-20261125-0001".to_string()),
            invoice_date: NaiveDate::from_ymd_opt(2026, 11, 25),
            lines: vec![InvoiceLine {
                line_no: 1,
                vendor_sku: "SKU-A".to_string(),
                qty: Decimal::from_str("10").unwrap(),
                uom: "EA".to_string(),
                unit_price: Decimal::from_str("24.5000").unwrap(),
            }],
            total: Some(Decimal::from_str("245.0000").unwrap()),
        };
        let text_810 = invoice_to_x12_810(&invoice, "VENDOR-A", "DECORPOS", 22);
        assert!(text_810.contains("ST*810*22~"));
        assert!(text_810.contains("BIG*20261125*INV-99*20261125*PO-20261125-0001~"));
        assert!(text_810.contains("TDS*245.0000~"));
        let back_810 = parse_810(&text_810).unwrap();
        assert_eq!(back_810, invoice);
    }
}
