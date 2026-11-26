//! Local HTTP API the POS talks to (spec §6.1, edge subset).
//!
//! Routes (JSON in/out, `Money` serializes as
//! `{"amount": "12.3400", "currency": "Usd"}`):
//!
//! * `GET /health`
//! * `POST /pos/tills/open` — `{cashier_id, opening_float}`
//! * `POST /pos/tills/{id}/close` — `{counted_cash}`
//! * `POST /pos/carts/price-preview` — [`SaleInput`](crate::types::SaleInput)
//! * `POST /pos/transactions` — [`SaleInput`](crate::types::SaleInput) (checkout)
//! * `POST /pos/transactions/{id}/void`
//! * `POST /pos/transactions/{id}/return` — [`Vec<ReturnLineInput>`]
//! * `POST /pos/holds` — [`SaleInput`](crate::types::SaleInput) (park; returns `{hold_id}`)
//! * `GET /pos/holds` — `{hold_ids: [Uuid]}` (parked holds, in-memory)
//! * `POST /pos/holds/{id}/resume` — returns the parked [`SaleInput`](crate::types::SaleInput)
//! * `POST /pos/quotes` — [`SaleInput`](crate::types::SaleInput) (returns `{transaction_id}`)
//! * `POST /pos/layaways` — `{sale, deposit}` (returns `{transaction_id}`)
//! * `GET /pos/sync/state`
//! * `POST /pos/sync/run` — syncs against the remote HQ when one is
//!   configured (`DF_HQ_URL` / `DF_HQ_TOKEN`, or
//!   [`Edge::set_remote_hq`](crate::edge::Edge::set_remote_hq)),
//!   else against the in-process [`MemoryHq`] configured with
//!   [`Edge::set_hq`](crate::edge::Edge::set_hq), else `503` with a
//!   clear message.
//! * `GET /pos/tinting/formulas?query=&customer_id=&job_id=` —
//!   search stored formulas by color name/code, or a customer's
//!   colour history (newest first; `job_id` scopes to one job)
//! * `POST /pos/tinting/dispense` — [`DispenseTintRequest`](crate::tinting::DispenseTintRequest)
//! * `GET /pos/tinting/colorants` — levels + shortage alerts
//! * `GET /pos/tinting/colors?manufacturer=&query=` — color search
//! * `GET /pos/reports/daily-usage?date=YYYY-MM-DD` — daily usage
//!   report (defaults to today, UTC)
//! * `GET /pos/reports/low-stock?date=YYYY-MM-DD` — low-stock
//!   report (defaults to today, UTC)
//! * `GET /bi/kpi?store_id=&start=&end=&as_of=` — KPI dashboard
//!   (sales, margin, inventory turns, AR aging, formula usage
//!   tracked only; defaults to today)
//! * `GET /bi/eco?start=&end=&format=json|csv` — PaintCare/eco-fee
//!   remittance by jurisdiction (defaults to the month to date)
//! * `GET /bi/inquiry?store_id=&start=&end=&customer_id=&product_id=&doc_type=`
//!   — ad-hoc inquiry over completed transactions
//! * `GET /bi/email-outbox` — queued scheduled-email deliveries
//!   (`manifest.json` in the configured outbox directory; 404 when
//!   no outbox directory is configured)
//! * `POST /pos/reports/low-stock/thresholds` — set the
//!   operator-configured product thresholds the low-stock report
//!   uses (`Vec<ProductThreshold>`)
//! * `GET /inventory/balances?store_id=` — stock positions
//!   (on-hand / committed / on-order / in-transit) per product
//! * `GET /inventory/reorder-points` + `POST /inventory/reorder-points` —
//!   min/max reorder points (set one per call)
//! * `POST /purchasing/replenishment/suggest` — min/max + velocity
//!   replenishment suggestions (one preview PO per vendor)
//! * `POST /purchasing/orders` + `GET /purchasing/orders?store_id=` —
//!   create / list purchase orders
//! * `POST /purchasing/orders/{id}/receive` — receive per-line
//!   quantities (posts `po_receive` ledger entries, updates status)
//! * `POST /inventory/transfers` + `GET /inventory/transfers` —
//!   draft / list inter-store transfers
//! * `POST /inventory/transfers/{id}/ship` — posts the in-transit leg
//! * `POST /inventory/transfers/{id}/receive` — closes in-transit at
//!   the source, gains stock at the destination
//! * `POST /inventory/counts` + `GET /inventory/counts` — start /
//!   list physical counts
//! * `POST /inventory/counts/{id}/scan` — counted qty per product
//! * `POST /inventory/counts/{id}/reconcile` — variance posts
//!   `count_adj` ledger entries
//! * `GET /inventory/shelf-tags?product_id=` — printable ZPL shelf tag
//! * `GET /pricing/rules` + `POST /pricing/rules` — bulk matrix
//!   pricing editor (min-margin guardrail enforced)
//! * `GET /ar/customers/{id}/summary` — AR balance + credit controls
//! * `GET /ar/aging?store_id=&as_of=` — 30/60/90/120 aging
//! * `GET /ar/collections?store_id=&as_of=` — collections workspace
//! * `POST /crm/activities` + `GET /crm/activities?customer_id=&activity_type=&completed=` —
//!   CRM activity log (create / list)
//! * `GET /crm/activities/{id}` — one activity
//! * `POST /crm/activities/{id}/update` — partial activity update
//! * `POST /crm/activities/{id}/delete` — delete an activity
//! * `POST /crm/activities/{id}/complete` — mark complete
//! * `POST /crm/activities/{id}/snooze` — [`SnoozeActivityRequest`](crate::crm::SnoozeActivityRequest)
//! * `GET /crm/reminders/due?as_of=` — reminder tickler (RFC 3339, defaults to now)
//! * `GET /crm/reminders/overdue?as_of=` — strictly-overdue reminders, most overdue first
//! * `POST /crm/appointments` + `GET /crm/appointments?date=&customer_id=&staff_id=` —
//!   appointments (create / list by day, customer or staff)
//! * `GET /crm/appointments/{id}` — one appointment
//! * `POST /crm/appointments/{id}/update` — partial update (reschedule guarded)
//! * `POST /crm/appointments/{id}/cancel` — cancel
//! * `POST /crm/appointments/{id}/delete` — delete
//! * `POST /crm/saved-views` + `GET /crm/saved-views` — saved filters (either dialect)
//! * `GET /crm/saved-views/{id}/run` — run a saved view over the activity log
//! * `GET /crm/saved-views/{id}/run-customers` — run a saved view over customers
//! * `POST /crm/saved-views/{id}/delete` — delete a saved view
//! * `GET /crm/reports/lapsed?store_id=&as_of=&inactive_days=&include_never=` — lapsed-contractor report
//! * `POST /deliveries` + `GET /deliveries/dispatch?status=&driver_id=&scheduled_date=` —
//!   delivery jobs (create / dispatch board)
//! * `GET /deliveries/{id}` — one delivery, with its POD when submitted
//! * `POST /deliveries/{id}/assign` — assign a driver
//! * `POST /deliveries/{id}/status` — status transition (`delivered` only via POD)
//! * `GET /deliveries/{id}/packing-slip` — packing slip + printable text
//! * `POST /deliveries/{id}/pod` — submit proof of delivery (base64 signature + photo ref)
//! * `GET /deliveries/{id}/pod` — the submitted POD, if any
//! * `POST /edi/purchase-orders/{id}/send` — build + store + stage the X12 850 via the file-drop transport stand-in
//! * `POST /edi/intake` — intake one vendor document (856/810/855/832 X12 or CSV, all parsed)
//! * `GET /edi/documents?po_id=` — EDI documents for a PO
//! * `GET /edi/documents/{id}` — one EDI document
//! * `GET /edi/acknowledgments?po_id=` — stored 855 acknowledgments for a PO
//! * `GET /edi/three-way-match?po_id=` — ordered vs shipped vs billed (`df-edi` report, persisted when both docs are stored)
//! * `GET /edi/match-reports?po_id=` — stored 3-way match reports for a PO
//! * `GET /edi/price-applications/{id}` — one 832 price-apply report
//! * `POST /edi/price-applications/{id}/mark-applied` — record the review decision (reprices nothing)
//! * `GET /edi/transport?vendor_id=` — transport audit trail for a vendor
//! * `POST /edi/catalog/import` — vendor catalog CSV import
//! * `GET /portal/customers/{id}` — portal customer + open AR balance (`portal_enabled` required)
//! * `GET /portal/customers/{id}/invoices` — the contractor's transactions, newest first
//! * `GET /portal/customers/{id}/statements` — the contractor's frozen monthly statements
//! * `GET /portal/customers/{id}/formulas` — the contractor's stored formulas (colour history)
//! * `GET /portal/customers/{id}/payments` — past portal bill payments, newest first
//! * `POST /portal/reorders` — reorder a stored formula as an open order (pickup/delivery)
//! * `POST /portal/payments` — online bill pay: mock-provider authorization, then FIFO AR
//!   allocation; the idempotency key makes a retry a no-op
//! * `GET /catalog/products` — public active-product catalog for the BOPIS storefront
//! * `POST /bopis/orders` — place a BOPIS pickup order (card authorized via the mock provider)
//! * `GET /bopis/orders/{id}` — a BOPIS order with lines + pickup details
//! * `POST /bopis/orders/{id}/complete-pickup` — counter pickup completion (stock to sold)
//! * `GET /mobile/kpis?store_id=` — Outside Sales KPIs: today's sales, open quotes/orders,
//!   total open AR, deliveries out
//! * `POST /scheduling/templates` + `GET /scheduling/templates` — shift templates
//! * `POST /scheduling/shifts` + `GET /scheduling/shifts?date=YYYY-MM-DD` — staff shifts
//!   (overlap-checked per staff member; all times UTC)
//! * `POST /scheduling/shifts/{id}/status` — `{"status": "scheduled"|"confirmed"|"cancelled"}`
//! * `POST /scheduling/timeclock/clock-in` + `POST /scheduling/timeclock/clock-out` —
//!   `{"staff_id": ...}` (one open entry per staff member)
//! * `GET /scheduling/timeclock?date=YYYY-MM-DD` — timeclock entries for one UTC date
//! * `GET /scheduling/forecast?from=YYYY-MM-DD&days=7&sales_per_labor_hour=` — sales-driven
//!   labor forecast (28 days of history; insufficient history is a 400, not an invention)
//! * `GET /scheduling/coverage?date=YYYY-MM-DD` — scheduled vs forecast hours
//!   (`adequate` within ±10%, `under`/`over` outside it, `unknown` with no forecast)
//! * `GET /ws/v1/store/{store_id}` — WebSocket; sends a `connected`
//!   event, then one JSON event per completed sale/return.
//!
//! Handlers are thin wrappers over [`Edge`](crate::edge::Edge)
//! methods; all the rules live there.
//!
//! Phase 7 (v1.1): every route except `GET /health`, the
//! `/pos/auth/*` login endpoints, and the public BOPIS storefront
//! routes (`GET /catalog/products`, `POST /bopis/orders`,
//! `GET /bopis/orders/{id}`) requires a `Bearer dfp_...` session
//! token (see `df_auth_http::gate` + `docs/auth.md` for the role →
//! route matrix). The gate answers 401 for a missing / invalid /
//! expired / revoked token and 403 for a valid token with the wrong
//! role. `POST /pos/tills/open` takes its cashier from the session,
//! ignoring the body's `cashier_id`; portal routes additionally
//! require the session's linked customer to match the requested
//! customer.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Extension, Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use df_reports::{
    AdHocInquiry, DailyUsageReport, EmailDelivery, InquiryRow, KpiDashboard, LowStockReport,
    ProductThreshold,
};
use df_tinter::TinterError;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

use crate::crm::{
    CreateActivityRequest, CreateAppointmentRequest, CreateSavedViewRequest, LapsedReport,
    SavedViewCustomer, SnoozeActivityRequest, UpdateActivityRequest, UpdateAppointmentRequest,
};
use crate::delivery::{
    AssignDeliveryRequest, CreateDeliveryRequest, DeliveryView, PackingSlipView,
    SetDeliveryStatusRequest, SubmitPodRequest,
};
use crate::edge::Edge;
use crate::edi::{
    CatalogImportResponse, IntakeRequest, IntakeResponse, InvoiceMatchReport,
    InvoiceMatchReportRow, MarkPriceApplicationAppliedRequest, PoAcknowledgmentRow,
    PriceCatalogApplicationRow, SendPoRequest, SendPoResponse, TransportRecord,
};
use crate::error::EdgeError;
use crate::inventory::{
    AgingView, ArCustomerSummary, BulkPricingRequest, CollectionsView, CountReconcileResponse,
    CreatePurchaseOrderRequest, CreateTransferRequest, PhysicalCount, PurchaseOrderView,
    ReceivePurchaseOrderRequest, ReplenishmentSuggestRequest, ScanCountRequest,
    SetReorderPointRequest, ShelfTagResponse, StartCountRequest, SuggestedPurchaseOrder, Transfer,
};
use crate::portal::{
    BopisOrderResponse, BopisOrderView, PortalCustomerView, PortalReorderResponse,
};
use crate::scheduling::{
    ClockRequest, CreateShiftTemplateRequest, CreateStaffShiftRequest, SetShiftStatusRequest,
    parse_ratio,
};
use crate::tinting::{ColorantsResponse, DispenseTintRequest, DispenseTintResponse};
use crate::types::{CompletedSale, PricePreview, ReturnLineInput, SaleInput, TenderInput, ZReport};
use df_auth::{OverrideCredential, Permission};
use df_auth_http::{
    AuthRule, AuthSession, Gate, LoginResponse, LogoutResponse, OverrideVerifyResponse, login_error,
};
use df_db::InventoryBalance;
use df_db::models::{
    Appointment, ColorCatalogEntry, CrmActivity, CrmSavedView, DeliveryPod, PortalPaymentRow,
    PricingRuleRow, Product, ReorderPoint, ShiftTemplate, StaffShift, StatementRow, TillSession,
    TimeclockEntry, TintFormula, Transaction,
};
use df_domain::Money;
use df_edi::EdiDocument;
use df_portal::{BillPayIntent, BillPayResult, BopisOrderRequest, MobileKpis, ReorderRequest};
use df_scheduling::{CoverageCheck, ForecastReport};
use df_sync::{ApplyReport, SyncState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub store_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

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

fn err_response(err: EdgeError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match &err {
        EdgeError::NotFound(_) => StatusCode::NOT_FOUND,
        EdgeError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        EdgeError::PaymentDeclined(_) => StatusCode::PAYMENT_REQUIRED,
        EdgeError::Till(_) | EdgeError::CreditBlocked(_) => StatusCode::CONFLICT,
        EdgeError::NoPaymentProvider(_) | EdgeError::NoHq(_) => StatusCode::SERVICE_UNAVAILABLE,
        EdgeError::Sync(_) | EdgeError::Payment(_) => StatusCode::BAD_GATEWAY,
        EdgeError::Tinter(TinterError::InsufficientColorant { .. }) => StatusCode::CONFLICT,
        EdgeError::Tinter(_) => StatusCode::BAD_REQUEST,
        EdgeError::Edi(
            df_edi::EdiError::Sqlx(_) | df_edi::EdiError::Db(_) | df_edi::EdiError::Io(_),
        ) => StatusCode::INTERNAL_SERVER_ERROR,
        EdgeError::Edi(_) => StatusCode::BAD_REQUEST,
        EdgeError::Crm(_) | EdgeError::Delivery(_) => StatusCode::BAD_REQUEST,
        EdgeError::Scheduling(df_scheduling::SchedulingError::ShiftOverlap { .. })
        | EdgeError::Scheduling(df_scheduling::SchedulingError::AlreadyClockedIn(_, _)) => {
            StatusCode::CONFLICT
        }
        EdgeError::Scheduling(_) => StatusCode::BAD_REQUEST,
        EdgeError::Portal(df_portal::PortalError::NotPortalEnabled) => StatusCode::FORBIDDEN,
        EdgeError::Portal(_) => StatusCode::BAD_REQUEST,
        EdgeError::Db(df_db::DbError::NotFound(_)) => StatusCode::NOT_FOUND,
        EdgeError::Db(df_db::DbError::InvalidData(_)) => StatusCode::BAD_REQUEST,
        EdgeError::Db(df_db::DbError::PodIncomplete { .. }) => StatusCode::BAD_REQUEST,
        EdgeError::Db(
            df_db::DbError::IllegalDeliveryTransition { .. }
            | df_db::DbError::AppointmentOverlap { .. },
        ) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(ErrorResponse {
            error: err.to_string(),
        }),
    )
}

type HandlerResult<T> = Result<Json<T>, (StatusCode, Json<ErrorResponse>)>;

async fn health(State(edge): State<Arc<Edge>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        store_id: edge.config().store_id,
    })
}

async fn open_till(
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

async fn close_till(
    State(edge): State<Arc<Edge>>,
    Path(till_id): Path<Uuid>,
    Json(body): Json<CloseTillRequest>,
) -> HandlerResult<ZReport> {
    edge.close_till(till_id, body.counted_cash)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn price_preview(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<SaleInput>,
) -> HandlerResult<PricePreview> {
    edge.price_preview(&body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn checkout(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<SaleInput>,
) -> HandlerResult<CompletedSale> {
    edge.checkout_with_default_provider(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn void_transaction(
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

async fn return_transaction(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(lines): Json<Vec<ReturnLineInput>>,
) -> HandlerResult<CompletedSale> {
    edge.record_return(id, lines)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn park_hold(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<SaleInput>,
) -> HandlerResult<HoldResponse> {
    edge.park_hold(body)
        .await
        .map(|hold_id| Json(HoldResponse { hold_id }))
        .map_err(err_response)
}

async fn list_holds(State(edge): State<Arc<Edge>>) -> HandlerResult<HoldsResponse> {
    Ok(Json(HoldsResponse {
        hold_ids: edge.list_holds().await,
    }))
}

async fn resume_hold(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<SaleInput> {
    edge.resume_hold(id).await.map(Json).map_err(err_response)
}

async fn record_quote(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<SaleInput>,
) -> HandlerResult<TransactionIdResponse> {
    edge.record_quote(body)
        .await
        .map(|transaction_id| Json(TransactionIdResponse { transaction_id }))
        .map_err(err_response)
}

async fn record_layaway(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<LayawayRequest>,
) -> HandlerResult<TransactionIdResponse> {
    edge.record_layaway_with_default_provider(body.sale, body.deposit)
        .await
        .map(|transaction_id| Json(TransactionIdResponse { transaction_id }))
        .map_err(err_response)
}

async fn sync_state(State(edge): State<Arc<Edge>>) -> HandlerResult<SyncState> {
    edge.sync_state().await.map(Json).map_err(err_response)
}

async fn sync_run(State(edge): State<Arc<Edge>>) -> HandlerResult<ApplyReport> {
    edge.sync_now_with_configured_hq()
        .await
        .map(Json)
        .map_err(err_response)
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

async fn dispense_tint(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<DispenseTintRequest>,
) -> HandlerResult<DispenseTintResponse> {
    edge.dispense_tint(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_formulas(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<FormulaQuery>,
) -> HandlerResult<Vec<TintFormula>> {
    edge.search_tint_formulas(query.query.as_deref(), query.customer_id, query.job_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_colors(
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

async fn list_colorants(State(edge): State<Arc<Edge>>) -> HandlerResult<ColorantsResponse> {
    edge.colorant_levels().await.map(Json).map_err(err_response)
}

async fn daily_usage(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ReportQuery>,
) -> Result<Json<DailyUsageReport>, (StatusCode, Json<ErrorResponse>)> {
    let date = report_date(&query)?;
    edge.daily_usage(date).await.map(Json).map_err(err_response)
}

async fn low_stock(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ReportQuery>,
) -> Result<Json<LowStockReport>, (StatusCode, Json<ErrorResponse>)> {
    let date = report_date(&query)?;
    edge.low_stock(date).await.map(Json).map_err(err_response)
}

async fn set_thresholds(
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

async fn bi_kpi(
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

async fn bi_eco(
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

async fn bi_inquiry(
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

async fn bi_email_outbox(
    State(edge): State<Arc<Edge>>,
) -> Result<Json<Vec<EmailDelivery>>, (StatusCode, Json<ErrorResponse>)> {
    edge.email_outbox().await.map(Json).map_err(err_response)
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreQuery {
    pub store_id: Option<Uuid>,
}

fn store_or_default(edge: &Edge, store_id: Option<Uuid>) -> Uuid {
    store_id.unwrap_or(edge.config().store_id)
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

async fn get_balances(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<InventoryBalance>> {
    edge.inventory_balances(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_reorder_points(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<ReorderPoint>> {
    edge.list_reorder_points(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

async fn set_reorder_point(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<SetReorderPointRequest>,
) -> HandlerResult<ReorderPoint> {
    edge.set_reorder_point(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn suggest_replenishment(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<ReplenishmentSuggestRequest>,
) -> HandlerResult<Vec<SuggestedPurchaseOrder>> {
    edge.suggest_replenishment(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn create_purchase_order(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreatePurchaseOrderRequest>,
) -> HandlerResult<PurchaseOrderView> {
    edge.create_purchase_order(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_purchase_orders(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<PurchaseOrderView>> {
    edge.list_purchase_orders(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

async fn receive_purchase_order(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<ReceivePurchaseOrderRequest>,
) -> HandlerResult<PurchaseOrderView> {
    edge.receive_purchase_order(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn create_transfer(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateTransferRequest>,
) -> HandlerResult<Transfer> {
    edge.create_transfer(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_transfers(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<Transfer>> {
    edge.list_transfers(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

async fn ship_transfer(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Transfer> {
    edge.ship_transfer(id).await.map(Json).map_err(err_response)
}

async fn receive_transfer(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Transfer> {
    edge.receive_transfer(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn start_count(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<StartCountRequest>,
) -> HandlerResult<PhysicalCount> {
    edge.start_count(body).await.map(Json).map_err(err_response)
}

async fn list_counts(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<BalancesQuery>,
) -> HandlerResult<Vec<PhysicalCount>> {
    edge.list_counts(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

async fn scan_count(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<ScanCountRequest>,
) -> HandlerResult<PhysicalCount> {
    edge.scan_count(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn reconcile_count(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<CountReconcileResponse> {
    edge.reconcile_count(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn shelf_tag(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ShelfTagQuery>,
) -> HandlerResult<ShelfTagResponse> {
    edge.shelf_tag(query.product_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_pricing_rules(State(edge): State<Arc<Edge>>) -> HandlerResult<Vec<PricingRuleRow>> {
    edge.list_pricing_rules()
        .await
        .map(Json)
        .map_err(err_response)
}

async fn bulk_pricing(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<BulkPricingRequest>,
) -> HandlerResult<Vec<PricingRuleRow>> {
    edge.bulk_update_pricing_rules(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn ar_customer_summary(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<ArCustomerSummary> {
    edge.ar_customer_summary(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn ar_aging(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ArAgingQuery>,
) -> Result<Json<AgingView>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = aging_date(&query.as_of)?;
    edge.ar_aging(store_or_default(&edge, query.store_id), as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn ar_collections(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ArAgingQuery>,
) -> Result<Json<CollectionsView>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = aging_date(&query.as_of)?;
    edge.ar_collections(store_or_default(&edge, query.store_id), as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

// --- CRM / delivery / EDI (Phase 4) ----------------------------------------

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    pub customer_id: Option<Uuid>,
    pub activity_type: Option<String>,
    pub completed: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct AppointmentQuery {
    pub date: Option<String>,
    pub customer_id: Option<Uuid>,
    pub staff_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct RemindersQuery {
    pub as_of: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LapsedQuery {
    pub store_id: Option<Uuid>,
    pub as_of: Option<String>,
    pub inactive_days: Option<i64>,
    pub include_never: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct DispatchQuery {
    pub status: Option<String>,
    pub driver_id: Option<Uuid>,
    pub scheduled_date: Option<String>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedResponse {
    pub id: Uuid,
    pub deleted: bool,
}

/// `?as_of=` as RFC 3339, defaulting to now (UTC). A malformed
/// value is a 400, not a silent default.
fn reminders_as_of(
    as_of: &Option<String>,
) -> Result<DateTime<Utc>, (StatusCode, Json<ErrorResponse>)> {
    match as_of {
        None => Ok(Utc::now()),
        Some(text) => DateTime::parse_from_rfc3339(text)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("as_of must be RFC 3339, got {text:?}"),
                    }),
                )
            }),
    }
}

/// `?date=YYYY-MM-DD` for the appointment day filter.
fn appointment_day(
    date: &Option<String>,
) -> Result<Option<NaiveDate>, (StatusCode, Json<ErrorResponse>)> {
    match date {
        None => Ok(None),
        Some(text) => NaiveDate::from_str(text).map(Some).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("date must be YYYY-MM-DD, got {text:?}"),
                }),
            )
        }),
    }
}

async fn create_activity(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateActivityRequest>,
) -> HandlerResult<CrmActivity> {
    edge.create_activity(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_activities(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ActivityQuery>,
) -> HandlerResult<Vec<CrmActivity>> {
    edge.list_activities(
        query.customer_id,
        query.activity_type.as_deref(),
        query.completed,
    )
    .await
    .map(Json)
    .map_err(err_response)
}

async fn get_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<CrmActivity> {
    edge.get_activity(id).await.map(Json).map_err(err_response)
}

async fn update_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateActivityRequest>,
) -> HandlerResult<CrmActivity> {
    edge.update_activity(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn delete_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeletedResponse> {
    edge.delete_activity(id)
        .await
        .map(|()| Json(DeletedResponse { id, deleted: true }))
        .map_err(err_response)
}

async fn complete_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<CrmActivity> {
    edge.complete_activity(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn snooze_activity(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SnoozeActivityRequest>,
) -> HandlerResult<CrmActivity> {
    edge.snooze_activity(id, body.snoozed_until)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn due_reminders(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<RemindersQuery>,
) -> Result<Json<Vec<CrmActivity>>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = reminders_as_of(&query.as_of)?;
    edge.due_reminders(as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn overdue_reminders(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<RemindersQuery>,
) -> Result<Json<Vec<CrmActivity>>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = reminders_as_of(&query.as_of)?;
    edge.overdue_reminders(as_of)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn create_appointment(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateAppointmentRequest>,
) -> HandlerResult<Appointment> {
    edge.create_appointment(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_appointments(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<AppointmentQuery>,
) -> Result<Json<Vec<Appointment>>, (StatusCode, Json<ErrorResponse>)> {
    let day = appointment_day(&query.date)?;
    edge.list_appointments(day, query.customer_id, query.staff_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn get_appointment(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Appointment> {
    edge.get_appointment(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn update_appointment(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAppointmentRequest>,
) -> HandlerResult<Appointment> {
    edge.update_appointment(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn cancel_appointment(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Appointment> {
    edge.cancel_appointment(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn delete_appointment(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeletedResponse> {
    edge.delete_appointment(id)
        .await
        .map(|()| Json(DeletedResponse { id, deleted: true }))
        .map_err(err_response)
}

async fn create_saved_view(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateSavedViewRequest>,
) -> HandlerResult<CrmSavedView> {
    edge.create_saved_view(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_saved_views(State(edge): State<Arc<Edge>>) -> HandlerResult<Vec<CrmSavedView>> {
    edge.list_saved_views()
        .await
        .map(Json)
        .map_err(err_response)
}

async fn run_saved_view(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Vec<CrmActivity>> {
    edge.run_saved_view(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn run_saved_view_customers(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Vec<SavedViewCustomer>> {
    edge.run_saved_view_customers(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn delete_saved_view(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeletedResponse> {
    edge.delete_saved_view(id)
        .await
        .map(|()| Json(DeletedResponse { id, deleted: true }))
        .map_err(err_response)
}

async fn lapsed_report(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<LapsedQuery>,
) -> Result<Json<LapsedReport>, (StatusCode, Json<ErrorResponse>)> {
    let as_of = match &query.as_of {
        None => Utc::now().date_naive(),
        Some(text) => NaiveDate::from_str(text).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("as_of must be YYYY-MM-DD, got {text:?}"),
                }),
            )
        })?,
    };
    let store_id = store_or_default(&edge, query.store_id);
    if store_id != edge.config().store_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "this edge serves store {} — got store_id {store_id}",
                    edge.config().store_id
                ),
            }),
        ));
    }
    edge.lapsed_contractors(
        as_of,
        query.inactive_days.unwrap_or(90),
        query.include_never.unwrap_or(true),
    )
    .await
    .map(Json)
    .map_err(err_response)
}

async fn create_delivery(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateDeliveryRequest>,
) -> HandlerResult<DeliveryView> {
    edge.create_delivery(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_deliveries(
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

async fn get_delivery(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<DeliveryView> {
    edge.get_delivery_view(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn assign_delivery(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignDeliveryRequest>,
) -> HandlerResult<DeliveryView> {
    edge.assign_delivery(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn set_delivery_status(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetDeliveryStatusRequest>,
) -> HandlerResult<DeliveryView> {
    edge.set_delivery_status(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn get_packing_slip(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<PackingSlipView> {
    edge.packing_slip(id).await.map(Json).map_err(err_response)
}

async fn submit_pod(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SubmitPodRequest>,
) -> HandlerResult<DeliveryView> {
    edge.submit_pod(id, body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn get_pod(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<Option<DeliveryPod>> {
    edge.get_pod(id).await.map(Json).map_err(err_response)
}

async fn send_purchase_order(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SendPoRequest>,
) -> HandlerResult<SendPoResponse> {
    edge.send_purchase_order(id, body.out_dir)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn intake_edi(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<IntakeRequest>,
) -> HandlerResult<IntakeResponse> {
    edge.intake_edi(body).await.map(Json).map_err(err_response)
}

async fn list_edi_documents(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<PoQuery>,
) -> HandlerResult<Vec<EdiDocument>> {
    edge.list_edi_documents(query.po_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn get_edi_document(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<EdiDocument> {
    edge.get_edi_document(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn three_way_match(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<PoQuery>,
) -> HandlerResult<InvoiceMatchReport> {
    edge.three_way_match(query.po_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_match_reports(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<PoQuery>,
) -> HandlerResult<Vec<InvoiceMatchReportRow>> {
    edge.list_match_reports(query.po_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_po_acknowledgments(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<PoQuery>,
) -> HandlerResult<Vec<PoAcknowledgmentRow>> {
    edge.list_po_acknowledgments(query.po_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn get_price_catalog_application(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<PriceCatalogApplicationRow> {
    edge.get_price_catalog_application(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn mark_price_application_applied(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<MarkPriceApplicationAppliedRequest>,
) -> HandlerResult<PriceCatalogApplicationRow> {
    edge.mark_price_application_applied(id, &body.reviewed_by)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_transport_records(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<TransportQuery>,
) -> HandlerResult<Vec<TransportRecord>> {
    edge.list_transport_records(&query.vendor_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn import_catalog(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CatalogImportBody>,
) -> HandlerResult<CatalogImportResponse> {
    edge.import_vendor_catalog(&body.csv_text, &body.vendor_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn ws_handler(
    State(edge): State<Arc<Edge>>,
    Path(store_id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if store_id != edge.config().store_id {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("unknown store {store_id}"),
            }),
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| handle_ws(edge, socket))
        .into_response()
}

async fn handle_ws(edge: Arc<Edge>, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = edge.subscribe_events();
    let hello = serde_json::json!({
        "type": "connected",
        "store_id": edge.config().store_id.to_string(),
    })
    .to_string();
    if sender.send(Message::Text(hello.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    if sender.send(Message::Text(event.into())).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            msg = receiver.next() => match msg {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {
                    // Client messages are ignored; this channel is
                    // server-push only in Phase 1.
                }
                Some(Err(_)) => break,
            },
        }
    }
}

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

async fn portal_customer(
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

async fn portal_invoices(
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

async fn portal_statements(
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

async fn portal_formulas(
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

async fn portal_payments_list(
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

async fn portal_reorder(
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

async fn portal_pay_bill(
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

async fn catalog_products(State(edge): State<Arc<Edge>>) -> HandlerResult<Vec<Product>> {
    edge.catalog_products()
        .await
        .map(Json)
        .map_err(err_response)
}

async fn bopis_order_create(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<BopisOrderRequest>,
) -> HandlerResult<BopisOrderResponse> {
    edge.record_bopis_order(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn bopis_order_get(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<BopisOrderView> {
    edge.bopis_order(id).await.map(Json).map_err(err_response)
}

async fn bopis_pickup_complete(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
) -> HandlerResult<BopisOrderView> {
    edge.complete_bopis_pickup(id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn mobile_kpis(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<StoreQuery>,
) -> HandlerResult<MobileKpis> {
    edge.mobile_kpis(store_or_default(&edge, query.store_id))
        .await
        .map(Json)
        .map_err(err_response)
}

// --- Scheduling+ (Phase 6) ---------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ShiftDayQuery {
    pub date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ForecastQuery {
    pub from: Option<String>,
    pub days: Option<i64>,
    pub sales_per_labor_hour: Option<String>,
}

/// `?date=YYYY-MM-DD`, defaulting to today (UTC). A malformed
/// value is a 400, not a silent default.
fn day_or_today(date: &Option<String>) -> Result<NaiveDate, (StatusCode, Json<ErrorResponse>)> {
    match appointment_day(date)? {
        Some(day) => Ok(day),
        None => Ok(Utc::now().date_naive()),
    }
}

async fn create_shift_template(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateShiftTemplateRequest>,
) -> HandlerResult<ShiftTemplate> {
    edge.create_shift_template(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_shift_templates(State(edge): State<Arc<Edge>>) -> HandlerResult<Vec<ShiftTemplate>> {
    edge.list_shift_templates()
        .await
        .map(Json)
        .map_err(err_response)
}

async fn create_staff_shift(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<CreateStaffShiftRequest>,
) -> HandlerResult<StaffShift> {
    edge.create_staff_shift(body)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_staff_shifts(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ShiftDayQuery>,
) -> HandlerResult<Vec<StaffShift>> {
    let day = day_or_today(&query.date)?;
    edge.list_staff_shifts(day)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn set_shift_status(
    State(edge): State<Arc<Edge>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetShiftStatusRequest>,
) -> HandlerResult<StaffShift> {
    edge.set_shift_status(id, &body.status)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn clock_in(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<ClockRequest>,
) -> HandlerResult<TimeclockEntry> {
    edge.clock_in(body.staff_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn clock_out(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<ClockRequest>,
) -> HandlerResult<TimeclockEntry> {
    edge.clock_out(body.staff_id)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn list_timeclock(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ShiftDayQuery>,
) -> HandlerResult<Vec<TimeclockEntry>> {
    let day = day_or_today(&query.date)?;
    edge.list_timeclock_entries(day)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn labor_forecast(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ForecastQuery>,
) -> HandlerResult<ForecastReport> {
    let from = day_or_today(&query.from)?;
    let ratio = match &query.sales_per_labor_hour {
        Some(text) => Some(parse_ratio(text).map_err(err_response)?),
        None => None,
    };
    edge.labor_forecast(from, query.days.unwrap_or(7), ratio)
        .await
        .map(Json)
        .map_err(err_response)
}

async fn labor_coverage(
    State(edge): State<Arc<Edge>>,
    Query(query): Query<ShiftDayQuery>,
) -> HandlerResult<CoverageCheck> {
    let day = day_or_today(&query.date)?;
    edge.labor_coverage(day)
        .await
        .map(Json)
        .map_err(err_response)
}

// --- auth endpoints (Phase 7, v1.1) ---------------------------------------
// Public by design: these are how a session is obtained. Login
// failures are a generic 401 (see `df_auth_http::login_error`) —
// the boundary must not reveal which emails exist or which
// accounts are active.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeLoginRequest {
    pub email: String,
    pub password: String,
    /// Portal remember-me (30-day session) — only meaningful for a
    /// `PortalContractor`; staff sessions are always 8 hours.
    pub remember_me: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeCounterSwitchRequest {
    /// Identifies the user: an Argon2id hash cannot be searched,
    /// so a bare PIN/badge cannot name its user (see
    /// `df_auth::login` docs).
    pub email: String,
    pub pin: Option<String>,
    pub badge_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeOverrideVerifyRequest {
    pub user_id: String,
    pub pin: String,
    pub reason_code: String,
}

type AuthResult<T> = Result<Json<T>, (StatusCode, Json<df_auth_http::ErrorBody>)>;

async fn edge_login(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<EdgeLoginRequest>,
) -> AuthResult<LoginResponse> {
    df_db::login_with_password(
        edge.pool(),
        &body.email,
        &body.password,
        body.remember_me.unwrap_or(false),
    )
    .await
    .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
    .map_err(login_error)
}

async fn edge_counter_switch(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<EdgeCounterSwitchRequest>,
) -> AuthResult<LoginResponse> {
    if let Some(pin) = &body.pin {
        return df_db::login_with_pin(edge.pool(), &body.email, pin)
            .await
            .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
            .map_err(login_error);
    }
    if let Some(badge) = &body.badge_token {
        return df_db::login_with_badge(edge.pool(), &body.email, badge)
            .await
            .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
            .map_err(login_error);
    }
    Err((
        StatusCode::BAD_REQUEST,
        Json(df_auth_http::ErrorBody {
            error: "counter-switch needs a pin or a badge_token".to_string(),
        }),
    ))
}

async fn edge_portal_login(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<EdgeLoginRequest>,
) -> AuthResult<LoginResponse> {
    df_db::login_portal(
        edge.pool(),
        &body.email,
        &body.password,
        body.remember_me.unwrap_or(false),
    )
    .await
    .map(|outcome| Json(LoginResponse::from_outcome(&outcome)))
    .map_err(login_error)
}

async fn edge_logout(
    State(edge): State<Arc<Edge>>,
    Extension(auth): Extension<AuthSession>,
) -> HandlerResult<LogoutResponse> {
    match df_db::revoke_session(edge.pool(), auth.session_id).await {
        Ok(()) => Ok(Json(LogoutResponse { revoked: true })),
        Err(err) => Err(err_response(err.into())),
    }
}

async fn edge_override_verify(
    State(edge): State<Arc<Edge>>,
    Json(body): Json<EdgeOverrideVerifyRequest>,
) -> AuthResult<OverrideVerifyResponse> {
    if body.reason_code.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(df_auth_http::ErrorBody {
                error: "reason_code is required".to_string(),
            }),
        ));
    }
    let user_id = Uuid::parse_str(&body.user_id).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(df_auth_http::ErrorBody {
                error: format!("user_id is not a UUID: {:?}", body.user_id),
            }),
        )
    })?;
    // Unknown approver id is a generic 401, like an unknown email.
    let user = df_db::get_user_by_id(edge.pool(), user_id)
        .await
        .map_err(|_| login_error(df_auth::AuthError::InvalidCredentials))?;
    df_db::verify_manager_override(edge.pool(), &user.email, &OverrideCredential::Pin(body.pin))
        .await
        .map(|approval| Json(OverrideVerifyResponse::from_approval(&approval)))
        .map_err(login_error)
}

pub fn router(edge: Edge) -> Router {
    let state = Arc::new(edge);
    let pool = state.pool().clone();

    // One gate per rule. `from_fn_with_state` carries the pool +
    // rule separately from the router state; each sub-router gets
    // the gate for its routes. The full role → route matrix lives
    // in docs/auth.md; the group names below mirror it.
    let gate = |rule: AuthRule| {
        middleware::from_fn_with_state(Gate::new(Some(pool.clone()), rule), df_auth_http::gate)
    };
    let authenticated_gate = gate(AuthRule::Authenticated);
    let checkout_gate = gate(AuthRule::AnyOf(vec![Permission::PosCheckout]));
    let till_gate = gate(AuthRule::AnyOf(vec![Permission::PosTillManage]));
    let void_gate = gate(AuthRule::AnyOf(vec![Permission::PosVoid]));
    let reports_gate = gate(AuthRule::AnyOf(vec![Permission::ReportView]));
    let inventory_view_gate = gate(AuthRule::AnyOf(vec![Permission::InventoryView]));
    let inventory_adjust_gate = gate(AuthRule::AnyOf(vec![Permission::InventoryAdjust]));
    let po_create_gate = gate(AuthRule::AnyOf(vec![Permission::PoCreate]));
    let po_receive_gate = gate(AuthRule::AnyOf(vec![Permission::PoReceive]));
    let pricing_gate = gate(AuthRule::AnyOf(vec![Permission::PricingManage]));
    let ar_view_gate = gate(AuthRule::AnyOf(vec![Permission::ArView]));
    let customer_gate = gate(AuthRule::AnyOf(vec![Permission::CustomerManage]));
    let delivery_gate = gate(AuthRule::AnyOf(vec![
        Permission::PosCheckout,
        Permission::InventoryView,
    ]));
    let tint_reads_gate = gate(AuthRule::AnyOf(vec![
        Permission::PosCheckout,
        Permission::InventoryView,
    ]));
    let suggest_gate = gate(AuthRule::AnyOf(vec![
        Permission::PoCreate,
        Permission::InventoryView,
    ]));
    let kpis_gate = gate(AuthRule::AnyOf(vec![
        Permission::PosCheckout,
        Permission::InventoryView,
        Permission::ArView,
    ]));
    let edi_gate = gate(AuthRule::AnyOf(vec![Permission::ReportView]));
    let portal_gate = gate(AuthRule::Portal);
    let scheduling_view_gate = gate(AuthRule::AnyOf(vec![Permission::SchedulingView]));
    let scheduling_manage_gate = gate(AuthRule::AnyOf(vec![Permission::SchedulingManage]));

    // Public by design: health, the login endpoints (how a session
    // is obtained), the public BOPIS storefront catalog + order
    // placement/status. Everything else below is gated.
    let public = Router::new()
        .route("/health", get(health))
        .route("/pos/auth/login", post(edge_login))
        .route("/pos/auth/counter-switch", post(edge_counter_switch))
        .route("/pos/auth/portal-login", post(edge_portal_login))
        .route("/pos/auth/override-verify", post(edge_override_verify))
        .route("/catalog/products", get(catalog_products))
        .route("/bopis/orders", post(bopis_order_create))
        .route("/bopis/orders/{id}", get(bopis_order_get));

    let logout = Router::new()
        .route("/pos/auth/logout", post(edge_logout))
        .route_layer(authenticated_gate.clone());

    let checkout_routes = Router::new()
        .route("/pos/carts/price-preview", post(price_preview))
        .route("/pos/transactions", post(checkout))
        .route("/pos/transactions/{id}/return", post(return_transaction))
        .route("/pos/holds", post(park_hold).get(list_holds))
        .route("/pos/holds/{id}/resume", post(resume_hold))
        .route("/pos/quotes", post(record_quote))
        .route("/pos/layaways", post(record_layaway))
        .route("/pos/sync/state", get(sync_state))
        .route("/pos/sync/run", post(sync_run))
        .route("/pos/tinting/dispense", post(dispense_tint))
        .route("/pos/tinting/formulas", get(list_formulas))
        .route("/pos/tinting/colors", get(list_colors))
        .route(
            "/bopis/orders/{id}/complete-pickup",
            post(bopis_pickup_complete),
        )
        .route_layer(checkout_gate);

    let till_routes = Router::new()
        .route("/pos/tills/open", post(open_till))
        .route("/pos/tills/{id}/close", post(close_till))
        .route_layer(till_gate);

    let void_routes = Router::new()
        .route("/pos/transactions/{id}/void", post(void_transaction))
        .route_layer(void_gate);

    let tint_read_routes = Router::new()
        .route("/pos/tinting/colorants", get(list_colorants))
        .route_layer(tint_reads_gate);

    let report_routes = Router::new()
        .route("/pos/reports/daily-usage", get(daily_usage))
        .route("/pos/reports/low-stock", get(low_stock))
        .route("/bi/kpi", get(bi_kpi))
        .route("/bi/eco", get(bi_eco))
        .route("/bi/inquiry", get(bi_inquiry))
        .route("/bi/email-outbox", get(bi_email_outbox))
        .route_layer(reports_gate);

    let pricing_routes = Router::new()
        .route("/pos/reports/low-stock/thresholds", post(set_thresholds))
        .route("/pricing/rules", get(list_pricing_rules).post(bulk_pricing))
        .route_layer(pricing_gate);

    let inventory_view_routes = Router::new()
        .route("/inventory/balances", get(get_balances))
        .route("/inventory/reorder-points", get(list_reorder_points))
        .route("/inventory/transfers", get(list_transfers))
        .route("/inventory/counts", get(list_counts))
        .route("/inventory/shelf-tags", get(shelf_tag))
        .route("/purchasing/orders", get(list_purchase_orders))
        .route_layer(inventory_view_gate);

    let inventory_adjust_routes = Router::new()
        .route("/inventory/reorder-points", post(set_reorder_point))
        .route("/inventory/transfers", post(create_transfer))
        .route("/inventory/transfers/{id}/ship", post(ship_transfer))
        .route("/inventory/transfers/{id}/receive", post(receive_transfer))
        .route("/inventory/counts", post(start_count))
        .route("/inventory/counts/{id}/scan", post(scan_count))
        .route("/inventory/counts/{id}/reconcile", post(reconcile_count))
        .route_layer(inventory_adjust_gate);

    let purchasing_routes = Router::new()
        .route("/purchasing/orders", post(create_purchase_order))
        .route_layer(po_create_gate);

    let receive_routes = Router::new()
        .route(
            "/purchasing/orders/{id}/receive",
            post(receive_purchase_order),
        )
        .route_layer(po_receive_gate);

    let suggest_routes = Router::new()
        .route(
            "/purchasing/replenishment/suggest",
            post(suggest_replenishment),
        )
        .route_layer(suggest_gate);

    let ar_routes = Router::new()
        .route("/ar/customers/{id}/summary", get(ar_customer_summary))
        .route("/ar/aging", get(ar_aging))
        .route("/ar/collections", get(ar_collections))
        .route_layer(ar_view_gate);

    let crm_routes = Router::new()
        .route(
            "/crm/activities",
            post(create_activity).get(list_activities),
        )
        .route("/crm/activities/{id}", get(get_activity))
        .route("/crm/activities/{id}/update", post(update_activity))
        .route("/crm/activities/{id}/delete", post(delete_activity))
        .route("/crm/activities/{id}/complete", post(complete_activity))
        .route("/crm/activities/{id}/snooze", post(snooze_activity))
        .route("/crm/reminders/due", get(due_reminders))
        .route("/crm/reminders/overdue", get(overdue_reminders))
        .route(
            "/crm/appointments",
            post(create_appointment).get(list_appointments),
        )
        .route("/crm/appointments/{id}", get(get_appointment))
        .route("/crm/appointments/{id}/update", post(update_appointment))
        .route("/crm/appointments/{id}/cancel", post(cancel_appointment))
        .route("/crm/appointments/{id}/delete", post(delete_appointment))
        .route(
            "/crm/saved-views",
            post(create_saved_view).get(list_saved_views),
        )
        .route("/crm/saved-views/{id}/run", get(run_saved_view))
        .route(
            "/crm/saved-views/{id}/run-customers",
            get(run_saved_view_customers),
        )
        .route("/crm/saved-views/{id}/delete", post(delete_saved_view))
        .route("/crm/reports/lapsed", get(lapsed_report))
        .route_layer(customer_gate);

    let delivery_routes = Router::new()
        .route("/deliveries", post(create_delivery))
        .route("/deliveries/dispatch", get(list_deliveries))
        .route("/deliveries/{id}", get(get_delivery))
        .route("/deliveries/{id}/assign", post(assign_delivery))
        .route("/deliveries/{id}/status", post(set_delivery_status))
        .route("/deliveries/{id}/packing-slip", get(get_packing_slip))
        .route("/deliveries/{id}/pod", post(submit_pod).get(get_pod))
        .route_layer(delivery_gate);

    let edi_routes = Router::new()
        .route("/edi/purchase-orders/{id}/send", post(send_purchase_order))
        .route("/edi/intake", post(intake_edi))
        .route("/edi/documents", get(list_edi_documents))
        .route("/edi/documents/{id}", get(get_edi_document))
        .route("/edi/acknowledgments", get(list_po_acknowledgments))
        .route("/edi/three-way-match", get(three_way_match))
        .route("/edi/match-reports", get(list_match_reports))
        .route(
            "/edi/price-applications/{id}",
            get(get_price_catalog_application),
        )
        .route(
            "/edi/price-applications/{id}/mark-applied",
            post(mark_price_application_applied),
        )
        .route("/edi/transport", get(list_transport_records))
        .route("/edi/catalog/import", post(import_catalog))
        .route_layer(edi_gate);

    let portal_routes = Router::new()
        .route("/portal/customers/{id}", get(portal_customer))
        .route("/portal/customers/{id}/invoices", get(portal_invoices))
        .route("/portal/customers/{id}/statements", get(portal_statements))
        .route("/portal/customers/{id}/formulas", get(portal_formulas))
        .route("/portal/customers/{id}/payments", get(portal_payments_list))
        .route("/portal/reorders", post(portal_reorder))
        .route("/portal/payments", post(portal_pay_bill))
        .route_layer(portal_gate);

    let kpi_routes = Router::new()
        .route("/mobile/kpis", get(mobile_kpis))
        .route_layer(kpis_gate);

    // Scheduling reads + timeclock need SchedulingView (every staff
    // role); template/shift management needs SchedulingManage
    // (Manager/Admin). Phase 7: PortalContractor holds neither, so
    // contractors cannot reach scheduling at all.
    let scheduling_read_routes = Router::new()
        .route("/scheduling/templates", get(list_shift_templates))
        .route("/scheduling/shifts", get(list_staff_shifts))
        .route("/scheduling/timeclock/clock-in", post(clock_in))
        .route("/scheduling/timeclock/clock-out", post(clock_out))
        .route("/scheduling/timeclock", get(list_timeclock))
        .route("/scheduling/forecast", get(labor_forecast))
        .route("/scheduling/coverage", get(labor_coverage))
        .route_layer(scheduling_view_gate);
    let scheduling_manage_routes = Router::new()
        .route("/scheduling/templates", post(create_shift_template))
        .route("/scheduling/shifts", post(create_staff_shift))
        .route("/scheduling/shifts/{id}/status", post(set_shift_status))
        .route_layer(scheduling_manage_gate);

    let ws_routes = Router::new()
        .route("/ws/v1/store/{store_id}", get(ws_handler))
        .route_layer(authenticated_gate);

    Router::new()
        .merge(public)
        .merge(logout)
        .merge(checkout_routes)
        .merge(till_routes)
        .merge(void_routes)
        .merge(tint_read_routes)
        .merge(report_routes)
        .merge(pricing_routes)
        .merge(inventory_view_routes)
        .merge(inventory_adjust_routes)
        .merge(purchasing_routes)
        .merge(receive_routes)
        .merge(suggest_routes)
        .merge(ar_routes)
        .merge(crm_routes)
        .merge(delivery_routes)
        .merge(edi_routes)
        .merge(portal_routes)
        .merge(kpi_routes)
        .merge(scheduling_read_routes)
        .merge(scheduling_manage_routes)
        .merge(ws_routes)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::test_support::{CapturingPrinter, sale_input, seed_product, test_edge};
    use crate::payments::{MockTerminal, TenderKind};
    use axum::body::Body;
    use axum::http::Request;
    use df_sync::MemoryHq;
    use tower::ServiceExt;

    fn money_json(amount: &str) -> serde_json::Value {
        serde_json::json!({"amount": amount, "currency": "Usd"})
    }

    fn sale_body(product_id: Uuid, tender: &str) -> serde_json::Value {
        serde_json::json!({
            "customer_id": null,
            "job_id": null,
            "lines": [{
                "product_id": product_id.to_string(),
                "sku": "BM-REGAL-1G",
                "description": "Regal Select Interior Gallon",
                "qty": "2",
                "uom": "gal",
                "unit_price": money_json("100.0000"),
                "unit_cost": money_json("50.0000"),
            }],
            "tenders": [{"kind": tender, "amount": null}],
            "po_number": null,
        })
    }

    /// Seed one user per role + two portal contractors and return
    /// their live tokens (see `df_auth_http::test_support`).
    async fn seeded_users(edge: &Edge) -> df_auth_http::test_support::SeededUsers {
        df_auth_http::test_support::seed_test_users(edge.pool(), edge.config().org_id).await
    }

    /// Parse a response body as JSON; an empty body (axum default
    /// 404/405) becomes `Null` instead of panicking.
    async fn body_json(response: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    async fn post_json(
        router: Router,
        uri: &str,
        body: serde_json::Value,
        token: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder()
            .uri(uri)
            .method("POST")
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let response = router
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        body_json(response).await
    }

    async fn get_json(
        router: Router,
        uri: &str,
        token: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().uri(uri);
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let response = router
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        body_json(response).await
    }

    #[tokio::test]
    async fn health_and_price_preview_over_http() {
        let edge = test_edge().await;
        let product_id = seed_product(&edge).await;
        let users = seeded_users(&edge).await;
        let router = edge.router();
        let (status, body) = get_json(router.clone(), "/health", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["version"], "1.2.0");

        let (status, body) = post_json(
            router,
            "/pos/carts/price-preview",
            sale_body(product_id, "Cash"),
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // 2 x $100 = $200 subtotal; eco 2 x $0.75 = $1.50; tax 7.25%
        // on $201.50 = $14.6088; no surcharge on cash.
        assert_eq!(body["subtotal"]["amount"], "200.0000");
        assert_eq!(body["eco_fee_total"]["amount"], "1.5000");
        assert_eq!(body["tax_total"]["amount"], "14.6088");
        assert_eq!(body["grand_total"]["amount"], "216.1088");
    }

    #[tokio::test]
    async fn checkout_void_and_sync_state_over_http() {
        let edge = test_edge().await;
        let product_id = seed_product(&edge).await;
        edge.set_payment_provider(Arc::new(MockTerminal::new()));
        edge.set_hq(Arc::new(MemoryHq::new()));
        let printer = Arc::new(CapturingPrinter::default());
        edge.set_printer(printer.clone());
        let users = seeded_users(&edge).await;
        let router = edge.router();

        let (status, body) = post_json(
            router.clone(),
            "/pos/transactions",
            sale_body(product_id, "Card"),
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Card: surcharge 3% of the $216.1088 pre-surcharge total.
        assert_eq!(body["grand_total"]["amount"], "222.5921");
        assert_eq!(body["pos_fee_total"]["amount"], "6.4833");
        assert_eq!(body["receipt_printed"], true);
        assert_eq!(printer.prints.lock().unwrap().len(), 1);
        let tx_id = body["transaction_id"].as_str().unwrap().to_string();

        let (status, body) = get_json(
            router.clone(),
            "/pos/sync/state",
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pending_count"], 2); // transaction + inventory

        let (status, body) = post_json(
            router.clone(),
            "/pos/sync/run",
            serde_json::json!({}),
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["applied"], 2);
        assert_eq!(body["duplicates"], 0);

        let (status, body) = get_json(
            router.clone(),
            "/pos/sync/state",
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pending_count"], 0);
        assert!(body["last_batch_id"].is_string());

        // Void is manager-only: the cashier gets 403, the manager voids.
        let (status, _) = post_json(
            router.clone(),
            &format!("/pos/transactions/{tx_id}/void"),
            serde_json::json!({}),
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, body) = post_json(
            router.clone(),
            &format!("/pos/transactions/{tx_id}/void"),
            serde_json::json!({}),
            Some(&users.manager.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "voided");
    }

    #[tokio::test]
    async fn holds_quotes_and_layaway_over_http() {
        let edge = test_edge().await;
        let product_id = seed_product(&edge).await;
        edge.set_payment_provider(Arc::new(MockTerminal::new()));
        let users = seeded_users(&edge).await;
        let router = edge.router();
        let cashier = Some(users.cashier.token.as_str());

        // Park a hold, list it, resume it.
        let (status, body) = post_json(
            router.clone(),
            "/pos/holds",
            sale_body(product_id, "Cash"),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let hold_id = body["hold_id"].as_str().unwrap().to_string();
        let (status, body) = get_json(router.clone(), "/pos/holds", cashier).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["hold_ids"], serde_json::json!([hold_id]));
        let (status, body) = post_json(
            router.clone(),
            &format!("/pos/holds/{hold_id}/resume"),
            serde_json::json!({}),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["lines"][0]["sku"], "BM-REGAL-1G");
        // Resuming twice is a 404.
        let (status, _) = post_json(
            router.clone(),
            &format!("/pos/holds/{hold_id}/resume"),
            serde_json::json!({}),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // A quote persists as an open transaction with no outbox.
        let (status, body) = post_json(
            router.clone(),
            "/pos/quotes",
            sale_body(product_id, "Cash"),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["transaction_id"].is_string());
        let (status, body) = get_json(router.clone(), "/pos/sync/state", cashier).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pending_count"], 0);

        // A layaway with a $50 cash deposit on a $216.1088 sale.
        let mut layaway = sale_body(product_id, "Cash");
        layaway["tenders"] = serde_json::json!([]);
        let (status, body) = post_json(
            router.clone(),
            "/pos/layaways",
            serde_json::json!({
                "sale": layaway,
                "deposit": {"kind": "Cash", "amount": money_json("50.0000")},
            }),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["transaction_id"].is_string());
        let tx_id = body["transaction_id"].as_str().unwrap().to_string();
        let tx = df_db::get_transaction(edge.pool(), tx_id.parse().unwrap())
            .await
            .unwrap();
        assert_eq!(tx.doc_type, "layaway");
        assert_eq!(tx.status, "open");
        assert_eq!(tx.deposit_paid.to_string(), "50.0000");
        assert_eq!(tx.grand_total.to_string(), "216.1088");
    }

    #[tokio::test]
    async fn sync_run_without_hq_is_503() {
        let edge = test_edge().await;
        let users = seeded_users(&edge).await;
        let router = edge.router();
        let (status, body) = post_json(
            router,
            "/pos/sync/run",
            serde_json::json!({}),
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["error"].as_str().unwrap().contains("no HQ sink"));
    }

    #[tokio::test]
    async fn ws_connect_receives_hello_and_sale_event() {
        use futures_util::StreamExt as _;
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message as WsMessage;

        let edge = test_edge().await;
        let product_id = seed_product(&edge).await;
        edge.set_payment_provider(Arc::new(MockTerminal::new()));
        let users = seeded_users(&edge).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = edge.router();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let store_id = edge.config().store_id;
        let token = users.cashier.token.clone();
        let (mut ws, _) =
            connect_async(format!("ws://{addr}/ws/v1/store/{store_id}?token={token}"))
                .await
                .unwrap();
        // Hello event.
        let hello = ws.next().await.unwrap().unwrap();
        let hello: serde_json::Value = match hello {
            WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
            other => panic!("unexpected ws message: {other:?}"),
        };
        assert_eq!(hello["type"], "connected");

        // A checkout broadcasts a sale event.
        let input = sale_input(product_id, "1", TenderKind::Cash);
        edge.checkout(input, &MockTerminal::new()).await.unwrap();
        let event = ws.next().await.unwrap().unwrap();
        let event: serde_json::Value = match event {
            WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
            other => panic!("unexpected ws message: {other:?}"),
        };
        assert_eq!(event["type"], "sale_completed");
        assert_eq!(event["doc_type"], "sale");
        server.abort();
    }

    #[tokio::test]
    async fn till_open_close_over_http() {
        let edge = test_edge().await;
        let product_id = seed_product(&edge).await;
        edge.set_payment_provider(Arc::new(MockTerminal::new()));
        let users = seeded_users(&edge).await;
        let router = edge.router();
        let cashier = Some(users.cashier.token.as_str());
        // The body's cashier_id is ignored: the till belongs to the
        // session user. Send a different id on purpose.
        let stranger = Uuid::now_v7();
        let (status, body) = post_json(
            router.clone(),
            "/pos/tills/open",
            serde_json::json!({"cashier_id": stranger.to_string(), "opening_float": money_json("150.0000")}),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["cashier_id"], users.cashier.id().to_string());
        let till_id = body["id"].as_str().unwrap().to_string();

        // A second open till is a conflict.
        let (status, _) = post_json(
            router.clone(),
            "/pos/tills/open",
            serde_json::json!({"cashier_id": stranger.to_string(), "opening_float": money_json("0.0000")}),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, _) = post_json(
            router.clone(),
            "/pos/transactions",
            sale_body(product_id, "Cash"),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_json(
            router.clone(),
            &format!("/pos/tills/{till_id}/close"),
            serde_json::json!({"counted_cash": money_json("370.0000")}),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Expected cash: $150 float + $216.1088 cash sale.
        assert_eq!(body["expected_cash"]["amount"], "366.1088");
        assert_eq!(body["over_short"]["amount"], "3.8912");
        assert_eq!(body["sale_count"], 1);
        assert_eq!(body["gross_sales"]["amount"], "216.1088");
    }

    #[tokio::test]
    async fn bi_kpi_eco_inquiry_and_outbox_over_http() {
        let edge = test_edge().await;
        let product_id = seed_product(&edge).await;
        edge.set_payment_provider(Arc::new(MockTerminal::new()));
        let users = seeded_users(&edge).await;
        let router = edge.router();
        let cashier = Some(users.cashier.token.as_str());
        let manager = Some(users.manager.token.as_str());
        // One cash sale today: 2 x $100 = $200 subtotal, eco
        // $1.50, tax $14.6088, grand $216.1088.
        let (status, _) = post_json(
            router.clone(),
            "/pos/transactions",
            sale_body(product_id, "Cash"),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let today = chrono::Utc::now().date_naive().to_string();
        let (status, body) = get_json(
            router.clone(),
            &format!("/bi/kpi?start={today}&end={today}&as_of={today}"),
            manager,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["sales"]["sale_count"], 1);
        assert_eq!(body["sales"]["gross_sales"]["amount"], "200.0000");
        assert_eq!(body["sales"]["net_sales"]["amount"], "200.0000");
        assert_eq!(body["sales"]["grand_total"]["amount"], "216.1088");
        // Margin: revenue 200, COGS 2 x 50 = 100, profit 100, 50%.
        assert_eq!(body["margin"]["revenue"]["amount"], "200.0000");
        assert_eq!(body["margin"]["cogs"]["amount"], "100.0000");
        assert_eq!(body["margin"]["gross_profit"]["amount"], "100.0000");
        assert_eq!(body["margin"]["margin_pct"], "50");
        // No AR activity and no formulas on this edge.
        assert_eq!(body["total_open_ar"]["amount"], "0.0000");
        assert_eq!(body["formulas"]["formulas_created"], 0);

        // Eco remittance as CSV: US-CA, architectural_paint, 1.50.
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/bi/eco?start={today}&end={today}&format=csv"))
                    .header("authorization", format!("Bearer {}", users.manager.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let csv = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(csv.contains("US,CA,MAIN,architectural_paint,1,1,2,1.5000"));
        assert!(csv.contains("TOTAL,,,,,,,1.5000"));

        // Ad-hoc inquiry: the one sale, by product and by type.
        let (status, body) = get_json(
            router.clone(),
            &format!("/bi/inquiry?start={today}&end={today}&product_id={product_id}"),
            manager,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["grand_total"]["amount"], "216.1088");
        let (status, body) = get_json(
            router.clone(),
            &format!("/bi/inquiry?start={today}&end={today}&doc_type=return"),
            manager,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.as_array().unwrap().is_empty());

        // No outbox directory configured on this edge: 404, not
        // an empty list.
        let (status, body) = get_json(router.clone(), "/bi/email-outbox", manager).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().unwrap().contains("no email outbox"));
    }

    // --- Phase 7 exit tests (a)–(d) -------------------------------------

    #[tokio::test]
    async fn unauthenticated_protected_routes_are_401() {
        let edge = test_edge().await;
        let product_id = seed_product(&edge).await;
        let router = edge.router();
        // No token at all, and a garbage token: both 401, never 403.
        for token in [None, Some("dfp_not-a-real-token")] {
            let (status, body) = get_json(router.clone(), "/pos/sync/state", token).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(body["error"].as_str().unwrap().contains("session"));
            let (status, _) = post_json(
                router.clone(),
                "/pos/transactions",
                sale_body(product_id, "Cash"),
                token,
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            let (status, _) = post_json(
                router.clone(),
                "/pos/tills/open",
                serde_json::json!({"cashier_id": Uuid::now_v7().to_string(), "opening_float": money_json("0.0000")}),
                token,
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        // Health stays public.
        let (status, _) = get_json(router, "/health", None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn edge_login_logout_flow() {
        use df_auth_http::test_support::{TEST_PASSWORD, seed_test_users};

        let edge = test_edge().await;
        let _ = seed_test_users(edge.pool(), edge.config().org_id).await;
        let router = edge.router();

        // Wrong password: generic 401, no hint about the account.
        let (status, body) = post_json(
            router.clone(),
            "/pos/auth/login",
            serde_json::json!({"email": "cashier@example.invalid", "password": "wrong password"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "invalid credentials");

        // Right password: a dfp_ token, role + expiry in the body.
        let (status, body) = post_json(
            router.clone(),
            "/pos/auth/login",
            serde_json::json!({"email": "cashier@example.invalid", "password": TEST_PASSWORD}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token = body["token"].as_str().unwrap().to_string();
        assert!(token.starts_with("dfp_"));
        assert_eq!(body["role"], "Cashier");
        assert!(body["expires_at"].is_string());
        assert!(body["customer_id"].is_null());

        // The token works on a protected route…
        let product_id = seed_product(&edge).await;
        let (status, _) = post_json(
            router.clone(),
            "/pos/carts/price-preview",
            sale_body(product_id, "Cash"),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // …logout revokes it…
        let (status, body) = post_json(
            router.clone(),
            "/pos/auth/logout",
            serde_json::json!({}),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["revoked"], true);

        // …and the revoked token is a 401 from then on.
        let (status, _) = post_json(
            router.clone(),
            "/pos/carts/price-preview",
            sale_body(product_id, "Cash"),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn edge_counter_switch_and_override_verify() {
        use df_auth_http::test_support::{TEST_PIN, seed_test_users};

        let edge = test_edge().await;
        let users = seed_test_users(edge.pool(), edge.config().org_id).await;
        let router = edge.router();

        // PIN switch for the cashier: a live session, same role.
        let (status, body) = post_json(
            router.clone(),
            "/pos/auth/counter-switch",
            serde_json::json!({"email": "cashier@example.invalid", "pin": TEST_PIN}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["token"].as_str().unwrap().starts_with("dfp_"));
        assert_eq!(body["role"], "Cashier");

        // Wrong PIN: 401. No PIN at all: 400.
        let (status, _) = post_json(
            router.clone(),
            "/pos/auth/counter-switch",
            serde_json::json!({"email": "cashier@example.invalid", "pin": "9999"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = post_json(
            router.clone(),
            "/pos/auth/counter-switch",
            serde_json::json!({"email": "cashier@example.invalid"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Manager override with the manager's PIN: approved.
        let (status, body) = post_json(
            router.clone(),
            "/pos/auth/override-verify",
            serde_json::json!({"user_id": users.manager.id().to_string(), "pin": TEST_PIN, "reason_code": "price_below_floor"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["approved"], true);
        assert_eq!(body["approver_role"], "Manager");
        assert_eq!(body["method"], "pin");

        // A cashier cannot approve, even with a correct PIN.
        let (status, _) = post_json(
            router.clone(),
            "/pos/auth/override-verify",
            serde_json::json!({"user_id": users.cashier.id().to_string(), "pin": TEST_PIN, "reason_code": "price_below_floor"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // Manager with a wrong PIN: generic 401.
        let (status, _) = post_json(
            router.clone(),
            "/pos/auth/override-verify",
            serde_json::json!({"user_id": users.manager.id().to_string(), "pin": "9999", "reason_code": "price_below_floor"}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn expired_token_is_401() {
        let edge = test_edge().await;
        let users = seeded_users(&edge).await;
        let product_id = seed_product(&edge).await;
        // Mint a session that expired a second ago.
        let (token, _) = df_db::create_session(
            edge.pool(),
            &users.cashier.user,
            chrono::Duration::seconds(-1),
            None,
        )
        .await
        .unwrap();
        let (status, _) = post_json(
            edge.router(),
            "/pos/carts/price-preview",
            sale_body(product_id, "Cash"),
            Some(&token),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn portal_scope_is_enforced() {
        let edge = test_edge().await;
        let users = seeded_users(&edge).await;
        let router = edge.router();
        let a = users.customer_a;
        let b = users.customer_b;

        // Contractor A sees their own portal…
        let (status, body) = get_json(
            router.clone(),
            &format!("/portal/customers/{a}"),
            Some(&users.contractor_a.token),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["customer"]["id"], a.to_string());

        // …but not contractor B's (403, not 404)…
        let (status, _) = get_json(
            router.clone(),
            &format!("/portal/customers/{b}"),
            Some(&users.contractor_a.token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = get_json(
            router.clone(),
            &format!("/portal/customers/{b}/statements"),
            Some(&users.contractor_a.token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // …and staff may not use portal routes at all.
        let (status, _) = get_json(
            router.clone(),
            &format!("/portal/customers/{a}"),
            Some(&users.cashier.token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // No token: 401, never 403.
        let (status, _) = get_json(router, &format!("/portal/customers/{a}"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}
