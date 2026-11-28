//! Read-only contractor AR summary, 30/60/90/120 aging, and collections views.

use std::collections::HashMap;

use chrono::NaiveDate;
use df_db::models::{ArLedgerEntry, Customer};
use df_domain::ar::{ArEntry, aging_totals};
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// One customer's AR position (spec §6.1 `GET /ar/customers/{id}/summary`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArCustomerSummary {
    pub customer_id: Uuid,
    pub account_number: String,
    pub company_name: Option<String>,
    pub customer_type: String,
    pub charge_enabled: bool,
    pub credit_limit: Money,
    pub credit_hold: bool,
    pub balance: Money,
    pub entry_count: usize,
    pub oldest_due_date: Option<NaiveDate>,
}

/// Per-customer aging row: buckets are
/// [current, 31-60, 61-90, 91-120, 120+].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgingCustomerRow {
    pub customer_id: Uuid,
    pub account_number: String,
    pub balance: Money,
    pub buckets: [Money; 5],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgingView {
    pub as_of: NaiveDate,
    pub totals: [Money; 5],
    pub customers: Vec<AgingCustomerRow>,
}

/// One collections-workspace row (spec §6.1 `GET /ar/collections`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionsRow {
    pub customer_id: Uuid,
    pub account_number: String,
    pub company_name: Option<String>,
    pub balance: Money,
    pub credit_limit: Money,
    pub credit_hold: bool,
    /// Amount in the 31-60 / 61-90 / 91-120 / 120+ buckets.
    pub overdue: Money,
    pub oldest_due_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionsView {
    pub as_of: NaiveDate,
    pub customers: Vec<CollectionsRow>,
}

fn zero_money(currency: Currency) -> Money {
    Money::zero(currency)
}

impl Edge {
    // --- AR reads (delegate to df-domain::ar math + df-db ledger) -------------

    /// One customer's AR position: ledger balance, credit controls
    /// and entry count.
    pub async fn ar_customer_summary(
        &self,
        customer_id: Uuid,
    ) -> Result<ArCustomerSummary, EdgeError> {
        let customer: Customer = df_db::get_customer(self.pool(), customer_id).await?;
        let entries = df_db::list_ar_entries_for_customer(self.pool(), customer_id).await?;
        let balance = entries
            .iter()
            .map(|entry| entry.amount)
            .fold(Decimal::ZERO, |acc, amount| acc + amount);
        let oldest_due_date = entries.iter().filter_map(|entry| entry.due_date).min();
        Ok(ArCustomerSummary {
            customer_id,
            account_number: customer.account_number,
            company_name: customer.company_name,
            customer_type: customer.customer_type,
            charge_enabled: customer.charge_enabled,
            credit_limit: Money::new(customer.credit_limit, self.config().currency),
            credit_hold: customer.credit_hold,
            balance: Money::new(balance, self.config().currency),
            entry_count: entries.len(),
            oldest_due_date,
        })
    }

    /// Bucket one customer's AR entries (30/60/90/120 aging) with
    /// `df-domain::ar`. Positive entries bucket by due date (no due
    /// date = current); payments and credit memos (negative amounts)
    /// are unallocated, so they reduce the Current bucket — the
    /// bucket sum always equals the customer's balance.
    fn age_customer_entries(
        entries: &[ArLedgerEntry],
        as_of: NaiveDate,
        currency: Currency,
    ) -> [Money; 5] {
        let mut open: Vec<ArEntry> = Vec::new();
        let mut credits = Money::zero(currency);
        for entry in entries {
            if entry.amount >= Decimal::ZERO {
                open.push(ArEntry {
                    id: entry.id,
                    customer_id: entry.customer_id,
                    job_id: entry.job_id,
                    balance: Money::new(entry.amount, currency),
                    due_date: entry.due_date.unwrap_or(entry.created_at.date_naive()),
                });
            } else {
                credits = credits
                    .checked_add(Money::new(entry.amount, currency))
                    .unwrap_or(credits);
            }
        }
        let mut buckets = aging_totals(&open, as_of, currency);
        if !credits.is_zero() {
            buckets[0] = buckets[0].checked_add(credits).unwrap_or(buckets[0]);
        }
        buckets
    }

    /// 30/60/90/120 aging for every customer with AR entries at a
    /// store (spec §6.1 `GET /ar/aging`).
    pub async fn ar_aging(&self, store_id: Uuid, as_of: NaiveDate) -> Result<AgingView, EdgeError> {
        let currency = self.config().currency;
        let entries = df_db::list_ar_entries_for_store(self.pool(), store_id).await?;
        let mut by_customer: HashMap<Uuid, Vec<&ArLedgerEntry>> = HashMap::new();
        for entry in &entries {
            by_customer
                .entry(entry.customer_id)
                .or_default()
                .push(entry);
        }
        let mut customers: Vec<AgingCustomerRow> = Vec::new();
        let mut totals = [
            zero_money(currency),
            zero_money(currency),
            zero_money(currency),
            zero_money(currency),
            zero_money(currency),
        ];
        for (customer_id, customer_entries) in &by_customer {
            let customer = df_db::get_customer(self.pool(), *customer_id).await?;
            let owned: Vec<ArLedgerEntry> = customer_entries
                .iter()
                .map(|entry| (*entry).clone())
                .collect();
            let buckets = Self::age_customer_entries(&owned, as_of, currency);
            let balance = owned
                .iter()
                .map(|entry| entry.amount)
                .fold(Decimal::ZERO, |acc, amount| acc + amount);
            for (index, bucket) in buckets.iter().enumerate() {
                totals[index] = totals[index].checked_add(*bucket).unwrap_or(totals[index]);
            }
            customers.push(AgingCustomerRow {
                customer_id: *customer_id,
                account_number: customer.account_number,
                balance: Money::new(balance, currency),
                buckets,
            });
        }
        customers.sort_by(|a, b| a.account_number.cmp(&b.account_number));
        Ok(AgingView {
            as_of,
            totals,
            customers,
        })
    }

    /// Collections workspace: customers who owe money and are
    /// overdue, on credit hold, or over their credit limit — most
    /// overdue first (spec §6.1 `GET /ar/collections`).
    pub async fn ar_collections(
        &self,
        store_id: Uuid,
        as_of: NaiveDate,
    ) -> Result<CollectionsView, EdgeError> {
        let currency = self.config().currency;
        let zero = zero_money(currency);
        let aging = self.ar_aging(store_id, as_of).await?;
        let mut rows: Vec<CollectionsRow> = Vec::new();
        for customer_row in &aging.customers {
            let customer = df_db::get_customer(self.pool(), customer_row.customer_id).await?;
            if customer_row.balance.amount() <= Decimal::ZERO {
                continue;
            }
            let overdue = customer_row.buckets[1..]
                .iter()
                .fold(zero, |acc, bucket| acc.checked_add(*bucket).unwrap_or(acc));
            let over_limit = customer_row.balance.amount() > customer.credit_limit;
            if overdue.amount() <= Decimal::ZERO && !customer.credit_hold && !over_limit {
                continue;
            }
            let entries =
                df_db::list_ar_entries_for_customer(self.pool(), customer_row.customer_id).await?;
            let oldest_due_date = entries
                .iter()
                .filter(|entry| entry.amount > Decimal::ZERO)
                .filter_map(|entry| entry.due_date)
                .min();
            rows.push(CollectionsRow {
                customer_id: customer_row.customer_id,
                account_number: customer_row.account_number.clone(),
                company_name: customer.company_name,
                balance: customer_row.balance,
                credit_limit: Money::new(customer.credit_limit, currency),
                credit_hold: customer.credit_hold,
                overdue,
                oldest_due_date,
            });
        }
        rows.sort_by(|a, b| {
            b.overdue
                .amount()
                .cmp(&a.overdue.amount())
                .then(b.balance.amount().cmp(&a.balance.amount()))
        });
        Ok(CollectionsView {
            as_of,
            customers: rows,
        })
    }
}
