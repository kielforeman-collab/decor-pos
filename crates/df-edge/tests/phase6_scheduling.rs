//! Phase 6 exit test (scheduling+): shift templates, staff
//! shifts with overlap rules, timeclock guards, the sales-driven
//! labor forecast and the coverage check — end to end through the
//! real `Edge` against an in-memory SQLite edge.
//!
//! History seeded (same shape as the `df-jobs` forecast test):
//! four Mondays (Nov 2/9/16/23 2026), 10 completed sales of
//! $100 each at 10:00 UTC per day, one 5-hour timeclock entry
//! (08:00–13:00) per day. Ratio $4,000 / 20h = $200 sales per
//! labor hour, so Monday Nov 30 forecasts 10 txns / $1,000 /
//! 5.00 required hours.

use chrono::{NaiveDate, TimeZone, Utc};
use df_db::models::{Organization, Store, TimeclockEntry, Transaction};
use df_domain::Currency;
use df_edge::{
    ClockRequest, CreateShiftTemplateRequest, CreateStaffShiftRequest, Edge, EdgeConfig,
    SetShiftStatusRequest,
};
use df_scheduling::{CoverageStatus, SchedulingError};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
}

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const STAFF_A: u128 = 0x5001;
const STAFF_B: u128 = 0x5002;

async fn test_edge() -> Edge {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    let edge = Edge::open(config).await.unwrap();
    let pool = edge.pool();
    df_db::insert_organization(
        pool,
        &Organization {
            id: Uuid::from_u128(ORG_ID),
            name: "Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({}),
            settings_json: serde_json::json!({}),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    df_db::insert_store(
        pool,
        &Store {
            id: Uuid::from_u128(STORE_ID),
            org_id: Uuid::from_u128(ORG_ID),
            code: "MAIN".to_string(),
            name: "Main Store".to_string(),
            country_code: "US".to_string(),
            region_code: "CA".to_string(),
            postal_code: "90210".to_string(),
            currency: "USD".to_string(),
            tax_zone_id: Uuid::now_v7(),
            timezone: "America/Los_Angeles".to_string(),
            is_warehouse: false,
        },
    )
    .await
    .unwrap();
    edge
}

async fn seed_monday_history(edge: &Edge) {
    let pool = edge.pool();
    let org_id = Uuid::from_u128(ORG_ID);
    let store_id = Uuid::from_u128(STORE_ID);
    for day in [2, 9, 16, 23] {
        for n in 0..10 {
            df_db::insert_transaction(
                pool,
                &Transaction {
                    id: Uuid::now_v7(),
                    org_id,
                    store_id,
                    till_session_id: None,
                    doc_number: format!("TX-202611{day:02}-{n:02}"),
                    doc_type: "sale".to_string(),
                    status: "completed".to_string(),
                    customer_id: None,
                    job_id: None,
                    po_number: None,
                    salesperson_id: None,
                    cashier_id: Some(Uuid::now_v7()),
                    currency: "USD".to_string(),
                    subtotal: dec("100"),
                    tax_total: Decimal::ZERO,
                    eco_fee_total: Decimal::ZERO,
                    pos_fee_total: Decimal::ZERO,
                    grand_total: dec("100"),
                    deposit_paid: Decimal::ZERO,
                    fulfillment_mode: "counter".to_string(),
                    scheduled_at: None,
                    completed_at: Some(at(2026, 11, day, 10, 0)),
                    created_at: at(2026, 11, day, 10, 0),
                },
            )
            .await
            .unwrap();
        }
        df_db::insert_timeclock_entry(
            pool,
            &TimeclockEntry {
                id: Uuid::now_v7(),
                org_id,
                store_id,
                staff_id: Uuid::from_u128(STAFF_A),
                clock_in: at(2026, 11, day, 8, 0),
                clock_out: Some(at(2026, 11, day, 13, 0)),
                source: "counter".to_string(),
            },
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn templates_validate_and_shifts_enforce_overlap() {
    let edge = test_edge().await;
    // A valid template.
    let template = edge
        .create_shift_template(CreateShiftTemplateRequest {
            name: "Monday opener".to_string(),
            weekday: 1,
            starts_at: "08:00".to_string(),
            ends_at: "16:00".to_string(),
            role: Some("cashier".to_string()),
            staff_id: None,
            required_count: 1,
        })
        .await
        .unwrap();
    assert_eq!(template.weekday, 1);
    assert_eq!(template.starts_at, "08:00");
    // End before start is refused.
    assert!(matches!(
        edge.create_shift_template(CreateShiftTemplateRequest {
            name: "Bad".to_string(),
            weekday: 1,
            starts_at: "16:00".to_string(),
            ends_at: "08:00".to_string(),
            role: None,
            staff_id: None,
            required_count: 1,
        })
        .await,
        Err(df_edge::EdgeError::Scheduling(
            SchedulingError::InvalidTemplateWindow { .. }
        ))
    ));
    // Weekday 7 is refused.
    assert!(matches!(
        edge.create_shift_template(CreateShiftTemplateRequest {
            name: "Bad".to_string(),
            weekday: 7,
            starts_at: "08:00".to_string(),
            ends_at: "16:00".to_string(),
            role: None,
            staff_id: None,
            required_count: 1,
        })
        .await,
        Err(df_edge::EdgeError::Scheduling(
            SchedulingError::InvalidWeekday(7)
        ))
    ));
    let templates = edge.list_shift_templates().await.unwrap();
    assert_eq!(templates.len(), 1);

    // One shift for staff A, 08:00–16:00 on Nov 30.
    let day = date(2026, 11, 30);
    let shift = edge
        .create_staff_shift(CreateStaffShiftRequest {
            staff_id: Uuid::from_u128(STAFF_A),
            shift_date: day,
            starts_at: at(2026, 11, 30, 8, 0),
            ends_at: at(2026, 11, 30, 16, 0),
            template_id: Some(template.id),
            notes: None,
        })
        .await
        .unwrap();
    assert_eq!(shift.status, "scheduled");
    // Overlapping shift for the same staff is refused (409 path).
    assert!(matches!(
        edge.create_staff_shift(CreateStaffShiftRequest {
            staff_id: Uuid::from_u128(STAFF_A),
            shift_date: day,
            starts_at: at(2026, 11, 30, 12, 0),
            ends_at: at(2026, 11, 30, 20, 0),
            template_id: None,
            notes: None,
        })
        .await,
        Err(df_edge::EdgeError::Scheduling(
            SchedulingError::ShiftOverlap { .. }
        ))
    ));
    // Adjacent is fine, and another staff member may overlap.
    edge.create_staff_shift(CreateStaffShiftRequest {
        staff_id: Uuid::from_u128(STAFF_A),
        shift_date: day,
        starts_at: at(2026, 11, 30, 16, 0),
        ends_at: at(2026, 11, 30, 20, 0),
        template_id: None,
        notes: None,
    })
    .await
    .unwrap();
    edge.create_staff_shift(CreateStaffShiftRequest {
        staff_id: Uuid::from_u128(STAFF_B),
        shift_date: day,
        starts_at: at(2026, 11, 30, 8, 0),
        ends_at: at(2026, 11, 30, 16, 0),
        template_id: None,
        notes: None,
    })
    .await
    .unwrap();
    // Cancelling the first shift frees its slot for staff A.
    let cancelled = edge.set_shift_status(shift.id, "cancelled").await.unwrap();
    assert_eq!(cancelled.status, "cancelled");
    edge.create_staff_shift(CreateStaffShiftRequest {
        staff_id: Uuid::from_u128(STAFF_A),
        shift_date: day,
        starts_at: at(2026, 11, 30, 9, 0),
        ends_at: at(2026, 11, 30, 12, 0),
        template_id: None,
        notes: None,
    })
    .await
    .unwrap();
    // An unknown status string is a 400, not a silent default.
    assert!(matches!(
        edge.set_shift_status(shift.id, "on-leave").await,
        Err(df_edge::EdgeError::Scheduling(
            SchedulingError::UnknownShiftStatus(_)
        ))
    ));
    let shifts = edge.list_staff_shifts(day).await.unwrap();
    assert_eq!(shifts.len(), 4);
}

#[tokio::test]
async fn timeclock_guards_one_open_entry() {
    let edge = test_edge().await;
    let staff = Uuid::from_u128(STAFF_A);
    // Clocking out cold is refused.
    assert!(matches!(
        edge.clock_out(staff).await,
        Err(df_edge::EdgeError::Scheduling(
            SchedulingError::NotClockedIn(_)
        ))
    ));
    let entry = edge.clock_in(staff).await.unwrap();
    assert!(entry.clock_out.is_none());
    // A second clock-in while open is refused.
    assert!(matches!(
        edge.clock_in(staff).await,
        Err(df_edge::EdgeError::Scheduling(
            SchedulingError::AlreadyClockedIn(_, _)
        ))
    ));
    // Another staff member is unaffected.
    edge.clock_in(Uuid::from_u128(STAFF_B)).await.unwrap();
    let closed = edge.clock_out(staff).await.unwrap();
    assert!(closed.clock_out.is_some());
    assert!(closed.clock_out.unwrap() > closed.clock_in);
    // The board shows both entries for today (UTC).
    let today = Utc::now().date_naive();
    let entries = edge.list_timeclock_entries(today).await.unwrap();
    assert_eq!(entries.len(), 2);
    // Clocking in again after closing works.
    edge.clock_in(staff).await.unwrap();
    let _ = edge.clock_out(staff).await.unwrap();
    // The request type the HTTP layer uses is the same shape.
    let _ = ClockRequest { staff_id: staff };
    let _ = SetShiftStatusRequest {
        status: "confirmed".to_string(),
    };
}

#[tokio::test]
async fn forecast_and_coverage_from_seeded_history() {
    let edge = test_edge().await;
    seed_monday_history(&edge).await;
    let from = date(2026, 11, 30);
    let report = edge.labor_forecast(from, 7, None).await.unwrap();
    assert_eq!(report.sales_per_labor_hour, dec("200"));
    assert_eq!(report.history_days, 4);
    assert_eq!(report.history_txns, 40);
    let monday = &report.days[0];
    assert_eq!(monday.date, from);
    assert_eq!(monday.forecast_sales, dec("1000.00"));
    assert_eq!(monday.required_hours, dec("5.00"));
    assert_eq!(monday.hourly.len(), 1);
    assert_eq!(monday.hourly[0].hour, 10);
    assert_eq!(monday.hourly[0].required_hours, dec("5.00"));
    // A ratio override replaces the history ratio: $1,000 / $250
    // = 4.00 hours.
    let overridden = edge
        .labor_forecast(from, 1, Some(dec("250")))
        .await
        .unwrap();
    assert_eq!(overridden.days[0].required_hours, dec("4.00"));
    assert!(!overridden.ratio_from_history);

    // Coverage: no shifts scheduled yet → 0h vs 5.00h forecast → Under.
    let check = edge.labor_coverage(from).await.unwrap();
    assert_eq!(check.forecast_hours, Some(dec("5.00")));
    assert_eq!(check.scheduled_hours, Decimal::ZERO);
    assert_eq!(check.status, CoverageStatus::Under);
    assert_eq!(check.delta_hours, Some(dec("-5.00")));

    // Schedule 8h for staff A → Over (+60%).
    edge.create_staff_shift(CreateStaffShiftRequest {
        staff_id: Uuid::from_u128(STAFF_A),
        shift_date: from,
        starts_at: at(2026, 11, 30, 8, 0),
        ends_at: at(2026, 11, 30, 16, 0),
        template_id: None,
        notes: None,
    })
    .await
    .unwrap();
    let check = edge.labor_coverage(from).await.unwrap();
    assert_eq!(check.scheduled_hours, dec("8"));
    assert_eq!(check.status, CoverageStatus::Over);

    // Cancel it, schedule exactly 5h → Adequate.
    let shifts = edge.list_staff_shifts(from).await.unwrap();
    edge.set_shift_status(shifts[0].id, "cancelled")
        .await
        .unwrap();
    edge.create_staff_shift(CreateStaffShiftRequest {
        staff_id: Uuid::from_u128(STAFF_A),
        shift_date: from,
        starts_at: at(2026, 11, 30, 8, 0),
        ends_at: at(2026, 11, 30, 13, 0),
        template_id: None,
        notes: None,
    })
    .await
    .unwrap();
    let check = edge.labor_coverage(from).await.unwrap();
    assert_eq!(check.status, CoverageStatus::Adequate);
    assert_eq!(check.delta_hours, Some(Decimal::ZERO));
}

#[tokio::test]
async fn forecast_without_history_is_insufficient_and_coverage_is_unknown() {
    let edge = test_edge().await;
    let from = date(2026, 11, 30);
    assert!(matches!(
        edge.labor_forecast(from, 7, None).await,
        Err(df_edge::EdgeError::Scheduling(
            SchedulingError::InsufficientHistory(_)
        ))
    ));
    // Coverage still answers: no forecast, but 0 scheduled hours
    // measured — Unknown, not an error.
    let check = edge.labor_coverage(from).await.unwrap();
    assert_eq!(check.status, CoverageStatus::Unknown);
    assert_eq!(check.forecast_hours, None);
    assert_eq!(check.scheduled_hours, Decimal::ZERO);
}
