//! Three-way invoice matching (Phase 4, v0.5).
//!
//! A vendor invoice is matched against the purchase order it
//! answers **and** the shipment (ASN) it bills for:
//!
//! * PO quantity / unit cost — what was ordered,
//! * received quantity — what the ASN says arrived,
//! * invoiced quantity / unit price — what the vendor billed.
//!
//! The result is an [`InvoiceMatchReport`]: per-line variances
//! with exact `Decimal` amounts, and an overall status of
//! `matched` or `variance`. **Variances never post anything** —
//! the report is the output, and a caller (or a later endpoint)
//! decides whether to pay, short-pay, or dispute. Lines the
//! vendor billed or shipped that are not on the PO are reported
//! as `not_on_po` lines, not dropped.
//!
//! Variance math, per PO line (all `Decimal`, no floats):
//!
//! * `qty_variance = invoiced_qty - received_qty`
//! * `qty_variance_amount = qty_variance * invoiced_unit_price`
//! * `price_variance_per_unit = invoiced_unit_price - po_unit_cost`
//! * `price_variance_amount = price_variance_per_unit * invoiced_qty`
//! * `total_variance_amount = qty_variance_amount + price_variance_amount`
//!
//! A line is `matched` only when both variances are zero (and the
//! SKU is on the PO). A line missing from the shipment or the
//! invoice matches against zero for the missing side, so the
//! shortfall shows up as a variance rather than vanishing.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::intake::{Invoice, Shipment};
use crate::purchase_order::PurchaseOrder;

/// Overall match outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStatus {
    Matched,
    Variance,
}

impl MatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::Variance => "variance",
        }
    }
}

impl std::str::FromStr for MatchStatus {
    type Err = crate::error::EdiError;

    fn from_str(value: &str) -> std::result::Result<Self, crate::error::EdiError> {
        match value {
            "matched" => Ok(Self::Matched),
            "variance" => Ok(Self::Variance),
            other => Err(crate::error::EdiError::Parse(format!(
                "unknown match status {other:?}"
            ))),
        }
    }
}

/// Why a line is (or is not) a variance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VarianceKind {
    /// Quantities and prices agree.
    None,
    /// Invoiced quantity differs from received quantity.
    Quantity,
    /// Invoiced unit price differs from the PO unit cost.
    Price,
    /// Both a quantity and a price variance.
    QuantityAndPrice,
    /// The PO line never arrived on the ASN (received = 0).
    MissingReceived,
    /// The PO line was never billed (invoiced = 0).
    MissingInvoiced,
    /// Billed or shipped, but not on the PO.
    NotOnPo,
}

impl VarianceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Quantity => "quantity",
            Self::Price => "price",
            Self::QuantityAndPrice => "quantity_and_price",
            Self::MissingReceived => "missing_received",
            Self::MissingInvoiced => "missing_invoiced",
            Self::NotOnPo => "not_on_po",
        }
    }
}

impl std::str::FromStr for VarianceKind {
    type Err = crate::error::EdiError;

    fn from_str(value: &str) -> std::result::Result<Self, crate::error::EdiError> {
        match value {
            "none" => Ok(Self::None),
            "quantity" => Ok(Self::Quantity),
            "price" => Ok(Self::Price),
            "quantity_and_price" => Ok(Self::QuantityAndPrice),
            "missing_received" => Ok(Self::MissingReceived),
            "missing_invoiced" => Ok(Self::MissingInvoiced),
            "not_on_po" => Ok(Self::NotOnPo),
            other => Err(crate::error::EdiError::Parse(format!(
                "unknown variance kind {other:?}"
            ))),
        }
    }
}

/// One matched line of the three-way match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchLine {
    /// PO line number, when the line is on the PO.
    pub po_line_no: Option<u32>,
    pub vendor_sku: String,
    pub po_qty: Decimal,
    pub received_qty: Decimal,
    pub invoiced_qty: Decimal,
    pub po_unit_cost: Decimal,
    pub invoiced_unit_price: Decimal,
    /// `invoiced_qty - received_qty`.
    pub qty_variance: Decimal,
    /// `invoiced_unit_price - po_unit_cost`.
    pub price_variance_per_unit: Decimal,
    /// `qty_variance * invoiced_unit_price`.
    pub qty_variance_amount: Decimal,
    /// `price_variance_per_unit * invoiced_qty`.
    pub price_variance_amount: Decimal,
    /// `qty_variance_amount + price_variance_amount`.
    pub total_variance_amount: Decimal,
    pub status: MatchStatus,
    pub variance_kind: VarianceKind,
}

/// The three-way match report for one invoice. Building it posts
/// nothing — it is the record a reviewer acts on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceMatchReport {
    /// UUIDv7 id.
    pub id: Uuid,
    pub po_number: String,
    pub invoice_number: String,
    pub shipment_id: String,
    pub status: MatchStatus,
    pub lines: Vec<MatchLine>,
    /// Sum of the PO lines' `qty * unit_cost`.
    pub po_total: Decimal,
    /// Sum of the invoice lines' `qty * unit_price`.
    pub invoiced_total: Decimal,
    /// Sum of the lines' `total_variance_amount`.
    pub total_variance_amount: Decimal,
    /// `invoice.total - lines_total`, when the vendor stated a
    /// total and it disagrees with the sum of its own lines.
    pub stated_total_difference: Option<Decimal>,
    pub created_at: DateTime<Utc>,
}

/// Inputs to [`match_line`], bundled so the function stays under
/// clippy's argument limit.
struct MatchLineInputs<'a> {
    po_line_no: Option<u32>,
    vendor_sku: &'a str,
    po_qty: Decimal,
    received_qty: Decimal,
    invoiced_qty: Decimal,
    po_unit_cost: Decimal,
    invoiced_unit_price: Decimal,
    on_po: bool,
}

fn match_line(inputs: MatchLineInputs<'_>) -> MatchLine {
    let MatchLineInputs {
        po_line_no,
        vendor_sku,
        po_qty,
        received_qty,
        invoiced_qty,
        po_unit_cost,
        invoiced_unit_price,
        on_po,
    } = inputs;
    let qty_variance = invoiced_qty - received_qty;
    let price_variance_per_unit = invoiced_unit_price - po_unit_cost;
    let qty_variance_amount = qty_variance * invoiced_unit_price;
    let price_variance_amount = price_variance_per_unit * invoiced_qty;
    let total_variance_amount = qty_variance_amount + price_variance_amount;
    let (status, variance_kind) = if !on_po {
        (MatchStatus::Variance, VarianceKind::NotOnPo)
    } else if received_qty == Decimal::ZERO && po_qty != Decimal::ZERO {
        (MatchStatus::Variance, VarianceKind::MissingReceived)
    } else if invoiced_qty == Decimal::ZERO && po_qty != Decimal::ZERO {
        (MatchStatus::Variance, VarianceKind::MissingInvoiced)
    } else if qty_variance != Decimal::ZERO && price_variance_per_unit != Decimal::ZERO {
        (MatchStatus::Variance, VarianceKind::QuantityAndPrice)
    } else if qty_variance != Decimal::ZERO {
        (MatchStatus::Variance, VarianceKind::Quantity)
    } else if price_variance_per_unit != Decimal::ZERO {
        (MatchStatus::Variance, VarianceKind::Price)
    } else {
        (MatchStatus::Matched, VarianceKind::None)
    };
    MatchLine {
        po_line_no,
        vendor_sku: vendor_sku.to_string(),
        po_qty,
        received_qty,
        invoiced_qty,
        po_unit_cost,
        invoiced_unit_price,
        qty_variance,
        price_variance_per_unit,
        qty_variance_amount,
        price_variance_amount,
        total_variance_amount,
        status,
        variance_kind,
    }
}

/// Run the three-way match for one PO, one shipment and one invoice.
///
/// Lines are matched by vendor SKU; quantities from repeated
/// shipment/invoice lines for the same SKU are summed. Every PO
/// line appears in the report (even when unshipped or unbilled),
/// and every billed/shipped SKU that is not on the PO appears as a
/// `not_on_po` line. Note: this function does **not** check that
/// the invoice's `po_number` names this PO — callers must verify
/// that before matching, so an invoice for another order is never
/// silently matched against this one.
pub fn match_three_way(
    po: &PurchaseOrder,
    shipment: &Shipment,
    invoice: &Invoice,
) -> InvoiceMatchReport {
    let mut lines: Vec<MatchLine> = Vec::new();
    for po_line in &po.lines {
        let received_qty = shipment
            .lines
            .iter()
            .filter(|line| line.vendor_sku == po_line.vendor_sku)
            .map(|line| line.qty_shipped)
            .fold(Decimal::ZERO, |acc, qty| acc + qty);
        let (invoiced_qty, invoiced_unit_price) = invoice
            .lines
            .iter()
            .filter(|line| line.vendor_sku == po_line.vendor_sku)
            .fold((Decimal::ZERO, Decimal::ZERO), |(qty, _price), line| {
                (qty + line.qty, line.unit_price)
            });
        lines.push(match_line(MatchLineInputs {
            po_line_no: Some(po_line.line_no),
            vendor_sku: &po_line.vendor_sku,
            po_qty: po_line.qty,
            received_qty,
            invoiced_qty,
            po_unit_cost: po_line.unit_cost,
            invoiced_unit_price,
            on_po: true,
        }));
    }
    // Billed SKUs not on the PO.
    for invoice_line in &invoice.lines {
        if po
            .lines
            .iter()
            .any(|po_line| po_line.vendor_sku == invoice_line.vendor_sku)
        {
            continue;
        }
        let received_qty = shipment
            .lines
            .iter()
            .filter(|line| line.vendor_sku == invoice_line.vendor_sku)
            .map(|line| line.qty_shipped)
            .fold(Decimal::ZERO, |acc, qty| acc + qty);
        lines.push(match_line(MatchLineInputs {
            po_line_no: None,
            vendor_sku: &invoice_line.vendor_sku,
            po_qty: Decimal::ZERO,
            received_qty,
            invoiced_qty: invoice_line.qty,
            po_unit_cost: Decimal::ZERO,
            invoiced_unit_price: invoice_line.unit_price,
            on_po: false,
        }));
    }
    // Shipped SKUs not on the PO and not billed either.
    for shipment_line in &shipment.lines {
        if po
            .lines
            .iter()
            .any(|po_line| po_line.vendor_sku == shipment_line.vendor_sku)
            || invoice
                .lines
                .iter()
                .any(|line| line.vendor_sku == shipment_line.vendor_sku)
        {
            continue;
        }
        lines.push(match_line(MatchLineInputs {
            po_line_no: None,
            vendor_sku: &shipment_line.vendor_sku,
            po_qty: Decimal::ZERO,
            received_qty: shipment_line.qty_shipped,
            invoiced_qty: Decimal::ZERO,
            po_unit_cost: Decimal::ZERO,
            invoiced_unit_price: Decimal::ZERO,
            on_po: false,
        }));
    }
    let status = if lines.iter().all(|line| line.status == MatchStatus::Matched) {
        MatchStatus::Matched
    } else {
        MatchStatus::Variance
    };
    let total_variance_amount = lines
        .iter()
        .map(|line| line.total_variance_amount)
        .fold(Decimal::ZERO, |acc, amount| acc + amount);
    let stated_total_difference = invoice.total.map(|stated| stated - invoice.lines_total());
    InvoiceMatchReport {
        id: Uuid::now_v7(),
        po_number: po.po_number.clone(),
        invoice_number: invoice.invoice_number.clone(),
        shipment_id: shipment.shipment_id.clone(),
        status,
        lines,
        po_total: po.total(),
        invoiced_total: invoice.lines_total(),
        total_variance_amount,
        stated_total_difference,
        created_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intake::{InvoiceLine, ShipmentLine};
    use crate::purchase_order::{LowStockItem, generate_purchase_orders};
    use chrono::NaiveDate;
    use std::str::FromStr as _;
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
    fn clean_match_is_matched_with_zero_variance() {
        let po = sample_po();
        let report = match_three_way(
            &po,
            &shipment("12", "6"),
            &invoice("12", "24.5000", "6", "15.0000"),
        );
        assert_eq!(report.status, MatchStatus::Matched);
        assert_eq!(report.total_variance_amount, Decimal::ZERO);
        assert_eq!(report.po_total, dec("384.0000"));
        assert_eq!(report.invoiced_total, dec("384.0000"));
        assert!(
            report
                .lines
                .iter()
                .all(|line| line.status == MatchStatus::Matched)
        );
    }

    #[test]
    fn quantity_variance_is_flagged_with_exact_amount() {
        let po = sample_po();
        // Received 12 of SKU-A but billed for 14: qty variance +2 at
        // the invoiced price 24.5000 = +49.0000.
        let report = match_three_way(
            &po,
            &shipment("12", "6"),
            &invoice("14", "24.5000", "6", "15.0000"),
        );
        assert_eq!(report.status, MatchStatus::Variance);
        assert_eq!(report.lines[0].variance_kind, VarianceKind::Quantity);
        assert_eq!(report.lines[0].qty_variance, dec("2"));
        assert_eq!(report.lines[0].qty_variance_amount, dec("49.0000"));
        assert_eq!(report.lines[0].price_variance_amount, Decimal::ZERO);
        assert_eq!(report.lines[0].total_variance_amount, dec("49.0000"));
        assert_eq!(report.lines[1].status, MatchStatus::Matched);
        assert_eq!(report.total_variance_amount, dec("49.0000"));
    }

    #[test]
    fn price_variance_is_flagged_with_exact_amount() {
        let po = sample_po();
        // Billed SKU-B at 16.0000 instead of the PO 15.0000, qty 6:
        // price variance +1.0000 * 6 = +6.0000.
        let report = match_three_way(
            &po,
            &shipment("12", "6"),
            &invoice("12", "24.5000", "6", "16.0000"),
        );
        assert_eq!(report.status, MatchStatus::Variance);
        assert_eq!(report.lines[1].variance_kind, VarianceKind::Price);
        assert_eq!(report.lines[1].price_variance_per_unit, dec("1.0000"));
        assert_eq!(report.lines[1].price_variance_amount, dec("6.0000"));
        assert_eq!(report.total_variance_amount, dec("6.0000"));
    }

    #[test]
    fn missing_receipt_and_unbilled_sku_are_flagged() {
        let po = sample_po();
        let mut inv = invoice("12", "24.5000", "6", "15.0000");
        inv.lines.push(InvoiceLine {
            line_no: 3,
            vendor_sku: "SKU-EXTRA".to_string(),
            qty: dec("2"),
            uom: "EA".to_string(),
            unit_price: dec("5.0000"),
        });
        // SKU-B never shipped: received 0.
        let report = match_three_way(&po, &shipment("12", "0"), &inv);
        assert_eq!(report.lines[1].variance_kind, VarianceKind::MissingReceived);
        let extra = report
            .lines
            .iter()
            .find(|line| line.vendor_sku == "SKU-EXTRA")
            .unwrap();
        assert_eq!(extra.variance_kind, VarianceKind::NotOnPo);
        assert_eq!(extra.po_line_no, None);
        assert_eq!(report.status, MatchStatus::Variance);
    }
}
