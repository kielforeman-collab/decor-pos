//! Appointments (spec §4 `crm_marketing`, §7 Phase 4;
//! migration `0006_phase4` `appointments`).
//!
//! A scheduled visit / measure / install slot for one contractor
//! (`customers.id`) and/or one staff member. Overlap is checked
//! against other *scheduled* appointments for the **same
//! contractor or the same staff member** — a contractor cannot be
//! in two places at once, and neither can a staff member.
//! Cancelled, completed, and no-show appointments never block a
//! slot. Adjacent appointments (one ends exactly when the next
//! starts) do not overlap; a zero-length or backwards window is
//! rejected at construction (and by the table CHECK).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CrmError, Result};

/// Appointment lifecycle status. The storage strings match the
/// `appointments.status` values in migration `0006_phase4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppointmentStatus {
    /// Booked and still to happen — the only status that blocks
    /// a slot in [`check_overlap`].
    Scheduled,
    /// Happened.
    Completed,
    /// Called off.
    Cancelled,
    /// Booked but nobody showed.
    NoShow,
}

impl AppointmentStatus {
    /// Stable storage string (used as the `status` column).
    pub fn as_str(self) -> &'static str {
        match self {
            AppointmentStatus::Scheduled => "scheduled",
            AppointmentStatus::Completed => "completed",
            AppointmentStatus::Cancelled => "cancelled",
            AppointmentStatus::NoShow => "no_show",
        }
    }
    /// Parse a storage string back.
    ///
    /// Deliberately an inherent method rather than `FromStr`
    /// (see `ActivityType::from_str`).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "scheduled" => Some(AppointmentStatus::Scheduled),
            "completed" => Some(AppointmentStatus::Completed),
            "cancelled" => Some(AppointmentStatus::Cancelled),
            "no_show" => Some(AppointmentStatus::NoShow),
            _ => None,
        }
    }
}

/// One row of `appointments` (migration `0006_phase4`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appointment {
    /// UUIDv7 primary key.
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    /// The contractor this appointment is for (`customers.id`),
    /// if it is for one.
    pub customer_id: Option<Uuid>,
    /// The staff member holding the slot, if one is assigned.
    pub staff_id: Option<Uuid>,
    /// Short title, e.g. "Measure — Miller kitchen".
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: AppointmentStatus,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Appointment {
    /// Build a new `Scheduled` appointment with a fresh UUIDv7
    /// id. Fails when `ends_at` is at or before `starts_at`.
    pub fn new(
        org_id: Uuid,
        store_id: Uuid,
        customer_id: Option<Uuid>,
        staff_id: Option<Uuid>,
        title: impl Into<String>,
        starts_at: DateTime<Utc>,
        ends_at: DateTime<Utc>,
    ) -> Result<Self> {
        if ends_at <= starts_at {
            return Err(CrmError::InvalidAppointmentWindow);
        }
        Ok(Self {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            customer_id,
            staff_id,
            title: title.into(),
            starts_at,
            ends_at,
            status: AppointmentStatus::Scheduled,
            notes: None,
            created_at: Utc::now(),
        })
    }

    /// Attach free-text notes.
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }

    /// True when this appointment's window intersects `other`'s.
    /// Adjacent windows (end == start) do not intersect.
    pub fn overlaps(&self, other: &Appointment) -> bool {
        self.starts_at < other.ends_at && other.starts_at < self.ends_at
    }

    /// True when `other` is a *scheduled* appointment that shares
    /// this one's contractor or staff member — i.e. a real
    /// double-booking conflict. Also reports *which* side collided.
    fn conflicts_with(&self, other: &Appointment) -> Option<(Option<Uuid>, Option<Uuid>)> {
        if other.id == self.id || other.status != AppointmentStatus::Scheduled {
            return None;
        }
        let same_contractor = match (self.customer_id, other.customer_id) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        };
        let same_staff = match (self.staff_id, other.staff_id) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        };
        if (same_contractor.is_some() || same_staff.is_some()) && self.overlaps(other) {
            Some((same_contractor, same_staff))
        } else {
            None
        }
    }
}

/// Check a candidate appointment against the existing ones.
/// Returns the first conflicting appointment, or `Ok(None)` when
/// the slot is free. Only *scheduled* appointments for the same
/// contractor or the same staff member are considered; the
/// candidate itself (same id) is skipped so a re-check after an
/// edit does not collide with its own stored row.
pub fn check_overlap<'a>(
    existing: &'a [Appointment],
    candidate: &Appointment,
) -> Result<Option<&'a Appointment>> {
    for other in existing {
        if let Some((shared_contractor, shared_staff)) = candidate.conflicts_with(other) {
            return Err(CrmError::Overlap {
                shared_contractor,
                shared_staff,
                conflicting_id: other.id,
            });
        }
    }
    Ok(None)
}

/// Appointments in `list` that start on `day` (UTC calendar day),
/// earliest start first. Pure function; the repository equivalent
/// is `df_db::list_appointments_for_store` + this filter.
pub fn appointments_for_day(list: &[Appointment], day: chrono::NaiveDate) -> Vec<&Appointment> {
    let mut out: Vec<&Appointment> = list
        .iter()
        .filter(|a| a.starts_at.date_naive() == day)
        .collect();
    out.sort_by_key(|a| a.starts_at);
    out
}

/// Appointments in `list` for one contractor (`customer_id`),
/// earliest start first.
pub fn appointments_for_contractor(list: &[Appointment], customer_id: Uuid) -> Vec<&Appointment> {
    let mut out: Vec<&Appointment> = list
        .iter()
        .filter(|a| a.customer_id == Some(customer_id))
        .collect();
    out.sort_by_key(|a| a.starts_at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    fn appt(
        customer: Option<Uuid>,
        staff: Option<Uuid>,
        start_hour: i64,
        end_hour: i64,
    ) -> Appointment {
        let base = Utc::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        Appointment::new(
            Uuid::now_v7(),
            Uuid::now_v7(),
            customer,
            staff,
            "Measure",
            base + TimeDelta::hours(start_hour),
            base + TimeDelta::hours(end_hour),
        )
        .unwrap()
    }

    #[test]
    fn invalid_window_rejected() {
        let now = Utc::now();
        assert_eq!(
            Appointment::new(Uuid::now_v7(), Uuid::now_v7(), None, None, "x", now, now),
            Err(CrmError::InvalidAppointmentWindow)
        );
        assert_eq!(
            Appointment::new(
                Uuid::now_v7(),
                Uuid::now_v7(),
                None,
                None,
                "x",
                now,
                now - TimeDelta::hours(1)
            ),
            Err(CrmError::InvalidAppointmentWindow)
        );
    }

    #[test]
    fn overlap_same_contractor_or_staff_only_and_adjacent_is_free() {
        let contractor = Uuid::now_v7();
        let staff = Uuid::now_v7();
        let existing = vec![
            appt(Some(contractor), Some(staff), 9, 11),
            appt(Some(Uuid::now_v7()), Some(Uuid::now_v7()), 10, 12),
        ];
        // Overlapping the same contractor's 09:00–11:00.
        let clash = appt(Some(contractor), Some(Uuid::now_v7()), 10, 12);
        let err = check_overlap(&existing, &clash).unwrap_err();
        assert_eq!(
            err,
            CrmError::Overlap {
                shared_contractor: Some(contractor),
                shared_staff: None,
                conflicting_id: existing[0].id,
            }
        );
        // Same clock time, different contractor AND different staff: free.
        let free_other = appt(Some(Uuid::now_v7()), Some(Uuid::now_v7()), 9, 10);
        assert!(check_overlap(&existing, &free_other).unwrap().is_none());
        // Same staff, different contractor: still a conflict.
        let staff_clash = appt(Some(Uuid::now_v7()), Some(staff), 10, 12);
        assert!(check_overlap(&existing, &staff_clash).is_err());
        // Adjacent to the same contractor's slot: free.
        let adjacent = appt(Some(contractor), Some(staff), 11, 12);
        assert!(check_overlap(&existing, &adjacent).unwrap().is_none());
        // A cancelled appointment does not block.
        let mut cancelled = existing[0].clone();
        cancelled.status = AppointmentStatus::Cancelled;
        let list = vec![cancelled, existing[1].clone()];
        assert!(check_overlap(&list, &clash).unwrap().is_none());
        // A re-check of the stored row itself does not self-collide.
        assert!(check_overlap(&existing, &existing[0]).unwrap().is_none());
    }

    #[test]
    fn appointments_for_day_filters_and_sorts() {
        let day = Utc::now().date_naive();
        let tomorrow = day + chrono::TimeDelta::days(1);
        let a = appt(None, None, 14, 15);
        let b = appt(None, None, 9, 10);
        let mut c = appt(None, None, 10, 11);
        c.starts_at = tomorrow.and_hms_opt(10, 0, 0).unwrap().and_utc();
        c.ends_at = tomorrow.and_hms_opt(11, 0, 0).unwrap().and_utc();
        let list = vec![a.clone(), c, b.clone()];
        let today = appointments_for_day(&list, day);
        assert_eq!(today.len(), 2);
        assert_eq!(today[0].id, b.id);
        assert_eq!(today[1].id, a.id);
    }
}
