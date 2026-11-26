//! Contractor AR on the edge (Phase 3, v0.4; spec §5.4/§7).
//!
//! The Rust API Worker C's HTTP endpoints call — no HTTP here.
//! Checkout calls [`Edge::check_charge_credit`] (wired in
//! `edge.rs`); the month-end and collections flows are the `pub`
//! methods below.
//!
//! * [`Edge::post_ar_payment`] — take a payment on a charge
//!   account, allocated FIFO (oldest charge first).
//! * [`Edge::assess_finance_charges`] — monthly finance charges
//!   on overdue balances, idempotent per statement period.
//! * [`Edge::collections_queue`] — overdue accounts ranked by
//!   oldest bucket, then amount.
//! * [`Edge::generate_statements`] — the month-end batch: one
//!   frozen [`Statement`] per charge account, persisted in the
//!   `statements` table. Re-running a period returns the frozen
//!   statements and posts nothing new.
//!
//! All money is [`Money`] — never floats. Finance-charge
//! idempotency keys on the deterministic period UUID from
//! `df-ar` ([`df_ar::statement_period_key`]).
//!
//! Transaction note: a finance-charge assessment commits in its
//! own transaction *before* the statement that includes it is
//! computed, so the statement builder reads it from the pool. If
//! the statement insert then fails, a re-run finds the charge
//! already posted (same period key) and the statement missing,
//! and computes the correct statement — the batch is safe to
//! retry. Statements are frozen at generation: a finance charge
//! assessed after a statement was generated belongs to the next
//! period's opening balance, not to the frozen statement.

use chrono::{NaiveDate, Utc};
use df_ar::{
    AgingSummary, ChargeAccount, CollectionsEntry, FinanceChargePosted, Statement, StatementLine,
    account_from_row, allocate_payment, collections_queue, finance_charge_for, open_charges,
    overdue_balance, period_for, statement_period_key,
};
use df_db::DbError;
use df_db::models::{ArAllocation, ArLedgerEntry, StatementRow};
use df_domain::{Currency, Money};
use df_sync::{Outbox, OutboxEntry};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// What [`Edge::post_ar_payment`] recorded: the payment ledger
/// entry plus the FIFO allocations it made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostedArPayment {
    pub payment_entry_id: Uuid,
    pub customer_id: Uuid,
    pub amount: Money,
    pub method: String,
    /// (charge entry id, allocated amount), oldest charge first.
    pub allocations: Vec<(Uuid, Decimal)>,
}

impl Edge {
    /// Map a customer row to its charge-account view.
    fn charge_account_for(&self, customer: &df_db::models::Customer) -> ChargeAccount {
        account_from_row(
            df_ar::AccountRow {
                id: customer.id,
                org_id: customer.org_id,
                account_number: &customer.account_number,
                charge_enabled: customer.charge_enabled,
                credit_limit: customer.credit_limit,
                payment_terms: &customer.payment_terms,
                terms_days: customer.terms_days,
                finance_rate_pct_monthly: customer.finance_rate_pct_monthly,
                on_credit_hold: customer.credit_hold,
            },
            self.config().currency,
        )
    }

    /// Open charges for a customer (oldest first), derived from the
    /// ledger + allocations.
    pub async fn open_charges_for(
        &self,
        customer_id: Uuid,
    ) -> Result<Vec<df_ar::OpenCharge>, EdgeError> {
        let entries = df_db::list_ar_entries_for_customer(self.pool(), customer_id).await?;
        let allocations = df_db::list_allocations_for_customer(self.pool(), customer_id).await?;
        Ok(open_charges(
            &entries
                .iter()
                .map(df_db::ledger_entry_to_domain)
                .collect::<Vec<_>>(),
            &allocations
                .iter()
                .map(df_db::allocation_to_domain)
                .collect::<Vec<_>>(),
        ))
    }

    /// Open AR balance for a customer, derived from the ledger.
    pub async fn open_balance_for(&self, customer_id: Uuid) -> Result<Money, EdgeError> {
        let open = self.open_charges_for(customer_id).await?;
        let total = open
            .iter()
            .map(|c| c.open_amount)
            .fold(Decimal::ZERO, |acc, amount| acc + amount);
        Ok(Money::new(total, self.config().currency))
    }

    /// The checkout credit check (called from [`Edge::checkout`]
    /// before any authorization or write). Returns the due date for
    /// the new charge, or `None` when no charge is being made or no
    /// credit record has synced to this edge yet.
    ///
    /// A customer with no row on this edge (reference data not
    /// synced down) is **not** blocked — the edge keeps selling
    /// and HQ enforces credit on sync (see `df-db` lib docs).
    /// A known account is enforced strictly: not charge-enabled,
    /// on credit hold, or at/over its credit limit all reject with
    /// [`EdgeError::CreditBlocked`] (HTTP 409).
    pub(crate) async fn check_charge_credit(
        &self,
        customer_id: Option<Uuid>,
        charge_total: Money,
    ) -> Result<Option<NaiveDate>, EdgeError> {
        if charge_total.is_zero() {
            return Ok(None);
        }
        let Some(customer_id) = customer_id else {
            return Ok(None);
        };
        let customer = match df_db::get_customer(self.pool(), customer_id).await {
            Ok(customer) => customer,
            Err(DbError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let account = self.charge_account_for(&customer);
        let open = self.open_balance_for(customer_id).await?;
        account
            .check_charge(open, charge_total)
            .map_err(|e| EdgeError::CreditBlocked(e.to_string()))?;
        Ok(Some(account.due_date(Utc::now().date_naive())))
    }

    /// Take a payment on a charge account and allocate it FIFO
    /// (oldest charge first). The payment ledger entry (negative
    /// amount) and its `ar_allocations` rows are written in one
    /// local transaction, plus one sync-outbox entry for the
    /// payment. A payment larger than the open balance allocates
    /// only up to the balance; the remainder is an unallocated
    /// credit on the account. Returns the allocations made.
    pub async fn post_ar_payment(
        &self,
        customer_id: Uuid,
        amount: Money,
        method: &str,
    ) -> Result<PostedArPayment, EdgeError> {
        if amount.is_negative() || amount.is_zero() {
            return Err(EdgeError::InvalidInput(format!(
                "AR payment must be positive: {}",
                amount.amount()
            )));
        }
        if amount.currency() != self.config().currency {
            return Err(EdgeError::InvalidInput(
                "AR payment must use the store currency".to_string(),
            ));
        }
        // A payment needs an account; unlike checkout there is no
        // sale to keep moving without one.
        df_db::get_customer(self.pool(), customer_id).await?;
        let open = self.open_charges_for(customer_id).await?;
        let payment_id = Uuid::now_v7();
        let now = Utc::now();
        let allocations = allocate_payment(&open, payment_id, amount.amount());
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_ar_entry_exec(
            &mut *db_tx,
            &ArLedgerEntry {
                id: payment_id,
                org_id: self.config().org_id,
                store_id: self.config().store_id,
                customer_id,
                job_id: None,
                entry_type: "payment".to_string(),
                reference_type: "payment".to_string(),
                reference_id: payment_id,
                amount: -amount.amount(),
                due_date: None,
                statement_id: None,
                created_at: now,
            },
        )
        .await?;
        for alloc in &allocations {
            df_db::insert_ar_allocation(
                &mut *db_tx,
                &ArAllocation {
                    id: Uuid::now_v7(),
                    payment_entry_id: alloc.payment_entry_id,
                    charge_entry_id: alloc.charge_entry_id,
                    allocated_amount: alloc.allocated_amount,
                    created_at: now,
                },
            )
            .await?;
        }
        Outbox::enqueue_exec(
            &mut *db_tx,
            &OutboxEntry::new(
                self.config().store_id,
                "ar_ledger",
                payment_id,
                serde_json::json!({
                    "customer_id": customer_id.to_string(),
                    "amount": (-amount.amount()).to_string(),
                    "entry_type": "payment",
                    "method": method,
                    "reference_type": "payment",
                    "reference_id": payment_id.to_string(),
                    "due_date": null,
                    "job_id": null,
                    "created_at": now.to_rfc3339(),
                    "allocations": allocations.iter().map(|a| serde_json::json!({
                        "charge_entry_id": a.charge_entry_id.to_string(),
                        "allocated_amount": a.allocated_amount.to_string(),
                    })).collect::<Vec<_>>(),
                }),
            ),
        )
        .await?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        Ok(PostedArPayment {
            payment_entry_id: payment_id,
            customer_id,
            amount,
            method: method.to_string(),
            allocations: allocations
                .iter()
                .map(|a| (a.charge_entry_id, a.allocated_amount))
                .collect(),
        })
    }

    /// Assess one customer's finance charge for the statement
    /// period containing `as_of`. Commits in its own transaction.
    /// Returns the posted entry, or `None` when nothing was posted
    /// (no overdue balance, zero rate, or this period already
    /// assessed — the existing `finance_charge` entry is found by
    /// its deterministic reference id).
    async fn assess_finance_charge_for_customer(
        &self,
        customer: &df_db::models::Customer,
        account: &ChargeAccount,
        as_of: NaiveDate,
        default_rate_pct: Decimal,
    ) -> Result<Option<FinanceChargePosted>, EdgeError> {
        let rate = account.finance_rate_pct_monthly.unwrap_or(default_rate_pct);
        if rate <= Decimal::ZERO {
            return Ok(None);
        }
        let (period_start, period_end) = period_for(as_of);
        let period_key = statement_period_key(customer.id, period_start);
        if df_db::find_ar_entry_by_reference(
            self.pool(),
            customer.id,
            "finance_charge",
            "statement_run",
            period_key,
        )
        .await?
        .is_some()
        {
            return Ok(None);
        }
        let open = self.open_charges_for(customer.id).await?;
        let overdue = overdue_balance(&open, as_of, self.config().currency);
        if overdue.is_zero() {
            return Ok(None);
        }
        let amount = finance_charge_for(overdue, rate);
        if amount.is_zero() {
            return Ok(None);
        }
        let entry_id = Uuid::now_v7();
        // The charge is dated the assessment date (`as_of`), not
        // the wall-clock run time: it accrues over the period, so a
        // month-end run executed a few days late still books the
        // charge inside its statement period (and the statement
        // identity holds). The outbox entry keeps the real run
        // time — that is sync metadata, not accounting.
        let assessed_at = as_of
            .and_hms_opt(0, 0, 0)
            .expect("midnight of a valid date")
            .and_utc();
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_ar_entry_exec(
            &mut *db_tx,
            &ArLedgerEntry {
                id: entry_id,
                org_id: self.config().org_id,
                store_id: self.config().store_id,
                customer_id: customer.id,
                job_id: None,
                entry_type: "finance_charge".to_string(),
                reference_type: "statement_run".to_string(),
                reference_id: period_key,
                amount: amount.amount(),
                due_date: Some(as_of),
                statement_id: None,
                created_at: assessed_at,
            },
        )
        .await?;
        Outbox::enqueue_exec(
            &mut *db_tx,
            &OutboxEntry::new(
                self.config().store_id,
                "ar_ledger",
                entry_id,
                serde_json::json!({
                    "customer_id": customer.id.to_string(),
                    "amount": amount.amount().to_string(),
                    "entry_type": "finance_charge",
                    "reference_type": "statement_run",
                    "reference_id": period_key.to_string(),
                    "due_date": as_of.to_string(),
                    "job_id": null,
                    "created_at": assessed_at.to_rfc3339(),
                }),
            ),
        )
        .await?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        Ok(Some(FinanceChargePosted {
            customer_id: customer.id,
            entry_id,
            period_start,
            period_end,
            overdue_balance: overdue,
            rate_pct: rate,
            amount,
        }))
    }

    /// Assess monthly finance charges for every charge account, as
    /// of `as_of`. Only overdue balances (31+ days past due) are
    /// charged, at each account's own rate when set, else
    /// `monthly_rate_pct`. Idempotent per statement period: the
    /// period is the calendar month of `as_of`, and re-running the
    /// same month posts nothing. Returns the charges actually
    /// posted.
    pub async fn assess_finance_charges(
        &self,
        as_of: NaiveDate,
        monthly_rate_pct: Decimal,
    ) -> Result<Vec<FinanceChargePosted>, EdgeError> {
        let customers = df_db::list_charge_customers(self.pool(), self.config().org_id).await?;
        let mut posted = Vec::new();
        for customer in &customers {
            let account = self.charge_account_for(customer);
            if let Some(entry) = self
                .assess_finance_charge_for_customer(customer, &account, as_of, monthly_rate_pct)
                .await?
            {
                posted.push(entry);
            }
        }
        Ok(posted)
    }

    /// The collections queue as of `as_of`: overdue charge
    /// accounts ranked by oldest bucket then amount, with the
    /// suggested next action per account.
    pub async fn collections_queue(
        &self,
        as_of: NaiveDate,
    ) -> Result<Vec<CollectionsEntry>, EdgeError> {
        let customers = df_db::list_charge_customers(self.pool(), self.config().org_id).await?;
        let mut accounts = Vec::with_capacity(customers.len());
        for customer in &customers {
            let open = self.open_charges_for(customer.id).await?;
            accounts.push((customer.id, customer.account_number.clone(), open));
        }
        Ok(collections_queue(&accounts, as_of, self.config().currency))
    }

    /// Generate the month-end statement batch for
    /// `[period_start, period_end]` (inclusive dates). One frozen
    /// [`Statement`] per charge account, computed from the
    /// `ar_ledger` and persisted in `statements`.
    ///
    /// * The finance charge for the period is assessed first
    ///   (idempotent — see [`Edge::assess_finance_charges`]), so it
    ///   lands in `finance_charge_total`. Accounts with no
    ///   `finance_rate_pct_monthly` set get no finance charge from
    ///   the batch itself (their rate is unset); a later
    ///   [`Edge::assess_finance_charges`] run with an explicit rate
    ///   still posts for the period.
    /// * `payments_total` counts payments **and credit memos** in
    ///   the period, as a positive magnitude.
    /// * Re-running the same period returns the frozen statements
    ///   and posts nothing: balances do not change and finance
    ///   charges are not double-posted.
    pub async fn generate_statements(
        &self,
        period_start: NaiveDate,
        period_end: NaiveDate,
    ) -> Result<Vec<Statement>, EdgeError> {
        if period_end < period_start {
            return Err(EdgeError::InvalidInput(format!(
                "statement period end {period_end} is before start {period_start}"
            )));
        }
        let customers = df_db::list_charge_customers(self.pool(), self.config().org_id).await?;
        let mut statements = Vec::with_capacity(customers.len());
        for customer in &customers {
            statements.push(
                self.generate_statement_for(customer, period_start, period_end)
                    .await?,
            );
        }
        Ok(statements)
    }

    /// One account's statement for a period: return the frozen row
    /// when the batch already ran, else assess the finance charge,
    /// compute from the ledger and persist.
    async fn generate_statement_for(
        &self,
        customer: &df_db::models::Customer,
        period_start: NaiveDate,
        period_end: NaiveDate,
    ) -> Result<Statement, EdgeError> {
        if let Some(row) =
            df_db::get_statement(self.pool(), customer.id, period_start, period_end).await?
        {
            return self.statement_from_row(customer, &row).await;
        }
        let account = self.charge_account_for(customer);
        // The batch assesses at each account's own rate only
        // (default rate 0 = no charge when the account sets none).
        self.assess_finance_charge_for_customer(customer, &account, period_end, Decimal::ZERO)
            .await?;
        let entries = df_db::list_ar_entries_for_customer(self.pool(), customer.id).await?;
        let allocations = df_db::list_allocations_for_customer(self.pool(), customer.id).await?;
        let statement =
            self.build_statement(customer, &entries, &allocations, period_start, period_end);
        df_db::insert_statement(self.pool(), &statement_row_from(&statement)).await?;
        Ok(statement)
    }

    /// Rebuild a [`Statement`] from a frozen row plus the ledger
    /// (the ledger is append-only, so the lines are stable).
    async fn statement_from_row(
        &self,
        customer: &df_db::models::Customer,
        row: &StatementRow,
    ) -> Result<Statement, EdgeError> {
        let entries = df_db::list_ar_entries_for_customer(self.pool(), customer.id).await?;
        let currency = self.config().currency;
        let aging = aging_from_json(&row.aging_json, currency)?;
        Ok(Statement {
            id: row.id,
            customer_id: row.customer_id,
            account_number: customer.account_number.clone(),
            currency,
            period_start: row.period_start,
            period_end: row.period_end,
            opening_balance: Money::new(row.opening_balance, currency),
            charges_total: Money::new(row.charges_total, currency),
            payments_total: Money::new(row.payments_total, currency),
            finance_charge_total: Money::new(row.finance_charge_total, currency),
            closing_balance: Money::new(row.closing_balance, currency),
            aging,
            lines: statement_lines(&entries, row.period_start, row.period_end, currency),
        })
    }

    /// Compute a statement from ledger entries + allocations (the
    /// finance charge for the period must already be posted).
    fn build_statement(
        &self,
        customer: &df_db::models::Customer,
        entries: &[ArLedgerEntry],
        allocations: &[ArAllocation],
        period_start: NaiveDate,
        period_end: NaiveDate,
    ) -> Statement {
        let currency = self.config().currency;
        let zero = Money::zero(currency);
        let mut opening = zero;
        let mut charges = zero;
        let mut payments = zero;
        let mut finance = zero;
        for entry in entries {
            let day = entry.created_at.date_naive();
            let amount = Money::new(entry.amount, currency);
            if day < period_start {
                opening = opening.checked_add(amount).unwrap_or(opening);
            } else if day <= period_end {
                match entry.entry_type.as_str() {
                    "charge" => charges = charges.checked_add(amount).unwrap_or(charges),
                    // Stored negative; totals are magnitudes.
                    "payment" | "credit_memo" => {
                        payments = payments.checked_sub(amount).unwrap_or(payments);
                    }
                    "finance_charge" => {
                        finance = finance.checked_add(amount).unwrap_or(finance);
                    }
                    _ => {}
                }
            }
        }
        let closing = opening
            .checked_add(charges)
            .and_then(|v| v.checked_sub(payments))
            .and_then(|v| v.checked_add(finance))
            .unwrap_or(opening);
        // Aging as of the period end, from the open charges (after
        // the finance charge was posted, so the charge itself, due
        // at period end, sits in the current bucket).
        let domain_entries: Vec<df_ar::LedgerEntry> =
            entries.iter().map(df_db::ledger_entry_to_domain).collect();
        let domain_allocations: Vec<df_ar::Allocation> = allocations
            .iter()
            .map(df_db::allocation_to_domain)
            .collect();
        let open = open_charges(&domain_entries, &domain_allocations);
        let aging = AgingSummary::from_open(&open, period_end, currency);
        Statement {
            id: Uuid::now_v7(),
            customer_id: customer.id,
            account_number: customer.account_number.clone(),
            currency,
            period_start,
            period_end,
            opening_balance: opening,
            charges_total: charges,
            payments_total: payments,
            finance_charge_total: finance,
            closing_balance: closing,
            aging,
            lines: statement_lines(entries, period_start, period_end, currency),
        }
    }
}

/// Statement lines: ledger entries dated inside the period, oldest
/// first, with the ledger's signed amounts.
fn statement_lines(
    entries: &[ArLedgerEntry],
    period_start: NaiveDate,
    period_end: NaiveDate,
    currency: Currency,
) -> Vec<StatementLine> {
    let mut lines: Vec<StatementLine> = entries
        .iter()
        .filter(|e| {
            let day = e.created_at.date_naive();
            day >= period_start && day <= period_end
        })
        .map(|e| StatementLine {
            entry_id: e.id,
            entry_date: e.created_at.date_naive(),
            entry_type: e.entry_type.clone(),
            reference: e.reference_id.to_string(),
            amount: Money::new(e.amount, currency),
            due_date: e.due_date,
        })
        .collect();
    lines.sort_by_key(|l| l.entry_date);
    lines
}

/// Rebuild the aging summary from a stored `aging_json` value
/// (`{"current": "…", "31-60": "…", …}` with decimal strings).
fn aging_from_json(
    value: &serde_json::Value,
    currency: Currency,
) -> Result<AgingSummary, EdgeError> {
    fn bucket(
        value: &serde_json::Value,
        key: &str,
        currency: Currency,
    ) -> Result<Money, EdgeError> {
        let text = value.get(key).and_then(|v| v.as_str()).ok_or_else(|| {
            EdgeError::InvalidInput(format!("statement aging_json is missing bucket {key}"))
        })?;
        Money::from_str_amount(text, currency).map_err(EdgeError::Domain)
    }
    Ok(AgingSummary {
        current: bucket(value, "current", currency)?,
        days_31_60: bucket(value, "31-60", currency)?,
        days_61_90: bucket(value, "61-90", currency)?,
        days_91_120: bucket(value, "91-120", currency)?,
        over_120: bucket(value, "120+", currency)?,
    })
}

/// Serialize an aging summary for the `statements.aging_json`
/// column (decimal strings, same keys `aging_from_json` reads).
fn aging_to_json(aging: &AgingSummary) -> serde_json::Value {
    serde_json::json!({
        "current": aging.current.amount().to_string(),
        "31-60": aging.days_31_60.amount().to_string(),
        "61-90": aging.days_61_90.amount().to_string(),
        "91-120": aging.days_91_120.amount().to_string(),
        "120+": aging.over_120.amount().to_string(),
    })
}

/// Map a [`Statement`] to its persistable row.
fn statement_row_from(statement: &Statement) -> StatementRow {
    StatementRow {
        id: statement.id,
        customer_id: statement.customer_id,
        period_start: statement.period_start,
        period_end: statement.period_end,
        opening_balance: statement.opening_balance.amount(),
        charges_total: statement.charges_total.amount(),
        payments_total: statement.payments_total.amount(),
        finance_charge_total: statement.finance_charge_total.amount(),
        closing_balance: statement.closing_balance.amount(),
        aging_json: aging_to_json(&statement.aging),
        pdf_sha256: None,
        created_at: Utc::now(),
    }
}
