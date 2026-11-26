//! Coverage check: scheduled hours vs forecast required hours for
//! one date (spec §4 `scheduling_plus`).
//!
//! A day is **adequate** when scheduled hours are within ±10% of
//! the forecast (the tolerance is a named constant so a store can
//! reason about it, not a magic number buried in the UI).
//! Below that it is **under**-covered, above it **over**-covered.
//! When no forecast exists (no history — see
//! [`crate::forecast::build_forecast`]) the check reports
//! [`CoverageStatus::Unknown`] with the scheduled hours it could
//! still measure, rather than failing the whole report.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// ±10% tolerance around the forecast.
pub const COVERAGE_TOLERANCE_PCT: Decimal = Decimal::from_parts(10, 0, 0, false, 0);

/// Whether a day's scheduled hours cover its forecast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    /// Scheduled hours within ±10% of forecast.
    Adequate,
    /// Scheduled hours more than 10% below forecast.
    Under,
    /// Scheduled hours more than 10% above forecast.
    Over,
    /// No forecast (insufficient history) — nothing to compare
    /// against.
    Unknown,
}

/// The coverage verdict for one date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageCheck {
    pub date: chrono::NaiveDate,
    /// Forecast required hours, when history supports one.
    pub forecast_hours: Option<Decimal>,
    /// Scheduled (active-shift) hours for the date.
    pub scheduled_hours: Decimal,
    /// `scheduled − forecast`, when a forecast exists.
    pub delta_hours: Option<Decimal>,
    pub status: CoverageStatus,
}

/// Compare scheduled hours against a forecast.
///
/// `forecast_hours` is `None` when no forecast exists — the check
/// then reports `Unknown` instead of guessing.
pub fn coverage_check(
    date: chrono::NaiveDate,
    forecast_hours: Option<Decimal>,
    scheduled_hours: Decimal,
) -> CoverageCheck {
    let Some(forecast) = forecast_hours else {
        return CoverageCheck {
            date,
            forecast_hours: None,
            scheduled_hours,
            delta_hours: None,
            status: CoverageStatus::Unknown,
        };
    };
    let delta = scheduled_hours - forecast;
    let tolerance = forecast * COVERAGE_TOLERANCE_PCT / Decimal::from(100);
    let status = if delta < -tolerance {
        CoverageStatus::Under
    } else if delta > tolerance {
        CoverageStatus::Over
    } else {
        CoverageStatus::Adequate
    };
    CoverageCheck {
        date,
        forecast_hours: Some(forecast),
        scheduled_hours,
        delta_hours: Some(delta),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str_exact(s).unwrap()
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()
    }

    #[test]
    fn exact_match_is_adequate() {
        let check = coverage_check(date(), Some(dec("8")), dec("8"));
        assert_eq!(check.status, CoverageStatus::Adequate);
        assert_eq!(check.delta_hours, Some(dec("0")));
    }

    #[test]
    fn within_ten_percent_is_adequate() {
        // 8.8 scheduled vs 8.0 forecast: +10% exactly — adequate.
        let check = coverage_check(date(), Some(dec("8")), dec("8.8"));
        assert_eq!(check.status, CoverageStatus::Adequate);
        // 7.2 scheduled vs 8.0 forecast: −10% exactly — adequate.
        let check = coverage_check(date(), Some(dec("8")), dec("7.2"));
        assert_eq!(check.status, CoverageStatus::Adequate);
    }

    #[test]
    fn below_tolerance_is_under() {
        let check = coverage_check(date(), Some(dec("8")), dec("7.19"));
        assert_eq!(check.status, CoverageStatus::Under);
        assert_eq!(check.delta_hours, Some(dec("-0.81")));
    }

    #[test]
    fn above_tolerance_is_over() {
        let check = coverage_check(date(), Some(dec("8")), dec("8.81"));
        assert_eq!(check.status, CoverageStatus::Over);
        assert_eq!(check.delta_hours, Some(dec("0.81")));
    }

    #[test]
    fn no_forecast_is_unknown_but_still_reports_hours() {
        let check = coverage_check(date(), None, dec("6"));
        assert_eq!(check.status, CoverageStatus::Unknown);
        assert_eq!(check.forecast_hours, None);
        assert_eq!(check.delta_hours, None);
        assert_eq!(check.scheduled_hours, dec("6"));
    }
}
