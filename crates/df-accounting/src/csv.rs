//! CSV export of the AR ledger and the inventory ledger (Phase 3,
//! v0.4; spec §4 `contractor_ar` — "accounting export/sync").
//!
//! One row per ledger entry, header row first, deterministic order
//! (`created_at`, then `id`). Amounts are exact decimal strings
//! (`rust_decimal::Decimal` — never floats), so a round-trip through
//! [`parse_ar_ledger_csv`] / [`parse_inventory_ledger_csv`] matches
//! to the cent. Fields are RFC 4180-escaped (quoted when they contain
//! a comma, quote or newline).

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use df_db::models::{ArLedgerEntry, InventoryLedgerEntry};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::error::AccountingError;

const AR_HEADER: &str = "id,org_id,store_id,customer_id,job_id,entry_type,reference_type,reference_id,amount,due_date,statement_id,created_at";
const INVENTORY_HEADER: &str = "id,org_id,store_id,product_id,lot_number,delta_on_hand,delta_committed,delta_on_order,delta_in_transit,unit_cost,reason_type,reference_id,created_at";

/// Escape one CSV field (RFC 4180).
fn escape_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn opt_uuid(value: Option<Uuid>) -> String {
    value.map(|id| id.to_string()).unwrap_or_default()
}

fn opt_date(value: Option<NaiveDate>) -> String {
    value.map(|d| d.to_string()).unwrap_or_default()
}

/// Minimal RFC 4180 parser: handles quoted fields (with `""`
/// escapes) and bare fields. Returns rows of fields; the caller
/// checks the header.
fn parse_csv(text: &str) -> Result<Vec<Vec<String>>, AccountingError> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            }
            '"' if field.is_empty() => {
                in_quotes = true;
            }
            ',' if !in_quotes => {
                row.push(std::mem::take(&mut field));
            }
            '\r' if !in_quotes => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\n' if !in_quotes => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(ch),
        }
    }
    if in_quotes {
        return Err(AccountingError::Parse(
            "unterminated quoted field".to_string(),
        ));
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    // Drop a trailing empty row from a final newline.
    while rows.last().is_some_and(|r| r.len() == 1 && r[0].is_empty()) {
        rows.pop();
    }
    Ok(rows)
}

fn parse_uuid(text: &str, column: &str) -> Result<Uuid, AccountingError> {
    Uuid::from_str(text)
        .map_err(|_| AccountingError::Parse(format!("{column} is not a UUID: {text}")))
}

fn parse_opt_uuid(text: &str, column: &str) -> Result<Option<Uuid>, AccountingError> {
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_uuid(text, column)?))
    }
}

fn parse_dec(text: &str, column: &str) -> Result<Decimal, AccountingError> {
    Decimal::from_str(text)
        .map_err(|_| AccountingError::Parse(format!("{column} is not a decimal: {text}")))
}

fn parse_opt_date(text: &str, column: &str) -> Result<Option<NaiveDate>, AccountingError> {
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(NaiveDate::from_str(text).map_err(|_| {
            AccountingError::Parse(format!("{column} is not a date: {text}"))
        })?))
    }
}

fn parse_dt(text: &str, column: &str) -> Result<DateTime<Utc>, AccountingError> {
    DateTime::parse_from_rfc3339(text)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| {
            AccountingError::Parse(format!("{column} is not an RFC 3339 timestamp: {text}"))
        })
}

fn sorted_ar(entries: &[ArLedgerEntry]) -> Vec<&ArLedgerEntry> {
    let mut entries: Vec<&ArLedgerEntry> = entries.iter().collect();
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    entries
}

fn sorted_inventory(entries: &[InventoryLedgerEntry]) -> Vec<&InventoryLedgerEntry> {
    let mut entries: Vec<&InventoryLedgerEntry> = entries.iter().collect();
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    entries
}

/// CSV export of AR ledger entries (header + one row per entry).
pub fn export_ar_ledger_csv(entries: &[ArLedgerEntry]) -> String {
    let mut out = String::from(AR_HEADER);
    out.push('\n');
    for entry in sorted_ar(entries) {
        let fields = [
            entry.id.to_string(),
            entry.org_id.to_string(),
            entry.store_id.to_string(),
            entry.customer_id.to_string(),
            opt_uuid(entry.job_id),
            entry.entry_type.clone(),
            entry.reference_type.clone(),
            entry.reference_id.to_string(),
            entry.amount.to_string(),
            opt_date(entry.due_date),
            opt_uuid(entry.statement_id),
            entry.created_at.to_rfc3339(),
        ];
        out.push_str(
            &fields
                .iter()
                .map(|f| escape_field(f))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

/// Parse [`export_ar_ledger_csv`] output back (round-trip check).
pub fn parse_ar_ledger_csv(text: &str) -> Result<Vec<ArLedgerEntry>, AccountingError> {
    let rows = parse_csv(text)?;
    let Some((header, body)) = rows.split_first() else {
        return Err(AccountingError::Parse("empty CSV".to_string()));
    };
    if header.join(",") != AR_HEADER {
        return Err(AccountingError::Parse(format!(
            "unexpected AR CSV header: {}",
            header.join(",")
        )));
    }
    let mut entries = Vec::with_capacity(body.len());
    for (i, row) in body.iter().enumerate() {
        if row.len() != 12 {
            return Err(AccountingError::Parse(format!(
                "row {} has {} fields, expected 12",
                i + 2,
                row.len()
            )));
        }
        entries.push(ArLedgerEntry {
            id: parse_uuid(&row[0], "id")?,
            org_id: parse_uuid(&row[1], "org_id")?,
            store_id: parse_uuid(&row[2], "store_id")?,
            customer_id: parse_uuid(&row[3], "customer_id")?,
            job_id: parse_opt_uuid(&row[4], "job_id")?,
            entry_type: row[5].clone(),
            reference_type: row[6].clone(),
            reference_id: parse_uuid(&row[7], "reference_id")?,
            amount: parse_dec(&row[8], "amount")?,
            due_date: parse_opt_date(&row[9], "due_date")?,
            statement_id: parse_opt_uuid(&row[10], "statement_id")?,
            created_at: parse_dt(&row[11], "created_at")?,
        });
    }
    Ok(entries)
}

/// CSV export of inventory ledger entries (header + one row per entry).
pub fn export_inventory_ledger_csv(entries: &[InventoryLedgerEntry]) -> String {
    let mut out = String::from(INVENTORY_HEADER);
    out.push('\n');
    for entry in sorted_inventory(entries) {
        let fields = [
            entry.id.to_string(),
            entry.org_id.to_string(),
            entry.store_id.to_string(),
            entry.product_id.to_string(),
            entry.lot_number.clone().unwrap_or_default(),
            entry.delta_on_hand.to_string(),
            entry.delta_committed.to_string(),
            entry.delta_on_order.to_string(),
            entry.delta_in_transit.to_string(),
            entry.unit_cost.to_string(),
            entry.reason_type.clone(),
            entry.reference_id.to_string(),
            entry.created_at.to_rfc3339(),
        ];
        out.push_str(
            &fields
                .iter()
                .map(|f| escape_field(f))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

/// Parse [`export_inventory_ledger_csv`] output back (round-trip check).
pub fn parse_inventory_ledger_csv(
    text: &str,
) -> Result<Vec<InventoryLedgerEntry>, AccountingError> {
    let rows = parse_csv(text)?;
    let Some((header, body)) = rows.split_first() else {
        return Err(AccountingError::Parse("empty CSV".to_string()));
    };
    if header.join(",") != INVENTORY_HEADER {
        return Err(AccountingError::Parse(format!(
            "unexpected inventory CSV header: {}",
            header.join(",")
        )));
    }
    let mut entries = Vec::with_capacity(body.len());
    for (i, row) in body.iter().enumerate() {
        if row.len() != 13 {
            return Err(AccountingError::Parse(format!(
                "row {} has {} fields, expected 13",
                i + 2,
                row.len()
            )));
        }
        entries.push(InventoryLedgerEntry {
            id: parse_uuid(&row[0], "id")?,
            org_id: parse_uuid(&row[1], "org_id")?,
            store_id: parse_uuid(&row[2], "store_id")?,
            product_id: parse_uuid(&row[3], "product_id")?,
            lot_number: if row[4].is_empty() {
                None
            } else {
                Some(row[4].clone())
            },
            delta_on_hand: parse_dec(&row[5], "delta_on_hand")?,
            delta_committed: parse_dec(&row[6], "delta_committed")?,
            delta_on_order: parse_dec(&row[7], "delta_on_order")?,
            delta_in_transit: parse_dec(&row[8], "delta_in_transit")?,
            unit_cost: parse_dec(&row[9], "unit_cost")?,
            reason_type: row[10].clone(),
            reference_id: parse_uuid(&row[11], "reference_id")?,
            created_at: parse_dt(&row[12], "created_at")?,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_db::models::ArLedgerEntry;

    fn ar_entry(id: u128, amount: &str, entry_type: &str, created: &str) -> ArLedgerEntry {
        ArLedgerEntry {
            id: Uuid::from_u128(id),
            org_id: Uuid::from_u128(0x2000),
            store_id: Uuid::from_u128(0x1000),
            customer_id: Uuid::from_u128(0x4000),
            job_id: Some(Uuid::from_u128(0x4100)),
            entry_type: entry_type.to_string(),
            reference_type: "transaction".to_string(),
            reference_id: Uuid::from_u128(0x5000),
            amount: Decimal::from_str(amount).unwrap(),
            due_date: Some(NaiveDate::from_str("2026-12-05").unwrap()),
            statement_id: None,
            created_at: DateTime::parse_from_rfc3339(created)
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    #[test]
    fn ar_csv_round_trips_exactly() {
        // Deliberately out of order: export sorts deterministically.
        let entries = vec![
            ar_entry(0x3, "-800.0000", "payment", "2026-11-20T10:00:00Z"),
            ar_entry(0x1, "500.0000", "charge", "2026-10-15T10:00:00Z"),
            ar_entry(0x2, "1200.0000", "charge", "2026-11-05T10:00:00Z"),
        ];
        let csv = export_ar_ledger_csv(&entries);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], AR_HEADER);
        // Sorted by created_at: Oct charge first.
        assert!(lines[1].contains("500.0000"));
        assert!(lines[3].contains("-800.0000"));
        let back = parse_ar_ledger_csv(&csv).unwrap();
        assert_eq!(back.len(), 3);
        // Compare as sorted sets: amounts match to the cent.
        let mut want: Vec<Decimal> = entries.iter().map(|e| e.amount).collect();
        let mut got: Vec<Decimal> = back.iter().map(|e| e.amount).collect();
        want.sort();
        got.sort();
        assert_eq!(want, got);
        // Full-row equality after sorting both by id.
        let mut want = entries.clone();
        let mut got = back.clone();
        want.sort_by_key(|e| e.id);
        got.sort_by_key(|e| e.id);
        assert_eq!(want, got);
    }

    #[test]
    fn csv_escapes_commas_and_quotes() {
        let mut entry = ar_entry(0x1, "10.0000", "charge", "2026-11-05T10:00:00Z");
        entry.reference_type = "note, with \"quotes\"".to_string();
        let csv = export_ar_ledger_csv(&[entry]);
        let back = parse_ar_ledger_csv(&csv).unwrap();
        assert_eq!(back[0].reference_type, "note, with \"quotes\"");
    }

    #[test]
    fn inventory_csv_round_trips_exactly() {
        let entry = InventoryLedgerEntry {
            id: Uuid::from_u128(0x1),
            org_id: Uuid::from_u128(0x2000),
            store_id: Uuid::from_u128(0x1000),
            product_id: Uuid::from_u128(0x3000),
            lot_number: Some("LOT-7, \"A\"".to_string()),
            delta_on_hand: Decimal::from_str("-3.5000").unwrap(),
            delta_committed: Decimal::ZERO,
            delta_on_order: Decimal::from_str("12.0000").unwrap(),
            delta_in_transit: Decimal::ZERO,
            unit_cost: Decimal::from_str("42.2500").unwrap(),
            reason_type: "sale".to_string(),
            reference_id: Uuid::from_u128(0x5000),
            created_at: DateTime::parse_from_rfc3339("2026-11-05T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let csv = export_inventory_ledger_csv(std::slice::from_ref(&entry));
        let back = parse_inventory_ledger_csv(&csv).unwrap();
        assert_eq!(back, vec![entry]);
    }

    #[test]
    fn csv_parse_rejects_bad_header_and_bad_amount() {
        assert!(parse_ar_ledger_csv("a,b,c\n").is_err());
        let mut entry = ar_entry(0x1, "10.0000", "charge", "2026-11-05T10:00:00Z");
        let mut csv = export_ar_ledger_csv(&[entry.clone()]);
        csv = csv.replace("10.0000", "ten");
        assert!(parse_ar_ledger_csv(&csv).is_err());
        entry.amount = Decimal::from_str("10.0000").unwrap();
        let _ = entry;
    }
}
