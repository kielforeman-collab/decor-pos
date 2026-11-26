//! Lapsed-contractor report (spec §4 `crm_marketing`, §7 Phase 4).
//!
//! A contractor is **lapsed** when their most recent completed
//! purchase is more than `lapsed_after_days` before the report
//! date. A contractor who has **never purchased** is reported
//! separately (`LapsedStatus::NeverPurchased`) — the two cases
//! need different follow-up (onboard vs. win back), so they are
//! never merged into one bucket. `trailing_spend` is the
//! contractor's completed-sale total over the trailing 365 days
//! before `as_of` (the repository computes it); it is `0` for a
//! lapsed contractor whose last purchase predates that window and
//! for a never-purchased one.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Why a contractor appears on the lapsed report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LapsedStatus {
    /// Last purchase is older than the configured window.
    Lapsed,
    /// No completed purchase on record at all.
    NeverPurchased,
}

/// One contractor's purchase history as the report consumes it.
/// The repository (`df_db::lapsed_contractor_report`) builds these
/// from `customers` + `transactions` + `ar_ledger`; the pure report
/// below only classifies them.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractorPurchaseSummary {
    pub customer_id: Uuid,
    pub account_number: String,
    pub company_name: Option<String>,
    /// Most recent completed sale, if any.
    pub last_purchase_at: Option<DateTime<Utc>>,
    /// Completed-sale total over the trailing 365 days before the
    /// report date.
    pub trailing_spend: Decimal,
    /// Current AR balance (sum of `ar_ledger` amounts).
    pub ar_balance: Decimal,
}

/// One row of the lapsed-contractor report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapsedContractor {
    pub customer_id: Uuid,
    pub account_number: String,
    pub company_name: Option<String>,
    pub status: LapsedStatus,
    pub last_purchase_at: Option<DateTime<Utc>>,
    pub trailing_spend: Decimal,
    /// Current AR balance (sum of `ar_ledger` amounts).
    pub ar_balance: Decimal,
}

/// Classify contractor summaries into the lapsed report.
///
/// * `lapsed_after_days` — a contractor whose last purchase is
///   strictly more than this many days before `as_of` is lapsed.
/// * `include_never_purchased` — when false, never-purchased
///   contractors are left off the report entirely; when true they
///   appear with status [`LapsedStatus::NeverPurchased`].
///
/// Rows come back never-purchased first, then most-inactive
/// first (oldest last purchase at the top), so the most at-risk
/// contractors head the list. This matches the ordering the edge
/// `lapsed_contractors` report uses.
pub fn lapsed_report(
    summaries: &[ContractorPurchaseSummary],
    as_of: DateTime<Utc>,
    lapsed_after_days: i64,
    include_never_purchased: bool,
) -> Vec<LapsedContractor> {
    let mut rows: Vec<LapsedContractor> = summaries
        .iter()
        .filter_map(|s| match s.last_purchase_at {
            Some(last) => {
                if as_of.signed_duration_since(last).num_days() > lapsed_after_days {
                    Some(LapsedContractor {
                        customer_id: s.customer_id,
                        account_number: s.account_number.clone(),
                        company_name: s.company_name.clone(),
                        status: LapsedStatus::Lapsed,
                        last_purchase_at: Some(last),
                        trailing_spend: s.trailing_spend,
                        ar_balance: s.ar_balance,
                    })
                } else {
                    None
                }
            }
            None => {
                if include_never_purchased {
                    Some(LapsedContractor {
                        customer_id: s.customer_id,
                        account_number: s.account_number.clone(),
                        company_name: s.company_name.clone(),
                        status: LapsedStatus::NeverPurchased,
                        last_purchase_at: None,
                        trailing_spend: s.trailing_spend,
                        ar_balance: s.ar_balance,
                    })
                } else {
                    None
                }
            }
        })
        .collect();
    rows.sort_by(|a, b| match (a.last_purchase_at, b.last_purchase_at) {
        (None, None) => a.account_number.cmp(&b.account_number),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(&y),
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn summary(days_ago: Option<i64>, spend: &str) -> ContractorPurchaseSummary {
        ContractorPurchaseSummary {
            customer_id: Uuid::now_v7(),
            account_number: format!(
                "C-{}",
                days_ago
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "never".to_string())
            ),
            company_name: Some("Acme Painting".to_string()),
            last_purchase_at: days_ago.map(|d| Utc::now() - TimeDelta::days(d)),
            trailing_spend: dec(spend),
            ar_balance: dec("0.0000"),
        }
    }

    #[test]
    fn lapsed_vs_recent_vs_never_are_distinct() {
        let now = Utc::now();
        let summaries = vec![
            summary(Some(200), "0.0000"),   // lapsed (> 90 days)
            summary(Some(10), "1250.0000"), // recent: excluded
            summary(None, "0.0000"),        // never purchased
            summary(Some(100), "300.0000"), // well over the line
            summary(Some(90), "300.0000"),  // exactly at the line: not lapsed
        ];
        let report = lapsed_report(&summaries, now, 90, true);
        assert_eq!(report.len(), 3);
        // Never-purchased first, then oldest purchase first.
        assert_eq!(report[0].status, LapsedStatus::NeverPurchased);
        assert_eq!(report[0].last_purchase_at, None);
        assert_eq!(report[1].status, LapsedStatus::Lapsed);
        assert_eq!(report[1].account_number, "C-200");
        assert_eq!(report[2].account_number, "C-100");
        // Excluding never-purchased drops only that row.
        let without_never = lapsed_report(&summaries, now, 90, false);
        assert_eq!(without_never.len(), 2);
        assert!(
            without_never
                .iter()
                .all(|r| r.status == LapsedStatus::Lapsed)
        );
    }
}
