//! CRM handlers (Phase 4): activities, reminders tickler, appointments,
//! saved views, and lapsed-contractor reports.

use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDate, Utc};
use df_db::models::{Appointment, CrmActivity, CrmSavedView};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ErrorResponse, HandlerResult, appointment_day, err_response, store_or_default};
use crate::crm::{
    CreateActivityRequest, CreateAppointmentRequest, CreateSavedViewRequest, LapsedReport,
    SavedViewCustomer, SnoozeActivityRequest, UpdateActivityRequest, UpdateAppointmentRequest,
};
use crate::edge::Edge;

// --- CRM / delivery / EDI (Phase 4) ----------------------------------------

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    pub customer_id: Option<Uuid>,
    pub activity_type: Option<String>,
    pub completed: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct AppointmentQuery {
    pub date: Option<String>,
    pub customer_id: Option<Uuid>,
    pub staff_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct RemindersQuery {
    pub as_of: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LapsedQuery {
    pub store_id: Option<Uuid>,
    pub as_of: Option<String>,
    pub inactive_days: Option<i64>,
    pub include_never: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedResponse {
    pub id: Uuid,
    pub deleted: bool,
}

/// `?as_of=` as RFC 3339, defaulting to now (UTC). A malformed
/// value is a 400, not a silent default.
fn reminders_as_of(
    as_of: &Option<String>,
) -> Result<DateTime<Utc>, (StatusCode, Json<ErrorResponse>)> {
    match as_of {
        None => Ok(Utc::now()),
        Some(text) => DateTime::parse_from_rfc3339(text)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("as_of must be RFC 3339, got {text:?}"),
                    }),
                )
            }),
    }
}

pub(super) async fn create_activity(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateActivityRequest>,
) -> HandlerResult<CrmActivity> {
    edge.create_activity(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_activities(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ActivityQuery>,
) -> HandlerResult<Vec<CrmActivity>> {
    edge.list_activities(
        query.customer_id,
        query.activity_type.as_deref(),
        query.completed,
    )
    .await
    .map(Json)
    .map_err(err_response)
}

pub(super) async fn get_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<CrmActivity> {
    edge.get_activity(id).await.map(Json).map_err(err_response)
}

pub(super) async fn update_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateActivityRequest>,
) -> HandlerResult<CrmActivity> {
    edge.update_activity(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn delete_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeletedResponse> {
    edge.delete_activity(id)
        .await
        .map(|()| Json(DeletedResponse { id, deleted: true }))
        .map_err(err_response)
}

pub(super) async fn complete_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<CrmActivity> {
    edge.complete_activity(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn snooze_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SnoozeActivityRequest>,
) -> HandlerResult<CrmActivity> {
    edge.snooze_activity(id, body.snoozed_until)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn due_reminders(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<RemindersQuery>,
) -> Result<Json<Vec<CrmActivity>>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = reminders_as_of(&query.as_of)?;
    edge.due_reminders(as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn overdue_reminders(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<RemindersQuery>,
) -> Result<Json<Vec<CrmActivity>>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = reminders_as_of(&query.as_of)?;
    edge.overdue_reminders(as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn create_appointment(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateAppointmentRequest>,
) -> HandlerResult<Appointment> {
    edge.create_appointment(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_appointments(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<AppointmentQuery>,
) -> Result<Json<Vec<Appointment>>, (StatusCode, Json<ErrorResponse>)> {
    let day = appointment_day(&query.date)?;
    edge.list_appointments(day, query.customer_id, query.staff_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_appointment(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Appointment> {
    edge.get_appointment(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn update_appointment(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAppointmentRequest>,
) -> HandlerResult<Appointment> {
    edge.update_appointment(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn cancel_appointment(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Appointment> {
    edge.cancel_appointment(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn delete_appointment(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeletedResponse> {
    edge.delete_appointment(id)
        .await
        .map(|()| Json(DeletedResponse { id, deleted: true }))
        .map_err(err_response)
}

pub(super) async fn create_saved_view(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateSavedViewRequest>,
) -> HandlerResult<CrmSavedView> {
    edge.create_saved_view(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_saved_views(
    State(edge): State<Arc<Edge>>,
) -> HandlerResult<Vec<CrmSavedView>> {
    edge.list_saved_views()
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn run_saved_view(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Vec<CrmActivity>> {
    edge.run_saved_view(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn run_saved_view_customers(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Vec<SavedViewCustomer>> {
    edge.run_saved_view_customers(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn delete_saved_view(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeletedResponse> {
    edge.delete_saved_view(id)
        .await
        .map(|()| Json(DeletedResponse { id, deleted: true }))
        .map_err(err_response)
}

pub(super) async fn lapsed_report(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<LapsedQuery>,
) -> Result<Json<LapsedReport>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = match &query.as_of {
        None => Utc::now().date_naive(),
        Some(text) => NaiveDate::from_str(text).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("as_of must be YYYY-MM-DD, got {text:?}"),
                }),
            )
        })?,
    };
    let store_id = store_or_default(&edge, query.store_id);
    if store_id != edge.config().store_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "this edge serves store {} — got store_id {store_id}",
                    edge.config().store_id
                ),
            }),
        ));
    }
    edge.lapsed_contractors(
        as_of,
        query.inactive_days.unwrap_or(90),
        query.include_never.unwrap_or(true),
    )
    .await
    .map(Json)
    .map_err(err_response)
}
