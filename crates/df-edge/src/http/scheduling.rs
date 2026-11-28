//! Scheduling+ handlers (Phase 6): shift templates, staff shifts,
//! timeclock, labor forecast, and coverage checks.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{NaiveDate, Utc};
use df_db::models::{ShiftTemplate, StaffShift, TimeclockEntry};
use df_scheduling::{CoverageCheck, ForecastReport};
use serde::Deserialize;
use uuid::Uuid;

use super::{ErrorResponse, HandlerResult, appointment_day, err_response};
use crate::edge::Edge;
use crate::scheduling::{
    ClockRequest, CreateShiftTemplateRequest, CreateStaffShiftRequest, SetShiftStatusRequest,
    parse_ratio,
};

// --- Scheduling+ (Phase 6) ---------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ShiftDayQuery {
    pub date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ForecastQuery {
    pub from: Option<String>,
    pub days: Option<i64>,
    pub sales_per_labor_hour: Option<String>,
}

/// `?date=YYYY-MM-DD`, defaulting to today (UTC). A malformed
/// value is a 400, not a silent default.
fn day_or_today(date: &Option<String>) -> Result<NaiveDate, (StatusCode, Json<ErrorResponse>)> {
    match appointment_day(date)? {
        Some(day) => Ok(day),
        None => Ok(Utc::now().date_naive()),
    }
}

pub(super) async fn create_shift_template(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateShiftTemplateRequest>,
) -> HandlerResult<ShiftTemplate> {
    edge.create_shift_template(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_shift_templates(
    State(edge): State<Arc<Edge>>,
) -> HandlerResult<Vec<ShiftTemplate>> {
    edge.list_shift_templates()
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn create_staff_shift(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateStaffShiftRequest>,
) -> HandlerResult<StaffShift> {
    edge.create_staff_shift(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_staff_shifts(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ShiftDayQuery>,
) -> HandlerResult<Vec<StaffShift>> {
    let day = day_or_today(&query.date)?;
    edge.list_staff_shifts(day)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn set_shift_status(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetShiftStatusRequest>,
) -> HandlerResult<StaffShift> {
    edge.set_shift_status(id, &body.status)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn clock_in(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<ClockRequest>,
) -> HandlerResult<TimeclockEntry> {
    edge.clock_in(body.staff_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn clock_out(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<ClockRequest>,
) -> HandlerResult<TimeclockEntry> {
    edge.clock_out(body.staff_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_timeclock(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ShiftDayQuery>,
) -> HandlerResult<Vec<TimeclockEntry>> {
    let day = day_or_today(&query.date)?;
    edge.list_timeclock_entries(day)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn labor_forecast(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ForecastQuery>,
) -> HandlerResult<ForecastReport> {
    let from = day_or_today(&query.from)?;
    let ratio = match &query.sales_per_labor_hour {
        Some(text) => Some(parse_ratio(text).map_err(err_response)?),
        None => None,
    };
    edge.labor_forecast(from, query.days.unwrap_or(7), ratio)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn labor_coverage(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ShiftDayQuery>,
) -> HandlerResult<CoverageCheck> {
    let day = day_or_today(&query.date)?;
    edge.labor_coverage(day)
        .await
        .map(Json)
        .map_err(err_response)
}
