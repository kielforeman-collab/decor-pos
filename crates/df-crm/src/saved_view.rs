//! Saved search views (spec §4 `crm_marketing`, §7 Phase 4).
//!
//! A named, serializable filter over customers, stored as JSON in
//! `crm_saved_views.filter_json` so the back-office can offer
//! one-click segments ("contractors over $1,000 balance", "no
//! purchase in 90 days") without hand-writing queries. Evaluation
//! is pure: the caller supplies one [`CustomerSnapshot`] per
//! customer (type, flags, AR balance, last purchase) and the
//! filter decides. The repository equivalent
//! (`df_db::evaluate_saved_view`) builds those snapshots from
//! `customers`, `ar_ledger`, and `transactions`.
//!
//! Deliberately absent: a territory predicate. The `customers`
//! table carries no territory / region column, so there is nothing
//! honest to match against — adding one would invent data. If a
//! territory attribute lands on customers later, add an
//! `Option<String>` field here and a matching snapshot field.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The per-customer facts a filter evaluates. Balances are
/// `Decimal` (never floats); `last_purchase_at` is `None` for a
/// customer who has never purchased.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomerSnapshot {
    pub customer_id: Uuid,
    pub customer_type: String,
    pub charge_enabled: bool,
    pub credit_hold: bool,
    /// Derived AR balance (sum of `ar_ledger` amounts).
    pub balance: Decimal,
    /// Most recent completed sale, if any.
    pub last_purchase_at: Option<DateTime<Utc>>,
}

/// A named filter over customers. Every field is optional; unset
/// fields do not constrain. All set fields must match (AND).
/// `#[serde(default)]` on the struct makes parsing lenient: a
/// stored JSON object missing keys (or carrying unknown keys)
/// still parses, with the missing predicates unconstrained.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomerFilter {
    /// e.g. `"contractor"`, `"retail"`, `"designer"`.
    pub customer_type: Option<String>,
    /// Charge account enabled?
    pub charge_enabled: Option<bool>,
    /// On credit hold?
    pub credit_hold: Option<bool>,
    /// Minimum AR balance (inclusive).
    pub min_balance: Option<Decimal>,
    /// Maximum AR balance (inclusive).
    pub max_balance: Option<Decimal>,
    /// Last purchase within this many days of `as_of` (inclusive).
    /// A customer who never purchased never matches this.
    pub last_purchase_within_days: Option<i64>,
    /// Last purchase more than this many days before `as_of`, or
    /// never purchased at all.
    pub last_purchase_before_days: Option<i64>,
    /// `Some(true)` — has purchased at least once;
    /// `Some(false)` — has never purchased.
    pub has_purchased: Option<bool>,
}

impl CustomerFilter {
    /// Evaluate this filter against one snapshot at `as_of`.
    pub fn matches(&self, snap: &CustomerSnapshot, as_of: DateTime<Utc>) -> bool {
        if let Some(t) = &self.customer_type {
            if &snap.customer_type != t {
                return false;
            }
        }
        if let Some(want) = self.charge_enabled {
            if snap.charge_enabled != want {
                return false;
            }
        }
        if let Some(want) = self.credit_hold {
            if snap.credit_hold != want {
                return false;
            }
        }
        if let Some(min) = self.min_balance {
            if snap.balance < min {
                return false;
            }
        }
        if let Some(max) = self.max_balance {
            if snap.balance > max {
                return false;
            }
        }
        if let Some(days) = self.last_purchase_within_days {
            match snap.last_purchase_at {
                Some(last) => {
                    if as_of.signed_duration_since(last).num_days() > days {
                        return false;
                    }
                }
                None => return false,
            }
        }
        if let Some(days) = self.last_purchase_before_days {
            // Never purchased counts as "before".
            if let Some(last) = snap.last_purchase_at {
                if as_of.signed_duration_since(last).num_days() <= days {
                    return false;
                }
            }
        }
        if let Some(want) = self.has_purchased {
            if snap.last_purchase_at.is_some() != want {
                return false;
            }
        }
        true
    }

    /// Filter a whole snapshot list at `as_of`, preserving order.
    pub fn apply<'a>(
        &self,
        snapshots: &'a [CustomerSnapshot],
        as_of: DateTime<Utc>,
    ) -> Vec<&'a CustomerSnapshot> {
        snapshots
            .iter()
            .filter(|s| self.matches(s, as_of))
            .collect()
    }
}

/// One row of `crm_saved_views`: a name plus its filter JSON.
/// `filter_json` holds a serialized [`CustomerFilter`] (this
/// crate's customer-segment dialect). See [`ActivityFilter`] for
/// the activity-log dialect the edge's `run_saved_view` uses —
/// the two dialects are deliberately distinct types, and
/// `df_db::evaluate_saved_view` refuses a view whose JSON is in
/// the activity dialect rather than silently misreading it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedView {
    /// UUIDv7 primary key.
    pub id: Uuid,
    pub org_id: Uuid,
    /// The store this view belongs to (unique per (store, name)).
    pub store_id: Uuid,
    /// Display name, e.g. "Lapsed contractors".
    pub name: String,
    pub filter: CustomerFilter,
    pub created_at: DateTime<Utc>,
}

impl SavedView {
    /// Build a new saved view with a fresh UUIDv7 id.
    pub fn new(
        org_id: Uuid,
        store_id: Uuid,
        name: impl Into<String>,
        filter: CustomerFilter,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            name: name.into(),
            filter,
            created_at: Utc::now(),
        }
    }

    /// Serialize the filter for the `filter_json` column.
    pub fn filter_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.filter).unwrap_or(serde_json::json!({}))
    }
}

/// The per-activity facts the activity-log filter dialect
/// evaluates. This is the dialect `df-edge`'s `run_saved_view`
/// documents (`activity_type`, `customer_id`, `completed`,
/// `due_before`): a stored view in that dialect filters the
/// store's *activities*, not its customers.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivitySnapshot {
    pub activity_type: String,
    pub customer_id: Option<Uuid>,
    pub completed: bool,
    pub due_at: Option<DateTime<Utc>>,
}

/// A stored filter over the activity log (the dialect
/// `df-edge::run_saved_view` uses). Unknown JSON keys are
/// ignored, matching that endpoint. This is a *different*
/// dialect from [`CustomerFilter`]: a view is one or the other,
/// never both.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActivityFilter {
    /// e.g. `"call"`, `"visit"`, `"follow_up"`.
    pub activity_type: Option<String>,
    pub customer_id: Option<Uuid>,
    /// `Some(true)` — completed only; `Some(false)` — open only.
    pub completed: Option<bool>,
    /// Due at or before this moment (activities with no due date
    /// never match).
    pub due_before: Option<DateTime<Utc>>,
}

impl ActivityFilter {
    /// Evaluate this filter against one activity snapshot.
    pub fn matches(&self, snap: &ActivitySnapshot) -> bool {
        if let Some(t) = &self.activity_type {
            if &snap.activity_type != t {
                return false;
            }
        }
        if let Some(customer) = self.customer_id {
            if snap.customer_id != Some(customer) {
                return false;
            }
        }
        if let Some(want) = self.completed {
            if snap.completed != want {
                return false;
            }
        }
        if let Some(before) = self.due_before {
            match snap.due_at {
                Some(due) => {
                    if due > before {
                        return false;
                    }
                }
                None => return false,
            }
        }
        true
    }

    /// Filter a whole snapshot list, preserving order.
    pub fn apply<'a>(&self, snapshots: &'a [ActivitySnapshot]) -> Vec<&'a ActivitySnapshot> {
        snapshots.iter().filter(|s| self.matches(s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn snap(ctype: &str, balance: &str, last_purchase_days_ago: Option<i64>) -> CustomerSnapshot {
        let now = Utc::now();
        CustomerSnapshot {
            customer_id: Uuid::now_v7(),
            customer_type: ctype.to_string(),
            charge_enabled: true,
            credit_hold: false,
            balance: dec(balance),
            last_purchase_at: last_purchase_days_ago.map(|d| now - TimeDelta::days(d)),
        }
    }

    #[test]
    fn filter_evaluates_all_predicates_as_and() {
        let now = Utc::now();
        let customers = vec![
            snap("contractor", "1500.0000", Some(10)),
            snap("contractor", "50.0000", Some(200)),
            snap("retail", "1500.0000", Some(5)),
            snap("contractor", "2000.0000", None),
        ];
        // Contractors with a balance >= 1000 who bought in the last 30 days.
        let filter = CustomerFilter {
            customer_type: Some("contractor".to_string()),
            min_balance: Some(dec("1000.0000")),
            last_purchase_within_days: Some(30),
            ..Default::default()
        };
        let hits = filter.apply(&customers, now);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].customer_id, customers[0].customer_id);

        // Never-purchased contractors.
        let never = CustomerFilter {
            customer_type: Some("contractor".to_string()),
            has_purchased: Some(false),
            ..Default::default()
        };
        let hits = never.apply(&customers, now);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].customer_id, customers[3].customer_id);

        // Lapsed: last purchase more than 90 days ago (never counts).
        let lapsed = CustomerFilter {
            last_purchase_before_days: Some(90),
            ..Default::default()
        };
        let hits = lapsed.apply(&customers, now);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn filter_round_trips_through_json() {
        let filter = CustomerFilter {
            customer_type: Some("contractor".to_string()),
            min_balance: Some(dec("1000.0000")),
            last_purchase_before_days: Some(90),
            ..Default::default()
        };
        let json = serde_json::to_string(&filter).unwrap();
        let back: CustomerFilter = serde_json::from_str(&json).unwrap();
        assert_eq!(back, filter);
    }

    #[test]
    fn empty_filter_matches_everyone() {
        let now = Utc::now();
        let customers = vec![snap("retail", "0.0000", None)];
        assert_eq!(CustomerFilter::default().apply(&customers, now).len(), 1);
    }

    #[test]
    fn activity_filter_matches_the_edge_dialect() {
        let now = Utc::now();
        let activities = vec![
            ActivitySnapshot {
                activity_type: "call".to_string(),
                customer_id: Some(Uuid::now_v7()),
                completed: false,
                due_at: Some(now - TimeDelta::hours(1)),
            },
            ActivitySnapshot {
                activity_type: "visit".to_string(),
                customer_id: None,
                completed: true,
                due_at: None,
            },
        ];
        // Open calls due before now.
        let filter = ActivityFilter {
            activity_type: Some("call".to_string()),
            completed: Some(false),
            due_before: Some(now),
            ..Default::default()
        };
        assert_eq!(filter.apply(&activities).len(), 1);
        // A due_before bound never matches an activity with no due date.
        let filter = ActivityFilter {
            due_before: Some(now),
            ..Default::default()
        };
        assert_eq!(filter.apply(&activities).len(), 1);
        // Unknown JSON keys are ignored, matching run_saved_view.
        let parsed: ActivityFilter =
            serde_json::from_str(r#"{"activity_type":"visit","bogus_key":1}"#).unwrap();
        assert_eq!(parsed.activity_type.as_deref(), Some("visit"));
        assert_eq!(parsed.apply(&activities).len(), 1);
    }
}
