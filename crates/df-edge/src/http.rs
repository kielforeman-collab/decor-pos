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
//! * `POST /pos/sync/run` — syncs against the in-process [`MemoryHq`]
//!   when one was configured with [`Edge::set_hq`](crate::edge::Edge::set_hq),
//!   else `503` with a clear message.
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
//! * `POST /pos/reports/low-stock/thresholds` — set the
//!   operator-configured product thresholds the low-stock report
//!   uses (`Vec<ProductThreshold>`)
//! * `GET /ws/v1/store/{store_id}` — WebSocket; sends a `connected`
//!   event, then one JSON event per completed sale/return.
//!
//! Handlers are thin wrappers over [`Edge`](crate::edge::Edge)
//! methods; all the rules live there.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use df_reports::{DailyUsageReport, LowStockReport, ProductThreshold};
use df_tinter::TinterError;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;
use crate::tinting::{ColorantsResponse, DispenseTintRequest, DispenseTintResponse};
use crate::types::{CompletedSale, PricePreview, ReturnLineInput, SaleInput, TenderInput, ZReport};
use df_db::models::{ColorCatalogEntry, TillSession, TintFormula, Transaction};
use df_domain::Money;
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
        EdgeError::Till(_) => StatusCode::CONFLICT,
        EdgeError::NoPaymentProvider(_) | EdgeError::NoHq(_) => StatusCode::SERVICE_UNAVAILABLE,
        EdgeError::Sync(_) | EdgeError::Payment(_) => StatusCode::BAD_GATEWAY,
        EdgeError::Tinter(TinterError::InsufficientColorant { .. }) => StatusCode::CONFLICT,
        EdgeError::Tinter(_) => StatusCode::BAD_REQUEST,
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
    Json(body): Json<OpenTillRequest>,
) -> HandlerResult<TillSession> {
    edge.open_till(body.cashier_id, body.opening_float)
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

/// Build the edge router. The caller passes an `Arc<Edge>` as state.
pub fn router(edge: Edge) -> Router {
    let state = Arc::new(edge);
    Router::new()
        .route("/health", get(health))
        .route("/pos/tills/open", post(open_till))
        .route("/pos/tills/{id}/close", post(close_till))
        .route("/pos/carts/price-preview", post(price_preview))
        .route("/pos/transactions", post(checkout))
        .route("/pos/transactions/{id}/void", post(void_transaction))
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
        .route("/pos/tinting/colorants", get(list_colorants))
        .route("/pos/reports/daily-usage", get(daily_usage))
        .route("/pos/reports/low-stock", get(low_stock))
        .route("/pos/reports/low-stock/thresholds", post(set_thresholds))
        .route("/ws/v1/store/{store_id}", get(ws_handler))
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

    #[tokio::test]
    async fn health_and_price_preview_over_http() {
        let edge = test_edge().await;
        let product_id = seed_product(&edge).await;
        let router = edge.router();
        let (status, body) = get_json(router.clone(), "/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
        assert_eq!(body["version"], "0.3.0");

        let (status, body) = post_json(
            router,
            "/pos/carts/price-preview",
            sale_body(product_id, "Cash"),
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
        let router = edge.router();

        let (status, body) = post_json(
            router.clone(),
            "/pos/transactions",
            sale_body(product_id, "Card"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Card: surcharge 3% of the $216.1088 pre-surcharge total.
        assert_eq!(body["grand_total"]["amount"], "222.5921");
        assert_eq!(body["pos_fee_total"]["amount"], "6.4833");
        assert_eq!(body["receipt_printed"], true);
        assert_eq!(printer.prints.lock().unwrap().len(), 1);
        let tx_id = body["transaction_id"].as_str().unwrap().to_string();

        let (status, body) = get_json(router.clone(), "/pos/sync/state").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pending_count"], 2); // transaction + inventory

        let (status, body) =
            post_json(router.clone(), "/pos/sync/run", serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["applied"], 2);
        assert_eq!(body["duplicates"], 0);

        let (status, body) = get_json(router.clone(), "/pos/sync/state").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pending_count"], 0);
        assert!(body["last_batch_id"].is_string());

        let (status, body) = post_json(
            router.clone(),
            &format!("/pos/transactions/{tx_id}/void"),
            serde_json::json!({}),
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
        let router = edge.router();

        // Park a hold, list it, resume it.
        let (status, body) =
            post_json(router.clone(), "/pos/holds", sale_body(product_id, "Cash")).await;
        assert_eq!(status, StatusCode::OK);
        let hold_id = body["hold_id"].as_str().unwrap().to_string();
        let (status, body) = get_json(router.clone(), "/pos/holds").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["hold_ids"], serde_json::json!([hold_id]));
        let (status, body) = post_json(
            router.clone(),
            &format!("/pos/holds/{hold_id}/resume"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["lines"][0]["sku"], "BM-REGAL-1G");
        // Resuming twice is a 404.
        let (status, _) = post_json(
            router.clone(),
            &format!("/pos/holds/{hold_id}/resume"),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // A quote persists as an open transaction with no outbox.
        let (status, body) =
            post_json(router.clone(), "/pos/quotes", sale_body(product_id, "Cash")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["transaction_id"].is_string());
        let (status, body) = get_json(router.clone(), "/pos/sync/state").await;
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
        let router = edge.router();
        let (status, body) = post_json(router, "/pos/sync/run", serde_json::json!({})).await;
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = edge.router();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let store_id = edge.config().store_id;
        let (mut ws, _) = connect_async(format!("ws://{addr}/ws/v1/store/{store_id}"))
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
        let router = edge.router();
        let cashier = Uuid::now_v7();
        let (status, body) = post_json(
            router.clone(),
            "/pos/tills/open",
            serde_json::json!({"cashier_id": cashier.to_string(), "opening_float": money_json("150.0000")}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let till_id = body["id"].as_str().unwrap().to_string();

        // A second open till is a conflict.
        let (status, _) = post_json(
            router.clone(),
            "/pos/tills/open",
            serde_json::json!({"cashier_id": cashier.to_string(), "opening_float": money_json("0.0000")}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (status, _) = post_json(
            router.clone(),
            "/pos/transactions",
            sale_body(product_id, "Cash"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = post_json(
            router.clone(),
            &format!("/pos/tills/{till_id}/close"),
            serde_json::json!({"counted_cash": money_json("370.0000")}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Expected cash: $150 float + $216.1088 cash sale.
        assert_eq!(body["expected_cash"]["amount"], "366.1088");
        assert_eq!(body["over_short"]["amount"], "3.8912");
        assert_eq!(body["sale_count"], 1);
        assert_eq!(body["gross_sales"]["amount"], "216.1088");
    }
}
