//! POS counter handlers: checkout, tills, holds, quotes, layaways, sync,
//! counter search/customers/invoices, tinting, and Phase 2 reports.

use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use chrono::NaiveDate;
use df_auth_http::AuthSession;
use df_db::models::{
    ColorCatalogEntry, Customer, PriceLevel, TillSession, TintFormula, Transaction,
};
use df_domain::Money;
use df_reports::{DailyUsageReport, LowStockReport, ProductThreshold};
use df_sync::{ApplyReport, SyncState};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ErrorResponse, HandlerResult, err_response};
use crate::counter::{ProductSearchResult, TransactionView};
use crate::edge::Edge;
use crate::invoice::InvoiceView;
use crate::tinting::{ColorantView, CreateTintFormulaRequest};
use crate::types::{CompletedSale, PricePreview, ReturnLineInput, SaleInput, TenderInput, ZReport};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenTillRequest {
    pub cashier_id: Uuid,
    pub opening_float: Money,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseTillRequest {
    pub counted_cash: Money,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoidResponse {
    pub transaction_id: Uuid,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldResponse {
    pub hold_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldsResponse {
    pub hold_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionIdResponse {
    pub transaction_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayawayRequest {
    pub sale: SaleInput,
    pub deposit: TenderInput,
}

pub(super) async fn open_till(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Json(body): Json<OpenTillRequest>,
) -> HandlerResult<TillSession> {
    // The session's user is the cashier: the caller-supplied
    // `cashier_id` is accepted for wire compatibility but ignored,
    // so a till can never be opened as another user. Checkout
    // inherits the till's cashier (see `Edge::checkout`), so the
    // sale's `cashier_id` is the session user too. See docs/auth.md.
    edge.open_till(auth.user_id, body.opening_float)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn close_till(
    State(edge): State<Arc<Edge>>,
    Path(till_id): Path<Uuid>,
    Json(body): Json<CloseTillRequest>,
) -> HandlerResult<ZReport> {
    edge.close_till(till_id, body.counted_cash)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn price_preview(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<SaleInput>,
) -> HandlerResult<PricePreview> {
    edge.price_preview(&body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn checkout(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Json(body): Json<SaleInput>,
) -> HandlerResult<CompletedSale> {
    edge.checkout_with_default_provider_as(body, Some(auth.user_id))
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn void_transaction(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<VoidResponse> {
    edge.void_transaction(id)
        .await
        .map(|tx: Transaction| {
            Json(VoidResponse {
                transaction_id: tx.id,
                status: tx.status,
            })
        })
        .map_err(err_response)
}

pub(super) async fn return_transaction(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Path(id): Path<Uuid>,
    Json(lines): Json<Vec<ReturnLineInput>>,
) -> HandlerResult<CompletedSale> {
    edge.record_return_as(id, lines, Some(auth.user_id))
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn park_hold(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Json(body): Json<SaleInput>,
) -> HandlerResult<HoldResponse> {
    edge.park_hold_as(body, Some(auth.user_id))
        .await
        .map(|hold_id| Json(HoldResponse { hold_id }))
        .map_err(err_response)
}

pub(super) async fn list_holds(State(edge): State<Arc<Edge>>) -> HandlerResult<HoldsResponse> {
    Ok(Json(HoldsResponse {
        hold_ids: edge.list_holds().await,
    }))
}

pub(super) async fn resume_hold(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<SaleInput> {
    edge.resume_hold(id).await.map(Json).map_err(err_response)
}

pub(super) async fn record_quote(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Json(body): Json<SaleInput>,
) -> HandlerResult<TransactionIdResponse> {
    edge.record_quote_as(body, Some(auth.user_id))
        .await
        .map(|transaction_id| Json(TransactionIdResponse { transaction_id }))
        .map_err(err_response)
}

pub(super) async fn record_layaway(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Json(body): Json<LayawayRequest>,
) -> HandlerResult<TransactionIdResponse> {
    edge.record_layaway_with_default_provider_as(body.sale, body.deposit, Some(auth.user_id))
        .await
        .map(|transaction_id| Json(TransactionIdResponse { transaction_id }))
        .map_err(err_response)
}

pub(super) async fn sync_state(State(edge): State<Arc<Edge>>) -> HandlerResult<SyncState> {
    edge.sync_state().await.map(Json).map_err(err_response)
}

pub(super) async fn sync_run(State(edge): State<Arc<Edge>>) -> HandlerResult<ApplyReport> {
    edge.sync_now_with_configured_hq()
        .await
        .map(Json)
        .map_err(err_response)
}

#[derive(Debug, Deserialize)]
pub struct DismissDeadLetterRequest {
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct DeadLetterActionResponse {
    pub id: Uuid,
    pub updated: bool,
}

pub(super) async fn list_sync_dead_letters(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<LimitQuery>,
) -> HandlerResult<Vec<df_sync::OutboxEntry>> {
    let limit = i64::try_from(query.limit.unwrap_or(100)).unwrap_or(500);
    edge.list_dead_letters(limit)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn requeue_sync_dead_letter(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeadLetterActionResponse> {
    let updated = edge.requeue_dead_letter(id).await.map_err(err_response)?;
    if !updated {
        return Err(err_response(crate::error::EdgeError::NotFound(format!(
            "dead-letter outbox entry {id} not found"
        ))));
    }
    Ok(Json(DeadLetterActionResponse { id, updated }))
}

pub(super) async fn dismiss_sync_dead_letter(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<DismissDeadLetterRequest>,
) -> HandlerResult<DeadLetterActionResponse> {
    let updated = edge
        .dismiss_dead_letter(id, &body.reason)
        .await
        .map_err(err_response)?;
    if !updated {
        return Err(err_response(crate::error::EdgeError::NotFound(format!(
            "dead-letter outbox entry {id} not found"
        ))));
    }
    Ok(Json(DeadLetterActionResponse { id, updated }))
}

// --- tinting + reports (Phase 2) ------------------------------------------

#[derive(Debug, Deserialize)]
pub struct FormulaQuery {
    pub query: Option<String>,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ColorQuery {
    pub manufacturer: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReportQuery {
    pub date: Option<String>,
}

/// `?date=YYYY-MM-DD`, defaulting to today (UTC). A malformed date
/// is a 400, not a silent default.
fn report_date(query: &ReportQuery) -> Result<NaiveDate, (StatusCode, Json<ErrorResponse>)> {
    match &query.date {
        None => Ok(chrono::Utc::now().date_naive()),
        Some(text) => NaiveDate::from_str(text).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("date must be YYYY-MM-DD, got {text:?}"),
                }),
            )
        }),
    }
}

pub(super) async fn list_formulas(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<FormulaQuery>,
) -> HandlerResult<Vec<TintFormula>> {
    edge.search_tint_formulas(query.query.as_deref(), query.customer_id, query.job_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn create_tint_formula(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateTintFormulaRequest>,
) -> Result<(StatusCode, Json<TintFormula>), (StatusCode, Json<ErrorResponse>)> {
    edge.create_tint_formula(body)
        .await
        .map(|formula| (StatusCode::CREATED, Json(formula)))
        .map_err(err_response)
}

// --- counter search / customers / transactions / invoice -----------

#[derive(Debug, Deserialize)]
pub struct ProductSearchQuery {
    pub q: Option<String>,
    pub sku: Option<String>,
    pub barcode: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct CustomerSearchQuery {
    pub q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LimitQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct SetPriceLevelRequest {
    pub price_level_id: Option<Uuid>,
}

pub(super) async fn search_products(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ProductSearchQuery>,
) -> HandlerResult<Vec<ProductSearchResult>> {
    edge.search_products(
        query.q.as_deref(),
        query.sku.as_deref(),
        query.barcode.as_deref(),
        query.limit,
    )
    .await
    .map(Json)
    .map_err(err_response)
}

pub(super) async fn search_customers(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<CustomerSearchQuery>,
) -> HandlerResult<Vec<Customer>> {
    edge.search_customers(query.q.as_deref())
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_customer(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Customer> {
    df_db::get_customer(edge.pool(), id)
        .await
        .map(Json)
        .map_err(|err| err_response(err.into()))
}

pub(super) async fn set_customer_price_level(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetPriceLevelRequest>,
) -> HandlerResult<Customer> {
    edge.set_customer_price_level(id, body.price_level_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn customer_transactions(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Query(query): Query<LimitQuery>,
) -> HandlerResult<Vec<TransactionView>> {
    edge.customer_transactions(id, query.limit)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_transaction(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<TransactionView> {
    edge.transaction_view(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_invoice(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<InvoiceView> {
    edge.invoice(id).await.map(Json).map_err(err_response)
}

pub(super) async fn list_price_levels(
    State(edge): State<Arc<Edge>>,
) -> HandlerResult<Vec<PriceLevel>> {
    edge.list_price_levels()
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_colors(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ColorQuery>,
) -> HandlerResult<Vec<ColorCatalogEntry>> {
    let manufacturer = query.manufacturer.as_deref().unwrap_or("");
    let q = query.query.as_deref().unwrap_or("");
    edge.search_colors(manufacturer, q)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_colorants(
    State(edge): State<Arc<Edge>>,
) -> HandlerResult<Vec<ColorantView>> {
    edge.list_colorants().await.map(Json).map_err(err_response)
}

pub(super) async fn daily_usage(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ReportQuery>,
) -> Result<Json<DailyUsageReport>, (StatusCode, Json<ErrorResponse>)> {
    let date = report_date(&query)?;
    edge.daily_usage(date).await.map(Json).map_err(err_response)
}

pub(super) async fn low_stock(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ReportQuery>,
) -> Result<Json<LowStockReport>, (StatusCode, Json<ErrorResponse>)> {
    let date = report_date(&query)?;
    edge.low_stock(date).await.map(Json).map_err(err_response)
}

pub(super) async fn set_thresholds(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<Vec<ProductThreshold>>,
) -> Json<ThresholdsResponse> {
    let count = body.len();
    edge.set_product_thresholds(body);
    Json(ThresholdsResponse {
        thresholds_set: count,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThresholdsResponse {
    pub thresholds_set: usize,
}
