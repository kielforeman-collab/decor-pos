//! Reminder tickler (spec §4 `crm_marketing`, §7 Phase 4).
//!
//! A reminder is a [`CrmActivity`](crate::activity::CrmActivity)
//! with a `due_at` — there is no separate reminders table
//! (migration `0006_phase4` folds the tickler into
//! `crm_activities`: `due_at` / `completed_at` /
//! `snoozed_until`). It is due when it is not completed, its
//! effective due time is at or before the moment being asked
//! about, and overdue when that moment is strictly past the
//! effective due time. The effective due time is the later of
//! `due_at` and `snoozed_until` (a snooze to *before* the
//! original due time cannot pull a reminder forward).
//!
//! Completing a reminder stamps `completed_at`; a completed
//! reminder is never due or overdue again, and cannot be snoozed.

use chrono::{DateTime, Utc};

use crate::activity::CrmActivity;
use crate::error::{CrmError, Result};

/// The moment this activity's reminder actually falls due: the
/// later of `due_at` and `snoozed_until`. `None` when the activity
/// carries no due date at all (it is not a reminder).
pub fn effective_due(activity: &CrmActivity) -> Option<DateTime<Utc>> {
    match (activity.due_at, activity.snoozed_until) {
        (None, _) => None,
        (Some(due), None) => Some(due),
        (Some(due), Some(until)) => Some(due.max(until)),
    }
}

/// Open, carrying a due date, and due at or before `as_of`.
pub fn is_due(activity: &CrmActivity, as_of: DateTime<Utc>) -> bool {
    !activity.is_completed()
        && effective_due(activity)
            .map(|due| due <= as_of)
            .unwrap_or(false)
}

/// Open, carrying a due date, and strictly past its effective due
/// time at `as_of`.
pub fn is_overdue(activity: &CrmActivity, as_of: DateTime<Utc>) -> bool {
    !activity.is_completed()
        && effective_due(activity)
            .map(|due| due < as_of)
            .unwrap_or(false)
}

/// Mark the reminder done at `at`. A completed reminder cannot be
/// completed again.
pub fn complete(activity: &mut CrmActivity, at: DateTime<Utc>) -> Result<()> {
    if activity.is_completed() {
        return Err(CrmError::AlreadyCompleted(activity.id));
    }
    activity.completed_at = Some(at);
    Ok(())
}

/// Push the reminder to `until`, which must be after `now`.
/// Snoozing a completed reminder is an error. Snoozing an
/// activity with no due date is a no-op error: there is nothing
/// to snooze.
pub fn snooze(activity: &mut CrmActivity, until: DateTime<Utc>, now: DateTime<Utc>) -> Result<()> {
    if activity.is_completed() {
        return Err(CrmError::AlreadyCompleted(activity.id));
    }
    if activity.due_at.is_none() {
        return Err(CrmError::NotAReminder(activity.id));
    }
    if until <= now {
        return Err(CrmError::SnoozeNotInFuture);
    }
    activity.snoozed_until = Some(until);
    Ok(())
}

/// Open reminders due at or before `as_of`, earliest effective
/// due first. Pure function over an in-memory slice; the
/// repository equivalent is
/// `df_db::list_reminders_due_for_store`.
pub fn due_reminders(activities: &[CrmActivity], as_of: DateTime<Utc>) -> Vec<&CrmActivity> {
    let mut due: Vec<&CrmActivity> = activities.iter().filter(|a| is_due(a, as_of)).collect();
    due.sort_by_key(|a| effective_due(a));
    due
}

/// Open reminders strictly past their effective due time at
/// `as_of`, most overdue first.
pub fn overdue_reminders(activities: &[CrmActivity], as_of: DateTime<Utc>) -> Vec<&CrmActivity> {
    let mut overdue: Vec<&CrmActivity> =
        activities.iter().filter(|a| is_overdue(a, as_of)).collect();
    overdue.sort_by_key(|a| effective_due(a));
    overdue
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::ActivityType;
    use chrono::TimeDelta;
    use uuid::Uuid;

    fn reminder(due_in_hours: i64) -> CrmActivity {
        CrmActivity::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            Some(Uuid::now_v7()),
            ActivityType::FollowUp,
            "Chase the Miller quote",
        )
        .with_due_at(Utc::now() + TimeDelta::hours(due_in_hours))
    }

    #[test]
    fn due_and_overdue_boundaries() {
        let now = Utc::now();
        let mut past = reminder(-3);
        let at_now = CrmActivity::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            None,
            ActivityType::FollowUp,
            "Due right now",
        )
        .with_due_at(now);
        let future = reminder(3);
        let plain_note = CrmActivity::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            None,
            ActivityType::Note,
            "No due date",
        );
        // Due at or before now; overdue only strictly before now.
        assert!(is_due(&past, now));
        assert!(is_overdue(&past, now));
        assert!(is_due(&at_now, now));
        assert!(!is_overdue(&at_now, now));
        assert!(!is_due(&future, now));
        assert!(!is_overdue(&future, now));
        // An activity with no due date is never a reminder.
        assert!(!is_due(&plain_note, now));
        assert_eq!(effective_due(&plain_note), None);
        // Completing removes it from both lists.
        complete(&mut past, now).unwrap();
        assert!(!is_due(&past, now));
        assert!(!is_overdue(&past, now));
        assert_eq!(
            complete(&mut past, now),
            Err(CrmError::AlreadyCompleted(past.id))
        );
        // A note with no due date cannot be snoozed.
        let mut note = plain_note;
        assert_eq!(
            snooze(&mut note, now + TimeDelta::hours(1), now),
            Err(CrmError::NotAReminder(note.id))
        );
    }

    #[test]
    fn snooze_pushes_effective_due_and_cannot_go_backwards() {
        let now = Utc::now();
        let mut r = reminder(-2);
        assert!(is_overdue(&r, now));
        // Snoozing to the past is rejected.
        assert_eq!(
            snooze(&mut r, now - TimeDelta::hours(1), now),
            Err(CrmError::SnoozeNotInFuture)
        );
        // Snooze to tomorrow: no longer due.
        snooze(&mut r, now + TimeDelta::hours(26), now).unwrap();
        assert_eq!(effective_due(&r), Some(now + TimeDelta::hours(26)));
        assert!(!is_due(&r, now));
        // Snoozing to before the original due time does not pull it
        // forward: effective due stays the original due time.
        let mut r2 = CrmActivity::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            None,
            ActivityType::FollowUp,
            "Later",
        )
        .with_due_at(now + TimeDelta::hours(10));
        snooze(&mut r2, now + TimeDelta::hours(1), now).unwrap();
        assert_eq!(effective_due(&r2), Some(now + TimeDelta::hours(10)));
        assert!(!is_due(&r2, now));
        // A completed reminder cannot be snoozed.
        complete(&mut r2, now).unwrap();
        assert_eq!(
            snooze(&mut r2, now + TimeDelta::hours(30), now),
            Err(CrmError::AlreadyCompleted(r2.id))
        );
    }

    #[test]
    fn due_list_is_sorted_earliest_first_and_skips_completed() {
        let now = Utc::now();
        let mut done = reminder(-5);
        complete(&mut done, now).unwrap();
        let list = vec![reminder(2), reminder(-1), done, reminder(-4)];
        let due = due_reminders(&list, now);
        assert_eq!(due.len(), 2);
        assert!(effective_due(due[0]) < effective_due(due[1]));
        let overdue = overdue_reminders(&list, now);
        assert_eq!(overdue.len(), 2);
    }
}
