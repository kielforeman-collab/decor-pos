//! BI & reporting handlers (Phase 6): KPIs, dashboards, eco remittance,
//! ad-hoc inquiries, and email outbox.

use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use chrono::{Datelike, NaiveDate};
use df_reports::{AdHocInquiry, DashboardAggregate, EmailDelivery, InquiryRow, KpiDashboard};
use serde::Deserialize;
use uuid::Uuid;

use super::{ErrorResponse, err_response, store_or_default};
use crate::edge::Edge;

// --- BI (Phase 6) ------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct BiKpiQuery {
    pub store_id: Option<Uuid>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub as_of: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BiEcoQuery {
    pub start: Option<String>,
    pub end: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BiInquiryQuery {
    pub store_id: Option<Uuid>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub customer_id: Option<Uuid>,
    pub product_id: Option<Uuid>,
    pub doc_type: Option<String>,
}

/// `?start=&end=` as `YYYY-MM-DD`, defaulting to `default_start`
/// / `default_end` when omitted. A malformed date is a 400, not a
/// silent default.
fn bi_date(
    label: &str,
    text: &Option<String>,
    default: NaiveDate,
) -> Result<NaiveDate, (StatusCode, Json<ErrorResponse>)> {
    match text {
        None => Ok(default),
        Some(text) => NaiveDate::from_str(text).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("{label} must be YYYY-MM-DD, got {text:?}"),
                }),
            )
        }),
    }
}

fn bi_range(
    start: &Option<String>,
    end: &Option<String>,
    default_start: NaiveDate,
    default_end: NaiveDate,
) -> Result<(NaiveDate, NaiveDate), (StatusCode, Json<ErrorResponse>)> {
    Ok((
        bi_date("start", start, default_start)?,
        bi_date("end", end, default_end)?,
    ))
}

pub(super) async fn bi_kpi(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BiKpiQuery>,
) -> Result<Json<KpiDashboard>, (StatusCode, Json<ErrorResponse>)> {
    let today = chrono::Utc::now().date_naive();
    let (start, end) = bi_range(&query.start, &query.end, today, today)?;
    let as_of = match &query.as_of {
        None => end,
        Some(text) => NaiveDate::from_str(text).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("as_of must be YYYY-MM-DD, got {text:?}"),
                }),
            )
        })?,
    };
    edge.kpi_dashboard(store_or_default(&edge, query.store_id), start, end, as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn bi_dashboard(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BiKpiQuery>,
) -> Result<Json<DashboardAggregate>, (StatusCode, Json<ErrorResponse>)> {
    let today = chrono::Utc::now().date_naive();
    let (start, end) = bi_range(&query.start, &query.end, today, today)?;
    edge.dashboard_aggregate(store_or_default(&edge, query.store_id), start, end)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn bi_eco(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BiEcoQuery>,
) -> Result<axum::response::Response, (StatusCode, Json<ErrorResponse>)> {
    let today = chrono::Utc::now().date_naive();
    let month_start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
    let (start, end) = bi_range(&query.start, &query.end, month_start, today)?;
    let report = edge
        .eco_remittance(start, end)
        .await
        .map_err(err_response)?;
    match query.format.as_deref() {
        Some("csv") => {
            let csv = df_reports::render_eco_csv(&report);
            Ok(axum::response::Response::builder()
                .header("content-type", "text/csv")
                .body(axum::body::Body::from(csv))
                .unwrap())
        }
        _ => Ok(axum::response::Response::builder()
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_string(&report).unwrap(),
            ))
            .unwrap()),
    }
}

pub(super) async fn bi_inquiry(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BiInquiryQuery>,
) -> Result<Json<Vec<InquiryRow>>, (StatusCode, Json<ErrorResponse>)> {
    let today = chrono::Utc::now().date_naive();
    let (start, end) = bi_range(&query.start, &query.end, today, today)?;
    let inquiry = AdHocInquiry {
        start,
        end,
        customer_id: query.customer_id,
        product_id: query.product_id,
        doc_type: query.doc_type,
    };
    edge.ad_hoc_inquiry(store_or_default(&edge, query.store_id), &inquiry)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn bi_email_outbox(
    State(edge): State<Arc<Edge>>,
) -> Result<Json<Vec<EmailDelivery>>, (StatusCode, Json<ErrorResponse>)> {
    edge.email_outbox().await.map(Json).map_err(err_response)
}
