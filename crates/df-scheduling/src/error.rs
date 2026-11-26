//! Scheduling+ domain errors.

use thiserror::Error;
use uuid::Uuid;

/// Errors raised by the pure scheduling domain rules in this crate.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchedulingError {
    /// A shift template's end time is at or before its start time.
    #[error("shift template ends at or before it starts ({starts}–{ends})")]
    InvalidTemplateWindow { starts: String, ends: String },
    /// A weekday outside 0 (Sunday) ..= 6 (Saturday).
    #[error("weekday {0} is out of range (0=Sunday..6=Saturday)")]
    InvalidWeekday(u8),
    /// A shift template requires at least one staff member.
    #[error("shift template required_count must be at least 1")]
    InvalidRequiredCount,
    /// A stored shift status string is not one of
    /// `scheduled` / `confirmed` / `cancelled`.
    #[error("unknown shift status {0:?}")]
    UnknownShiftStatus(String),
    /// A shift ends at or before it starts.
    #[error("shift ends at or before it starts")]
    InvalidShiftWindow,
    /// A new or moved shift overlaps an existing shift for the same
    /// staff member.
    #[error("shift overlaps existing shift {conflicting_id} for staff {staff_id}")]
    ShiftOverlap {
        staff_id: Uuid,
        conflicting_id: Uuid,
    },
    /// A clock-in was attempted while the staff member already has
    /// an open (unclosed) timeclock entry.
    #[error("staff {0} already has an open timeclock entry {1}")]
    AlreadyClockedIn(Uuid, Uuid),
    /// A clock-out was attempted with no open timeclock entry.
    #[error("staff {0} has no open timeclock entry to close")]
    NotClockedIn(Uuid),
    /// A clock-out time is at or before its clock-in time.
    #[error("clock-out is at or before clock-in")]
    InvalidClockWindow,
    /// A forecast cannot be built: there is no usable history
    /// (no completed sales, or no recorded labor hours, in the
    /// window) and no explicit sales-per-labor-hour ratio was given.
    #[error("insufficient history to forecast: {0}")]
    InsufficientHistory(String),
    /// The sales-per-labor-hour ratio must be positive.
    #[error("sales per labor hour must be positive, got {0}")]
    InvalidRatio(rust_decimal::Decimal),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, SchedulingError>;
