//! Timeclock entries: clock-in / clock-out records for one staff
//! member at one store (spec §4 `scheduling_plus`, §5.5
//! `timeclock_entries`).
//!
//! Rules: a staff member has at most one open entry at a time
//! (a second clock-in is refused), and a clock-out needs an open
//! entry (clocking out cold is refused). Worked time is `Decimal`
//! hours (minutes / 60) — never floats. Entries are recorded in
//! UTC; the store's local day is derived by the caller when it
//! aggregates.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Result, SchedulingError};

/// One clock-in / clock-out pair. `clock_out` is `None` while the
/// entry is open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeclockEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub staff_id: Uuid,
    pub clock_in: DateTime<Utc>,
    pub clock_out: Option<DateTime<Utc>>,
    /// Where the punch came from: `"counter"`, `"pwa"`, `"import"`.
    pub source: String,
}

impl TimeclockEntry {
    /// Whether this entry is still open (clocked in, not yet out).
    pub fn is_open(&self) -> bool {
        self.clock_out.is_none()
    }

    /// Worked hours for a closed entry, as `Decimal` (minutes / 60).
    /// `None` while the entry is open.
    pub fn worked_hours(&self) -> Option<Decimal> {
        let out = self.clock_out?;
        if out <= self.clock_in {
            return None;
        }
        let minutes = (out - self.clock_in).num_minutes();
        Some(Decimal::from(minutes) / Decimal::from(60))
    }
}

/// Clock in: refuse when the staff member already has an open
/// entry. Returns the new open entry.
pub fn clock_in(
    org_id: Uuid,
    store_id: Uuid,
    staff_id: Uuid,
    at: DateTime<Utc>,
    source: &str,
    existing: &[TimeclockEntry],
) -> Result<TimeclockEntry> {
    if let Some(open) = existing
        .iter()
        .find(|entry| entry.staff_id == staff_id && entry.is_open())
    {
        return Err(SchedulingError::AlreadyClockedIn(staff_id, open.id));
    }
    Ok(TimeclockEntry {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        staff_id,
        clock_in: at,
        clock_out: None,
        source: source.to_string(),
    })
}

/// Clock out: close the staff member's open entry at `at`. Refuses
/// when there is no open entry, or when `at` is at or before the
/// clock-in. Returns the closed entry.
pub fn clock_out(
    staff_id: Uuid,
    at: DateTime<Utc>,
    existing: &mut [TimeclockEntry],
) -> Result<TimeclockEntry> {
    let open = existing
        .iter_mut()
        .find(|entry| entry.staff_id == staff_id && entry.is_open())
        .ok_or(SchedulingError::NotClockedIn(staff_id))?;
    if at <= open.clock_in {
        return Err(SchedulingError::InvalidClockWindow);
    }
    open.clock_out = Some(at);
    Ok(open.clone())
}

/// Total worked hours across closed entries (open entries count
/// nothing — their hours are not final until clock-out).
pub fn total_worked_hours(entries: &[TimeclockEntry]) -> Decimal {
    entries
        .iter()
        .filter_map(TimeclockEntry::worked_hours)
        .fold(Decimal::ZERO, |acc, hours| acc + hours)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 30, hour, minute, 0).unwrap()
    }

    #[test]
    fn clock_in_then_out_reports_decimal_hours() {
        let staff = Uuid::from_u128(0x5001);
        let mut entries = Vec::new();
        let entry = clock_in(
            Uuid::now_v7(),
            Uuid::now_v7(),
            staff,
            at(8, 0),
            "counter",
            &entries,
        )
        .unwrap();
        entries.push(entry);
        let closed = clock_out(staff, at(16, 30), &mut entries).unwrap();
        assert_eq!(
            closed.worked_hours(),
            Some(Decimal::from_str_exact("8.5").unwrap())
        );
    }

    #[test]
    fn second_clock_in_is_refused() {
        let staff = Uuid::from_u128(0x5001);
        let first = clock_in(
            Uuid::now_v7(),
            Uuid::now_v7(),
            staff,
            at(8, 0),
            "counter",
            &[],
        )
        .unwrap();
        let err = clock_in(
            Uuid::now_v7(),
            Uuid::now_v7(),
            staff,
            at(9, 0),
            "counter",
            &[first],
        );
        assert!(matches!(err, Err(SchedulingError::AlreadyClockedIn(_, _))));
    }

    #[test]
    fn clock_out_without_clock_in_is_refused() {
        let staff = Uuid::from_u128(0x5001);
        let mut entries = Vec::new();
        assert_eq!(
            clock_out(staff, at(16, 0), &mut entries),
            Err(SchedulingError::NotClockedIn(staff))
        );
    }

    #[test]
    fn clock_out_before_clock_in_is_refused() {
        let staff = Uuid::from_u128(0x5001);
        let mut entries = vec![
            clock_in(
                Uuid::now_v7(),
                Uuid::now_v7(),
                staff,
                at(8, 0),
                "counter",
                &[],
            )
            .unwrap(),
        ];
        assert_eq!(
            clock_out(staff, at(7, 59), &mut entries),
            Err(SchedulingError::InvalidClockWindow)
        );
        // The entry is still open after the refused close.
        assert!(entries[0].is_open());
    }

    #[test]
    fn open_entries_count_no_hours() {
        let staff = Uuid::from_u128(0x5001);
        let open = clock_in(
            Uuid::now_v7(),
            Uuid::now_v7(),
            staff,
            at(8, 0),
            "counter",
            &[],
        )
        .unwrap();
        assert_eq!(total_worked_hours(&[open]), Decimal::ZERO);
    }
}
