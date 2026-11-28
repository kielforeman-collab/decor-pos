//! HQ report handlers (Phase 2): daily usage, low stock, shortage alerts.

use std::str::FromStr;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use df_domain::Currency;

use crate::schemas::{ReportQuery, StoreQuery};
use crate::{ApiResult, ApiState, db_err, err, org_id, pool, report_date, store_id};

// --- Phase 2 reports (spec §6.1) ---------------------------------------------

#[utoipa::path(
    get,
    path = "/api/v1/reports/daily-usage",
    responses(
        (status = 200, description = "Daily usage report"),
        (status = 400, description = "store_id is required or date is malformed"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn daily_usage(
    State(state): State<ApiState>,
    Query(query): Query<ReportQuery>,
) -> ApiResult<serde_json::Value> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let store = store_id(query.store_id, &state)?;
    let date = report_date(query.date)?;
    let store_row = df_db::get_store(&pool, store).await.map_err(db_err)?;
    let currency = Currency::from_str(&store_row.currency)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    df_reports::daily_usage_report(&pool, org, store, date, currency)
        .await
        .map(|report| Json(serde_json::to_value(report).expect("report serialises")))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[utoipa::path(
    get,
    path = "/api/v1/reports/low-stock",
    responses(
        (status = 200, description = "Low-stock report (colorants; products need thresholds — see docs)"),
        (status = 400, description = "store_id is required or date is malformed"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn low_stock(
    State(state): State<ApiState>,
    Query(query): Query<ReportQuery>,
) -> ApiResult<serde_json::Value> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    let date = report_date(query.date)?;
    // No product thresholds are supplied here: there is no
    // min-level table yet (see `df-reports` docs), so this endpoint
    // reports colorants and leaves `products` empty. The edge
    // low-stock endpoint takes operator-configured thresholds.
    df_reports::low_stock_report(&pool, store, date, &[])
        .await
        .map(|report| Json(serde_json::to_value(report).expect("report serialises")))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[utoipa::path(
    get,
    path = "/api/v1/reports/shortage-alerts",
    responses(
        (status = 200, description = "Colorant shortage alerts"),
        (status = 400, description = "store_id is required"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn shortage_alerts(
    State(state): State<ApiState>,
    Query(query): Query<StoreQuery>,
) -> ApiResult<serde_json::Value> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    df_reports::colorant_shortage_alerts(&pool, store)
        .await
        .map(|alerts| Json(serde_json::json!({ "alerts": alerts })))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
