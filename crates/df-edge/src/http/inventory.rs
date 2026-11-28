//! Inventory, purchasing, vendor, transfer, physical count, pricing,
//! and contractor AR handlers (Phase 3+).

use std::str::FromStr;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::NaiveDate;
use df_db::InventoryBalance;
use df_db::models::{PricingRuleRow, ReorderPoint, Store};
use df_domain::Money;
use serde::Deserialize;
use uuid::Uuid;

use super::{ErrorResponse, HandlerResult, err_response, store_or_default};
use crate::ar::PostedArPayment;
use crate::edge::Edge;
use crate::error::EdgeError;
use crate::inventory::{
    AgingView, ArCustomerSummary, BulkPricingRequest, CollectionsView, CountReconcileResponse,
    CreatePurchaseOrderRequest, CreateTransferRequest, ImportProductsRequest, PhysicalCount,
    PurchaseOrderView, ReceivePurchaseOrderRequest, ReplenishmentSuggestRequest, ScanCountRequest,
    SetReorderPointRequest, ShelfTagResponse, StartCountRequest, SuggestedPurchaseOrder, Transfer,
};
use crate::vendors::{CreateVendorRequest, UpdateVendorRequest, VendorDetail, VendorView};

// --- inventory / purchasing / AR (Phase 3) ---------------------------------

#[derive(Debug, Deserialize)]
pub struct BalancesQuery {
    pub store_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ShelfTagQuery {
    pub product_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct ArAgingQuery {
    pub store_id: Option<Uuid>,
    pub as_of: Option<String>,
}

/// Counter AR payment (`POST /ar/payments`): record a payment
/// against a charge account. `method` is a label only (`Cash`,
/// `Card`, `E-Transfer`, `Cheque`) — no provider authorization
/// happens here; the payment posts to the AR ledger with FIFO
/// allocation via `Edge::post_ar_payment`.
#[derive(Debug, Deserialize)]
pub struct ArPaymentRequest {
    pub customer_id: Uuid,
    pub amount: Money,
    pub method: String,
}

/// `?as_of=YYYY-MM-DD`, defaulting to today (UTC). A malformed date
/// is a 400, not a silent default.
fn aging_date(as_of: &Option<String>) -> Result<NaiveDate, (StatusCode, Json<ErrorResponse>)> {
    match as_of {
        None => Ok(chrono::Utc::now().date_naive()),
        Some(text) => NaiveDate::from_str(text).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("as_of must be YYYY-MM-DD, got {text:?}"),
                }),
            )
        }),
    }
}

pub(super) async fn get_balances(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<InventoryBalance>> {
    edge.inventory_balances(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

/// Stores in this org, by code — the destination picker for
/// inter-store transfers (the back office only ever typed a store
/// UUID by hand; there was no list endpoint).
pub(super) async fn list_stores(State(edge): State<Arc<Edge>>) -> HandlerResult<Vec<Store>> {
    df_db::list_stores_for_org(edge.pool(), edge.config().org_id)
        .await
        .map(Json)
        .map_err(EdgeError::from)
        .map_err(err_response)
}

pub(super) async fn list_reorder_points(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<ReorderPoint>> {
    edge.list_reorder_points(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn set_reorder_point(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<SetReorderPointRequest>,
) -> HandlerResult<ReorderPoint> {
    edge.set_reorder_point(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn import_products(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<ImportProductsRequest>,
) -> HandlerResult<df_db::ProductImportResult> {
    edge.import_products(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn suggest_replenishment(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<ReplenishmentSuggestRequest>,
) -> HandlerResult<Vec<SuggestedPurchaseOrder>> {
    edge.suggest_replenishment(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn create_purchase_order(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreatePurchaseOrderRequest>,
) -> HandlerResult<PurchaseOrderView> {
    edge.create_purchase_order(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_purchase_orders(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<PurchaseOrderView>> {
    edge.list_purchase_orders(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn receive_purchase_order(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<ReceivePurchaseOrderRequest>,
) -> HandlerResult<PurchaseOrderView> {
    edge.receive_purchase_order(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

// --- vendors ---------------------------------------------------------------

pub(super) async fn list_vendors(State(edge): State<Arc<Edge>>) -> HandlerResult<Vec<VendorView>> {
    edge.list_vendors().await.map(Json).map_err(err_response)
}

pub(super) async fn get_vendor(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<VendorDetail> {
    edge.get_vendor(id).await.map(Json).map_err(err_response)
}

pub(super) async fn create_vendor(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateVendorRequest>,
) -> HandlerResult<VendorView> {
    edge.create_vendor(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn update_vendor(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateVendorRequest>,
) -> HandlerResult<VendorView> {
    edge.update_vendor(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn create_transfer(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateTransferRequest>,
) -> HandlerResult<Transfer> {
    edge.create_transfer(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_transfers(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<Transfer>> {
    edge.list_transfers(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn ship_transfer(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Transfer> {
    edge.ship_transfer(id).await.map(Json).map_err(err_response)
}

pub(super) async fn receive_transfer(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Transfer> {
    edge.receive_transfer(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn start_count(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<StartCountRequest>,
) -> HandlerResult<PhysicalCount> {
    edge.start_count(body).await.map(Json).map_err(err_response)
}

pub(super) async fn list_counts(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<PhysicalCount>> {
    edge.list_counts(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn scan_count(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<ScanCountRequest>,
) -> HandlerResult<PhysicalCount> {
    edge.scan_count(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn reconcile_count(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<CountReconcileResponse> {
    edge.reconcile_count(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn shelf_tag(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ShelfTagQuery>,
) -> HandlerResult<ShelfTagResponse> {
    edge.shelf_tag(query.product_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_pricing_rules(
    State(edge): State<Arc<Edge>>,
) -> HandlerResult<Vec<PricingRuleRow>> {
    edge.list_pricing_rules()
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn bulk_pricing(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<BulkPricingRequest>,
) -> HandlerResult<Vec<PricingRuleRow>> {
    edge.bulk_update_pricing_rules(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn ar_customer_summary(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<ArCustomerSummary> {
    edge.ar_customer_summary(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn ar_aging(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ArAgingQuery>,
) -> Result<Json<AgingView>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = aging_date(&query.as_of)?;
    edge.ar_aging(store_or_default(&edge, query.store_id), as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn ar_collections(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ArAgingQuery>,
) -> Result<Json<CollectionsView>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = aging_date(&query.as_of)?;
    edge.ar_collections(store_or_default(&edge, query.store_id), as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn post_ar_payment(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<ArPaymentRequest>,
) -> HandlerResult<PostedArPayment> {
    edge.post_ar_payment(body.customer_id, body.amount, &body.method)
        .await
        .map(Json)
        .map_err(err_response)
}
