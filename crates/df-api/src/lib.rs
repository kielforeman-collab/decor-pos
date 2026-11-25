//! `df-api` — Axum REST API with utoipa OpenAPI 3.1 (Phase 2, v0.3).
//!
//! Working: `/health` and the OpenAPI document. Auth routes still
//! return `501 Not Implemented` — there is no auth database wired in
//! Phase 2, and no credential is ever accepted, logged, or stored
//! here.
//!
//! Phase 2 admin surface (spec §6.1), backed by `df-db` repos over
//! the pool in [`ApiState`]: color search, tint formulas
//! (list/search by color and by customer, create), colorant levels
//! (list, set level/threshold), and reports (daily usage, low
//! stock, shortage alerts). The same repos run over `AnyPool`, so
//! these handlers work against SQLite (edge/dev) and PostgreSQL
//! (HQ) alike — but the **live-Postgres round-trip is still
//! unverified** (no PG 16+ in this VM), so treat the HQ path as
//! compile-checked, not tested.
//!
//! When no pool is configured (the default `df-api` binary with no
//! `--db` flag), the admin endpoints return `503` with a clear
//! message instead of failing obscurely. Start the binary with
//! `--db <url> --org-id <uuid>` to enable them, or use
//! `df-server reports` / `df-server jobs run` for the CLI path.

use std::str::FromStr;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use df_db::AnyPool;
use df_db::models::{ColorCatalogEntry, ColorantLevelRow, TintFormula};
use df_domain::Currency;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::OpenApi;
use utoipa::ToSchema;
use uuid::Uuid;

/// Shared, cheaply-cloned API state.
///
/// `pool` / `org_id` / `store_id` are `None` until the binary is
/// started with `--db` / `--org-id` / `--store-id` (see `main.rs`);
/// the admin handlers 503 until then.
#[derive(Debug, Clone)]
pub struct ApiState {
    pub version: &'static str,
    pub pool: Option<AnyPool>,
    pub org_id: Option<Uuid>,
    pub store_id: Option<Uuid>,
}

impl Default for ApiState {
    fn default() -> Self {
        Self {
            version: "0.3.0",
            pool: None,
            org_id: None,
            store_id: None,
        }
    }
}

// --- schemas ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CounterSwitchRequest {
    /// 4–6 digit counter PIN, or a badge token.
    pub pin: Option<String>,
    pub badge_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PasskeyVerifyRequest {
    pub user_id: String,
    pub response_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OverrideVerifyRequest {
    pub user_id: String,
    pub pin: String,
    pub reason_code: String,
}

/// One colorant level with its catalog identity (admin list view).
/// UUIDs and decimals are strings here, matching their JSON form —
/// `Uuid` / `Decimal` do not implement utoipa's `ToSchema`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ColorantLevelResponse {
    pub colorant_id: String,
    pub code: String,
    pub name: String,
    pub on_hand_fl_oz: String,
    pub low_threshold_fl_oz: String,
    pub is_low: bool,
}

/// Body for `POST /api/v1/tinting/colorants/levels`. UUIDs and
/// decimals are strings, matching their JSON form.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetColorantLevelRequest {
    pub colorant_id: String,
    pub store_id: String,
    pub on_hand_fl_oz: String,
    pub low_threshold_fl_oz: String,
}

#[derive(Debug, Deserialize)]
pub struct ColorSearchQuery {
    pub manufacturer: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FormulaQuery {
    pub query: Option<String>,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct StoreQuery {
    pub store_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ReportQuery {
    pub store_id: Option<Uuid>,
    pub date: Option<String>,
}

// --- helpers ---------------------------------------------------------------

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
}

fn not_implemented(feature: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorResponse {
            error: format!("{feature} is not implemented in Phase 0 (v0.1)"),
        }),
    )
}

/// The configured pool, or a 503 explaining how to configure one.
fn pool(state: &ApiState) -> Result<AnyPool, (StatusCode, Json<ErrorResponse>)> {
    state.pool.clone().ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no database configured for this API instance — restart df-api with --db <url> --org-id <uuid>",
        )
    })
}

fn org_id(state: &ApiState) -> Result<Uuid, (StatusCode, Json<ErrorResponse>)> {
    state.org_id.ok_or_else(|| {
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            "no org configured for this API instance — restart df-api with --org-id <uuid>",
        )
    })
}

/// Store id from the query, falling back to the instance default.
fn store_id(
    query_store: Option<Uuid>,
    state: &ApiState,
) -> Result<Uuid, (StatusCode, Json<ErrorResponse>)> {
    query_store.or(state.store_id).ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "store_id is required (query ?store_id= or df-api --store-id)",
        )
    })
}

/// `?date=YYYY-MM-DD`, defaulting to today (UTC).
fn report_date(date: Option<String>) -> Result<NaiveDate, (StatusCode, Json<ErrorResponse>)> {
    match date {
        None => Ok(chrono::Utc::now().date_naive()),
        Some(text) => NaiveDate::from_str(&text).map_err(|_| {
            err(
                StatusCode::BAD_REQUEST,
                format!("date must be YYYY-MM-DD, got {text:?}"),
            )
        }),
    }
}

fn db_err(e: df_db::DbError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match &e {
        df_db::DbError::NotFound(_) => StatusCode::NOT_FOUND,
        df_db::DbError::InvalidData(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    err(status, e.to_string())
}

// --- handlers --------------------------------------------------------------

#[utoipa::path(
    get,
    path = "/health",
    responses((status = 200, description = "Service is up", body = HealthResponse))
)]
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 501, description = "Not implemented in Phase 0", body = ErrorResponse),
        (status = 200, description = "Authenticated (future)", body = ErrorResponse)
    )
)]
pub async fn login(Json(_body): Json<LoginRequest>) -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("password login")
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/counter-switch",
    request_body = CounterSwitchRequest,
    responses((status = 501, description = "Not implemented in Phase 0", body = ErrorResponse))
)]
pub async fn counter_switch(
    Json(_body): Json<CounterSwitchRequest>,
) -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("counter PIN/badge switch")
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/passkey/verify",
    request_body = PasskeyVerifyRequest,
    responses((status = 501, description = "Not implemented in Phase 0", body = ErrorResponse))
)]
pub async fn passkey_verify(
    Json(_body): Json<PasskeyVerifyRequest>,
) -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("passkey verification")
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/oidc/callback",
    responses((status = 501, description = "Not implemented in Phase 0", body = ErrorResponse))
)]
pub async fn oidc_callback() -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("OIDC callback")
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/override-verify",
    request_body = OverrideVerifyRequest,
    responses((status = 501, description = "Not implemented in Phase 0", body = ErrorResponse))
)]
pub async fn override_verify(
    Json(_body): Json<OverrideVerifyRequest>,
) -> (StatusCode, Json<ErrorResponse>) {
    not_implemented("manager override verification")
}

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

// --- OpenAPI document ------------------------------------------------------

#[derive(OpenApi)]
#[openapi(
    paths(
        health,
        login,
        counter_switch,
        passkey_verify,
        oidc_callback,
        override_verify,
        search_colors,
        list_formulas,
        list_colorants,
        set_colorant_level,
        daily_usage,
        low_stock,
        shortage_alerts
    ),
    components(schemas(
        HealthResponse,
        ErrorResponse,
        LoginRequest,
        CounterSwitchRequest,
        PasskeyVerifyRequest,
        OverrideVerifyRequest,
        ColorantLevelResponse,
        SetColorantLevelRequest
    )),
    tags(
        (name = "auth", description = "Authentication & counter switching"),
        (name = "tinting", description = "Paint & tinting admin (Phase 2)"),
        (name = "reports", description = "Daily usage, low-stock and shortage reports (Phase 2)")
    )
)]
pub struct ApiDoc;

/// Serve the generated OpenAPI 3.1 document as JSON.
pub async fn openapi_json() -> Json<serde_json::Value> {
    let mut doc = ApiDoc::openapi();
    doc.info.title = "decor-pos API".to_string();
    doc.info.version = env!("CARGO_PKG_VERSION").to_string();
    Json(serde_json::to_value(doc).expect("OpenAPI document serialises"))
}

/// Build the router. Admin handlers read their pool/org/store from
/// the passed [`ApiState`]; with the default state they 503.
pub fn create_router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api-docs/openapi.json", get(openapi_json))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/counter-switch", post(counter_switch))
        .route("/api/v1/auth/passkey/verify", post(passkey_verify))
        .route("/api/v1/auth/oidc/callback", get(oidc_callback))
        .route("/api/v1/auth/override-verify", post(override_verify))
        .route("/api/v1/colors/search", get(search_colors))
        .route(
            "/api/v1/tinting/formulas",
            get(list_formulas).post(create_formula),
        )
        .route("/api/v1/tinting/colorants", get(list_colorants))
        .route("/api/v1/tinting/colorants/levels", post(set_colorant_level))
        .route("/api/v1/reports/daily-usage", get(daily_usage))
        .route("/api/v1/reports/low-stock", get(low_stock))
        .route("/api/v1/reports/shortage-alerts", get(shortage_alerts))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use chrono::Utc;
    use df_db::models::{ColorCatalogEntry, Colorant, Organization, Store};
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use tower::ServiceExt;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    async fn seeded_state() -> ApiState {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
        df_db::insert_organization(
            &pool,
            &Organization {
                id: org_id,
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
            &pool,
            &Store {
                id: store_id,
                org_id,
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
        df_db::insert_color(
            &pool,
            &ColorCatalogEntry {
                id: Uuid::from_u128(0x6000),
                org_id,
                manufacturer: "Benjamin Moore".to_string(),
                color_code: "OC-17".to_string(),
                color_name: "White Dove".to_string(),
                collection: None,
                rgb_hex: None,
                recommended_base: None,
            },
        )
        .await
        .unwrap();
        // The formula's base product must exist (SQLite enforces
        // the `tint_formulas.base_product_id -> products(id)` FK).
        df_db::insert_product(
            &pool,
            &df_db::models::Product {
                id: Uuid::from_u128(0x3000),
                org_id,
                sku: "BM-REGAL-1G".to_string(),
                manufacturer_code: Some("BM".to_string()),
                name: "Regal Select Interior Gallon".to_string(),
                department_id: Uuid::now_v7(),
                class_id: None,
                product_line_id: None,
                primary_vendor_id: None,
                product_type: "paint_base".to_string(),
                base_uom: "gal".to_string(),
                list_price: dec("100.0000"),
                standard_cost: dec("50.0000"),
                avg_cost: dec("50.0000"),
                min_margin_pct: None,
                sheen: Some("Eggshell".to_string()),
                paint_base_code: Some("1X".to_string()),
                container_volume_ml: Some(3785),
                eco_fee_category: Some("architectural_paint".to_string()),
                tax_class_id: Uuid::now_v7(),
                prompt_template_id: None,
                is_active: true,
            },
        )
        .await
        .unwrap();
        let y3 = Uuid::from_u128(0x5001);
        df_db::insert_colorant(
            &pool,
            &Colorant {
                id: y3,
                org_id,
                system_name: "Gennex".to_string(),
                code: "Y3".to_string(),
                name: "Yellow Oxide".to_string(),
                cost_per_fl_oz: dec("0.4200"),
                shots_per_fl_oz: 48,
            },
        )
        .await
        .unwrap();
        df_db::upsert_colorant_level(
            &pool,
            &ColorantLevelRow {
                colorant_id: y3,
                store_id,
                on_hand_fl_oz: dec("1"),
                low_threshold_fl_oz: dec("2"),
                updated_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        ApiState {
            version: "0.3.0",
            pool: Some(pool),
            org_id: Some(org_id),
            store_id: Some(store_id),
        }
    }

    async fn get_json(router: Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn post_json(
        router: Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let response = router
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let router = create_router(ApiState::default());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["version"], "0.3.0");
    }

    #[tokio::test]
    async fn openapi_document_is_3_1() {
        let router = create_router(ApiState::default());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api-docs/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["openapi"], "3.1.0");
        assert!(value["paths"]["/health"].is_object());
        assert!(value["paths"]["/api/v1/auth/login"].is_object());
        assert!(value["paths"]["/api/v1/tinting/formulas"].is_object());
        assert!(value["paths"]["/api/v1/reports/daily-usage"].is_object());
    }

    #[tokio::test]
    async fn auth_routes_are_501_in_phase_0() {
        let router = create_router(ApiState::default());
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/auth/login")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"email":"a@example.invalid","password":"not-a-real-password"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn admin_endpoints_503_without_a_database() {
        let router = create_router(ApiState::default());
        let (status, body) = get_json(
            router,
            "/api/v1/reports/shortage-alerts?store_id=00000000-0000-7000-8000-000000000001",
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["error"].as_str().unwrap().contains("--db"));
    }

    #[tokio::test]
    async fn tinting_admin_round_trip() {
        let state = seeded_state().await;
        let store_id = state.store_id.unwrap();
        let org_id = state.org_id.unwrap();
        let router = create_router(state);
        let _ = org_id;

        // Color search finds White Dove.
        let (status, body) = get_json(
            router.clone(),
            "/api/v1/colors/search?manufacturer=Benjamin%20Moore&query=dove",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["color_code"], "OC-17");

        // Create a formula, then find it by customer + by color.
        let customer_id = Uuid::from_u128(0x4000);
        let formula_id = Uuid::from_u128(0x7000);
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/tinting/formulas",
            serde_json::json!({
                "id": formula_id.to_string(),
                "org_id": Uuid::from_u128(0x2000).to_string(),
                "store_id": store_id.to_string(),
                "customer_id": customer_id.to_string(),
                "job_id": null,
                "color_id": Uuid::from_u128(0x6000).to_string(),
                "color_code_snapshot": "OC-17",
                "color_name_snapshot": "White Dove",
                "base_product_id": Uuid::from_u128(0x3000).to_string(),
                "container_volume_ml": 3785,
                "sheen": "Eggshell",
                "room_or_usage": null,
                "is_custom_match": false,
                "colorants_json": [{"code": "Y3", "oz": 0, "shots": 14}],
                "total_tint_cost": "0.1225",
                "tint_surcharge": "0.0000",
                "dispenser_name": "virtual-simulator",
                "external_formula_ref": null,
                "dispensed_at": Utc::now().to_rfc3339(),
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["id"], formula_id.to_string());
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/tinting/formulas?customer_id={customer_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);
        let (status, body) = get_json(
            router.clone(),
            "/api/v1/tinting/formulas?query=white%20dove",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);

        // Colorant levels: Y3 is low (1 on hand, threshold 2).
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/tinting/colorants?store_id={store_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["levels"].as_array().unwrap().len(), 1);
        assert_eq!(body["levels"][0]["code"], "Y3");
        assert_eq!(body["levels"][0]["is_low"], true);
        assert_eq!(body["alerts"].as_array().unwrap().len(), 1);

        // Raise the level above threshold; the alert clears.
        let (status, body) = post_json(
            router.clone(),
            "/api/v1/tinting/colorants/levels",
            serde_json::json!({
                "colorant_id": Uuid::from_u128(0x5001).to_string(),
                "store_id": store_id.to_string(),
                "on_hand_fl_oz": "10",
                "low_threshold_fl_oz": "2",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["on_hand_fl_oz"], "10");
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/reports/shortage-alerts?store_id={store_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["alerts"].as_array().unwrap().is_empty());

        // Reports work (empty day, no tints yet).
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/reports/daily-usage?store_id={store_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["tints_dispensed"], 0);
        let (status, body) = get_json(
            router.clone(),
            &format!("/api/v1/reports/low-stock?store_id={store_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["colorants"].as_array().unwrap().is_empty());
    }
}
