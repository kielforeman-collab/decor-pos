//! Delivery & proof-of-delivery handlers (Phase 4).

use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::NaiveDate;
use df_db::models::DeliveryPod;
use serde::Deserialize;
use uuid::Uuid;

use super::{ErrorResponse, HandlerResult, err_response};
use crate::delivery::{
    AssignDeliveryRequest, CreateDeliveryRequest, DeliveryView, PackingSlipView,
    SetDeliveryStatusRequest, SubmitPodRequest,
};
use crate::edge::Edge;

#[derive(Debug, Deserialize)]
pub struct DispatchQuery {
    pub status: Option<String>,
    pub driver_id: Option<Uuid>,
    pub scheduled_date: Option<String>,
}

pub(super) async fn create_delivery(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateDeliveryRequest>,
) -> HandlerResult<DeliveryView> {
    edge.create_delivery(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_deliveries(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<DispatchQuery>,
) -> Result<Json<Vec<DeliveryView>>, (StatusCode, Json<ErrorResponse>)> {
    let scheduled_date = match &query.scheduled_date {
        None => None,
        Some(text) => Some(NaiveDate::from_str(text).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("scheduled_date must be YYYY-MM-DD, got {text:?}"),
                }),
            )
        })?),
    };
    edge.list_deliveries(query.status.as_deref(), query.driver_id, scheduled_date)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_delivery(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeliveryView> {
    edge.get_delivery_view(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn assign_delivery(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignDeliveryRequest>,
) -> HandlerResult<DeliveryView> {
    edge.assign_delivery(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn set_delivery_status(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetDeliveryStatusRequest>,
) -> HandlerResult<DeliveryView> {
    edge.set_delivery_status(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_packing_slip(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<PackingSlipView> {
    edge.packing_slip(id).await.map(Json).map_err(err_response)
}

pub(super) async fn submit_pod(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SubmitPodRequest>,
) -> HandlerResult<DeliveryView> {
    edge.submit_pod(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_pod(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Option<DeliveryPod>> {
    edge.get_pod(id).await.map(Json).map_err(err_response)
}
