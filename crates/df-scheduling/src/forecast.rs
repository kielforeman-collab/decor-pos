//! Sales-driven labor forecasting (spec §4 `scheduling_plus`:
//! "labor-hour forecasting from historical POS transaction
//! volume").
//!
//! The model is deliberately small and auditable:
//!
//! 1. **Sales per labor hour** — total completed sales in the
//!    history window divided by total worked (timeclock) hours in
//!    the same window. The caller may override the ratio (e.g. a
//!    manager's planning target); otherwise it is derived from
//!    history and the forecast refuses to run when history has no
//!    sales or no labor hours, rather than inventing a number.
//! 2. **Per-day-of-week averages** — for a target date, average
//!    daily sales and transaction counts over history days with
//!    the same weekday (Mondays forecast from Mondays). A weekday
//!    with no history falls back to the overall daily average.
//! 3. **Per-hour buckets** — the same averaging per hour of day
//!    (UTC hour of the completed sale), so the forecast shows
//!    *when* the labor is needed, not just how much.
//! 4. **Required hours** — forecast sales / sales-per-labor-hour,
//!    rounded to 2 decimal places.
//!
//! All money and hours are `rust_decimal::Decimal` — never floats.
//! History is passed in as plain points, so this module needs no
//! database: the edge (or a job) reads transactions + timeclock
//! entries and hands them over.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Result, SchedulingError};

/// One completed sale in the history window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SalePoint {
    /// When the sale completed (UTC).
    pub at: DateTime<Utc>,
    /// Grand total of the sale.
    pub amount: Decimal,
}

/// One closed timeclock entry's worked time in the history window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LaborPoint {
    /// When the clock-in happened (UTC) — used only to bound the
    /// window; hours are the worked time.
    pub at: DateTime<Utc>,
    /// Worked hours, as `Decimal`.
    pub hours: Decimal,
}

/// Sales per labor hour: total sales / total labor hours. Both
/// inputs must be positive.
pub fn sales_per_labor_hour(total_sales: Decimal, total_labor_hours: Decimal) -> Result<Decimal> {
    if total_sales <= Decimal::ZERO {
        return Err(SchedulingError::InsufficientHistory(
            "no completed sales in the history window".to_string(),
        ));
    }
    if total_labor_hours <= Decimal::ZERO {
        return Err(SchedulingError::InsufficientHistory(
            "no worked timeclock hours in the history window".to_string(),
        ));
    }
    Ok(total_sales / total_labor_hours)
}

/// Forecast for one hour of the day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HourBucket {
    /// Hour of day, 0–23 (UTC).
    pub hour: u8,
    /// Average transactions in this hour, over same-weekday
    /// history days (2dp).
    pub avg_txns: Decimal,
    /// Average sales in this hour, over same-weekday history days
    /// (2dp).
    pub avg_sales: Decimal,
    /// Labor hours this bucket calls for (2dp).
    pub required_hours: Decimal,
}

/// Forecast for one target date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayForecast {
    pub date: NaiveDate,
    /// 0 = Sunday ..= 6 = Saturday.
    pub weekday: u8,
    /// Average transactions for this weekday (2dp).
    pub forecast_txns: Decimal,
    /// Average sales for this weekday (2dp).
    pub forecast_sales: Decimal,
    /// Labor hours the day calls for (2dp).
    pub required_hours: Decimal,
    /// Per-hour buckets with activity (same-weekday average).
    pub hourly: Vec<HourBucket>,
}

/// A labor forecast for a store over a set of target dates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForecastReport {
    pub store_id: Uuid,
    pub generated_at: DateTime<Utc>,
    /// Distinct history days the averages rest on.
    pub history_days: u32,
    /// Completed sales in the history window.
    pub history_txns: u32,
    /// Total completed sales in the history window.
    pub history_sales: Decimal,
    /// Total worked timeclock hours in the history window.
    pub history_labor_hours: Decimal,
    /// Sales per labor hour the forecast used (derived from
    /// history, or the caller's override).
    pub sales_per_labor_hour: Decimal,
    /// Whether the ratio came from history or an override.
    pub ratio_from_history: bool,
    pub days: Vec<DayForecast>,
}

fn round2(value: Decimal) -> Decimal {
    // round_dp alone does not force the scale (1000 stays "1000"
    // instead of "1000.00"), so rescale after rounding: the JSON
    // form carries the 2dp the UI and the job summary display.
    let mut rounded = value.round_dp(2);
    rounded.rescale(2);
    rounded
}

/// Build a forecast for `targets` from `sales` + `labor` history.
///
/// * `sales` — completed sales in the window (any status other
///   than completed must be filtered out by the caller).
/// * `labor` — closed timeclock entries in the same window.
/// * `ratio_override` — when `Some`, the ratio is taken from the
///   caller instead of history (must be positive); history is
///   still needed for the per-weekday averages.
pub fn build_forecast(
    store_id: Uuid,
    sales: &[SalePoint],
    labor: &[LaborPoint],
    targets: &[NaiveDate],
    ratio_override: Option<Decimal>,
) -> Result<ForecastReport> {
    if sales.is_empty() {
        return Err(SchedulingError::InsufficientHistory(
            "no completed sales in the history window".to_string(),
        ));
    }
    // Group sales by UTC calendar day, and count distinct days.
    let mut by_day: BTreeMap<NaiveDate, Vec<&SalePoint>> = BTreeMap::new();
    for sale in sales {
        by_day.entry(sale.at.date_naive()).or_default().push(sale);
    }
    let history_days = by_day.len() as u32;
    let history_sales: Decimal = sales.iter().map(|sale| sale.amount).sum();
    let history_labor_hours: Decimal = labor.iter().map(|point| point.hours).sum();
    let (ratio, ratio_from_history) = match ratio_override {
        Some(ratio) => {
            if ratio <= Decimal::ZERO {
                return Err(SchedulingError::InvalidRatio(ratio));
            }
            (ratio, false)
        }
        None => (
            sales_per_labor_hour(history_sales, history_labor_hours)?,
            true,
        ),
    };
    let overall_daily_sales = history_sales / Decimal::from(history_days);
    let overall_daily_txns = Decimal::from(sales.len() as u32) / Decimal::from(history_days);

    let days = targets
        .iter()
        .map(|date| {
            let weekday = date.weekday().num_days_from_sunday() as u8;
            // History days sharing this weekday.
            let same_weekday: Vec<&Vec<&SalePoint>> = by_day
                .iter()
                .filter(|(day, _)| day.weekday().num_days_from_sunday() as u8 == weekday)
                .map(|(_, sales)| sales)
                .collect();
            let (forecast_sales, forecast_txns, hourly) = if same_weekday.is_empty() {
                // No history for this weekday: fall back to the
                // overall daily average, with no hourly detail.
                (overall_daily_sales, overall_daily_txns, Vec::new())
            } else {
                let day_count = Decimal::from(same_weekday.len() as u32);
                let weekday_sales: Decimal = same_weekday
                    .iter()
                    .flat_map(|sales| sales.iter())
                    .map(|sale| sale.amount)
                    .sum();
                let weekday_txns: u32 = same_weekday.iter().map(|sales| sales.len() as u32).sum();
                let forecast_sales = weekday_sales / day_count;
                let forecast_txns = Decimal::from(weekday_txns) / day_count;
                // Per-hour buckets over the same-weekday days.
                let mut by_hour: BTreeMap<u8, Vec<&SalePoint>> = BTreeMap::new();
                for sale in same_weekday.iter().flat_map(|sales| sales.iter()) {
                    by_hour.entry(sale.at.hour() as u8).or_default().push(*sale);
                }
                let hourly = by_hour
                    .into_iter()
                    .map(|(hour, hour_sales)| {
                        let avg_sales =
                            hour_sales.iter().map(|sale| sale.amount).sum::<Decimal>() / day_count;
                        let avg_txns = Decimal::from(hour_sales.len() as u32) / day_count;
                        HourBucket {
                            hour,
                            avg_txns: round2(avg_txns),
                            avg_sales: round2(avg_sales),
                            required_hours: round2(avg_sales / ratio),
                        }
                    })
                    .collect();
                (forecast_sales, forecast_txns, hourly)
            };
            DayForecast {
                date: *date,
                weekday,
                forecast_txns: round2(forecast_txns),
                forecast_sales: round2(forecast_sales),
                required_hours: round2(forecast_sales / ratio),
                hourly,
            }
        })
        .collect();

    Ok(ForecastReport {
        store_id,
        generated_at: Utc::now(),
        history_days,
        history_txns: sales.len() as u32,
        history_sales,
        history_labor_hours,
        sales_per_labor_hour: ratio,
        ratio_from_history,
        days,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Four Mondays (Nov 2/9/16/23 2026), 10 sales of $100 each
    /// per day, all at 10:00 UTC, 5 worked hours per day.
    /// Totals: 40 txns, $4,000 sales, 20 labor hours → ratio
    /// $200 sales per labor hour. A Monday forecasts 10 txns /
    /// $1,000 / 5.00 hours; the 10:00 bucket forecasts 10 txns /
    /// $1,000 / 5.00 hours.
    fn monday_history() -> (Vec<SalePoint>, Vec<LaborPoint>) {
        let mut sales = Vec::new();
        let mut labor = Vec::new();
        for day in [2, 9, 16, 23] {
            for _ in 0..10 {
                sales.push(SalePoint {
                    at: Utc.with_ymd_and_hms(2026, 11, day, 10, 0, 0).unwrap(),
                    amount: Decimal::from_str_exact("100").unwrap(),
                });
            }
            labor.push(LaborPoint {
                at: Utc.with_ymd_and_hms(2026, 11, day, 8, 0, 0).unwrap(),
                hours: Decimal::from_str_exact("5").unwrap(),
            });
        }
        (sales, labor)
    }

    #[test]
    fn monday_forecast_uses_same_weekday_average() {
        let (sales, labor) = monday_history();
        let report = build_forecast(
            Uuid::now_v7(),
            &sales,
            &labor,
            &[NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()],
            None,
        )
        .unwrap();
        assert_eq!(report.history_days, 4);
        assert_eq!(report.history_txns, 40);
        assert_eq!(
            report.history_sales,
            Decimal::from_str_exact("4000").unwrap()
        );
        assert_eq!(
            report.history_labor_hours,
            Decimal::from_str_exact("20").unwrap()
        );
        assert_eq!(
            report.sales_per_labor_hour,
            Decimal::from_str_exact("200").unwrap()
        );
        assert!(report.ratio_from_history);
        let day = &report.days[0];
        assert_eq!(day.weekday, 1);
        assert_eq!(day.forecast_txns, Decimal::from_str_exact("10").unwrap());
        assert_eq!(day.forecast_sales, Decimal::from_str_exact("1000").unwrap());
        assert_eq!(day.required_hours, Decimal::from_str_exact("5.00").unwrap());
        assert_eq!(day.hourly.len(), 1);
        assert_eq!(day.hourly[0].hour, 10);
        assert_eq!(
            day.hourly[0].avg_txns,
            Decimal::from_str_exact("10").unwrap()
        );
        assert_eq!(
            day.hourly[0].avg_sales,
            Decimal::from_str_exact("1000").unwrap()
        );
        assert_eq!(
            day.hourly[0].required_hours,
            Decimal::from_str_exact("5.00").unwrap()
        );
    }

    #[test]
    fn weekday_without_history_falls_back_to_overall_average() {
        let (sales, labor) = monday_history();
        let report = build_forecast(
            Uuid::now_v7(),
            &sales,
            &labor,
            &[NaiveDate::from_ymd_opt(2026, 12, 1).unwrap()],
            None,
        )
        .unwrap();
        // Tuesday has no history: overall daily average is the
        // same 10 txns / $1,000 / 5.00 hours here, but with no
        // hourly detail.
        let day = &report.days[0];
        assert_eq!(day.weekday, 2);
        assert_eq!(day.forecast_txns, Decimal::from_str_exact("10").unwrap());
        assert_eq!(day.forecast_sales, Decimal::from_str_exact("1000").unwrap());
        assert_eq!(day.required_hours, Decimal::from_str_exact("5.00").unwrap());
        assert!(day.hourly.is_empty());
    }

    #[test]
    fn ratio_override_replaces_history_ratio() {
        let (sales, labor) = monday_history();
        let report = build_forecast(
            Uuid::now_v7(),
            &sales,
            &labor,
            &[NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()],
            Some(Decimal::from_str_exact("250").unwrap()),
        )
        .unwrap();
        assert!(!report.ratio_from_history);
        assert_eq!(
            report.sales_per_labor_hour,
            Decimal::from_str_exact("250").unwrap()
        );
        // $1,000 / $250 per hour = 4.00 hours.
        assert_eq!(
            report.days[0].required_hours,
            Decimal::from_str_exact("4.00").unwrap()
        );
    }

    #[test]
    fn no_sales_is_insufficient_history() {
        let (_, labor) = monday_history();
        assert!(matches!(
            build_forecast(Uuid::now_v7(), &[], &labor, &[], None),
            Err(SchedulingError::InsufficientHistory(_))
        ));
    }

    #[test]
    fn no_labor_hours_is_insufficient_history() {
        let (sales, _) = monday_history();
        assert!(matches!(
            build_forecast(
                Uuid::now_v7(),
                &sales,
                &[],
                &[NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()],
                None
            ),
            Err(SchedulingError::InsufficientHistory(_))
        ));
    }

    #[test]
    fn zero_or_negative_ratio_override_is_rejected() {
        let (sales, labor) = monday_history();
        assert_eq!(
            build_forecast(
                Uuid::now_v7(),
                &sales,
                &labor,
                &[NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()],
                Some(Decimal::ZERO)
            ),
            Err(SchedulingError::InvalidRatio(Decimal::ZERO))
        );
    }

    #[test]
    fn mixed_weekday_history_averages_only_matching_days() {
        // Mondays: $1,000/day; one Tuesday: $200/day. A Monday
        // target forecasts $1,000; a Tuesday target $200.
        let (mut sales, labor) = monday_history();
        sales.push(SalePoint {
            at: Utc.with_ymd_and_hms(2026, 11, 3, 11, 0, 0).unwrap(),
            amount: Decimal::from_str_exact("200").unwrap(),
        });
        sales.push(SalePoint {
            at: Utc.with_ymd_and_hms(2026, 11, 3, 11, 0, 0).unwrap(),
            amount: Decimal::from_str_exact("200").unwrap(),
        });
        let report = build_forecast(
            Uuid::now_v7(),
            &sales,
            &labor,
            &[
                NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(),
                NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(),
            ],
            None,
        )
        .unwrap();
        assert_eq!(
            report.days[0].forecast_sales,
            Decimal::from_str_exact("1000").unwrap()
        );
        assert_eq!(
            report.days[1].forecast_sales,
            Decimal::from_str_exact("400").unwrap()
        );
        assert_eq!(report.days[1].hourly.len(), 1);
        assert_eq!(report.days[1].hourly[0].hour, 11);
    }
}
