//! Collections queue (spec §6.1 `GET /ar/collections` data).
//!
//! Overdue charge accounts ranked by how far gone they are, then
//! by how much they owe: the oldest aging bucket first (120+
//! before 91-120 before 61-90 before 31-60), then the largest
//! overdue balance first. Each entry carries the suggested
//! next action for that bucket — a suggestion for the operator,
//! not an automated step.

use chrono::NaiveDate;
use df_domain::ar::{AgingBucket, bucket_for};
use df_domain::{Currency, Money};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ledger::{OpenCharge, is_overdue};

/// Suggested next action for an account whose oldest overdue
/// charge sits in `bucket`.
pub fn next_action_for(bucket: AgingBucket) -> &'static str {
    match bucket {
        AgingBucket::Current => "no action — nothing overdue",
        AgingBucket::Days31_60 => "send payment reminder",
        AgingBucket::Days61_90 => "call customer",
        AgingBucket::Days91_120 => "send demand letter",
        AgingBucket::Over120 => "escalate to collections",
    }
}

/// One overdue account in the collections queue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionsEntry {
    pub customer_id: Uuid,
    pub account_number: String,
    /// Everything still owed (all buckets).
    pub total_open: Money,
    /// What is overdue (31+ days past due).
    pub total_overdue: Money,
    /// Due date of the oldest overdue charge.
    pub oldest_due_date: NaiveDate,
    /// Aging bucket of the oldest overdue charge.
    pub oldest_bucket: AgingBucket,
    /// Suggested next action for `oldest_bucket`.
    pub next_action: String,
}

/// Build the queue from one account's open charges. Returns `None`
/// when the account has nothing overdue.
pub fn queue_entry_for(
    customer_id: Uuid,
    account_number: &str,
    open: &[OpenCharge],
    as_of: NaiveDate,
    currency: Currency,
) -> Option<CollectionsEntry> {
    let total_open = open
        .iter()
        .map(|c| Money::new(c.open_amount, currency))
        .fold(Money::zero(currency), |acc, m| {
            acc.checked_add(m).unwrap_or(acc)
        });
    let overdue: Vec<&OpenCharge> = open.iter().filter(|c| is_overdue(c, as_of)).collect();
    if overdue.is_empty() {
        return None;
    }
    let total_overdue = overdue
        .iter()
        .map(|c| Money::new(c.open_amount, currency))
        .fold(Money::zero(currency), |acc, m| {
            acc.checked_add(m).unwrap_or(acc)
        });
    let oldest = overdue
        .iter()
        .min_by_key(|c| (c.due_date, c.created_at))
        .expect("overdue is non-empty");
    let oldest_bucket = bucket_for(oldest.due_date, as_of);
    Some(CollectionsEntry {
        customer_id,
        account_number: account_number.to_string(),
        total_open,
        total_overdue,
        oldest_due_date: oldest.due_date,
        oldest_bucket,
        next_action: next_action_for(oldest_bucket).to_string(),
    })
}

/// Rank the queue: oldest bucket first (120+ is rank 4), then the
/// largest overdue balance first.
pub fn collections_queue(
    accounts: &[(Uuid, String, Vec<OpenCharge>)],
    as_of: NaiveDate,
    currency: Currency,
) -> Vec<CollectionsEntry> {
    let mut queue: Vec<CollectionsEntry> = accounts
        .iter()
        .filter_map(|(id, number, open)| queue_entry_for(*id, number, open, as_of, currency))
        .collect();
    fn bucket_rank(bucket: AgingBucket) -> u8 {
        match bucket {
            AgingBucket::Current => 0,
            AgingBucket::Days31_60 => 1,
            AgingBucket::Days61_90 => 2,
            AgingBucket::Days91_120 => 3,
            AgingBucket::Over120 => 4,
        }
    }
    queue.sort_by(|a, b| {
        bucket_rank(b.oldest_bucket)
            .cmp(&bucket_rank(a.oldest_bucket))
            .then(b.total_overdue.amount().cmp(&a.total_overdue.amount()))
    });
    queue
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono::Utc;
    use rust_decimal::Decimal;

    fn charge(id: u128, amount: &str, due: NaiveDate) -> OpenCharge {
        OpenCharge {
            id: Uuid::from_u128(id),
            customer_id: Uuid::from_u128(1),
            job_id: None,
            due_date: due,
            created_at: Utc.with_ymd_and_hms(2026, 8, 15, 0, 0, 0).unwrap(),
            open_amount: Decimal::from_str_exact(amount).unwrap(),
        }
    }

    #[test]
    fn queue_ranks_oldest_bucket_then_amount() {
        let as_of = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let accounts = vec![
            // 31-60 bucket, owes 500.
            (
                Uuid::from_u128(1),
                "C-1".to_string(),
                vec![charge(
                    10,
                    "500.0000",
                    NaiveDate::from_ymd_opt(2026, 9, 14).unwrap(),
                )],
            ),
            // 120+ bucket, owes 100 — outranks the 500.
            (
                Uuid::from_u128(2),
                "C-2".to_string(),
                vec![charge(
                    11,
                    "100.0000",
                    NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
                )],
            ),
            // 120+ bucket, owes 200 — first.
            (
                Uuid::from_u128(3),
                "C-3".to_string(),
                vec![charge(
                    12,
                    "200.0000",
                    NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(),
                )],
            ),
            // Nothing overdue: excluded.
            (
                Uuid::from_u128(4),
                "C-4".to_string(),
                vec![charge(
                    13,
                    "50.0000",
                    NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
                )],
            ),
        ];
        let queue = collections_queue(&accounts, as_of, Currency::Usd);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].account_number, "C-3");
        assert_eq!(queue[0].next_action, "escalate to collections");
        assert_eq!(queue[1].account_number, "C-2");
        assert_eq!(queue[2].account_number, "C-1");
        assert_eq!(queue[2].next_action, "send payment reminder");
    }
}
