//! Vendor EDI handlers (Phase 4): X12 850/855/856/810/832, 3-way match,
//! transport records, and vendor catalog CSV import.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use df_edi::EdiDocument;
use serde::Deserialize;
use uuid::Uuid;

use super::{HandlerResult, err_response};
use crate::edge::Edge;
use crate::edi::{
    CatalogImportResponse, IntakeRequest, IntakeResponse, InvoiceMatchReport,
    InvoiceMatchReportRow, MarkPriceApplicationAppliedRequest, PoAcknowledgmentRow,
    PriceCatalogApplicationRow, SendPoRequest, SendPoResponse, TransportRecord,
};

#[derive(Debug, Deserialize)]
pub struct PoQuery {
    pub po_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct TransportQuery {
    pub vendor_id: String,
}

#[derive(Debug, Deserialize)]
pub struct CatalogImportBody {
    pub csv_text: String,
    pub vendor_id: String,
}

pub(super) async fn send_purchase_order(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SendPoRequest>,
) -> HandlerResult<SendPoResponse> {
    edge.send_purchase_order(id, body.out_dir)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn intake_edi(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<IntakeRequest>,
) -> HandlerResult<IntakeResponse> {
    edge.intake_edi(body).await.map(Json).map_err(err_response)
}

pub(super) async fn list_edi_documents(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<PoQuery>,
) -> HandlerResult<Vec<EdiDocument>> {
    edge.list_edi_documents(query.po_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_edi_document(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<EdiDocument> {
    edge.get_edi_document(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn three_way_match(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<PoQuery>,
) -> HandlerResult<InvoiceMatchReport> {
    edge.three_way_match(query.po_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_match_reports(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<PoQuery>,
) -> HandlerResult<Vec<InvoiceMatchReportRow>> {
    edge.list_match_reports(query.po_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_po_acknowledgments(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<PoQuery>,
) -> HandlerResult<Vec<PoAcknowledgmentRow>> {
    edge.list_po_acknowledgments(query.po_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn get_price_catalog_application(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<PriceCatalogApplicationRow> {
    edge.get_price_catalog_application(id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn mark_price_application_applied(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<MarkPriceApplicationAppliedRequest>,
) -> HandlerResult<PriceCatalogApplicationRow> {
    edge.mark_price_application_applied(id, &body.reviewed_by)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn list_transport_records(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<TransportQuery>,
) -> HandlerResult<Vec<TransportRecord>> {
    edge.list_transport_records(&query.vendor_id)
        .await
        .map(Json)
        .map_err(err_response)
}

pub(super) async fn import_catalog(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CatalogImportBody>,
) -> HandlerResult<CatalogImportResponse> {
    edge.import_vendor_catalog(&body.csv_text, &body.vendor_id)
        .await
        .map(Json)
        .map_err(err_response)
}
