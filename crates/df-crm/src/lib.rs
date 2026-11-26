//! `df-crm` — CRM domain for decor-pos (Phase 4, v0.5; spec §4
//! `crm_marketing`, §7 Phase 4 row).
//!
//! Pure domain logic over the existing `customers` charge-account
//! rows and the migration `0006_phase4` tables — this crate
//! invents no customer table and talks to no database, no network,
//! and no vendor API. Persistence lives in `df-db` (`crm.rs`,
//! `crm_reports.rs`, migrations `0006_phase4` /
//! `0007_phase4_crm_delivery`); HTTP endpoints and UI are wired
//! by another worker against this public API (the edge wiring
//! layer in `df-edge::{crm, delivery}` currently carries these
//! rules inline and must not grow a second copy of them).
//!
//! ## What is implemented
//!
//! * [`activity`] — [`CrmActivity`](activity::CrmActivity) log
//!   (calls, visits, emails, quotes, notes, follow-ups) against
//!   a customer, with [`follow_ups_due`](activity::follow_ups_due).
//! * [`reminder`] — the reminder tickler over activities with a
//!   due date: [`is_due`](reminder::is_due) /
//!   [`is_overdue`](reminder::is_overdue) at a moment,
//!   [`complete`](reminder::complete),
//!   [`snooze`](reminder::snooze), and the
//!   [`due_reminders`](reminder::due_reminders) /
//!   [`overdue_reminders`](reminder::overdue_reminders) list
//!   helpers. A reminder is an activity with a `due_at` — there
//!   is no separate reminders table.
//! * [`appointment`] — [`Appointment`](appointment::Appointment)
//!   scheduling with per-contractor / per-staff
//!   [`check_overlap`](appointment::check_overlap) (cancelled,
//!   completed, and no-show appointments never block a slot)
//!   and [`appointments_for_day`](appointment::appointments_for_day).
//! * [`saved_view`] — [`CustomerFilter`](saved_view::CustomerFilter)
//!   (serializable, stored as `filter_json`) + evaluator over
//!   [`CustomerSnapshot`](saved_view::CustomerSnapshot), plus
//!   [`SavedView`](saved_view::SavedView) and the activity-log
//!   filter dialect ([`ActivityFilter`](saved_view::ActivityFilter))
//!   the edge's `run_saved_view` uses — the two dialects are
//!   distinct types and must not be confused.
//! * [`lapsed`] — [`lapsed_report`](lapsed::lapsed_report):
//!   contractors with no purchase in N days, with never-purchased
//!   contractors reported as a distinct status, never merged with
//!   lapsed ones.
//!
//! ## Example
//!
//! ```rust
//! use chrono::{TimeDelta, Utc};
//! use df_crm::activity::{ActivityType, CrmActivity};
//! use df_crm::reminder::{complete, due_reminders};
//! use uuid::Uuid;
//!
//! let now = Utc::now();
//! let mut reminders = vec![CrmActivity::new(
//!     Uuid::now_v7(),
//!     Uuid::now_v7(),
//!     Some(Uuid::now_v7()),
//!     ActivityType::FollowUp,
//!     "Chase the Miller quote",
//! )
//! .with_due_at(now - TimeDelta::hours(2))];
//! assert_eq!(due_reminders(&reminders, now).len(), 1);
//! complete(&mut reminders[0], now).unwrap();
//! assert!(due_reminders(&reminders, now).is_empty());
//! ```
//!
//! Money is `rust_decimal::Decimal` throughout — never floats.
//! Benjamin Moore COLORx is deferred post-v1.0 (spec §9) and
//! appears nowhere in this crate.

pub mod activity;
pub mod appointment;
pub mod error;
pub mod lapsed;
pub mod reminder;
pub mod saved_view;

pub use activity::{ActivityType, CrmActivity, follow_ups_due};
pub use appointment::{
    Appointment, AppointmentStatus, appointments_for_contractor, appointments_for_day,
    check_overlap,
};
pub use error::{CrmError, Result};
pub use lapsed::{ContractorPurchaseSummary, LapsedContractor, LapsedStatus, lapsed_report};
pub use reminder::{
    complete, due_reminders, effective_due, is_due, is_overdue, overdue_reminders, snooze,
};
pub use saved_view::{
    ActivityFilter, ActivitySnapshot, CustomerFilter, CustomerSnapshot, SavedView,
};
