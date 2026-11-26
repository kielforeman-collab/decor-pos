//! Concrete staff shifts: one staff member working one span on
//! one date (spec §4 `scheduling_plus`, §5.5 `staff_shifts`).
//!
//! A shift is minted from a [`ShiftTemplate`](crate::shift_template::ShiftTemplate)
//! or created ad hoc. Two shifts for the same staff member whose
//! spans overlap are refused by [`check_overlap`]; cancelled
//! shifts never block a slot. Hours are `Decimal` (minutes / 60)
//! — never floats.

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Result, SchedulingError};

/// Lifecycle of a concrete shift. Only `scheduled` and
/// `confirmed` shifts count toward scheduled hours; `cancelled`
/// shifts are kept for the audit trail and never block overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShiftStatus {
    Scheduled,
    Confirmed,
    Cancelled,
}

impl ShiftStatus {
    /// The stored status string.
    pub fn as_str(self) -> &'static str {
        match self {
            ShiftStatus::Scheduled => "scheduled",
            ShiftStatus::Confirmed => "confirmed",
            ShiftStatus::Cancelled => "cancelled",
        }
    }

    /// Whether the shift counts toward scheduled hours and blocks
    /// overlap for its staff member.
    pub fn is_active(self) -> bool {
        !matches!(self, ShiftStatus::Cancelled)
    }
}

impl std::str::FromStr for ShiftStatus {
    type Err = SchedulingError;

    /// Parse the stored status string (`"scheduled"`,
    /// `"confirmed"`, `"cancelled"`).
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "scheduled" => Ok(ShiftStatus::Scheduled),
            "confirmed" => Ok(ShiftStatus::Confirmed),
            "cancelled" => Ok(ShiftStatus::Cancelled),
            other => Err(SchedulingError::UnknownShiftStatus(other.to_string())),
        }
    }
}

/// One concrete shift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaffShift {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    /// Template this shift was minted from, if any.
    pub template_id: Option<Uuid>,
    pub staff_id: Uuid,
    /// Local calendar date the shift belongs to.
    pub shift_date: NaiveDate,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: ShiftStatus,
    pub notes: Option<String>,
}

impl StaffShift {
    /// Validate the window: end must be after start.
    pub fn validate(&self) -> Result<()> {
        if self.ends_at <= self.starts_at {
            return Err(SchedulingError::InvalidShiftWindow);
        }
        Ok(())
    }

    /// Length of the shift in hours, as `Decimal` (minutes / 60,
    /// scale 4) — never a float.
    pub fn duration_hours(&self) -> Decimal {
        let minutes = (self.ends_at - self.starts_at).num_minutes();
        Decimal::from(minutes) / Decimal::from(60)
    }
}

/// Refuse a shift that overlaps an existing *active* shift for the
/// same staff member. Cancelled shifts never block. Returns the
/// conflicting shift's id on overlap.
pub fn check_overlap(candidate: &StaffShift, existing: &[StaffShift]) -> Result<()> {
    candidate.validate()?;
    for other in existing {
        if other.id == candidate.id || other.staff_id != candidate.staff_id {
            continue;
        }
        if !other.status.is_active() {
            continue;
        }
        if candidate.starts_at < other.ends_at && other.starts_at < candidate.ends_at {
            return Err(SchedulingError::ShiftOverlap {
                staff_id: candidate.staff_id,
                conflicting_id: other.id,
            });
        }
    }
    Ok(())
}

/// Total scheduled hours for a set of shifts (active ones only),
/// as `Decimal`.
pub fn scheduled_hours(shifts: &[StaffShift]) -> Decimal {
    shifts
        .iter()
        .filter(|shift| shift.status.is_active())
        .map(StaffShift::duration_hours)
        .fold(Decimal::ZERO, |acc, hours| acc + hours)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn shift(id: u128, staff: u128, start_hour: u32, end_hour: u32) -> StaffShift {
        let day = NaiveDate::from_ymd_opt(2026, 11, 30).unwrap();
        StaffShift {
            id: Uuid::from_u128(id),
            org_id: Uuid::now_v7(),
            store_id: Uuid::now_v7(),
            template_id: None,
            staff_id: Uuid::from_u128(staff),
            shift_date: day,
            starts_at: Utc
                .with_ymd_and_hms(2026, 11, 30, start_hour, 0, 0)
                .unwrap(),
            ends_at: Utc.with_ymd_and_hms(2026, 11, 30, end_hour, 0, 0).unwrap(),
            status: ShiftStatus::Scheduled,
            notes: None,
        }
    }

    #[test]
    fn duration_is_decimal_hours() {
        assert_eq!(
            shift(1, 1, 8, 16).duration_hours(),
            Decimal::from_str_exact("8").unwrap()
        );
        assert_eq!(
            shift(1, 1, 8, 12).duration_hours(),
            Decimal::from_str_exact("4").unwrap()
        );
    }

    #[test]
    fn end_before_start_is_rejected() {
        let bad = shift(1, 1, 16, 8);
        assert_eq!(bad.validate(), Err(SchedulingError::InvalidShiftWindow));
    }

    #[test]
    fn same_staff_overlap_is_refused() {
        let existing = vec![shift(1, 0x5001, 8, 16)];
        let clash = shift(2, 0x5001, 12, 20);
        assert!(matches!(
            check_overlap(&clash, &existing),
            Err(SchedulingError::ShiftOverlap { .. })
        ));
    }

    #[test]
    fn adjacent_shifts_do_not_overlap() {
        let existing = vec![shift(1, 0x5001, 8, 16)];
        let next = shift(2, 0x5001, 16, 20);
        assert!(check_overlap(&next, &existing).is_ok());
    }

    #[test]
    fn different_staff_may_overlap() {
        let existing = vec![shift(1, 0x5001, 8, 16)];
        let other = shift(2, 0x5002, 8, 16);
        assert!(check_overlap(&other, &existing).is_ok());
    }

    #[test]
    fn cancelled_shifts_never_block() {
        let mut existing = shift(1, 0x5001, 8, 16);
        existing.status = ShiftStatus::Cancelled;
        let clash = shift(2, 0x5001, 12, 20);
        assert!(check_overlap(&clash, &[existing]).is_ok());
    }

    #[test]
    fn scheduled_hours_counts_active_only() {
        let mut cancelled = shift(3, 0x5001, 8, 10);
        cancelled.status = ShiftStatus::Cancelled;
        let hours = scheduled_hours(&[shift(1, 0x5001, 8, 16), shift(2, 0x5002, 9, 13), cancelled]);
        assert_eq!(hours, Decimal::from_str_exact("12").unwrap());
    }
}
