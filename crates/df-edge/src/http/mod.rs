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
//! * `GET /pos/transactions/{id}` — one transaction with lines + payments
//! * `GET /pos/transactions/{id}/invoice` — letter-size invoice render data
//! * `GET /pos/products/search?q=&sku=&barcode=&limit=` — counter product search (cap 50)
//! * `GET /pos/customers/search?q=` — counter customer search
//! * `GET /pos/customers/{id}` — one customer
//! * `POST /pos/customers/{id}/price-level` — `{price_level_id: Uuid|null}`
//! * `GET /pos/customers/{id}/transactions?limit=` — a customer's transactions, newest first,
//!   each with lines + payments
//! * `GET /pricing/levels` — the four seeded price levels
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
//! * `POST /pos/tinting/formulas` — create a formula (formula
//!   tracking only; colorant codes must exist, else 400)
//! * `GET /pos/tinting/colorants` — org colorant catalog (formula tracking)
//! * `GET /pos/tinting/colors?manufacturer=&query=` — color search
//! * `GET /pos/reports/daily-usage?date=YYYY-MM-DD` — daily usage
//!   report (defaults to today, UTC)
//! * `GET /pos/reports/low-stock?date=YYYY-MM-DD` — low-stock
//!   report (defaults to today, UTC)
//! * `GET /bi/kpi?store_id=&start=&end=&as_of=` — KPI dashboard
//!   (sales, margin, inventory turns, AR aging, formula usage
//!   tracked only; defaults to today)
//! * `GET /bi/dashboard?store_id=&start=&end=` — dashboard
//!   aggregate (sales, daily, by-category, tenders, top products
//!   / customers, tint, low stock; defaults to today)
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
//! * `POST /pos/inventory/import` — GUI product import: parsed
//!   spreadsheet rows as JSON (`{store_id, rows}`), upserted by
//!   SKU; Manager / Admin only (`PricingManage`)
//! * `GET /inventory/reorder-points` + `POST /inventory/reorder-points` —
//!   min/max reorder points (set one per call)
//! * `POST /purchasing/replenishment/suggest` — min/max + velocity
//!   replenishment suggestions (one preview PO per vendor)
//! * `POST /purchasing/orders` + `GET /purchasing/orders?store_id=` —
//!   create / list purchase orders
//! * `GET /vendors` + `GET /vendors/{id}` — vendors for this org
//!   (with PO counts) / one vendor + its POs, newest first
//! * `POST /vendors` + `POST /vendors/{id}` — create a vendor /
//!   partial update (name is the PO key and is never editable)
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
//! `/pos/auth/*` login endpoints, the first-run setup endpoints
//! (`GET /pos/auth/setup-status`, `POST /pos/auth/setup` — public
//! because no user exists yet to hold a session), and the public
//! BOPIS storefront
//! routes (`GET /catalog/products`, `POST /bopis/orders`,
//! `GET /bopis/orders/{id}`) requires a `Bearer <redacted>` session
//! token (see `df_auth_http::gate` + `docs/auth.md` for the role →
//! route matrix). The gate answers 401 for a missing / invalid /
//! expired / revoked token and 403 for a valid token with the wrong
//! role. `POST /pos/tills/open` takes its cashier from the session,
//! ignoring the body's `cashier_id`; portal routes additionally
//! require the session's linked customer to match the requested
//! customer.

mod auth;
mod bi;
mod crm;
mod delivery;
mod edi;
mod inventory;
mod portal;
mod pos;
mod scheduling;
#[cfg(test)]
mod tests;

pub use auth::{EdgeCounterSwitchRequest, EdgeLoginRequest, EdgeOverrideVerifyRequest};
pub use bi::{BiEcoQuery, BiInquiryQuery, BiKpiQuery};
pub use crm::{ActivityQuery, AppointmentQuery, DeletedResponse, LapsedQuery, RemindersQuery};
pub use delivery::DispatchQuery;
pub use edi::{CatalogImportBody, PoQuery, TransportQuery};
pub use inventory::{ArAgingQuery, ArPaymentRequest, BalancesQuery, ShelfTagQuery};
pub use pos::{
    CloseTillRequest, ColorQuery, CustomerSearchQuery, FormulaQuery, HoldResponse, HoldsResponse,
    LayawayRequest, LimitQuery, OpenTillRequest, ProductSearchQuery, ReportQuery,
    SetPriceLevelRequest, ThresholdsResponse, TransactionIdResponse, VoidResponse,
};
pub use scheduling::{ForecastQuery, ShiftDayQuery};

use std::str::FromStr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use df_auth::Permission;
use df_auth_http::{AuthRule, Gate};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

use auth::{
    edge_counter_switch, edge_login, edge_logout, edge_override_verify, edge_portal_login,
    edge_setup, edge_setup_status,
};
use bi::{bi_dashboard, bi_eco, bi_email_outbox, bi_inquiry, bi_kpi};
use crm::{
    cancel_appointment, complete_activity, create_activity, create_appointment, create_saved_view,
    delete_activity, delete_appointment, delete_saved_view, due_reminders, get_activity,
    get_appointment, lapsed_report, list_activities, list_appointments, list_saved_views,
    overdue_reminders, run_saved_view, run_saved_view_customers, snooze_activity, update_activity,
    update_appointment,
};
use delivery::{
    assign_delivery, create_delivery, get_delivery, get_packing_slip, get_pod, list_deliveries,
    set_delivery_status, submit_pod,
};
use edi::{
    get_edi_document, get_price_catalog_application, import_catalog, intake_edi,
    list_edi_documents, list_match_reports, list_po_acknowledgments, list_transport_records,
    mark_price_application_applied, send_purchase_order, three_way_match,
};
use inventory::{
    ar_aging, ar_collections, ar_customer_summary, bulk_pricing, create_purchase_order,
    create_transfer, create_vendor, get_balances, get_vendor, import_products, list_counts,
    list_pricing_rules, list_purchase_orders, list_reorder_points, list_stores, list_transfers,
    list_vendors, post_ar_payment, receive_purchase_order, receive_transfer, reconcile_count,
    scan_count, set_reorder_point, shelf_tag, ship_transfer, start_count, suggest_replenishment,
    update_vendor,
};
use portal::{
    bopis_order_create, bopis_order_get, bopis_pickup_complete, catalog_products, mobile_kpis,
    portal_customer, portal_formulas, portal_invoices, portal_pay_bill, portal_payments_list,
    portal_reorder, portal_statements,
};
use pos::{
    checkout, close_till, create_tint_formula, customer_transactions, daily_usage, get_customer,
    get_invoice, get_transaction, list_colorants, list_colors, list_formulas, list_holds,
    list_price_levels, low_stock, open_till, park_hold, price_preview, record_layaway,
    record_quote, resume_hold, return_transaction, search_customers, search_products,
    set_customer_price_level, set_thresholds, sync_run, sync_state, void_transaction,
};
use scheduling::{
    clock_in, clock_out, create_shift_template, create_staff_shift, labor_coverage, labor_forecast,
    list_shift_templates, list_staff_shifts, list_timeclock, set_shift_status,
};

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
pub struct StoreQuery {
    pub store_id: Option<Uuid>,
}

fn store_or_default(edge: &Edge, store_id: Option<Uuid>) -> Uuid {
    store_id.unwrap_or(edge.config().store_id)
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

fn err_response(err: EdgeError) -> (StatusCode, Json<ErrorResponse>) {
    let status = match &err {
        EdgeError::NotFound(_) => StatusCode::NOT_FOUND,
        EdgeError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        EdgeError::PaymentDeclined(_) => StatusCode::PAYMENT_REQUIRED,
        EdgeError::Till(_) | EdgeError::CreditBlocked(_) => StatusCode::CONFLICT,
        EdgeError::NoPaymentProvider(_) | EdgeError::NoHq(_) => StatusCode::SERVICE_UNAVAILABLE,
        EdgeError::Sync(_) | EdgeError::Payment(_) => StatusCode::BAD_GATEWAY,
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
    let ar_payment_gate = gate(AuthRule::AnyOf(vec![Permission::ArPayment]));
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
    // is obtained), the first-run setup endpoints (how the first
    // admin is created, before any user exists), the public BOPIS
    // storefront catalog + order placement/status. Everything else
    // below is gated.
    let public = Router::new()
        .route("/health", get(health))
        .route("/pos/auth/login", post(edge_login))
        .route("/pos/auth/setup-status", get(edge_setup_status))
        .route("/pos/auth/setup", post(edge_setup))
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
        .route("/pos/transactions/{id}", get(get_transaction))
        .route("/pos/transactions/{id}/invoice", get(get_invoice))
        .route("/pos/transactions/{id}/return", post(return_transaction))
        .route("/pos/products/search", get(search_products))
        .route("/pricing/levels", get(list_price_levels))
        .route("/pos/holds", post(park_hold).get(list_holds))
        .route("/pos/holds/{id}/resume", post(resume_hold))
        .route("/pos/quotes", post(record_quote))
        .route("/pos/layaways", post(record_layaway))
        .route("/pos/sync/state", get(sync_state))
        .route("/pos/sync/run", post(sync_run))
        .route(
            "/pos/tinting/formulas",
            get(list_formulas).post(create_tint_formula),
        )
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
        .route("/bi/dashboard", get(bi_dashboard))
        .route("/bi/eco", get(bi_eco))
        .route("/bi/inquiry", get(bi_inquiry))
        .route("/bi/email-outbox", get(bi_email_outbox))
        .route_layer(reports_gate);

    let pricing_routes = Router::new()
        .route("/pos/reports/low-stock/thresholds", post(set_thresholds))
        .route("/pricing/rules", get(list_pricing_rules).post(bulk_pricing))
        .route_layer(pricing_gate);

    // GUI product import: `PricingManage` (Manager / Admin only —
    // Warehouse holds `InventoryAdjust` but not this). The back
    // office parses the spreadsheet in the browser and posts rows
    // as JSON; the file itself is never uploaded. Its own gate
    // (same rule as `pricing_gate`) because a layer value moves
    // on first use.
    let product_import_gate = gate(AuthRule::AnyOf(vec![Permission::PricingManage]));
    let product_import_routes = Router::new()
        .route("/pos/inventory/import", post(import_products))
        .route_layer(product_import_gate);

    let inventory_view_routes = Router::new()
        .route("/inventory/balances", get(get_balances))
        .route("/inventory/reorder-points", get(list_reorder_points))
        .route("/inventory/transfers", get(list_transfers))
        .route("/inventory/counts", get(list_counts))
        .route("/inventory/shelf-tags", get(shelf_tag))
        .route("/purchasing/orders", get(list_purchase_orders))
        .route("/vendors", get(list_vendors))
        .route("/vendors/{id}", get(get_vendor))
        .route("/stores", get(list_stores))
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
        .route("/vendors", post(create_vendor))
        .route("/vendors/{id}", post(update_vendor))
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

    let ar_payment_routes = Router::new()
        .route("/ar/payments", post(post_ar_payment))
        .route_layer(ar_payment_gate);

    let crm_routes = Router::new()
        .route("/pos/customers/search", get(search_customers))
        .route("/pos/customers/{id}", get(get_customer))
        .route(
            "/pos/customers/{id}/price-level",
            post(set_customer_price_level),
        )
        .route(
            "/pos/customers/{id}/transactions",
            get(customer_transactions),
        )
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
        .merge(product_import_routes)
        .merge(inventory_view_routes)
        .merge(inventory_adjust_routes)
        .merge(purchasing_routes)
        .merge(receive_routes)
        .merge(suggest_routes)
        .merge(ar_routes)
        .merge(ar_payment_routes)
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
