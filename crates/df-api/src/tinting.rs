//! Paint & tinting admin handlers (Phase 2).

use std::str::FromStr;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use df_db::models::{ColorCatalogEntry, ColorantLevelRow, TintFormula};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::schemas::{
    ColorSearchQuery, ColorantLevelResponse, ErrorResponse, FormulaQuery, SetColorantLevelRequest,
    StoreQuery,
};
use crate::{ApiResult, ApiState, db_err, err, org_id, pool, store_id};

// --- Phase 2 tinting admin (spec §6.1) --------------------------------------

#[utoipa::path(
    get,
    path = "/api/v1/colors/search",
    responses(
        (status = 200, description = "Matching colors"),
        (status = 400, description = "manufacturer is required"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn search_colors(
    State(state): State<ApiState>,
    Query(query): Query<ColorSearchQuery>,
) -> ApiResult<Vec<ColorCatalogEntry>> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let manufacturer = query.manufacturer.as_deref().unwrap_or("").trim();
    if manufacturer.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "manufacturer is required (?manufacturer=)",
        ));
    }
    df_db::search_colors(
        &pool,
        org,
        manufacturer,
        query.query.as_deref().unwrap_or(""),
    )
    .await
    .map(Json)
    .map_err(db_err)
}

#[utoipa::path(
    get,
    path = "/api/v1/tinting/formulas",
    responses(
        (status = 200, description = "Matching formulas, newest first"),
        (status = 400, description = "A color query or customer_id is required"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn list_formulas(
    State(state): State<ApiState>,
    Query(query): Query<FormulaQuery>,
) -> ApiResult<Vec<TintFormula>> {
    let pool = pool(&state)?;
    if let Some(customer_id) = query.customer_id {
        return df_db::list_tint_formulas_for_customer_job(&pool, customer_id, query.job_id)
            .await
            .map(Json)
            .map_err(db_err);
    }
    match query.query {
        Some(q) => {
            let org = org_id(&state)?;
            df_db::search_tint_formulas_by_color(&pool, org, &q)
                .await
                .map(Json)
                .map_err(db_err)
        }
        None => Err(err(
            StatusCode::BAD_REQUEST,
            "formula search needs ?query= (color) or ?customer_id=",
        )),
    }
}

/// Create a tint formula (admin). Not in the utoipa path list: the
/// `TintFormula` row type carries `Uuid`/`Decimal` fields, which do
/// not implement utoipa's `ToSchema`, so the endpoint is documented
/// in the module docs instead of the generated OpenAPI document.
pub async fn create_formula(
    State(state): State<ApiState>,
    Json(formula): Json<TintFormula>,
) -> Result<(StatusCode, Json<TintFormula>), (StatusCode, Json<ErrorResponse>)> {
    let pool = pool(&state)?;
    if !formula.colorants_json.is_array() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "colorants_json must be an array of {code, oz, shots} lines",
        ));
    }
    df_db::insert_tint_formula(&pool, &formula)
        .await
        .map(|()| (StatusCode::CREATED, Json(formula)))
        .map_err(db_err)
}

#[utoipa::path(
    get,
    path = "/api/v1/tinting/colorants",
    responses(
        (status = 200, description = "Levels plus shortage alerts"),
        (status = 400, description = "store_id is required"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn list_colorants(
    State(state): State<ApiState>,
    Query(query): Query<StoreQuery>,
) -> ApiResult<serde_json::Value> {
    let pool = pool(&state)?;
    let store = store_id(query.store_id, &state)?;
    let rows = df_db::list_colorant_levels_for_store(&pool, store)
        .await
        .map_err(db_err)?;
    let mut levels = Vec::with_capacity(rows.len());
    for row in rows {
        let colorant = df_db::get_colorant(&pool, row.colorant_id)
            .await
            .map_err(db_err)?;
        levels.push(ColorantLevelResponse {
            colorant_id: row.colorant_id.to_string(),
            code: colorant.code,
            name: colorant.name,
            on_hand_fl_oz: row.on_hand_fl_oz.to_string(),
            low_threshold_fl_oz: row.low_threshold_fl_oz.to_string(),
            is_low: row.on_hand_fl_oz <= row.low_threshold_fl_oz,
        });
    }
    levels.sort_by(|a, b| a.code.cmp(&b.code));
    let alerts = df_reports::colorant_shortage_alerts(&pool, store)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(
        serde_json::json!({ "levels": levels, "alerts": alerts }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/tinting/colorants/levels",
    request_body = SetColorantLevelRequest,
    responses(
        (status = 200, description = "Level stored"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn set_colorant_level(
    State(state): State<ApiState>,
    Json(body): Json<SetColorantLevelRequest>,
) -> ApiResult<ColorantLevelRow> {
    let pool = pool(&state)?;
    let parse_uuid = |text: &str, what: &str| {
        Uuid::parse_str(text).map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                format!("{what} is not a UUID: {text:?}"),
            )
        })
    };
    let parse_dec = |text: &str, what: &str| {
        Decimal::from_str(text).map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                format!("{what} is not a decimal: {text:?}"),
            )
        })
    };
    let level = ColorantLevelRow {
        colorant_id: parse_uuid(&body.colorant_id, "colorant_id")?,
        store_id: parse_uuid(&body.store_id, "store_id")?,
        on_hand_fl_oz: parse_dec(&body.on_hand_fl_oz, "on_hand_fl_oz")?,
        low_threshold_fl_oz: parse_dec(&body.low_threshold_fl_oz, "low_threshold_fl_oz")?,
        updated_at: chrono::Utc::now(),
    };
    df_db::upsert_colorant_level(&pool, &level)
        .await
        .map(|()| Json(level))
        .map_err(db_err)
}
