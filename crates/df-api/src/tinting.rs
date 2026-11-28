//! Paint & tinting formula-tracking admin handlers (Phase 2).

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use df_db::models::{ColorCatalogEntry, Colorant, TintFormula};

use crate::schemas::{ColorSearchQuery, ErrorResponse, FormulaQuery};
use crate::{ApiResult, ApiState, db_err, err, org_id, pool};

// --- Phase 2 formula tracking admin (spec §6.1) -----------------------------

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
        (status = 200, description = "Org colorant catalog"),
        (status = 503, description = "No database configured")
    )
)]
pub async fn list_colorants(State(state): State<ApiState>) -> ApiResult<Vec<Colorant>> {
    let pool = pool(&state)?;
    let org = org_id(&state)?;
    let mut colorants = df_db::list_colorants_for_org(&pool, org)
        .await
        .map_err(db_err)?;
    colorants.sort_by(|a, b| a.code.cmp(&b.code));
    Ok(Json(colorants))
}
