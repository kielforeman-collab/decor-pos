//! QuickBooks IIF export of the AR ledger and the inventory ledger
//! (Phase 3, v0.4; spec §4 `contractor_ar` — "accounting export/sync").
//!
//! IIF is QuickBooks Desktop's tab-delimited import format:
//! `!TRNS` / `!SPLNS` header rows, one `TRNS` row plus its `SPLNS`
//! distribution rows per ledger entry, closed by `!ENDTRNS`.
//! The convention used here (and by QuickBooks itself): the SPLNS
//! amounts are the negative of the TRNS amount, so every transaction
//! balances — [`parse_iif_transactions`] checks exactly that.
//!
//! Account names are the conventional QuickBooks defaults
//! (`Accounts Receivable`, `Sales Income`, `Undeposited Funds`,
//! `Finance Charge Income`, `Inventory Asset`,
//! `Inventory Adjustments`). A store that renamed those accounts in
//! QuickBooks must map them in its accounting plugin — the file
//! adapters take the names as parameters for that reason.
//!
//! Dates are `MM/DD/YYYY`, the format QuickBooks IIF expects.
//! Amounts are exact decimal strings (never floats).

use std::str::FromStr;

use chrono::NaiveDate;
use df_db::models::{ArLedgerEntry, InventoryLedgerEntry};
use rust_decimal::Decimal;

use crate::error::AccountingError;

/// QuickBooks account names used by the IIF export. Defaults match
/// a fresh QuickBooks company file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IifAccounts {
    pub accounts_receivable: String,
    pub sales_income: String,
    pub undeposited_funds: String,
    pub finance_charge_income: String,
    pub inventory_asset: String,
    pub inventory_adjustments: String,
}

impl Default for IifAccounts {
    fn default() -> Self {
        IifAccounts {
            accounts_receivable: "Accounts Receivable".to_string(),
            sales_income: "Sales Income".to_string(),
            undeposited_funds: "Undeposited Funds".to_string(),
            finance_charge_income: "Finance Charge Income".to_string(),
            inventory_asset: "Inventory Asset".to_string(),
            inventory_adjustments: "Inventory Adjustments".to_string(),
        }
    }
}

/// One parsed IIF transaction: the TRNS row plus its SPLNS rows.
#[derive(Debug, Clone, PartialEq)]
pub struct IifTransaction {
    pub trns_type: String,
    pub date: NaiveDate,
    pub account: String,
    pub name: String,
    pub amount: Decimal,
    pub docnum: String,
    pub memo: String,
    pub splits: Vec<IifSplit>,
}

/// One parsed IIF split (SPLNS) row.
#[derive(Debug, Clone, PartialEq)]
pub struct IifSplit {
    pub trns_type: String,
    pub date: NaiveDate,
    pub account: String,
    pub name: String,
    pub amount: Decimal,
    pub docnum: String,
    pub memo: String,
}

const TRNS_HEADER: &str = "!TRNS\tTRNSID\tTRNSTYPE\tDATE\tACCNT\tNAME\tAMOUNT\tDOCNUM\tMEMO";
const SPLNS_HEADER: &str = "!SPLNS\tSPLID\tTRNSTYPE\tDATE\tACCNT\tNAME\tAMOUNT\tDOCNUM\tMEMO";
const ENDTRNS: &str = "!ENDTRNS";

/// One parsed IIF data row: (type, date, account, name, amount,
/// docnum, memo).
type IifRow = (String, NaiveDate, String, String, Decimal, String, String);

fn iif_date(date: NaiveDate) -> String {
    date.format("%m/%d/%Y").to_string()
}

fn trns_row(
    trns_type: &str,
    date: NaiveDate,
    account: &str,
    name: &str,
    amount: Decimal,
    docnum: &str,
    memo: &str,
) -> String {
    format!(
        "TRNS\t\t{trns_type}\t{}\t{account}\t{name}\t{amount}\t{docnum}\t{memo}",
        iif_date(date)
    )
}

fn splns_row(
    trns_type: &str,
    date: NaiveDate,
    account: &str,
    name: &str,
    amount: Decimal,
    docnum: &str,
    memo: &str,
) -> String {
    format!(
        "SPLNS\t\t{trns_type}\t{}\t{account}\t{name}\t{amount}\t{docnum}\t{memo}",
        iif_date(date)
    )
}

fn sorted_ar(entries: &[ArLedgerEntry]) -> Vec<&ArLedgerEntry> {
    let mut entries: Vec<&ArLedgerEntry> = entries.iter().collect();
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    entries
}

/// IIF export of AR ledger entries. Each entry becomes one balanced
/// transaction: charges and finance charges are `INVOICE`s against
/// `Accounts Receivable` (offset `Sales Income` /
/// `Finance Charge Income`); payments and credit memos are
/// `PAYMENT` / `CREDIT MEMO` rows against `Accounts Receivable`
/// (offset `Undeposited Funds`).
pub fn export_ar_ledger_iif(entries: &[ArLedgerEntry], accounts: &IifAccounts) -> String {
    let mut out = String::new();
    out.push_str(TRNS_HEADER);
    out.push('\n');
    out.push_str(SPLNS_HEADER);
    out.push('\n');
    for entry in sorted_ar(entries) {
        let date = entry.created_at.date_naive();
        let name = entry.customer_id.to_string();
        let docnum = entry.reference_id.to_string();
        let (trns_type, offset) = match entry.entry_type.as_str() {
            "payment" => ("PAYMENT", accounts.undeposited_funds.as_str()),
            "credit_memo" => ("CREDIT MEMO", accounts.undeposited_funds.as_str()),
            "finance_charge" => ("INVOICE", accounts.finance_charge_income.as_str()),
            _ => ("INVOICE", accounts.sales_income.as_str()),
        };
        out.push_str(&trns_row(
            trns_type,
            date,
            &accounts.accounts_receivable,
            &name,
            entry.amount,
            &docnum,
            &entry.entry_type,
        ));
        out.push('\n');
        out.push_str(&splns_row(
            trns_type,
            date,
            offset,
            &name,
            -entry.amount,
            &docnum,
            &entry.entry_type,
        ));
        out.push('\n');
        out.push_str(ENDTRNS);
        out.push('\n');
    }
    out
}

/// IIF export of inventory ledger entries. Each entry becomes one
/// balanced `GENERAL JOURNAL`: the value change
/// (`delta_on_hand * unit_cost`) against `Inventory Asset`, offset
/// `Inventory Adjustments`. Entries with a zero value change are
/// still exported (a zero-amount journal keeps the audit trail).
pub fn export_inventory_ledger_iif(
    entries: &[InventoryLedgerEntry],
    accounts: &IifAccounts,
) -> String {
    let mut out = String::new();
    out.push_str(TRNS_HEADER);
    out.push('\n');
    out.push_str(SPLNS_HEADER);
    out.push('\n');
    let mut entries: Vec<&InventoryLedgerEntry> = entries.iter().collect();
    entries.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    for entry in entries {
        let date = entry.created_at.date_naive();
        let name = entry.product_id.to_string();
        let docnum = entry.reference_id.to_string();
        let value = entry.delta_on_hand * entry.unit_cost;
        out.push_str(&trns_row(
            "GENERAL JOURNAL",
            date,
            &accounts.inventory_asset,
            &name,
            value,
            &docnum,
            &entry.reason_type,
        ));
        out.push('\n');
        out.push_str(&splns_row(
            "GENERAL JOURNAL",
            date,
            &accounts.inventory_adjustments,
            &name,
            -value,
            &docnum,
            &entry.reason_type,
        ));
        out.push('\n');
        out.push_str(ENDTRNS);
        out.push('\n');
    }
    out
}

/// Parse IIF text into transactions, validating the structure:
/// `!TRNS` / `!SPLNS` headers first, every `TRNS` followed by its
/// `SPLNS` rows and an `!ENDTRNS`, and every transaction balanced
/// (splits sum to the negative of the TRNS amount). Used by the
/// exit test and by [`crate::plugin::IifAccountingPlugin::import_payments`].
pub fn parse_iif_transactions(text: &str) -> Result<Vec<IifTransaction>, AccountingError> {
    let mut lines = text.lines().peekable();
    let mut transactions = Vec::new();
    let mut current: Option<IifTransaction> = None;
    let mut saw_trns_header = false;
    let mut saw_splns_header = false;
    for (lineno, line) in lines.by_ref().enumerate() {
        let line_no = lineno + 1;
        if line.starts_with("!TRNS") {
            if line != TRNS_HEADER {
                return Err(AccountingError::Parse(format!(
                    "line {line_no}: unexpected !TRNS header: {line}"
                )));
            }
            saw_trns_header = true;
            continue;
        }
        if line.starts_with("!SPLNS") {
            if line != SPLNS_HEADER {
                return Err(AccountingError::Parse(format!(
                    "line {line_no}: unexpected !SPLNS header: {line}"
                )));
            }
            saw_splns_header = true;
            continue;
        }
        if line == ENDTRNS {
            match current.take() {
                Some(tx) => {
                    let split_sum = tx
                        .splits
                        .iter()
                        .map(|s| s.amount)
                        .fold(Decimal::ZERO, |acc, amount| acc + amount);
                    if split_sum != -tx.amount {
                        return Err(AccountingError::Parse(format!(
                            "line {line_no}: transaction {trns_type} {docnum} is unbalanced: TRNS {} but splits sum to {split_sum}",
                            tx.amount,
                            trns_type = tx.trns_type,
                            docnum = tx.docnum
                        )));
                    }
                    transactions.push(tx);
                }
                None => {
                    return Err(AccountingError::Parse(format!(
                        "line {line_no}: !ENDTRNS with no open transaction"
                    )));
                }
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let kind = fields.first().copied().unwrap_or("");
        let get = |idx: usize| fields.get(idx).copied().unwrap_or("");
        let parse_row = |kind: &str| -> Result<IifRow, AccountingError> {
            if fields.len() != 9 {
                return Err(AccountingError::Parse(format!(
                    "line {line_no}: {kind} row has {} tab-separated fields, expected 9",
                    fields.len()
                )));
            }
            let trns_type = get(2).to_string();
            let date = NaiveDate::parse_from_str(get(3), "%m/%d/%Y").map_err(|_| {
                AccountingError::Parse(format!(
                    "line {line_no}: bad IIF date (expected MM/DD/YYYY): {}",
                    get(3)
                ))
            })?;
            let amount = Decimal::from_str(get(6)).map_err(|_| {
                AccountingError::Parse(format!("line {line_no}: bad IIF amount: {}", get(6)))
            })?;
            Ok((
                trns_type,
                date,
                get(4).to_string(),
                get(5).to_string(),
                amount,
                get(7).to_string(),
                get(8).to_string(),
            ))
        };
        match kind {
            "TRNS" => {
                if !saw_trns_header || !saw_splns_header {
                    return Err(AccountingError::Parse(format!(
                        "line {line_no}: TRNS row before !TRNS/!SPLNS headers"
                    )));
                }
                if current.is_some() {
                    return Err(AccountingError::Parse(format!(
                        "line {line_no}: TRNS row before the previous transaction's !ENDTRNS"
                    )));
                }
                let (trns_type, date, account, name, amount, docnum, memo) = parse_row("TRNS")?;
                current = Some(IifTransaction {
                    trns_type,
                    date,
                    account,
                    name,
                    amount,
                    docnum,
                    memo,
                    splits: Vec::new(),
                });
            }
            "SPLNS" => {
                let Some(tx) = current.as_mut() else {
                    return Err(AccountingError::Parse(format!(
                        "line {line_no}: SPLNS row with no open TRNS"
                    )));
                };
                let (trns_type, date, account, name, amount, docnum, memo) = parse_row("SPLNS")?;
                tx.splits.push(IifSplit {
                    trns_type,
                    date,
                    account,
                    name,
                    amount,
                    docnum,
                    memo,
                });
            }
            other => {
                return Err(AccountingError::Parse(format!(
                    "line {line_no}: unknown IIF row type {other:?}"
                )));
            }
        }
    }
    if current.is_some() {
        return Err(AccountingError::Parse(
            "IIF ends with an open transaction (missing !ENDTRNS)".to_string(),
        ));
    }
    Ok(transactions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use uuid::Uuid;

    fn ar_entry(id: u128, amount: &str, entry_type: &str, created: &str) -> ArLedgerEntry {
        ArLedgerEntry {
            id: Uuid::from_u128(id),
            org_id: Uuid::from_u128(0x2000),
            store_id: Uuid::from_u128(0x1000),
            customer_id: Uuid::from_u128(0x4000),
            job_id: None,
            entry_type: entry_type.to_string(),
            reference_type: "transaction".to_string(),
            reference_id: Uuid::from_u128(0x5000),
            amount: Decimal::from_str(amount).unwrap(),
            due_date: None,
            statement_id: None,
            created_at: DateTime::parse_from_rfc3339(created)
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    #[test]
    fn ar_iif_parses_and_balances() {
        let entries = vec![
            ar_entry(0x1, "1200.0000", "charge", "2026-11-05T10:00:00Z"),
            ar_entry(0x2, "-800.0000", "payment", "2026-11-20T10:00:00Z"),
            ar_entry(0x3, "18.0000", "finance_charge", "2026-11-30T10:00:00Z"),
        ];
        let iif = export_ar_ledger_iif(&entries, &IifAccounts::default());
        assert!(
            iif.starts_with("!TRNS\tTRNSID\tTRNSTYPE\tDATE\tACCNT\tNAME\tAMOUNT\tDOCNUM\tMEMO\n")
        );
        let txs = parse_iif_transactions(&iif).unwrap();
        assert_eq!(txs.len(), 3);
        assert_eq!(txs[0].trns_type, "INVOICE");
        assert_eq!(txs[0].account, "Accounts Receivable");
        assert_eq!(txs[0].amount, Decimal::from_str("1200.0000").unwrap());
        assert_eq!(txs[0].splits.len(), 1);
        assert_eq!(txs[0].splits[0].account, "Sales Income");
        assert_eq!(
            txs[0].splits[0].amount,
            Decimal::from_str("-1200.0000").unwrap()
        );
        assert_eq!(txs[1].trns_type, "PAYMENT");
        assert_eq!(txs[1].splits[0].account, "Undeposited Funds");
        assert_eq!(txs[2].splits[0].account, "Finance Charge Income");
        // Every transaction balances: splits sum to -TRNS amount.
        for tx in &txs {
            let sum = tx
                .splits
                .iter()
                .map(|s| s.amount)
                .fold(Decimal::ZERO, |acc, a| acc + a);
            assert_eq!(sum, -tx.amount);
        }
    }

    #[test]
    fn inventory_iif_values_on_hand_change() {
        let entry = InventoryLedgerEntry {
            id: Uuid::from_u128(0x1),
            org_id: Uuid::from_u128(0x2000),
            store_id: Uuid::from_u128(0x1000),
            product_id: Uuid::from_u128(0x3000),
            lot_number: None,
            delta_on_hand: Decimal::from_str("-2.0000").unwrap(),
            delta_committed: Decimal::ZERO,
            delta_on_order: Decimal::ZERO,
            delta_in_transit: Decimal::ZERO,
            unit_cost: Decimal::from_str("50.0000").unwrap(),
            reason_type: "sale".to_string(),
            reference_id: Uuid::from_u128(0x5000),
            created_at: DateTime::parse_from_rfc3339("2026-11-05T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let iif = export_inventory_ledger_iif(&[entry], &IifAccounts::default());
        let txs = parse_iif_transactions(&iif).unwrap();
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].trns_type, "GENERAL JOURNAL");
        assert_eq!(txs[0].amount, Decimal::from_str("-100.0000").unwrap());
        assert_eq!(
            txs[0].splits[0].amount,
            Decimal::from_str("100.0000").unwrap()
        );
    }

    #[test]
    fn iif_parse_rejects_unbalanced_and_truncated() {
        let entries = vec![ar_entry(0x1, "100.0000", "charge", "2026-11-05T10:00:00Z")];
        let iif = export_ar_ledger_iif(&entries, &IifAccounts::default());
        // Break the balance: flip the split sign.
        let broken = iif.replace("\t-100.0000\t", "\t100.0000\t");
        assert!(parse_iif_transactions(&broken).is_err());
        // Truncate before !ENDTRNS.
        let truncated = iif.lines().take(3).collect::<Vec<_>>().join("\n");
        assert!(parse_iif_transactions(&truncated).is_err());
    }
}
