//! X12 855 purchase-order acknowledgments (Phase 4, v0.5).
//!
//! A vendor answers an 850 with an 855: per line, accepted,
//! rejected, or changed (different quantity, price, or promised
//! date). This module builds and parses 855 documents and ties an
//! acknowledgment back to the original PO — an ack for a different
//! PO number is an error, never a silent merge.
//!
//! ## Segment profile used by this crate
//!
//! ```text
//! ST*855*<control>~
//! BAK*00*AC*<po_number>**<ack date CCYYMMDD>~
//! ACK*<line>*<qty acked>*<uom>*<IA|DR|IQ>~
//! LIN*<line>*VP*<vendor sku>~                             (identifies the item — an ACK has no SKU field)
//! PO1*<line>*<qty>*<uom>*<unit cost>**VP*<vendor sku>~   (only when a changed price is carried)
//! DTM*010*<promised date CCYYMMDD>~                      (only when a promised date is carried)
//! SE*...~
//! ```
//!
//! `BAK01` is the purpose code (`00` original), `BAK02` the ack
//! type (`AC` acknowledge). `ACK04` is the action code: `IA`
//! (accepted), `DR` (rejected), `IQ` (change). The `PO1` after an
//! `ACK` carries a changed unit cost (an 855 has no price segment
//! of its own); a `DTM*010` after an `ACK` carries the vendor's
//! promised ship date for that line. Money is `Decimal` throughout.
//!
//! Applying an ack never mutates the stored PO: [`apply_acknowledgment_to_po`]
//! returns an [`AckApplication`] report (accepted / rejected /
//! changed counts, per-line quantity differences, price-change
//! flags). Callers decide what to do with rejected or changed
//! lines — e.g. re-order, accept the change, or cancel the line.

use std::str::FromStr;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::{EdiError, Result};
use crate::purchase_order::PurchaseOrder;
use crate::x12::{
    Segment, envelope_close, envelope_open, parse_document, parse_x12_date, render_document,
    require, wrap_transaction, x12_date,
};

/// Per-line vendor action on an 855.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AckCode {
    /// Line accepted as ordered.
    Accepted,
    /// Line rejected (not supplied).
    Rejected,
    /// Line changed (quantity, price, or date differs).
    Changed,
}

impl AckCode {
    /// X12 action code (`IA` / `DR` / `IQ`).
    pub fn as_x12(self) -> &'static str {
        match self {
            Self::Accepted => "IA",
            Self::Rejected => "DR",
            Self::Changed => "IQ",
        }
    }

    /// Parse an X12 action code.
    pub fn from_x12(value: &str) -> Result<Self> {
        match value.trim() {
            "IA" => Ok(Self::Accepted),
            "DR" => Ok(Self::Rejected),
            "IQ" => Ok(Self::Changed),
            other => Err(EdiError::Parse(format!(
                "unknown ACK action code {other:?} (expected IA, DR or IQ)"
            ))),
        }
    }
}

/// One acknowledged line on an 855.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AckLine {
    pub line_no: u32,
    pub vendor_sku: String,
    pub ack_code: AckCode,
    /// Quantity the vendor acknowledges (0 for a rejected line).
    pub qty_acknowledged: Decimal,
    pub uom: String,
    /// Unit cost, when the vendor changed the price (carried on a
    /// following `PO1` segment). `None` means "price as ordered".
    pub unit_cost: Option<Decimal>,
    /// Vendor's promised ship date for this line, when given.
    pub promised_date: Option<NaiveDate>,
}

/// A purchase-order acknowledgment: header + acknowledged lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoAcknowledgment {
    /// The PO number this ack answers (BAK03).
    pub po_number: String,
    /// Ack date (BAK05), when the vendor gives one.
    pub ack_date: Option<NaiveDate>,
    /// Vendor EDI id, from the envelope when present.
    pub vendor_id: Option<String>,
    pub lines: Vec<AckLine>,
}

/// Serialize an acknowledgment as an X12 855.
///
/// Envelope: `ISA/GS/ST/BAK/ACK…/SE/GE/IEA`, functional group `FA`,
/// real control numbers and a correct `SE` count. The ack date is
/// taken from `ack.ack_date`; when it is `None` the BAK date element
/// is left blank.
pub fn po_acknowledgment_to_x12_855(
    ack: &PoAcknowledgment,
    sender_id: &str,
    receiver_id: &str,
    control: u32,
) -> String {
    let date = ack
        .ack_date
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 11, 25).unwrap());
    let time = chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap();
    let mut segments = envelope_open("FA", sender_id, receiver_id, date, time, control);
    let mut body = vec![Segment::new(
        "BAK",
        &[
            "00",
            "AC",
            &ack.po_number,
            "",
            &ack.ack_date.map_or_else(String::new, x12_date),
        ],
    )];
    for line in &ack.lines {
        body.push(Segment::new(
            "ACK",
            &[
                &line.line_no.to_string(),
                &line.qty_acknowledged.to_string(),
                &line.uom,
                line.ack_code.as_x12(),
            ],
        ));
        body.push(Segment::new(
            "LIN",
            &[&line.line_no.to_string(), "VP", &line.vendor_sku],
        ));
        if let Some(cost) = line.unit_cost {
            body.push(Segment::new(
                "PO1",
                &[
                    &line.line_no.to_string(),
                    &line.qty_acknowledged.to_string(),
                    &line.uom,
                    &cost.to_string(),
                    "",
                    "VP",
                    &line.vendor_sku,
                ],
            ));
        }
        if let Some(promised) = line.promised_date {
            body.push(Segment::new("DTM", &["010", &x12_date(promised)]));
        }
    }
    segments.extend(wrap_transaction("855", control, &body));
    segments.extend(envelope_close(control, control));
    render_document(&segments)
}

fn parse_decimal_field(value: &str, what: &str) -> Result<Decimal> {
    Decimal::from_str(value.trim())
        .map_err(|e| EdiError::InvalidDecimal(format!("{what}: {value:?}"), e))
}

/// Parse an X12 855 document back into a [`PoAcknowledgment`].
///
/// Requires `ST*855` and a `BAK` with a PO number; every `ACK` must
/// carry a line number, quantity, UOM and a known action code. A
/// `LIN`, `PO1` or `DTM*010` following an `ACK` attaches to that
/// line (one with no preceding `ACK` is an error).
pub fn parse_855(text: &str) -> Result<PoAcknowledgment> {
    let segments = parse_document(text)?;
    let st = require(&segments, "ST")?;
    if st.get(1) != Some("855") {
        return Err(EdiError::Parse(format!(
            "expected transaction set 855, found {:?}",
            st.get(1)
        )));
    }
    let bak = require(&segments, "BAK")?;
    let po_number = bak
        .get_trimmed(3)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| EdiError::Parse("BAK03 purchase order number is missing".to_string()))?
        .to_string();
    let ack_date = match bak.get_trimmed(5) {
        Some(value) if !value.is_empty() => Some(parse_x12_date(value)?),
        _ => None,
    };
    let vendor_id = segments
        .iter()
        .find(|s| s.tag == "GS")
        .and_then(|gs| gs.get_trimmed(2))
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut lines: Vec<AckLine> = Vec::new();
    let mut pending: Option<AckLine> = None;
    let flush = |pending: &mut Option<AckLine>, lines: &mut Vec<AckLine>| {
        if let Some(line) = pending.take() {
            lines.push(line);
        }
    };
    for segment in &segments {
        match segment.tag.as_str() {
            "ACK" => {
                flush(&mut pending, &mut lines);
                let line_no: u32 = segment.get_trimmed(1).unwrap_or("0").parse().map_err(|_| {
                    EdiError::Parse(format!("ACK line number is not an integer: {segment:?}"))
                })?;
                let qty_acknowledged =
                    parse_decimal_field(segment.get(2).unwrap_or(""), "ACK quantity")?;
                let uom = segment.get_trimmed(3).unwrap_or("").to_string();
                let ack_code = AckCode::from_x12(segment.get_trimmed(4).unwrap_or(""))?;
                pending = Some(AckLine {
                    line_no,
                    vendor_sku: String::new(),
                    ack_code,
                    qty_acknowledged,
                    uom,
                    unit_cost: None,
                    promised_date: None,
                });
            }
            "LIN" => {
                let Some(mut line) = pending.take() else {
                    return Err(EdiError::Parse(
                        "LIN item with no preceding ACK segment".to_string(),
                    ));
                };
                // LIN02 qualifier (usually VP), LIN03 the vendor SKU.
                line.vendor_sku = segment.get_trimmed(3).unwrap_or("").to_string();
                pending = Some(line);
            }
            "PO1" => {
                let Some(mut line) = pending.take() else {
                    return Err(EdiError::Parse(
                        "PO1 unit cost with no preceding ACK segment".to_string(),
                    ));
                };
                line.unit_cost = Some(parse_decimal_field(
                    segment.get(4).unwrap_or(""),
                    "PO1 unit price on 855",
                )?);
                line.vendor_sku = match segment.get_trimmed(6) {
                    Some("VP") => segment.get_trimmed(7).unwrap_or("").to_string(),
                    Some(other) if !other.is_empty() => other.to_string(),
                    _ => segment.get_trimmed(7).unwrap_or("").to_string(),
                };
                pending = Some(line);
            }
            "DTM" => {
                if segment.get_trimmed(1) != Some("010") {
                    continue;
                }
                let Some(mut line) = pending.take() else {
                    return Err(EdiError::Parse(
                        "DTM promised date with no preceding ACK segment".to_string(),
                    ));
                };
                line.promised_date = Some(parse_x12_date(segment.get_trimmed(2).unwrap_or(""))?);
                pending = Some(line);
            }
            _ => {}
        }
    }
    flush(&mut pending, &mut lines);
    if lines.is_empty() {
        return Err(EdiError::MissingSegment("ACK"));
    }
    Ok(PoAcknowledgment {
        po_number,
        ack_date,
        vendor_id,
        lines,
    })
}

/// The outcome of applying one ack line to its PO line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AckLineResult {
    pub line_no: u32,
    pub vendor_sku: String,
    pub ack_code: AckCode,
    /// Quantity ordered on the PO (0 when the ack line has no PO line).
    pub po_qty: Decimal,
    /// Quantity the vendor acknowledged.
    pub ack_qty: Decimal,
    /// `ack_qty - po_qty` (exact Decimal; negative = short).
    pub qty_difference: Decimal,
    /// True when the ack carries a unit cost different from the PO's.
    pub unit_cost_changed: bool,
}

/// The result of tying an acknowledgment to its PO. This is a
/// report, not a mutation: the stored PO is left untouched and the
/// caller decides how to handle rejected / changed lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AckApplication {
    pub po_number: String,
    pub accepted_lines: usize,
    pub rejected_lines: usize,
    pub changed_lines: usize,
    pub line_results: Vec<AckLineResult>,
    /// Ack lines whose SKU matches no PO line (vendor error — the
    /// ack is still parsed, but these lines are not applied).
    pub unmatched_skus: Vec<String>,
}

/// Tie an acknowledgment to the PO it answers.
///
/// Errors when the ack's PO number differs from the PO's — an ack
/// for another order must never be merged into this one. Lines are
/// matched by vendor SKU (the PO's `line_no` is reported from the
/// PO line when found, otherwise from the ack line).
pub fn apply_acknowledgment_to_po(
    po: &PurchaseOrder,
    ack: &PoAcknowledgment,
) -> Result<AckApplication> {
    if ack.po_number != po.po_number {
        return Err(EdiError::Parse(format!(
            "855 answers PO {:?} but was applied to PO {:?}",
            ack.po_number, po.po_number
        )));
    }
    let mut application = AckApplication {
        po_number: po.po_number.clone(),
        accepted_lines: 0,
        rejected_lines: 0,
        changed_lines: 0,
        line_results: Vec::new(),
        unmatched_skus: Vec::new(),
    };
    for line in &ack.lines {
        match line.ack_code {
            AckCode::Accepted => application.accepted_lines += 1,
            AckCode::Rejected => application.rejected_lines += 1,
            AckCode::Changed => application.changed_lines += 1,
        }
        let po_line = po
            .lines
            .iter()
            .find(|po_line| po_line.vendor_sku == line.vendor_sku);
        let Some(po_line) = po_line else {
            application.unmatched_skus.push(line.vendor_sku.clone());
            application.line_results.push(AckLineResult {
                line_no: line.line_no,
                vendor_sku: line.vendor_sku.clone(),
                ack_code: line.ack_code,
                po_qty: Decimal::ZERO,
                ack_qty: line.qty_acknowledged,
                qty_difference: line.qty_acknowledged,
                unit_cost_changed: false,
            });
            continue;
        };
        application.line_results.push(AckLineResult {
            line_no: po_line.line_no,
            vendor_sku: line.vendor_sku.clone(),
            ack_code: line.ack_code,
            po_qty: po_line.qty,
            ack_qty: line.qty_acknowledged,
            qty_difference: line.qty_acknowledged - po_line.qty,
            unit_cost_changed: line.unit_cost.is_some_and(|cost| cost != po_line.unit_cost),
        });
    }
    Ok(application)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::purchase_order::{LowStockItem, generate_purchase_orders};
    use uuid::Uuid;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn sample_po() -> PurchaseOrder {
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

    fn sample_ack(po: &PurchaseOrder) -> PoAcknowledgment {
        PoAcknowledgment {
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
        }
    }

    #[test]
    fn ack_855_round_trip() {
        let po = sample_po();
        let ack = sample_ack(&po);
        let text = po_acknowledgment_to_x12_855(&ack, "VENDOR-A", "DECORPOS", 7);
        assert!(text.contains("ST*855*7~"));
        assert!(text.contains("BAK*00*AC*PO-20261125-0001**20261126~"));
        assert!(text.contains("ACK*1*12*GAL*IA~"));
        assert!(text.contains("ACK*2*4*GAL*IQ~"));
        assert!(text.contains("PO1*2*4*GAL*16.0000**VP*SKU-B~"));
        assert!(text.contains("DTM*010*20261128~"));
        let back = parse_855(&text).unwrap();
        assert_eq!(back, ack);
    }

    #[test]
    fn apply_ack_reports_accepted_changed_and_short_qty() {
        let po = sample_po();
        let ack = sample_ack(&po);
        let application = apply_acknowledgment_to_po(&po, &ack).unwrap();
        assert_eq!(application.accepted_lines, 1);
        assert_eq!(application.changed_lines, 1);
        assert_eq!(application.rejected_lines, 0);
        assert_eq!(application.line_results[0].qty_difference, dec("0"));
        assert!(!application.line_results[0].unit_cost_changed);
        assert_eq!(application.line_results[1].qty_difference, dec("-2"));
        assert!(application.line_results[1].unit_cost_changed);
        assert!(application.unmatched_skus.is_empty());
    }

    #[test]
    fn apply_ack_rejects_wrong_po_and_flags_unmatched_sku() {
        let po = sample_po();
        let mut ack = sample_ack(&po);
        ack.po_number = "PO-OTHER".to_string();
        assert!(apply_acknowledgment_to_po(&po, &ack).is_err());
        ack.po_number = po.po_number.clone();
        ack.lines.push(AckLine {
            line_no: 3,
            vendor_sku: "SKU-UNKNOWN".to_string(),
            ack_code: AckCode::Rejected,
            qty_acknowledged: dec("0"),
            uom: "EA".to_string(),
            unit_cost: None,
            promised_date: None,
        });
        let application = apply_acknowledgment_to_po(&po, &ack).unwrap();
        assert_eq!(application.unmatched_skus, vec!["SKU-UNKNOWN".to_string()]);
        assert_eq!(application.rejected_lines, 1);
    }

    #[test]
    fn parse_855_rejects_bad_set_bad_code_and_orphan_price() {
        assert!(parse_855("ST*850*1~BAK*00*AC*PO-1**20261126~ACK*1*1*EA*IA~").is_err());
        assert!(parse_855("ST*855*1~BAK*00*AC*PO-1**20261126~ACK*1*1*EA*XX~").is_err());
        assert!(parse_855("ST*855*1~BAK*00*AC*PO-1**20261126~").is_err()); // no ACK
        assert!(
            parse_855("ST*855*1~BAK*00*AC*PO-1**20261126~PO1*1*1*EA*1.0000**VP*SKU-A~").is_err()
        );
    }
}
