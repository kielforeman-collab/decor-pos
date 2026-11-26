//! Shift templates: the recurring weekly pattern a store schedules
//! from (spec §4 `scheduling_plus`).
//!
//! A template says "every Tuesday, 08:00–16:00, one cashier" —
//! concrete [`StaffShift`](crate::shift::StaffShift) rows are
//! minted from it (or created ad hoc). Times are wall-clock
//! `"HH:MM"` strings in the store's local time; the template
//! carries no timezone itself, so the edge resolves it against
//! the store row when it instantiates a shift.

use chrono::NaiveTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Result, SchedulingError};

/// Weekday numbering: 0 = Sunday ..= 6 = Saturday (matches
/// `chrono::Weekday::num_days_from_sunday`).
pub fn validate_weekday(weekday: u8) -> Result<()> {
    if weekday > 6 {
        return Err(SchedulingError::InvalidWeekday(weekday));
    }
    Ok(())
}

/// Parse an `"HH:MM"` wall-clock time.
pub fn parse_time_of_day(value: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").map_err(|_| SchedulingError::InvalidTemplateWindow {
        starts: value.to_string(),
        ends: String::new(),
    })
}

/// A recurring weekly shift pattern for one store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShiftTemplate {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub name: String,
    /// 0 = Sunday ..= 6 = Saturday.
    pub weekday: u8,
    /// Wall-clock start, `"HH:MM"`.
    pub starts_at: String,
    /// Wall-clock end, `"HH:MM"` — must be after `starts_at`
    /// (overnight templates are not supported; split them into two
    /// templates or create ad-hoc shifts).
    pub ends_at: String,
    /// Role the template staffs, e.g. `"cashier"`, `"floor"`.
    pub role: Option<String>,
    /// When set, the template is pinned to one staff member;
    /// otherwise any staff member may take the instantiated shift.
    pub staff_id: Option<Uuid>,
    /// How many staff the template calls for on that weekday.
    pub required_count: u32,
    pub is_active: bool,
}

impl ShiftTemplate {
    /// Validate the template: weekday in range, both times parse,
    /// end after start, at least one person required.
    pub fn validate(&self) -> Result<()> {
        validate_weekday(self.weekday)?;
        let starts = parse_time_of_day(&self.starts_at)?;
        let ends = parse_time_of_day(&self.ends_at)?;
        if ends <= starts {
            return Err(SchedulingError::InvalidTemplateWindow {
                starts: self.starts_at.clone(),
                ends: self.ends_at.clone(),
            });
        }
        if self.required_count == 0 {
            return Err(SchedulingError::InvalidRequiredCount);
        }
        Ok(())
    }

    /// Length of one instantiated shift, in minutes.
    pub fn duration_minutes(&self) -> Result<i64> {
        let starts = parse_time_of_day(&self.starts_at)?;
        let ends = parse_time_of_day(&self.ends_at)?;
        Ok((ends - starts).num_minutes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> ShiftTemplate {
        ShiftTemplate {
            id: Uuid::now_v7(),
            org_id: Uuid::now_v7(),
            store_id: Uuid::now_v7(),
            name: "Tuesday opener".to_string(),
            weekday: 2,
            starts_at: "08:00".to_string(),
            ends_at: "16:00".to_string(),
            role: Some("cashier".to_string()),
            staff_id: None,
            required_count: 1,
            is_active: true,
        }
    }

    #[test]
    fn valid_template_passes_and_reports_duration() {
        let template = template();
        assert!(template.validate().is_ok());
        assert_eq!(template.duration_minutes().unwrap(), 480);
    }

    #[test]
    fn end_before_start_is_rejected() {
        let mut template = template();
        template.ends_at = "07:59".to_string();
        assert!(matches!(
            template.validate(),
            Err(SchedulingError::InvalidTemplateWindow { .. })
        ));
    }

    #[test]
    fn equal_start_and_end_is_rejected() {
        let mut template = template();
        template.ends_at = "08:00".to_string();
        assert!(template.validate().is_err());
    }

    #[test]
    fn weekday_out_of_range_is_rejected() {
        let mut template = template();
        template.weekday = 7;
        assert_eq!(template.validate(), Err(SchedulingError::InvalidWeekday(7)));
    }

    #[test]
    fn zero_required_count_is_rejected() {
        let mut template = template();
        template.required_count = 0;
        assert!(template.validate().is_err());
    }
}
