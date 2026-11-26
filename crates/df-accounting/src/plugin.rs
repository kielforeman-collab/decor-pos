//! The `AccountingPlugin` trait and its file-based adapters
//! (Phase 3, v0.4; spec §6.2 `AccountingPlugin`).
//!
//! Phase 3 ships **file-based** accounting integration only:
//! [`CsvAccountingPlugin`] writes CSV exports and
//! [`IifAccountingPlugin`] writes QuickBooks IIF, and both can import
//! a payments file back. No live API calls, no credentials, no
//! network — a plugin that talks to QuickBooks Online or Xero over
//! HTTP is a **Phase 4+ / vendor-pack** concern (spec §6.2 lists
//! `push_daily_journal`, `sync_customers`, `sync_ar_payments` and
//! `health_check` as the live-sync interface; those methods are
//! deliberately NOT on this trait yet, so no one mistakes a file
//! export for a live sync).
//!
//! The trait is file-based on purpose: the store operator drops the
//! export into QuickBooks/Xero (or their importer) by hand, and
//! imports a payments file the same way. All amounts are
//! `rust_decimal::Decimal` — never floats.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use df_db::models::{ArLedgerEntry, Customer, InventoryLedgerEntry};
use rust_decimal::Decimal;

use crate::csv::{export_ar_ledger_csv, export_inventory_ledger_csv};
use crate::error::AccountingError;
use crate::iif::{
    IifAccounts, export_ar_ledger_iif, export_inventory_ledger_iif, parse_iif_transactions,
};

/// A payment imported from an accounting file: the customer's
/// account number, the payment date, the exact amount and the
/// reference the accountant recorded. Applying it to the AR ledger
/// is the caller's job (`df-ar` owns payment posting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedPayment {
    pub account_number: String,
    pub date: NaiveDate,
    pub amount: Decimal,
    pub reference: String,
    pub memo: Option<String>,
}

/// Summary of one export: how many rows were written and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub rows: usize,
    pub bytes: usize,
    pub path: PathBuf,
}

/// File-based accounting plugin: export ledgers/customers to a file
/// and import payments from a file. Implementations must be pure
/// file I/O — no network, no credentials.
pub trait AccountingPlugin {
    fn name(&self) -> &str;

    /// Daily GL journal for the AR entries (charges, payments,
    /// finance charges) as debit/credit rows.
    fn export_journal(
        &self,
        entries: &[ArLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError>;

    fn export_customers(
        &self,
        customers: &[Customer],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError>;

    fn export_ar_ledger(
        &self,
        entries: &[ArLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError>;

    fn export_inventory_ledger(
        &self,
        entries: &[InventoryLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError>;

    /// Read payments back from a file this plugin (or the
    /// accountant's software) wrote. Returns the payments in file
    /// order; amounts are exact decimals.
    fn import_payments(&self, path: &Path) -> Result<Vec<ImportedPayment>, AccountingError>;
}

fn write_file(out: &Path, text: &str, rows: usize) -> Result<ExportSummary, AccountingError> {
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(out, text)?;
    Ok(ExportSummary {
        rows,
        bytes: text.len(),
        path: out.to_path_buf(),
    })
}

/// CSV accounting plugin: ledgers as CSV (see [`crate::csv`]),
/// journal as a debit/credit CSV, customers as CSV, payments
/// imported from a CSV with header
/// `account_number,date,amount,reference,memo`.
pub struct CsvAccountingPlugin;

impl CsvAccountingPlugin {
    /// GL journal CSV for AR entries: one debit row + one credit row
    /// per entry. Charges debit Accounts Receivable / credit Sales
    /// Income; payments debit Undeposited Funds / credit Accounts
    /// Receivable; finance charges debit Accounts Receivable /
    /// credit Finance Charge Income.
    pub fn journal_csv(entries: &[ArLedgerEntry]) -> String {
        let mut out = String::from("date,account,debit,credit,reference,memo\n");
        let mut entries: Vec<&ArLedgerEntry> = entries.iter().collect();
        entries.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        for entry in entries {
            let date = entry.created_at.date_naive();
            let (debit_account, credit_account) = match entry.entry_type.as_str() {
                "payment" | "credit_memo" => ("Undeposited Funds", "Accounts Receivable"),
                "finance_charge" => ("Accounts Receivable", "Finance Charge Income"),
                _ => ("Accounts Receivable", "Sales Income"),
            };
            let amount = entry.amount.abs();
            let reference = entry.reference_id.to_string();
            // The sides above assume the entry's normal sign
            // (charges positive, payments negative); an entry with
            // the opposite sign is a reversal, so its sides flip.
            let normal_sign = match entry.entry_type.as_str() {
                "payment" | "credit_memo" => entry.amount <= Decimal::ZERO,
                _ => entry.amount >= Decimal::ZERO,
            };
            let (debit, credit) = if normal_sign {
                (debit_account, credit_account)
            } else {
                (credit_account, debit_account)
            };
            out.push_str(&format!(
                "{date},{debit},{amount},,{reference},{}\n",
                entry.entry_type
            ));
            out.push_str(&format!(
                "{date},{credit},,{amount},{reference},{}\n",
                entry.entry_type
            ));
        }
        out
    }

    /// Customers as CSV: account, type, company/name, contact,
    /// credit limit, terms, charge flag.
    pub fn customers_csv(customers: &[Customer]) -> String {
        let mut out = String::from(
            "account_number,customer_type,company_name,first_name,last_name,email,phone,credit_limit,payment_terms,charge_enabled\n",
        );
        let mut customers: Vec<&Customer> = customers.iter().collect();
        customers.sort_by(|a, b| a.account_number.cmp(&b.account_number));
        for customer in customers {
            let esc = |value: &str| -> String {
                if value.contains(',') || value.contains('"') || value.contains('\n') {
                    format!("\"{}\"", value.replace('"', "\"\""))
                } else {
                    value.to_string()
                }
            };
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{}\n",
                esc(&customer.account_number),
                esc(&customer.customer_type),
                esc(customer.company_name.as_deref().unwrap_or("")),
                esc(customer.first_name.as_deref().unwrap_or("")),
                esc(customer.last_name.as_deref().unwrap_or("")),
                esc(customer.email.as_deref().unwrap_or("")),
                esc(customer.phone.as_deref().unwrap_or("")),
                customer.credit_limit,
                esc(&customer.payment_terms),
                customer.charge_enabled,
            ));
        }
        out
    }

    /// Parse a payments CSV (`account_number,date,amount,reference,memo`).
    pub fn parse_payments_csv(text: &str) -> Result<Vec<ImportedPayment>, AccountingError> {
        let mut lines = text.lines();
        let Some(header) = lines.next() else {
            return Err(AccountingError::Parse("empty payments CSV".to_string()));
        };
        if header != "account_number,date,amount,reference,memo" {
            return Err(AccountingError::Parse(format!(
                "unexpected payments CSV header: {header}"
            )));
        }
        let mut payments = Vec::new();
        for (lineno, line) in lines.enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() != 5 {
                return Err(AccountingError::Parse(format!(
                    "line {}: expected 5 fields, got {}",
                    lineno + 2,
                    fields.len()
                )));
            }
            payments.push(ImportedPayment {
                account_number: fields[0].to_string(),
                date: NaiveDate::parse_from_str(fields[1], "%Y-%m-%d").map_err(|_| {
                    AccountingError::Parse(format!(
                        "line {}: bad date (expected YYYY-MM-DD): {}",
                        lineno + 2,
                        fields[1]
                    ))
                })?,
                amount: fields[2].parse::<Decimal>().map_err(|_| {
                    AccountingError::Parse(format!(
                        "line {}: bad amount: {}",
                        lineno + 2,
                        fields[2]
                    ))
                })?,
                reference: fields[3].to_string(),
                memo: if fields[4].is_empty() {
                    None
                } else {
                    Some(fields[4].to_string())
                },
            });
        }
        Ok(payments)
    }
}

impl AccountingPlugin for CsvAccountingPlugin {
    fn name(&self) -> &str {
        "csv"
    }

    fn export_journal(
        &self,
        entries: &[ArLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError> {
        let text = Self::journal_csv(entries);
        write_file(out, &text, entries.len() * 2)
    }

    fn export_customers(
        &self,
        customers: &[Customer],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError> {
        let text = Self::customers_csv(customers);
        write_file(out, &text, customers.len())
    }

    fn export_ar_ledger(
        &self,
        entries: &[ArLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError> {
        let text = export_ar_ledger_csv(entries);
        write_file(out, &text, entries.len())
    }

    fn export_inventory_ledger(
        &self,
        entries: &[InventoryLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError> {
        let text = export_inventory_ledger_csv(entries);
        write_file(out, &text, entries.len())
    }

    fn import_payments(&self, path: &Path) -> Result<Vec<ImportedPayment>, AccountingError> {
        let text = fs::read_to_string(path)?;
        Self::parse_payments_csv(&text)
    }
}

/// QuickBooks IIF accounting plugin: ledgers as IIF (see
/// [`crate::iif`]), journal as a balanced `GENERAL JOURNAL` IIF,
/// customers as `!CUST` rows, payments imported from IIF `PAYMENT`
/// transactions.
#[derive(Default)]
pub struct IifAccountingPlugin {
    pub accounts: IifAccounts,
}

impl IifAccountingPlugin {
    /// GL journal as IIF: one balanced `GENERAL JOURNAL` per AR
    /// entry (same debit/credit mapping as the CSV journal).
    pub fn journal_iif(entries: &[ArLedgerEntry], accounts: &IifAccounts) -> String {
        let mut out = String::from(
            "!TRNS\tTRNSID\tTRNSTYPE\tDATE\tACCNT\tNAME\tAMOUNT\tDOCNUM\tMEMO\n!SPLNS\tSPLID\tTRNSTYPE\tDATE\tACCNT\tNAME\tAMOUNT\tDOCNUM\tMEMO\n",
        );
        let mut entries: Vec<&ArLedgerEntry> = entries.iter().collect();
        entries.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        for entry in entries {
            let date = entry.created_at.date_naive().format("%m/%d/%Y");
            let name = entry.customer_id.to_string();
            let docnum = entry.reference_id.to_string();
            let (debit_account, credit_account) = match entry.entry_type.as_str() {
                "payment" | "credit_memo" => (
                    accounts.undeposited_funds.as_str(),
                    accounts.accounts_receivable.as_str(),
                ),
                "finance_charge" => (
                    accounts.accounts_receivable.as_str(),
                    accounts.finance_charge_income.as_str(),
                ),
                _ => (
                    accounts.accounts_receivable.as_str(),
                    accounts.sales_income.as_str(),
                ),
            };
            let amount = entry.amount.abs();
            // The sides above assume the entry's normal sign
            // (charges positive, payments negative); an entry with
            // the opposite sign is a reversal, so its sides flip.
            let normal_sign = match entry.entry_type.as_str() {
                "payment" | "credit_memo" => entry.amount <= Decimal::ZERO,
                _ => entry.amount >= Decimal::ZERO,
            };
            let (debit, credit) = if normal_sign {
                (debit_account, credit_account)
            } else {
                (credit_account, debit_account)
            };
            out.push_str(&format!(
                "TRNS\t\tGENERAL JOURNAL\t{date}\t{debit}\t{name}\t{amount}\t{docnum}\t{}\n",
                entry.entry_type
            ));
            out.push_str(&format!(
                "SPLNS\t\tGENERAL JOURNAL\t{date}\t{credit}\t{name}\t-{amount}\t{docnum}\t{}\n",
                entry.entry_type
            ));
            out.push_str("!ENDTRNS\n");
        }
        out
    }

    /// Customers as QuickBooks `!CUST` import rows.
    pub fn customers_iif(customers: &[Customer]) -> String {
        let mut out =
            String::from("!CUST\tNAME\tREFNUM\tBADDR1\tPHONE1\tEMAIL\tTERMS\tCREDITLIMIT\n");
        let mut customers: Vec<&Customer> = customers.iter().collect();
        customers.sort_by(|a, b| a.account_number.cmp(&b.account_number));
        for customer in customers {
            let name = customer.company_name.clone().unwrap_or_else(|| {
                let first = customer.first_name.clone().unwrap_or_default();
                let last = customer.last_name.clone().unwrap_or_default();
                format!("{first} {last}").trim().to_string()
            });
            out.push_str(&format!(
                "CUST\t{}\t{}\t\t{}\t{}\t{}\t{}\n",
                name,
                customer.account_number,
                customer.phone.clone().unwrap_or_default(),
                customer.email.clone().unwrap_or_default(),
                customer.payment_terms,
                customer.credit_limit,
            ));
        }
        out
    }
}

impl AccountingPlugin for IifAccountingPlugin {
    fn name(&self) -> &str {
        "iif"
    }

    fn export_journal(
        &self,
        entries: &[ArLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError> {
        let text = Self::journal_iif(entries, &self.accounts);
        write_file(out, &text, entries.len())
    }

    fn export_customers(
        &self,
        customers: &[Customer],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError> {
        let text = Self::customers_iif(customers);
        write_file(out, &text, customers.len())
    }

    fn export_ar_ledger(
        &self,
        entries: &[ArLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError> {
        let text = export_ar_ledger_iif(entries, &self.accounts);
        write_file(out, &text, entries.len())
    }

    fn export_inventory_ledger(
        &self,
        entries: &[InventoryLedgerEntry],
        out: &Path,
    ) -> Result<ExportSummary, AccountingError> {
        let text = export_inventory_ledger_iif(entries, &self.accounts);
        write_file(out, &text, entries.len())
    }

    fn import_payments(&self, path: &Path) -> Result<Vec<ImportedPayment>, AccountingError> {
        let text = fs::read_to_string(path)?;
        let transactions = parse_iif_transactions(&text)?;
        let mut payments = Vec::new();
        for tx in transactions {
            if tx.trns_type != "PAYMENT" {
                continue;
            }
            payments.push(ImportedPayment {
                account_number: tx.name.clone(),
                date: tx.date,
                // A payment reduces AR, so the TRNS amount is
                // negative; the imported payment is its magnitude.
                amount: tx.amount.abs(),
                reference: tx.docnum.clone(),
                memo: if tx.memo.is_empty() {
                    None
                } else {
                    Some(tx.memo.clone())
                },
            });
        }
        Ok(payments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use std::str::FromStr;
    use uuid::Uuid;

    fn ar_entry(id: u128, amount: &str, entry_type: &str) -> ArLedgerEntry {
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
            created_at: DateTime::parse_from_rfc3339("2026-11-05T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    fn customer() -> Customer {
        Customer {
            id: Uuid::from_u128(0x4000),
            org_id: Uuid::from_u128(0x2000),
            account_number: "C-1042".to_string(),
            customer_type: "contractor".to_string(),
            company_name: Some("Harbourview Painting Ltd.".to_string()),
            first_name: None,
            last_name: None,
            email: Some("ap@harbourview.example".to_string()),
            phone: Some("604-555-0100".to_string()),
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: true,
            credit_limit: Decimal::from_str("5000.0000").unwrap(),
            payment_terms: "NET30".to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
        }
    }

    #[test]
    fn csv_journal_balances_per_entry() {
        let entries = vec![
            ar_entry(0x1, "1200.0000", "charge"),
            ar_entry(0x2, "-800.0000", "payment"),
            ar_entry(0x3, "18.0000", "finance_charge"),
        ];
        let csv = CsvAccountingPlugin::journal_csv(&entries);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "date,account,debit,credit,reference,memo");
        // Two rows per entry, debits == credits per entry.
        assert_eq!(lines.len(), 1 + 6);
        for chunk in lines[1..].chunks(2) {
            let debit: Decimal = chunk[0].split(',').nth(2).unwrap().parse().unwrap();
            let credit: Decimal = chunk[1].split(',').nth(3).unwrap().parse().unwrap();
            assert_eq!(debit, credit);
        }
        assert!(csv.contains("Accounts Receivable,1200.0000,"));
        assert!(csv.contains("Undeposited Funds,800.0000,"));
    }

    #[test]
    fn iif_journal_parses_and_balances() {
        let entries = vec![
            ar_entry(0x1, "1200.0000", "charge"),
            ar_entry(0x2, "-800.0000", "payment"),
        ];
        let iif = IifAccountingPlugin::journal_iif(&entries, &IifAccounts::default());
        let txs = parse_iif_transactions(&iif).unwrap();
        assert_eq!(txs.len(), 2);
        assert!(txs.iter().all(|tx| tx.trns_type == "GENERAL JOURNAL"));
    }

    #[test]
    fn csv_payments_import_round_trips() {
        let text = "account_number,date,amount,reference,memo\nC-1042,2026-11-20,800.00,CHK-101,November payment\n";
        let payments = CsvAccountingPlugin::parse_payments_csv(text).unwrap();
        assert_eq!(payments.len(), 1);
        assert_eq!(payments[0].account_number, "C-1042");
        assert_eq!(payments[0].amount, Decimal::from_str("800.00").unwrap());
        assert_eq!(payments[0].reference, "CHK-101");
    }

    #[test]
    fn iif_payments_import_reads_payment_transactions() {
        let entries = vec![
            ar_entry(0x1, "1200.0000", "charge"),
            ar_entry(0x2, "-800.0000", "payment"),
        ];
        let iif = export_ar_ledger_iif(&entries, &IifAccounts::default());
        let dir = std::env::temp_dir().join(format!("df-acct-test-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ar.iif");
        std::fs::write(&path, &iif).unwrap();
        let payments = IifAccountingPlugin::default()
            .import_payments(&path)
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(payments.len(), 1);
        assert_eq!(payments[0].amount, Decimal::from_str("800.0000").unwrap());
    }

    #[test]
    fn plugins_write_files_to_disk() {
        let entries = vec![ar_entry(0x1, "1200.0000", "charge")];
        let dir = std::env::temp_dir().join(format!("df-acct-test-{}", Uuid::now_v7()));
        let csv_path = dir.join("ar.csv");
        let summary = CsvAccountingPlugin
            .export_ar_ledger(&entries, &csv_path)
            .unwrap();
        assert_eq!(summary.rows, 1);
        assert!(summary.bytes > 0);
        assert_eq!(summary.path, csv_path);
        let back =
            crate::csv::parse_ar_ledger_csv(&std::fs::read_to_string(&csv_path).unwrap()).unwrap();
        assert_eq!(back, entries);
        let iif_path = dir.join("ar.iif");
        let summary = IifAccountingPlugin::default()
            .export_ar_ledger(&entries, &iif_path)
            .unwrap();
        assert_eq!(summary.rows, 1);
        assert!(parse_iif_transactions(&std::fs::read_to_string(&iif_path).unwrap()).is_ok());
        let cust_path = dir.join("customers.csv");
        let summary = CsvAccountingPlugin
            .export_customers(&[customer()], &cust_path)
            .unwrap();
        assert_eq!(summary.rows, 1);
        assert!(
            std::fs::read_to_string(&cust_path)
                .unwrap()
                .contains("C-1042")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
