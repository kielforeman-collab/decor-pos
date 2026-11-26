//! `df-scheduling` — Scheduling+ domain for decor-pos (Phase 6,
//! v1.0; spec §4 `scheduling_plus`, §7 Phase 6 row).
//!
//! Pure domain logic over the migration `0011_phase6` tables —
//! this crate invents no persistence and talks to no database, no
//! network, and no vendor API. Persistence lives in `df-db`
//! (`scheduling.rs`); HTTP endpoints and UI are wired by the edge
//! (`df-edge::scheduling`), which delegates to this public API and
//! must not grow a second copy of these rules.
//!
//! ## What is implemented
//!
//! * [`shift_template`] — [`ShiftTemplate`](shift_template::ShiftTemplate):
//!   a recurring weekly pattern ("every Tuesday 08:00–16:00, one
//!   cashier"), with weekday/time validation. Overnight templates
//!   are not supported — split them or create ad-hoc shifts.
//! * [`shift`] — [`StaffShift`](shift::StaffShift): one concrete
//!   shift, with [`check_overlap`](shift::check_overlap) (same
//!   staff, colliding hours; cancelled shifts never block) and
//!   [`scheduled_hours`](shift::scheduled_hours).
//! * [`timeclock`] — [`TimeclockEntry`](timeclock::TimeclockEntry):
//!   clock-in refuses a second open entry, clock-out refuses with
//!   no open entry; worked hours are `Decimal` (minutes / 60).
//! * [`forecast`] — sales-driven labor forecasting:
//!   [`sales_per_labor_hour`](forecast::sales_per_labor_hour) from
//!   history (or a caller override), per-day-of-week averages with
//!   an overall-average fallback, per-hour buckets, and required
//!   hours = forecast sales / ratio. Refuses to run with no sales
//!   or no labor history rather than inventing a number.
//! * [`coverage`] — [`coverage_check`](coverage::coverage_check):
//!   scheduled vs forecast hours, adequate within ±10%,
//!   under/over outside it, `Unknown` when no forecast exists.
//!
//! ## Example
//!
//! ```rust
//! use chrono::NaiveDate;
//! use df_scheduling::coverage::{CoverageStatus, coverage_check};
//! use rust_decimal::Decimal;
//!
//! // Forecast says Monday needs 5.00 labor hours; 5.00 scheduled
//! // is adequate (within ±10%).
//! let check = coverage_check(
//!     NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(),
//!     Some(Decimal::from_str_exact("5").unwrap()),
//!     Decimal::from_str_exact("5").unwrap(),
//! );
//! assert_eq!(check.status, CoverageStatus::Adequate);
//! ```
//!
//! Money and hours are `rust_decimal::Decimal` throughout — never
//! floats. Benjamin Moore COLORx is deferred post-v1.0 (spec §9)
//! and appears nowhere in this crate; tinting is formula tracking
//! only and this crate never touches a tint machine.

pub mod coverage;
pub mod error;
pub mod forecast;
pub mod shift;
pub mod shift_template;
pub mod timeclock;

pub use coverage::{COVERAGE_TOLERANCE_PCT, CoverageCheck, CoverageStatus, coverage_check};
pub use error::{Result, SchedulingError};
pub use forecast::{
    DayForecast, ForecastReport, HourBucket, LaborPoint, SalePoint, build_forecast,
    sales_per_labor_hour,
};
pub use shift::{ShiftStatus, StaffShift, check_overlap, scheduled_hours};
pub use shift_template::{ShiftTemplate, parse_time_of_day, validate_weekday};
pub use timeclock::{TimeclockEntry, clock_in, clock_out, total_worked_hours};
