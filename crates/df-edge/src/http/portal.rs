//! Contractor portal, retail BOPIS storefront, and mobile KPI handlers (Phase 5).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use df_auth_http::AuthSession;
use df_db::models::{PortalPaymentRow, Product, StatementRow, TintFormula, Transaction};
use df_portal::{BillPayIntent, BillPayResult, BopisOrderRequest, MobileKpis, ReorderRequest};
use uuid::Uuid;

use super::{ErrorResponse, HandlerResult, StoreQuery, err_response, store_or_default};
use crate::edge::Edge;
use crate::portal::{
    BopisOrderResponse, BopisOrderView, PortalCustomerView, PortalReorderResponse,
};

// --- contractor portal / BOPIS / mobile KPIs (Phase 5, v0.6) -------------
/// Build the edge router. The caller passes an `Arc<Edge>` as state.
/// Portal scope (F4): the session may access exactly the
/// contractor customer it was issued for. Anything else is a 403 —
/// never a 404, so the check does not leak which customers exist.
fn check_portal_scope(
    auth: &AuthSession,
    customer_id: Uuid,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if auth.may_access_customer(customer_id) {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "portal session is scoped to a different customer".to_string(),
            }),
        ))
    }
}

pub(super) async fn portal_customer(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Path(id): Path<Uuid>,
) -> HandlerResult<PortalCustomerView> {
    check_portal_scope(&auth, id)?;
    edge.portal_customer(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn portal_invoices(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Vec<Transaction>> {
    check_portal_scope(&auth, id)?;
    edge.portal_invoices(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn portal_statements(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Vec<StatementRow>> {
    check_portal_scope(&auth, id)?;
    edge.portal_statements(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn portal_formulas(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Vec<TintFormula>> {
    check_portal_scope(&auth, id)?;
    edge.portal_formulas(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn portal_payments_list(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Vec<PortalPaymentRow>> {
    check_portal_scope(&auth, id)?;
    edge.portal_payments(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn portal_reorder(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Json(body): Json<ReorderRequest>,
) -> HandlerResult<PortalReorderResponse> {
    check_portal_scope(&auth, body.customer_id)?;
    edge.portal_reorder(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn portal_pay_bill(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
    Json(body): Json<BillPayIntent>,
) -> HandlerResult<BillPayResult> {
    check_portal_scope(&auth, body.customer_id)?;
    edge.portal_pay_bill(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn catalog_products(State(edge): State<Arc<Edge>>) -> HandlerResult<Vec<Product>> {
    edge.catalog_products()
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn bopis_order_create(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<BopisOrderRequest>,
) -> HandlerResult<BopisOrderResponse> {
    edge.record_bopis_order(body)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn bopis_order_get(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<BopisOrderView> {
    edge.bopis_order(id).await.map(Json).map_err(err_response)
}

pub(super) async fn bopis_pickup_complete(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<BopisOrderView> {
    edge.complete_bopis_pickup(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn mobile_kpis(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<StoreQuery>,
) -> HandlerResult<MobileKpis> {
    edge.mobile_kpis(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}
