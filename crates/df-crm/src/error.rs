//! CRM domain errors.

use thiserror::Error;
use uuid::Uuid;

/// Errors raised by the pure CRM domain rules in this crate.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CrmError {
    /// An appointment ends at or before it starts.
    #[error("appointment ends at or before it starts")]
    InvalidAppointmentWindow,
    /// A new or moved appointment overlaps an existing one for the
    /// same contractor and/or the same staff member. At least one
    /// of `shared_contractor` / `shared_staff` is set.
    #[error("appointment overlaps existing appointment {conflicting_id}")]
    Overlap {
        /// The contractor shared with the conflicting appointment,
        /// if the clash is by contractor.
        shared_contractor: Option<Uuid>,
        /// The staff member shared with the conflicting
        /// appointment, if the clash is by staff.
        shared_staff: Option<Uuid>,
        /// The existing appointment that collides.
        conflicting_id: Uuid,
    },
    /// A reminder was snoozed to a time that is not in the future
    /// relative to the caller's `now`.
    #[error("snooze target must be after now")]
    SnoozeNotInFuture,
    /// A reminder that is already completed cannot be completed or
    /// snoozed again.
    #[error("reminder {0} is already completed")]
    AlreadyCompleted(Uuid),
    /// Snooze / complete was attempted on an activity that carries
    /// no due date — it is not a reminder.
    #[error("activity {0} has no due date and is not a reminder")]
    NotAReminder(Uuid),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, CrmError>;
