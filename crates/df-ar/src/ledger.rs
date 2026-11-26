//! Open AR entries and FIFO payment allocation (spec §5.4
//! `ar_ledger` / `ar_allocations`).
//!
//! The ledger is append-only, so "what is still owed on a charge"
//! is derived: a charge (or finance charge) minus the allocations
//! that point at it, minus credit memos applied oldest-first.
//! [`open_charges`] is that derivation; [`allocate_payment`] plans
//! a new payment against the open charges, oldest first.

use chrono::{DateTime, NaiveDate, Utc};
use df_domain::ar::{ArEntry, bucket_for};
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A minimal ledger-entry view: the fields the AR math needs.
/// Callers map their row type (`df-db::models::ArLedgerEntry`) into
/// this; `df-ar` stays free of any database dependency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub job_id: Option<Uuid>,
    /// `'charge' | 'payment' | 'credit_memo' | 'finance_charge'`.
    /// Amount is positive when it increases the balance.
    pub entry_type: String,
    pub amount: Decimal,
    pub due_date: Option<NaiveDate>,
    pub created_at: DateTime<Utc>,
}

/// One `ar_allocations` row: part of a payment applied to one
/// charge entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Allocation {
    pub payment_entry_id: Uuid,
    pub charge_entry_id: Uuid,
    pub allocated_amount: Decimal,
}

/// One still-open charge: what a customer owes on a single ledger
/// entry, after allocations and credit memos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenCharge {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub job_id: Option<Uuid>,
    /// Charges without a due date (legacy / un-synced terms) sort
    /// as due on their creation date and never count as overdue.
    pub due_date: NaiveDate,
    pub created_at: DateTime<Utc>,
    pub open_amount: Decimal,
}

/// Derive the open charges for one customer from the full ledger
/// history and the allocations that point at it.
///
/// * Charge-like entries (`charge`, `finance_charge`) open a
///   balance; payments reduce it only through `ar_allocations`
///   rows; credit memos (negative amounts, no allocation rows)
///   reduce the oldest open charges first.
/// * Entries are processed oldest-first (by due date, then
///   creation) so the result is deterministic.
/// * A charge fully covered by allocations / memos drops out;
///   over-applied amounts are clamped at zero, never negative.
pub fn open_charges(entries: &[LedgerEntry], allocations: &[Allocation]) -> Vec<OpenCharge> {
    let mut open: Vec<OpenCharge> = entries
        .iter()
        .filter(|e| e.entry_type == "charge" || e.entry_type == "finance_charge")
        .map(|e| OpenCharge {
            id: e.id,
            customer_id: e.customer_id,
            job_id: e.job_id,
            due_date: e.due_date.unwrap_or_else(|| e.created_at.date_naive()),
            created_at: e.created_at,
            open_amount: e.amount,
        })
        .collect();
    open.sort_by_key(|a| (a.due_date, a.created_at));

    for alloc in allocations {
        if let Some(charge) = open.iter_mut().find(|c| c.id == alloc.charge_entry_id) {
            charge.open_amount -= alloc.allocated_amount;
            if charge.open_amount < Decimal::ZERO {
                charge.open_amount = Decimal::ZERO;
            }
        }
    }

    // Credit memos: negative amounts, no allocation rows. Apply
    // oldest-first, in creation order.
    let mut memos: Vec<&LedgerEntry> = entries
        .iter()
        .filter(|e| e.entry_type == "credit_memo")
        .collect();
    memos.sort_by_key(|e| e.created_at);
    for memo in memos {
        let mut remaining = -memo.amount;
        if remaining <= Decimal::ZERO {
            continue;
        }
        for charge in open.iter_mut() {
            if remaining <= Decimal::ZERO {
                break;
            }
            let applied = remaining.min(charge.open_amount);
            charge.open_amount -= applied;
            remaining -= applied;
        }
    }

    open.retain(|c| c.open_amount > Decimal::ZERO);
    open
}

/// Total still owed across the open charges.
pub fn open_balance(open: &[OpenCharge]) -> Decimal {
    open.iter()
        .map(|c| c.open_amount)
        .fold(Decimal::ZERO, |acc, amount| acc + amount)
}

/// Convert open charges to the [`ArEntry`] shape
/// [`df_domain::ar::aging_totals`] consumes, so aging reuses the
/// Phase 0 math instead of re-implementing buckets.
pub fn aging_entries(open: &[OpenCharge], currency: Currency) -> Vec<ArEntry> {
    open.iter()
        .map(|c| ArEntry {
            id: c.id,
            customer_id: c.customer_id,
            job_id: c.job_id,
            balance: Money::new(c.open_amount, currency),
            due_date: c.due_date,
        })
        .collect()
}

/// Plan a payment against open charges, oldest first (FIFO).
/// `open` must already be in oldest-first order (as
/// [`open_charges`] returns). Returns one [`Allocation`] per charge
/// touched, in the order applied; a payment larger than the open
/// balance allocates only up to the balance (the remainder is an
/// unallocated credit on the account). Zero / negative payments
/// allocate nothing.
pub fn allocate_payment(
    open: &[OpenCharge],
    payment_entry_id: Uuid,
    payment_amount: Decimal,
) -> Vec<Allocation> {
    let mut remaining = payment_amount;
    let mut out = Vec::new();
    if remaining <= Decimal::ZERO {
        return out;
    }
    for charge in open {
        if remaining <= Decimal::ZERO {
            break;
        }
        let applied = remaining.min(charge.open_amount);
        if applied <= Decimal::ZERO {
            continue;
        }
        out.push(Allocation {
            payment_entry_id,
            charge_entry_id: charge.id,
            allocated_amount: applied,
        });
        remaining -= applied;
    }
    out
}

/// Is this open charge overdue as of `as_of` (more than 30 days
/// past its due date, i.e. outside the `Current` bucket)?
pub fn is_overdue(charge: &OpenCharge, as_of: NaiveDate) -> bool {
    bucket_for(charge.due_date, as_of) != df_domain::ar::AgingBucket::Current
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn entry(
        id: u128,
        kind: &str,
        amount: &str,
        due: NaiveDate,
        created: DateTime<Utc>,
    ) -> LedgerEntry {
        LedgerEntry {
            id: Uuid::from_u128(id),
            customer_id: Uuid::from_u128(1),
            job_id: None,
            entry_type: kind.to_string(),
            amount: Decimal::from_str_exact(amount).unwrap(),
            due_date: Some(due),
            created_at: created,
        }
    }

    #[test]
    fn open_charges_apply_allocations_and_memos_oldest_first() {
        let entries = vec![
            entry(10, "charge", "400.0000", date(2026, 9, 14), dt(2026, 8, 15)),
            entry(11, "charge", "300.0000", date(2026, 11, 4), dt(2026, 10, 5)),
            entry(
                12,
                "finance_charge",
                "3.7500",
                date(2026, 10, 31),
                dt(2026, 10, 31),
            ),
            entry(
                13,
                "credit_memo",
                "-50.0000",
                date(2026, 10, 20),
                dt(2026, 10, 20),
            ),
        ];
        let allocations = vec![Allocation {
            payment_entry_id: Uuid::from_u128(20),
            charge_entry_id: Uuid::from_u128(10),
            allocated_amount: Decimal::from_str_exact("150.0000").unwrap(),
        }];
        let open = open_charges(&entries, &allocations);
        // Oldest charge: 400 - 150 allocated - 50 memo = 200.
        assert_eq!(open.len(), 3);
        assert_eq!(open[0].id, Uuid::from_u128(10));
        assert_eq!(
            open[0].open_amount,
            Decimal::from_str_exact("200.0000").unwrap()
        );
        assert_eq!(
            open[1].open_amount,
            Decimal::from_str_exact("3.7500").unwrap()
        );
        assert_eq!(
            open[2].open_amount,
            Decimal::from_str_exact("300.0000").unwrap()
        );
        assert_eq!(
            open_balance(&open),
            Decimal::from_str_exact("503.7500").unwrap()
        );
    }

    #[test]
    fn fully_paid_charge_drops_out() {
        let entries = vec![entry(
            10,
            "charge",
            "100.0000",
            date(2026, 9, 1),
            dt(2026, 8, 1),
        )];
        let allocations = vec![Allocation {
            payment_entry_id: Uuid::from_u128(20),
            charge_entry_id: Uuid::from_u128(10),
            allocated_amount: Decimal::from_str_exact("100.0000").unwrap(),
        }];
        assert!(open_charges(&entries, &allocations).is_empty());
    }

    #[test]
    fn allocate_payment_is_fifo_and_caps_at_balance() {
        let open = vec![
            OpenCharge {
                id: Uuid::from_u128(10),
                customer_id: Uuid::from_u128(1),
                job_id: None,
                due_date: date(2026, 9, 14),
                created_at: dt(2026, 8, 15),
                open_amount: Decimal::from_str_exact("250.0000").unwrap(),
            },
            OpenCharge {
                id: Uuid::from_u128(11),
                customer_id: Uuid::from_u128(1),
                job_id: None,
                due_date: date(2026, 11, 4),
                created_at: dt(2026, 10, 5),
                open_amount: Decimal::from_str_exact("300.0000").unwrap(),
            },
        ];
        let payment_id = Uuid::from_u128(20);
        let allocs = allocate_payment(
            &open,
            payment_id,
            Decimal::from_str_exact("300.0000").unwrap(),
        );
        assert_eq!(allocs.len(), 2);
        assert_eq!(allocs[0].charge_entry_id, Uuid::from_u128(10));
        assert_eq!(
            allocs[0].allocated_amount,
            Decimal::from_str_exact("250.0000").unwrap()
        );
        assert_eq!(
            allocs[1].allocated_amount,
            Decimal::from_str_exact("50.0000").unwrap()
        );
        // Overpayment allocates only the open balance.
        let allocs = allocate_payment(&open, payment_id, Decimal::from_str_exact("9999").unwrap());
        assert_eq!(
            allocs
                .iter()
                .map(|a| a.allocated_amount)
                .fold(Decimal::ZERO, |a, b| a + b),
            Decimal::from_str_exact("550.0000").unwrap()
        );
        // Zero payment allocates nothing.
        assert!(allocate_payment(&open, payment_id, Decimal::ZERO).is_empty());
    }
}
