//! CRM activity log (spec §4 `crm_marketing`, §7 Phase 4;
//! migration `0006_phase4` `crm_activities`).
//!
//! One row per interaction with a customer / contractor: a call,
//! a visit, an email, a quote, a note, or a follow-up. Activities
//! link to the existing `customers` charge-account rows by
//! `customer_id` — there is no second customer table — and are
//! scoped to one store (`store_id`).
//!
//! A **reminder is an activity with a `due_at`** (see
//! [`crate::reminder`]): it is due when it is not completed, its
//! due time has passed, and it has not been snoozed past the
//! check time. There is no separate reminders table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What kind of interaction this activity records. The closed
/// list matches the `crm_activities.activity_type` values in
/// migration `0006_phase4` (and `df-edge`'s `ACTIVITY_TYPES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    /// Phone call.
    Call,
    /// Email sent or received.
    Email,
    /// In-person visit (store, job site, showroom).
    Visit,
    /// Quote given / discussed.
    Quote,
    /// Free-text note with no external contact.
    Note,
    /// Follow-up touch (usually carries a `due_at`).
    FollowUp,
}

impl ActivityType {
    /// Stable storage string (used as the `activity_type` column).
    pub fn as_str(self) -> &'static str {
        match self {
            ActivityType::Call => "call",
            ActivityType::Email => "email",
            ActivityType::Visit => "visit",
            ActivityType::Quote => "quote",
            ActivityType::Note => "note",
            ActivityType::FollowUp => "follow_up",
        }
    }
    /// Parse a storage string back. Unknown values are rejected so
    /// a bad row fails loudly instead of silently becoming a note.
    ///
    /// Deliberately an inherent method rather than `FromStr`:
    /// the edge wiring layer calls it as `ActivityType::from_str`
    /// with `.ok_or_else`, and `Option` is the honest return
    /// here (there is no parse error to report).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "call" => Some(ActivityType::Call),
            "email" => Some(ActivityType::Email),
            "visit" => Some(ActivityType::Visit),
            "quote" => Some(ActivityType::Quote),
            "note" => Some(ActivityType::Note),
            "follow_up" => Some(ActivityType::FollowUp),
            _ => None,
        }
    }
}

/// One row of `crm_activities` (migration `0006_phase4`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrmActivity {
    /// UUIDv7 primary key.
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    /// The customer / contractor this activity is about
    /// (`customers.id`), if it is about one at all.
    pub customer_id: Option<Uuid>,
    pub activity_type: ActivityType,
    /// Short subject line.
    pub subject: String,
    /// Free-text notes, if any.
    pub notes: Option<String>,
    /// Staff user who logged it, if known.
    pub created_by: Option<Uuid>,
    /// When a follow-up / reminder touch is owed, if any. An
    /// activity with a `due_at` is a tickler item (see
    /// [`crate::reminder`]).
    pub due_at: Option<DateTime<Utc>>,
    /// Set when the activity / reminder is done; NULL while open.
    pub completed_at: Option<DateTime<Utc>>,
    /// Snooze target, if the reminder was snoozed. The effective
    /// due time is the later of `due_at` and this value.
    pub snoozed_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl CrmActivity {
    /// Build a new activity with a fresh UUIDv7 id.
    pub fn new(
        org_id: Uuid,
        store_id: Uuid,
        customer_id: Option<Uuid>,
        activity_type: ActivityType,
        subject: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            customer_id,
            activity_type,
            subject: subject.into(),
            notes: None,
            created_by: None,
            due_at: None,
            completed_at: None,
            snoozed_until: None,
            created_at: Utc::now(),
        }
    }

    /// Attach free-text notes.
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    /// Attach a due date, turning this activity into a tickler
    /// item.
    pub fn with_due_at(mut self, due_at: DateTime<Utc>) -> Self {
        self.due_at = Some(due_at);
        self
    }

    /// True once the activity / reminder is done.
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }

    /// True when a follow-up is owed and nothing has discharged
    /// it: a due date is set and the activity is not completed.
    pub fn follow_up_outstanding(&self) -> bool {
        self.due_at.is_some() && !self.is_completed()
    }
}

/// Activities whose follow-up is outstanding and due at or before
/// `as_of` (effective due time, so snoozed items stay hidden
/// until their snooze passes), oldest due first. Pure function
/// over an in-memory slice; the repository equivalent is
/// `df_db::list_reminders_due_for_store`.
pub fn follow_ups_due(activities: &[CrmActivity], as_of: DateTime<Utc>) -> Vec<&CrmActivity> {
    let mut due: Vec<&CrmActivity> = activities
        .iter()
        .filter(|a| crate::reminder::is_due(a, as_of))
        .collect();
    due.sort_by_key(|a| crate::reminder::effective_due(a));
    due
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn activity(due: Option<DateTime<Utc>>) -> CrmActivity {
        CrmActivity::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            Some(Uuid::now_v7()),
            ActivityType::Call,
            "Called about the Miller job",
        )
        .with_due_at(due.unwrap_or_else(Utc::now))
    }

    #[test]
    fn activity_type_round_trips_through_storage_string() {
        for t in [
            ActivityType::Call,
            ActivityType::Email,
            ActivityType::Visit,
            ActivityType::Quote,
            ActivityType::Note,
            ActivityType::FollowUp,
        ] {
            assert_eq!(ActivityType::from_str(t.as_str()), Some(t));
        }
        assert_eq!(ActivityType::from_str("carrier-pigeon"), None);
    }

    #[test]
    fn follow_ups_due_only_returns_outstanding_and_due() {
        let now = Utc::now();
        let mut overdue = activity(Some(now - TimeDelta::hours(2)));
        let not_yet = CrmActivity::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            None,
            ActivityType::FollowUp,
            "Later",
        )
        .with_due_at(now + TimeDelta::hours(2));
        let mut discharged = activity(Some(now - TimeDelta::hours(5)));
        discharged.completed_at = Some(now - TimeDelta::hours(1));
        let no_due = CrmActivity::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            None,
            ActivityType::Note,
            "Just a note",
        );
        let list = vec![not_yet, discharged, overdue.clone(), no_due];
        let due = follow_ups_due(&list, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, overdue.id);
        // Completing it removes it from the due list.
        overdue.completed_at = Some(now);
        let mut list2 = list.clone();
        list2[2] = overdue;
        assert!(follow_ups_due(&list2, now).is_empty());
    }
}
