//! Packing slips (spec §4 `delivery_logistics`, §7 Phase 4).
//!
//! A printable slip for one delivery, rendered from the order and
//! its lines. Three renderings, in the `df-hw` style (plain-text
//! 42-column layout, ESC/POS bytes, ZPL label bytes):
//!
//! * [`PackingSlip::render_text`] — plain text for a screen,
//!   a PDF pre-pass, or a line printer.
//! * [`PackingSlip::render_escpos`] — ESC/POS bytes for the
//!   80 mm thermal printer the counter already uses.
//! * [`PackingSlip::render_zpl`] — ZPL for a label printer.
//!
//! Money is `rust_decimal::Decimal` throughout — never floats.
//! Quantities print as given; only the slip text rounds nothing.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Width of the plain-text / ESC-POS slip in monospaced columns
/// (80 mm paper at the usual font — same as `df-hw` receipts).
pub const SLIP_WIDTH: usize = 42;

/// One packed line on a slip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackingSlipLine {
    pub description: String,
    pub qty: Decimal,
    pub uom_code: String,
}

/// Everything a packing slip needs. All values are supplied by
/// the caller (computed by `df-domain` / read from `df-db`); this
/// module only formats them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackingSlip {
    /// UUIDv7 primary key (row of `packing_slips`, if persisted).
    pub id: Uuid,
    pub delivery_id: Uuid,
    pub store_name: String,
    pub doc_number: String,
    pub customer_name: String,
    /// Delivery address lines, in order.
    pub address_lines: Vec<String>,
    pub lines: Vec<PackingSlipLine>,
    pub driver_name: Option<String>,
    pub vehicle: Option<String>,
    /// Higher number = more urgent (see `df-delivery` dispatch).
    pub priority: i32,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl PackingSlip {
    /// Build a new slip with a fresh UUIDv7 id.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        delivery_id: Uuid,
        store_name: impl Into<String>,
        doc_number: impl Into<String>,
        customer_name: impl Into<String>,
        address_lines: Vec<String>,
        lines: Vec<PackingSlipLine>,
        priority: i32,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            delivery_id,
            store_name: store_name.into(),
            doc_number: doc_number.into(),
            customer_name: customer_name.into(),
            address_lines,
            lines,
            driver_name: None,
            vehicle: None,
            priority,
            notes: None,
            created_at: Utc::now(),
        }
    }

    /// Total quantity across all lines (for a quick "N items"
    /// check at the door).
    pub fn total_qty(&self) -> Decimal {
        self.lines.iter().map(|l| l.qty).sum()
    }

    /// Plain-text 42-column slip.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        let rule = "-".repeat(SLIP_WIDTH);
        out.push_str(&center(&self.store_name, SLIP_WIDTH));
        out.push('\n');
        out.push_str(&center("PACKING SLIP", SLIP_WIDTH));
        out.push('\n');
        out.push_str(&rule);
        out.push('\n');
        out.push_str(&format!("Order: {}\n", self.doc_number));
        out.push_str(&format!("Customer: {}\n", self.customer_name));
        for line in &self.address_lines {
            out.push_str(&format!("  {line}\n"));
        }
        if let Some(driver) = &self.driver_name {
            out.push_str(&format!("Driver: {driver}\n"));
        }
        if let Some(vehicle) = &self.vehicle {
            out.push_str(&format!("Vehicle: {vehicle}\n"));
        }
        out.push_str(&format!("Priority: {}\n", self.priority));
        out.push_str(&rule);
        out.push('\n');
        for line in &self.lines {
            out.push_str(&format!(
                "{} x {} {}\n",
                line.qty.normalize(),
                line.uom_code,
                line.description
            ));
        }
        out.push_str(&rule);
        out.push('\n');
        out.push_str(&format!("Total items: {}\n", self.total_qty().normalize()));
        if let Some(notes) = &self.notes {
            out.push_str(&format!("Notes: {notes}\n"));
        }
        out.push('\n');
        out.push_str("Received by: ______________________\n");
        out.push_str(&rule);
        out.push('\n');
        out
    }

    /// ESC/POS bytes for an 80 mm thermal printer: init, centered
    /// bold header, lines, totals, signature line, feed, partial
    /// cut. Same command style as `df-hw` receipts.
    pub fn render_escpos(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b@"); // Init.
        out.extend_from_slice(b"\x1b\x61\x01"); // Center.
        out.extend_from_slice(b"\x1b\x45\x01"); // Bold on.
        out.extend_from_slice(self.store_name.as_bytes());
        out.extend_from_slice(b"\nPACKING SLIP\n");
        out.extend_from_slice(b"\x1b\x45\x00"); // Bold off.
        out.extend_from_slice(b"\x1b\x61\x00"); // Left align.
        out.extend_from_slice(self.render_text_body().as_bytes());
        out.extend_from_slice(b"\n\n\n\x1dV\x01"); // Feed + partial cut.
        out
    }

    /// ZPL label bytes: header, order/customer, one row per line,
    /// totals, signature line. Coordinates assume a 4-inch label
    /// at 203 dpi; long descriptions are truncated to fit.
    pub fn render_zpl(&self) -> String {
        let mut out = String::from("^XA\n^CI28\n");
        out.push_str(&format!(
            "^FO20,20^A0N,40,40^FD{}^FS\n",
            zpl_escape(&self.store_name)
        ));
        out.push_str("^FO20,70^A0N,30,30^FDPACKING SLIP^FS\n");
        out.push_str(&format!(
            "^FO20,110^A0N,28,28^FDOrder: {}^FS\n",
            zpl_escape(&self.doc_number)
        ));
        out.push_str(&format!(
            "^FO20,145^A0N,28,28^FD{}^FS\n",
            zpl_escape(&self.customer_name)
        ));
        let mut y = 180;
        for line in &self.lines {
            out.push_str(&format!(
                "^FO20,{y}^A0N,26,26^FD{} x {} {}^FS\n",
                line.qty.normalize(),
                zpl_escape(&line.uom_code),
                zpl_escape(&truncate(&line.description, 40))
            ));
            y += 35;
        }
        out.push_str(&format!(
            "^FO20,{y}^A0N,28,28^FDTotal items: {}^FS\n",
            self.total_qty().normalize()
        ));
        let y = y + 40;
        out.push_str(&format!(
            "^FO20,{y}^A0N,26,26^FDReceived by: __________________^FS\n"
        ));
        out.push_str("^XZ\n");
        out
    }

    /// The body of the text slip (everything after the centered
    /// header), shared by the plain-text and ESC/POS renderings.
    fn render_text_body(&self) -> String {
        let full = self.render_text();
        // Drop the first two centered header lines; the ESC/POS
        // path prints its own bold centered header.
        let mut lines = full.lines();
        let _ = lines.next();
        let _ = lines.next();
        let mut body: String = lines.collect::<Vec<&str>>().join("\n");
        body.push('\n');
        body
    }
}

fn center(text: &str, width: usize) -> String {
    if text.len() >= width {
        return text.to_string();
    }
    let pad = (width - text.len()) / 2;
    format!("{}{}", " ".repeat(pad), text)
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect()
    }
}

fn zpl_escape(text: &str) -> String {
    // ZPL field data: ^ and ~ start commands, so neutralise them.
    text.replace('^', "'").replace('~', "-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn slip() -> PackingSlip {
        let mut s = PackingSlip::new(
            Uuid::now_v7(),
            "Test Paint Co",
            "ORD-1001",
            "Acme Painting",
            vec!["12 Main St".to_string(), "Vancouver, BC".to_string()],
            vec![
                PackingSlipLine {
                    description: "Eggshell White — 1 gal".to_string(),
                    qty: Decimal::from_str("2").unwrap(),
                    uom_code: "gal".to_string(),
                },
                PackingSlipLine {
                    description: "2 in brush".to_string(),
                    qty: Decimal::from_str("3").unwrap(),
                    uom_code: "ea".to_string(),
                },
            ],
            2,
        );
        s.driver_name = Some("Dana Driver".to_string());
        s.vehicle = Some("Van 2".to_string());
        s
    }

    #[test]
    fn text_slip_contains_order_lines_and_totals() {
        let text = slip().render_text();
        assert!(text.contains("PACKING SLIP"));
        assert!(text.contains("ORD-1001"));
        assert!(text.contains("Acme Painting"));
        assert!(text.contains("12 Main St"));
        assert!(text.contains("2 x gal Eggshell White"));
        assert!(text.contains("3 x ea 2 in brush"));
        assert!(text.contains("Driver: Dana Driver"));
        assert!(text.contains("Vehicle: Van 2"));
        assert!(text.contains("Total items: 5"));
        assert!(text.contains("Received by:"));
        // No line exceeds the 42-column width.
        assert!(text.lines().all(|l| l.chars().count() <= SLIP_WIDTH));
    }

    #[test]
    fn escpos_slip_has_init_bold_and_cut() {
        let bytes = slip().render_escpos();
        assert!(bytes.starts_with(b"\x1b@"));
        assert!(bytes.windows(3).any(|w| w == b"\x1b\x45\x01"));
        assert!(bytes.ends_with(b"\x1dV\x01"));
        assert!(bytes.windows(b"ORD-1001".len()).any(|w| w == b"ORD-1001"));
    }

    #[test]
    fn zpl_slip_is_a_well_formed_label() {
        let zpl = slip().render_zpl();
        assert!(zpl.starts_with("^XA\n"));
        assert!(zpl.ends_with("^XZ\n"));
        assert!(zpl.contains("PACKING SLIP"));
        assert!(zpl.contains("ORD-1001"));
        assert!(zpl.contains("2 x gal Eggshell White"));
        assert!(zpl.contains("Total items: 5"));
    }

    #[test]
    fn total_qty_sums_lines() {
        assert_eq!(slip().total_qty(), Decimal::from_str("5").unwrap());
    }
}
