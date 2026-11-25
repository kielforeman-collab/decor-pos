//! ESC/POS receipt generation (pure — no printer attached).
//!
//! [`ReceiptBuilder::receipt_bytes`] turns a [`ReceiptData`] into the byte
//! stream for a standard 80 mm thermal printer: init, centered bold
//! double-size header, left-aligned line items, a totals block, payment
//! lines, change due, and a partial cut.

use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use rust_decimal::RoundingStrategy;

/// Receipt width in monospaced columns (80 mm paper at the usual font).
pub const RECEIPT_WIDTH: usize = 42;

/// One sold line on a receipt. Money stays at scale 4 internally; only
/// the printed text is rounded to 2 decimals (see [`format_money`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptLine {
    pub description: String,
    pub qty: Decimal,
    pub unit_price: Money,
    pub line_total: Money,
}

/// A payment shown on the receipt, e.g. `Cash $40.00`.
///
/// Deliberately a plain struct, not `df_payments::PaymentResult`:
/// `df-hw` must not depend on `df-payments`. Callers map a payment
/// result into a `PaymentSummary` (tender label + amount) before
/// printing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSummary {
    pub tender_label: String,
    pub amount: Money,
}

/// Everything a receipt needs. All totals are supplied by the caller
/// (computed by `df-domain`); this crate only formats them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptData {
    pub store_name: String,
    pub doc_number: String,
    pub lines: Vec<ReceiptLine>,
    pub subtotal: Money,
    pub tax_total: Money,
    pub eco_fee_total: Money,
    pub pos_fee_total: Money,
    pub grand_total: Money,
    pub payments: Vec<PaymentSummary>,
    pub change_due: Money,
}

/// Builds ESC/POS receipt bytes from a [`ReceiptData`].
pub struct ReceiptBuilder;

impl ReceiptBuilder {
    /// Produce the full byte stream: `ESC @` init, header, items, totals,
    /// payments, change due, feed, and a partial cut (`GS V 1`).
    pub fn receipt_bytes(&self, receipt: &ReceiptData) -> Vec<u8> {
        let mut out = Vec::new();
        // ESC @ — initialize printer.
        out.extend_from_slice(&[0x1B, 0x40]);
        // Centered, bold + double-height/double-width header.
        out.extend_from_slice(&[0x1B, 0x61, 0x01]);
        out.extend_from_slice(&[0x1B, 0x21, 0x38]);
        push_line(&mut out, &receipt.store_name);
        push_line(&mut out, &receipt.doc_number);
        // Back to normal text, left-aligned.
        out.extend_from_slice(&[0x1B, 0x21, 0x00]);
        out.extend_from_slice(&[0x1B, 0x61, 0x00]);

        push_line(&mut out, &"-".repeat(RECEIPT_WIDTH));
        for line in &receipt.lines {
            push_line(&mut out, &truncate(&line.description, RECEIPT_WIDTH));
            let detail = format!(
                "  {} x {}",
                fmt_qty(line.qty),
                format_money(line.unit_price)
            );
            push_right_aligned(&mut out, &detail, &format_money(line.line_total));
        }
        push_line(&mut out, &"-".repeat(RECEIPT_WIDTH));

        push_kv(&mut out, "Subtotal", receipt.subtotal);
        push_kv(&mut out, "Tax", receipt.tax_total);
        push_kv(&mut out, "Eco fees", receipt.eco_fee_total);
        push_kv(&mut out, "POS fee", receipt.pos_fee_total);
        push_kv(&mut out, "TOTAL", receipt.grand_total);
        push_line(&mut out, &"-".repeat(RECEIPT_WIDTH));
        for payment in &receipt.payments {
            push_kv(&mut out, &payment.tender_label, payment.amount);
        }
        push_kv(&mut out, "Change due", receipt.change_due);

        out.extend_from_slice(b"\n\n\n");
        // GS V 1 — partial cut.
        out.extend_from_slice(&[0x1D, 0x56, 0x01]);
        out
    }
}

/// Format money as standard display text with 2 decimals, e.g. `$12.34`
/// (USD) or `CA$12.34` (CAD). Rounds half away from zero; the underlying
/// `Money` keeps its scale-4 value.
pub fn format_money(money: Money) -> String {
    let symbol = match money.currency() {
        Currency::Usd => "$",
        Currency::Cad => "CA$",
    };
    let rounded = money
        .amount()
        .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    if rounded.is_sign_negative() {
        format!("-{symbol}{}", rounded.abs())
    } else {
        format!("{symbol}{rounded}")
    }
}

fn push_line(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(text.as_bytes());
    out.push(b'\n');
}

fn truncate(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

/// Quantities print without trailing zeros: `2`, `0.5`, `1.25`.
fn fmt_qty(qty: Decimal) -> String {
    qty.normalize().to_string()
}

/// One line with `left` starting at column 0 and `right` ending at the
/// right edge. If they do not fit, they are joined with a single space.
fn push_right_aligned(out: &mut Vec<u8>, left: &str, right: &str) {
    let left_w = left.chars().count();
    let right_w = right.chars().count();
    let line = if left_w + right_w >= RECEIPT_WIDTH {
        format!("{left} {right}")
    } else {
        format!("{left}{:>width$}", right, width = RECEIPT_WIDTH - left_w)
    };
    push_line(out, &line);
}

fn push_kv(out: &mut Vec<u8>, label: &str, amount: Money) {
    push_right_aligned(out, label, &format_money(amount));
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::Currency;

    fn money(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    fn sample_receipt() -> ReceiptData {
        ReceiptData {
            store_name: "Decor Fusion Test Store".to_string(),
            doc_number: "INV-000123".to_string(),
            lines: vec![
                ReceiptLine {
                    description: "Premium Interior Eggshell 1 gal".to_string(),
                    qty: Decimal::from(2),
                    unit_price: money("45.9900"),
                    line_total: money("91.9800"),
                },
                ReceiptLine {
                    description: "2 in Angle Brush".to_string(),
                    qty: Decimal::from(1),
                    unit_price: money("12.5000"),
                    line_total: money("12.5000"),
                },
            ],
            subtotal: money("104.4800"),
            tax_total: money("12.5400"),
            eco_fee_total: money("0.6500"),
            pos_fee_total: money("0.0000"),
            grand_total: money("117.6700"),
            payments: vec![PaymentSummary {
                tender_label: "Cash".to_string(),
                amount: money("120.0000"),
            }],
            change_due: money("2.3300"),
        }
    }

    #[test]
    fn receipt_bytes_start_with_init_and_end_with_partial_cut() {
        let bytes = ReceiptBuilder.receipt_bytes(&sample_receipt());
        assert!(bytes.starts_with(&[0x1B, 0x40]), "must start with ESC @");
        assert!(
            bytes.ends_with(&[0x1D, 0x56, 0x01]),
            "must end with GS V 1 partial cut"
        );
    }

    #[test]
    fn receipt_bytes_contain_header_lines_and_totals() {
        let bytes = ReceiptBuilder.receipt_bytes(&sample_receipt());
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Decor Fusion Test Store"));
        assert!(text.contains("INV-000123"));
        assert!(text.contains("Premium Interior Eggshell 1 gal"));
        assert!(text.contains("2 x $45.99"));
        assert!(text.contains("$91.98"));
        assert!(text.contains("Subtotal"));
        assert!(text.contains("$104.48"));
        assert!(text.contains("Tax"));
        assert!(text.contains("$12.54"));
        assert!(text.contains("Eco fees"));
        assert!(text.contains("$0.65"));
        assert!(text.contains("TOTAL"));
        assert!(text.contains("$117.67"));
        assert!(text.contains("Cash"));
        assert!(text.contains("$120.00"));
        assert!(text.contains("Change due"));
        assert!(text.contains("$2.33"));
    }

    #[test]
    fn receipt_lines_fit_42_columns() {
        let bytes = ReceiptBuilder.receipt_bytes(&sample_receipt());
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            // Strip ESC/POS command sequences (ESC + 2 bytes, GS + 2
            // bytes) and control characters before measuring.
            let stripped = strip_escpos(line);
            assert!(
                stripped.chars().count() <= RECEIPT_WIDTH,
                "line too long: {stripped:?}"
            );
        }
    }

    /// Remove ESC/POS command bytes from one receipt line for measuring.
    fn strip_escpos(line: &str) -> String {
        let bytes = line.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                0x1B | 0x1D => i += 3, // ESC x n / GS x n
                b if b.is_ascii_control() => i += 1,
                _ => {
                    // ASCII text in these tests; take one byte as a char.
                    out.push(bytes[i] as char);
                    i += 1;
                }
            }
        }
        out
    }

    #[test]
    fn format_money_rounds_to_two_decimals() {
        assert_eq!(format_money(money("12.3400")), "$12.34");
        assert_eq!(format_money(money("12.3450")), "$12.35");
        assert_eq!(format_money(money("0.0000")), "$0.00");
        assert_eq!(format_money(money("-5.0000")), "-$5.00");
        assert_eq!(
            format_money(Money::from_str_amount("12.3400", Currency::Cad).unwrap()),
            "CA$12.34"
        );
    }
}
