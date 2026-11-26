//! Monthly contractor statements (Phase 3, v0.4; spec §4 `contractor_ar`,
//! §6.1 `POST /ar/statements/run`, §7).
//!
//! The data type is [`df_ar::Statement`] (Worker A) — this crate
//! consumes it directly, it does not define its own. [`build_statements`]
//! builds a period's statements from the append-only `ar_ledger` via
//! `df-db` (opening balance = entries before the period, period lines
//! in `created_at` order, aging via `df-ar`'s
//! [`df_ar::open_charges`] + [`df_ar::AgingSummary::from_open`], so
//! the statement agrees with `df-ar`'s collections queue).
//!
//! Rendering is real Typst: [`render_statement_pdf`] writes the Typst
//! template ([`render_statement_typst`]) to a scratch directory and
//! invokes a real `typst` binary (`typst compile`) — no fake PDFs.
//! The binary is resolved by [`resolve_typst_binary`]: an explicit
//! path first, then the `DF_TYPST_BIN` environment variable, then
//! `typst` on `PATH`, then the developer install this VM carries at
//! `~/workspace/tools/typst/`. Typst 0.15.1 was verified here
//! (Nov 26, 2026).
//!
//! A `df_ar::Statement` carries no display names (no customer name,
//! org/store name, due date) — those live in [`StatementHeader`],
//! built from the customer/org/store rows by [`header_for`].
//!
//! Persistence: [`persist_statement`] freezes a statement row via
//! `df-db` (`insert_statement`, idempotent on re-run) and returns
//! its id; the caller renders the PDF and records the hash with
//! [`df_db::set_statement_pdf_sha256`]. [`statement_pdf_sha256`]
//! computes the hash.
//!
//! Caveat, stated plainly: `df-ar`'s open-charge derivation only
//! reduces a charge through `ar_allocations` rows (and credit memos).
//! A payment with no allocation rows still counts in
//! `payments_total` and the closing balance, but does NOT reduce the
//! aging buckets — so a statement whose payments were never
//! allocated will show aging totalling more than the closing
//! balance. Allocate payments (Worker A / edge checkout) and the
//! two agree.
//!
//! All money is `rust_decimal::Decimal` — never floats.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{Datelike, NaiveDate};
use df_ar::{AgingSummary, Statement, StatementLine};
use df_db::models::{ArLedgerEntry, Customer, StatementRow};
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::error::ReportError;

/// Display context for a statement PDF: everything the
/// `df_ar::Statement` deliberately does not carry (names, address,
/// due date). Built by [`header_for`] from the customer/org/store
/// rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementHeader {
    pub customer_name: String,
    pub billing_address: Option<String>,
    pub org_name: String,
    pub store_name: String,
    /// When the closing balance is due (customer terms from the
    /// period end).
    pub due_date: NaiveDate,
}

/// Build the display header for a statement from its customer row.
/// The due date comes from the customer's terms
/// (`df_ar::parse_terms`, `terms_days` override wins).
pub fn header_for(
    customer: &Customer,
    org_name: &str,
    store_name: &str,
    period_end: NaiveDate,
) -> StatementHeader {
    let customer_name = customer
        .company_name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            let first = customer.first_name.clone().unwrap_or_default();
            let last = customer.last_name.clone().unwrap_or_default();
            format!("{first} {last}").trim().to_string()
        });
    let due_date = match customer.terms_days {
        Some(days) => period_end + chrono::Duration::days(i64::from(days)),
        None => df_ar::parse_terms(&customer.payment_terms).due_date(period_end),
    };
    StatementHeader {
        customer_name,
        billing_address: None,
        org_name: org_name.to_string(),
        store_name: store_name.to_string(),
        due_date,
    }
}

/// Format money as `$1,234.56` (negative as `-$1,234.56`), two
/// decimals, thousands separators. Pure decimal formatting — no floats.
pub fn format_money(money: Money) -> String {
    let amount = money.amount();
    let sign = if amount.is_sign_negative() { "-" } else { "" };
    let scaled = amount.abs().round_dp(2);
    // At scale 2 the mantissa is exactly the total cents.
    let total_cents = scaled.mantissa();
    let dollars = total_cents / 100;
    let cents_part = total_cents % 100;
    let dollars_text = dollars.to_string();
    let mut grouped = String::new();
    for (i, ch) in dollars_text.chars().enumerate() {
        if i > 0 && (dollars_text.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}${grouped}.{cents_part:02}")
}

/// Escape text for Typst markup: every character that would start
/// markup, code, math or a comment gets a backslash.
fn escape_typst(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '>' | '@' | '[' | ']' | '{' | '}' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn money_cell(money: Money) -> String {
    if money.is_zero() {
        "—".to_string()
    } else {
        escape_typst(&format_money(money))
    }
}

/// The period label for a statement (`"YYYY-MM"` from `period_start`).
pub fn period_label(statement: &Statement) -> String {
    format!(
        "{:04}-{:02}",
        statement.period_start.year(),
        statement.period_start.month()
    )
}

/// Render the Typst source for a statement: header (store/org,
/// account, period), summary box, line-item table, aging summary and
/// a remittance stub.
pub fn render_statement_typst(statement: &Statement, header: &StatementHeader) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "#set page(paper: \"us-letter\", margin: (x: 2cm, y: 2cm))"
    );
    let _ = writeln!(out, "#set text(size: 10pt)");
    let _ = writeln!(out, "#set table(stroke: 0.5pt + gray)");
    let _ = writeln!(
        out,
        "= Monthly Statement — {}",
        escape_typst(&period_label(statement))
    );
    let _ = writeln!(
        out,
        "{}  ·  {}",
        escape_typst(&header.org_name),
        escape_typst(&header.store_name)
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "== Account");
    let _ = writeln!(
        out,
        "{}  ·  Account {}",
        escape_typst(&header.customer_name),
        escape_typst(&statement.account_number)
    );
    if let Some(address) = &header.billing_address {
        let _ = writeln!(out, "{}", escape_typst(address));
    }
    let _ = writeln!(
        out,
        "Statement date: {}  ·  Amount due by {}",
        statement.period_end, header.due_date
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "== Summary");
    let _ = writeln!(
        out,
        "#table(columns: (1fr, auto), [Opening balance], [{}], [Charges], [{}], [Payments received], [({})], [Finance charges], [{}], [*Amount due*], [*{}*])",
        money_cell(statement.opening_balance),
        money_cell(statement.charges_total),
        escape_typst(&format_money(statement.payments_total)),
        money_cell(statement.finance_charge_total),
        escape_typst(&format_money(statement.closing_balance)),
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "== Activity");
    if statement.lines.is_empty() {
        let _ = writeln!(out, "No activity this period.");
    } else {
        let _ = writeln!(
            out,
            "#table(columns: (auto, 1fr, auto, auto, auto, auto), [*Date*], [*Description*], [*Reference*], [*Charges*], [*Payments*], [*Balance*],"
        );
        let mut running = statement.opening_balance;
        for line in &statement.lines {
            running = running
                .checked_add(line.amount)
                .expect("same currency, addition cannot fail");
            let (charge, payment) = if line.amount.amount() >= Decimal::ZERO {
                (line.amount, Money::zero(line.amount.currency()))
            } else {
                (
                    Money::zero(line.amount.currency()),
                    Money::new(-line.amount.amount(), line.amount.currency()),
                )
            };
            let _ = writeln!(
                out,
                "[{}], [{}], [{}], [{}], [{}], [{}],",
                line.entry_date,
                escape_typst(&line.entry_type),
                escape_typst(&line.reference),
                money_cell(charge),
                money_cell(payment),
                escape_typst(&format_money(running)),
            );
        }
        let _ = writeln!(out, ")");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "== Aging summary (as of {})", statement.period_end);
    let aging = &statement.aging;
    let _ = writeln!(
        out,
        "#table(columns: (1fr, 1fr, 1fr, 1fr, 1fr, 1fr), [*Current*], [*31–60 days*], [*61–90 days*], [*91–120 days*], [*120+ days*], [*Total*], [{}], [{}], [{}], [{}], [{}], [*{}*])",
        escape_typst(&format_money(aging.current)),
        escape_typst(&format_money(aging.days_31_60)),
        escape_typst(&format_money(aging.days_61_90)),
        escape_typst(&format_money(aging.days_91_120)),
        escape_typst(&format_money(aging.over_120)),
        escape_typst(&format_money(
            aging
                .total()
                .unwrap_or_else(|_| Money::zero(statement.currency))
        )),
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "== Remittance stub");
    let _ = writeln!(
        out,
        "Please remit *{}* by {} for account {}. Make cheques payable to {}.",
        escape_typst(&format_money(statement.closing_balance)),
        header.due_date,
        escape_typst(&statement.account_number),
        escape_typst(&header.org_name),
    );
    let _ = writeln!(
        out,
        "Statement {}  ·  {}",
        statement.id,
        escape_typst(&period_label(statement))
    );
    out
}

/// Resolve the `typst` binary: an explicit path first, then the
/// `DF_TYPST_BIN` environment variable, then `typst` on `PATH`,
/// then the developer install this VM carries at
/// `~/workspace/tools/typst/`. Returns `None` when no usable binary
/// is found.
pub fn resolve_typst_binary(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
        return None;
    }
    if let Ok(path) = std::env::var("DF_TYPST_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if Command::new("typst").arg("--version").output().is_ok() {
        return Some(PathBuf::from("typst"));
    }
    if let Ok(home) = std::env::var("HOME") {
        let path =
            PathBuf::from(home).join("workspace/tools/typst/typst-x86_64-unknown-linux-musl/typst");
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Render a statement to real PDF bytes via Typst. Fails with
/// [`ReportError::Typst`] when no Typst binary is resolvable or
/// `typst compile` fails — never a fake PDF.
pub fn render_statement_pdf(
    statement: &Statement,
    header: &StatementHeader,
    typst_bin: Option<&Path>,
) -> Result<Vec<u8>, ReportError> {
    let typst = resolve_typst_binary(typst_bin).ok_or_else(|| {
        ReportError::Typst(
            "no typst binary found: pass --typst-bin, set DF_TYPST_BIN, or install typst (see df-reports statement docs)".to_string(),
        )
    })?;
    let source = render_statement_typst(statement, header);
    let dir = std::env::temp_dir().join(format!("df-statement-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir)?;
    let input = dir.join("statement.typ");
    let output = dir.join("statement.pdf");
    std::fs::write(&input, source)?;
    let status = Command::new(&typst)
        .arg("compile")
        .arg(&input)
        .arg(&output)
        .output()
        .map_err(|e| ReportError::Typst(format!("failed to run typst ({typst:?}): {e}")))?;
    let result = (|| -> Result<Vec<u8>, ReportError> {
        if !status.status.success() {
            return Err(ReportError::Typst(format!(
                "typst compile failed: {}",
                String::from_utf8_lossy(&status.stderr)
            )));
        }
        let bytes = std::fs::read(&output)?;
        if !bytes.starts_with(b"%PDF") {
            return Err(ReportError::Typst(
                "typst produced output that is not a PDF".to_string(),
            ));
        }
        Ok(bytes)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// SHA-256 hex of rendered PDF bytes — the value the `statements`
/// row stores (via [`df_db::set_statement_pdf_sha256`]).
pub fn statement_pdf_sha256(pdf_bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(pdf_bytes);
    format!("{:x}", hasher.finalize())
}

// --- batch builder -----------------------------------------------------------

/// Parse a `"YYYY-MM"` period into its first day.
fn period_start(period: &str) -> Result<NaiveDate, ReportError> {
    let (year, month) = period.split_once('-').ok_or_else(|| {
        ReportError::InvalidInput(format!("period must be YYYY-MM, got {period:?}"))
    })?;
    let year: i32 = year.parse().map_err(|_| {
        ReportError::InvalidInput(format!("period must be YYYY-MM, got {period:?}"))
    })?;
    let month: u32 = month.parse().map_err(|_| {
        ReportError::InvalidInput(format!("period must be YYYY-MM, got {period:?}"))
    })?;
    NaiveDate::from_ymd_opt(year, month, 1)
        .ok_or_else(|| ReportError::InvalidInput(format!("period must be YYYY-MM, got {period:?}")))
}

fn last_day_of_month(date: NaiveDate) -> NaiveDate {
    let (year, month) = (date.year(), date.month());
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .unwrap()
        .pred_opt()
        .unwrap()
}

/// Build every statement for `period` (`"YYYY-MM"`) for one org +
/// store, as [`df_ar::Statement`]s. Customers with no opening
/// balance and no period activity are skipped (a zero statement is
/// not mailed). Statements are sorted by account number.
///
/// Aging is `df-ar`'s: open charges derived from the ledger +
/// allocations as of the period end (see the module docs for the
/// unallocated-payment caveat).
pub async fn build_statements(
    pool: &df_db::AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    period: &str,
    currency: Currency,
) -> Result<Vec<Statement>, ReportError> {
    let start = period_start(period)?;
    let end = last_day_of_month(start);
    let zero = Money::zero(currency);
    let customers = df_db::list_charge_customers(pool, org_id).await?;
    let mut statements = Vec::new();
    for customer in customers {
        let entries = df_db::list_ar_entries_for_customer(pool, customer.id).await?;
        let entries: Vec<ArLedgerEntry> = entries
            .into_iter()
            .filter(|e| e.store_id == store_id)
            .collect();
        let mut opening = zero;
        let mut charges_total = zero;
        let mut payments_total = zero;
        let mut finance_charge_total = zero;
        let mut lines: Vec<StatementLine> = Vec::new();
        for entry in &entries {
            let day = entry.created_at.date_naive();
            if day < start {
                opening = opening.checked_add(Money::new(entry.amount, currency))?;
                continue;
            }
            if day > end {
                continue;
            }
            match entry.entry_type.as_str() {
                "finance_charge" => {
                    finance_charge_total =
                        finance_charge_total.checked_add(Money::new(entry.amount, currency))?;
                }
                "payment" | "credit_memo" => {
                    payments_total =
                        payments_total.checked_add(Money::new(-entry.amount, currency))?;
                }
                _ => {
                    if entry.amount >= Decimal::ZERO {
                        charges_total =
                            charges_total.checked_add(Money::new(entry.amount, currency))?;
                    } else {
                        payments_total =
                            payments_total.checked_add(Money::new(-entry.amount, currency))?;
                    }
                }
            }
            lines.push(StatementLine {
                entry_id: entry.id,
                entry_date: day,
                entry_type: entry.entry_type.clone(),
                reference: entry.reference_id.to_string(),
                amount: Money::new(entry.amount, currency),
                due_date: entry.due_date,
            });
        }
        let closing = opening
            .checked_add(charges_total)?
            .checked_sub(payments_total)?
            .checked_add(finance_charge_total)?;
        if opening.is_zero() && lines.is_empty() {
            continue;
        }
        // Aging as of the period end, from the ledger + allocations
        // dated on or before it (later entries do not exist yet for
        // this statement).
        let cutoff = end + chrono::Duration::days(1);
        let domain_entries: Vec<df_ar::LedgerEntry> = entries
            .iter()
            .filter(|e| e.created_at.date_naive() < cutoff)
            .map(df_db::ledger_entry_to_domain)
            .collect();
        let allocations = df_db::list_allocations_for_customer(pool, customer.id).await?;
        let domain_allocations: Vec<df_ar::Allocation> = allocations
            .iter()
            .filter(|a| a.created_at.date_naive() < cutoff)
            .map(df_db::allocation_to_domain)
            .collect();
        let open = df_ar::open_charges(&domain_entries, &domain_allocations);
        let aging = AgingSummary::from_open(&open, end, currency);
        let statement = Statement {
            id: Uuid::now_v7(),
            customer_id: customer.id,
            account_number: customer.account_number.clone(),
            currency,
            period_start: start,
            period_end: end,
            opening_balance: opening,
            charges_total,
            payments_total,
            finance_charge_total,
            closing_balance: closing,
            aging,
            lines,
        };
        statement.check_identity().map_err(|computed| {
            ReportError::InvalidInput(format!(
                "statement {} identity failed: computed closing {computed}, stored {}",
                statement.id,
                statement.closing_balance.amount()
            ))
        })?;
        statements.push(statement);
    }
    Ok(statements)
}

/// Convert a built statement to its `df-db` row for persistence
/// (aging serialised as JSON; `pdf_sha256` is `None` until the PDF
/// is rendered).
pub fn to_statement_row(statement: &Statement) -> Result<StatementRow, ReportError> {
    Ok(StatementRow {
        id: statement.id,
        customer_id: statement.customer_id,
        period_start: statement.period_start,
        period_end: statement.period_end,
        opening_balance: statement.opening_balance.amount(),
        charges_total: statement.charges_total.amount(),
        payments_total: statement.payments_total.amount(),
        finance_charge_total: statement.finance_charge_total.amount(),
        closing_balance: statement.closing_balance.amount(),
        aging_json: serde_json::to_value(statement.aging)
            .map_err(|e| ReportError::InvalidInput(format!("aging did not serialise: {e}")))?,
        pdf_sha256: None,
        created_at: chrono::Utc::now(),
    })
}

/// Freeze a statement row: insert it unless the batch already ran
/// for this account + period, in which case return the existing
/// row's id. Returns the `statements` row id the PDF hash belongs to.
pub async fn persist_statement(
    pool: &df_db::AnyPool,
    statement: &Statement,
) -> Result<Uuid, ReportError> {
    if let Some(existing) = df_db::get_statement(
        pool,
        statement.customer_id,
        statement.period_start,
        statement.period_end,
    )
    .await?
    {
        return Ok(existing.id);
    }
    let row = to_statement_row(statement)?;
    df_db::insert_statement(pool, &row).await?;
    Ok(row.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use chrono::Utc;
    use df_db::models::{ArAllocation, Customer, Organization, Store};
    use std::str::FromStr as _;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn money(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    /// A realistic statement: opening $500, two November charges
    /// ($1,200 + $300), a partial payment of $800, a $18 finance
    /// charge, closing $1,218, with an aging spread.
    fn realistic_statement() -> (Statement, StatementHeader) {
        let currency = Currency::Usd;
        let statement = Statement {
            id: Uuid::from_u128(0x9001),
            customer_id: Uuid::from_u128(0x9002),
            account_number: "C-1042".to_string(),
            currency,
            period_start: NaiveDate::from_str("2026-11-01").unwrap(),
            period_end: NaiveDate::from_str("2026-11-30").unwrap(),
            opening_balance: money("500.0000"),
            charges_total: money("1500.0000"),
            payments_total: money("800.0000"),
            finance_charge_total: money("18.0000"),
            closing_balance: money("1218.0000"),
            aging: AgingSummary {
                current: money("1000.0000"),
                days_31_60: money("200.0000"),
                days_61_90: money("18.0000"),
                days_91_120: money("0"),
                over_120: money("0"),
            },
            lines: vec![
                StatementLine {
                    entry_id: Uuid::from_u128(0x9101),
                    entry_date: NaiveDate::from_str("2026-11-05").unwrap(),
                    entry_type: "charge".to_string(),
                    reference: "INV-1042".to_string(),
                    amount: money("1200.0000"),
                    due_date: Some(NaiveDate::from_str("2026-12-05").unwrap()),
                },
                StatementLine {
                    entry_id: Uuid::from_u128(0x9102),
                    entry_date: NaiveDate::from_str("2026-11-12").unwrap(),
                    entry_type: "charge".to_string(),
                    reference: "INV-1043".to_string(),
                    amount: money("300.0000"),
                    due_date: Some(NaiveDate::from_str("2026-12-12").unwrap()),
                },
                StatementLine {
                    entry_id: Uuid::from_u128(0x9103),
                    entry_date: NaiveDate::from_str("2026-11-20").unwrap(),
                    entry_type: "payment".to_string(),
                    reference: "CHK-101".to_string(),
                    amount: money("-800.0000"),
                    due_date: None,
                },
                StatementLine {
                    entry_id: Uuid::from_u128(0x9104),
                    entry_date: NaiveDate::from_str("2026-11-30").unwrap(),
                    entry_type: "finance_charge".to_string(),
                    reference: "STAT-2026-11".to_string(),
                    amount: money("18.0000"),
                    due_date: Some(NaiveDate::from_str("2026-11-30").unwrap()),
                },
            ],
        };
        let header = StatementHeader {
            customer_name: "Harbourview Painting Ltd.".to_string(),
            billing_address: Some("12 Marine Drive, Vancouver, BC".to_string()),
            org_name: "Test Paint Co".to_string(),
            store_name: "Main Store".to_string(),
            due_date: NaiveDate::from_str("2026-12-30").unwrap(),
        };
        (statement, header)
    }

    #[test]
    fn realistic_statement_reconciles() {
        let (statement, _header) = realistic_statement();
        assert_eq!(statement.closing_balance, money("1218.0000"));
        statement.check_identity().unwrap();
        assert_eq!(statement.aging.total().unwrap(), money("1218.0000"));
    }

    #[test]
    fn format_money_groups_thousands() {
        assert_eq!(format_money(money("1234.5600")), "$1,234.56");
        assert_eq!(format_money(money("0")), "$0.00");
        assert_eq!(format_money(money("-12.5000")), "-$12.50");
        assert_eq!(format_money(money("1000000.0000")), "$1,000,000.00");
    }

    #[test]
    fn typst_template_contains_all_sections_and_figures() {
        let (statement, header) = realistic_statement();
        let source = render_statement_typst(&statement, &header);
        for needle in [
            "Monthly Statement",
            "Harbourview Painting Ltd.",
            "C-1042",
            "2026-11",
            "$1,218.00",
            "$1,500.00",
            "$800.00",
            "$18.00",
            "$500.00",
            "INV-1042",
            "CHK-101",
            "Remittance stub",
        ] {
            assert!(source.contains(needle), "template missing {needle:?}");
        }
    }

    #[test]
    fn typst_escapes_markup_characters() {
        let (statement, mut header) = realistic_statement();
        header.customer_name = "A&B *Paint* #1".to_string();
        let source = render_statement_typst(&statement, &header);
        assert!(source.contains("A&B \\*Paint\\* \\#1"));
        assert!(!source.contains("A&B *Paint* #1"));
    }

    /// Exit test (a): render a real PDF and check it with
    /// `pdftotext` when available. Typst emits CID fonts, so the
    /// figures are NOT recoverable from the raw content streams —
    /// `pdftotext` is the extractor here; when it is absent the
    /// test still asserts the PDF header and a non-trivial size.
    #[test]
    fn render_statement_pdf_is_a_real_pdf_with_the_key_figures() {
        let (statement, header) = realistic_statement();
        let pdf = render_statement_pdf(&statement, &header, None)
            .expect("typst must render the statement (see DF_TYPST_BIN)");
        assert!(pdf.starts_with(b"%PDF"), "bytes do not start with %PDF");
        assert!(pdf.len() > 5_000, "PDF suspiciously small: {}", pdf.len());
        let sha = statement_pdf_sha256(&pdf);
        assert_eq!(sha.len(), 64);
        assert_eq!(sha, statement_pdf_sha256(&pdf));
        // Key figures as extracted text.
        if Command::new("pdftotext").arg("-v").output().is_err() {
            eprintln!("pdftotext not installed — skipping extracted-text assertions");
            return;
        }
        let dir = std::env::temp_dir().join(format!("df-statement-test-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let pdf_path = dir.join("statement.pdf");
        std::fs::write(&pdf_path, &pdf).unwrap();
        let output = Command::new("pdftotext")
            .arg(&pdf_path)
            .arg("-")
            .output()
            .expect("pdftotext runs");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(output.status.success(), "pdftotext failed");
        let text = String::from_utf8_lossy(&output.stdout);
        for figure in [
            "Harbourview Painting",
            "C-1042",
            "1,218.00",
            "1,500.00",
            "800.00",
            "18.00",
            "500.00",
            "INV-1042",
        ] {
            assert!(
                text.contains(figure),
                "extracted PDF text missing {figure:?}:\n{text}"
            );
        }
    }

    async fn seeded_pool() -> (df_db::AnyPool, Uuid, Uuid, Uuid) {
        let pool = test_pool().await;
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
        let customer_id = Uuid::from_u128(0x4000);
        df_db::insert_organization(
            &pool,
            &Organization {
                id: org_id,
                name: "Test Paint Co".to_string(),
                default_currency: "USD".to_string(),
                enabled_modules: serde_json::json!({}),
                settings_json: serde_json::json!({}),
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        df_db::insert_store(
            &pool,
            &Store {
                id: store_id,
                org_id,
                code: "MAIN".to_string(),
                name: "Main Store".to_string(),
                country_code: "US".to_string(),
                region_code: "CA".to_string(),
                postal_code: "90210".to_string(),
                currency: "USD".to_string(),
                tax_zone_id: Uuid::now_v7(),
                timezone: "America/Los_Angeles".to_string(),
                is_warehouse: false,
            },
        )
        .await
        .unwrap();
        df_db::insert_customer(
            &pool,
            &Customer {
                id: customer_id,
                org_id,
                account_number: "C-1042".to_string(),
                customer_type: "contractor".to_string(),
                company_name: Some("Harbourview Painting Ltd.".to_string()),
                first_name: None,
                last_name: None,
                email: None,
                phone: None,
                price_level_id: None,
                salesperson_id: None,
                tax_exempt_cert: None,
                tax_exempt_expires: None,
                charge_enabled: true,
                credit_limit: dec("5000.0000"),
                payment_terms: "NET30".to_string(),
                terms_days: None,
                finance_rate_pct_monthly: None,
                credit_hold: false,
                require_po_number: false,
                require_job_name: false,
                portal_enabled: false,
            },
        )
        .await
        .unwrap();
        (pool, org_id, store_id, customer_id)
    }

    fn ar_entry(
        org_id: Uuid,
        store_id: Uuid,
        customer_id: Uuid,
        amount: &str,
        due: &str,
        created: &str,
        entry_type: &str,
    ) -> ArLedgerEntry {
        ArLedgerEntry {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            customer_id,
            job_id: None,
            entry_type: entry_type.to_string(),
            reference_type: "transaction".to_string(),
            reference_id: Uuid::now_v7(),
            amount: dec(amount),
            due_date: Some(NaiveDate::from_str(due).unwrap()),
            statement_id: None,
            created_at: chrono::DateTime::parse_from_rfc3339(created)
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    #[tokio::test]
    async fn build_statements_from_the_ledger() {
        let (pool, org_id, store_id, customer_id) = seeded_pool().await;
        // An October charge (opening balance) + a November charge
        // and a November payment allocated against the October
        // charge (df-ar only reduces open charges via allocations).
        let oct_charge = ar_entry(
            org_id,
            store_id,
            customer_id,
            "500.0000",
            "2026-11-14",
            "2026-10-15T10:00:00Z",
            "charge",
        );
        let nov_charge = ar_entry(
            org_id,
            store_id,
            customer_id,
            "1200.0000",
            "2026-12-05",
            "2026-11-05T10:00:00Z",
            "charge",
        );
        let payment = ar_entry(
            org_id,
            store_id,
            customer_id,
            "-800.0000",
            "2026-12-05",
            "2026-11-20T10:00:00Z",
            "payment",
        );
        for entry in [&oct_charge, &nov_charge, &payment] {
            df_db::insert_ar_entry(&pool, entry).await.unwrap();
        }
        df_db::insert_ar_allocation(
            &pool,
            &ArAllocation {
                id: Uuid::now_v7(),
                payment_entry_id: payment.id,
                charge_entry_id: oct_charge.id,
                allocated_amount: dec("500.0000"),
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        df_db::insert_ar_allocation(
            &pool,
            &ArAllocation {
                id: Uuid::now_v7(),
                payment_entry_id: payment.id,
                charge_entry_id: nov_charge.id,
                allocated_amount: dec("300.0000"),
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();

        let statements = build_statements(&pool, org_id, store_id, "2026-11", Currency::Usd)
            .await
            .unwrap();
        assert_eq!(statements.len(), 1);
        let statement = &statements[0];
        assert_eq!(statement.account_number, "C-1042");
        assert_eq!(statement.opening_balance, money("500.0000"));
        assert_eq!(statement.charges_total, money("1200.0000"));
        assert_eq!(statement.payments_total, money("800.0000"));
        assert_eq!(statement.closing_balance, money("900.0000"));
        statement.check_identity().unwrap();
        assert_eq!(statement.lines.len(), 2);
        // Aging: the $800 payment is allocated $500 to the October
        // charge and $300 to the November charge, leaving $900 open
        // on the November charge (due Dec 5 — current on Nov 30).
        assert_eq!(statement.aging.current, money("900.0000"));
        assert_eq!(statement.aging.total().unwrap(), money("900.0000"));

        // Persist + hash round-trip through df-db's repo.
        let row_id = persist_statement(&pool, statement).await.unwrap();
        let again = persist_statement(&pool, statement).await.unwrap();
        assert_eq!(row_id, again, "re-run must not duplicate the row");
        df_db::set_statement_pdf_sha256(&pool, row_id, "deadbeef")
            .await
            .unwrap();
        let stored = df_db::get_statement(
            &pool,
            customer_id,
            statement.period_start,
            statement.period_end,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(stored.pdf_sha256.as_deref(), Some("deadbeef"));
        assert_eq!(stored.closing_balance, dec("900.0000"));
    }

    #[tokio::test]
    async fn build_statements_skips_zero_accounts() {
        let (pool, org_id, store_id, _customer_id) = seeded_pool().await;
        let statements = build_statements(&pool, org_id, store_id, "2026-11", Currency::Usd)
            .await
            .unwrap();
        assert!(statements.is_empty());
    }

    #[test]
    fn header_for_uses_terms_for_the_due_date() {
        let customer = Customer {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            account_number: "C-1".to_string(),
            customer_type: "contractor".to_string(),
            company_name: Some("Acme Painting".to_string()),
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: true,
            credit_limit: dec("0"),
            payment_terms: "NET15".to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
        };
        let header = header_for(
            &customer,
            "Test Paint Co",
            "Main Store",
            NaiveDate::from_str("2026-11-30").unwrap(),
        );
        assert_eq!(header.customer_name, "Acme Painting");
        assert_eq!(header.due_date, NaiveDate::from_str("2026-12-15").unwrap());
    }
}
