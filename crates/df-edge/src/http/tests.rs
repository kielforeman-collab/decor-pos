use super::*;
use crate::edge::test_support::{CapturingPrinter, sale_input, seed_product, test_edge};
use crate::payments::{MockTerminal, TenderKind};
use axum::body::Body;
use axum::http::Request;
use df_db::models::Customer;
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
    assert_eq!(body["version"], "1.2.4");

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
async fn vendors_and_dashboard_over_http() {
    let edge = test_edge().await;
    let product_id = seed_product(&edge).await;
    let users = seeded_users(&edge).await;
    let router = edge.router();
    let manager = Some(users.manager.token.as_str());
    let store_id = edge.config().store_id;
    let today = chrono::Utc::now().date_naive();

    // No vendors on a fresh edge.
    let (status, body) = get_json(router.clone(), "/vendors", manager).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));

    // Creating a PO auto-creates its vendor row (the same
    // backfill rule the migration applies to older POs).
    let (status, body) = post_json(
        router.clone(),
        "/purchasing/orders",
        serde_json::json!({
            "vendor_id": "Benjamin Moore",
            "store_id": store_id.to_string(),
            "lines": [{"product_id": product_id.to_string(), "qty": "2"}],
        }),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["vendor_id"], "Benjamin Moore");

    let (status, body) = get_json(router.clone(), "/vendors", manager).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["name"], "Benjamin Moore");
    assert_eq!(body[0]["display_name"], "Benjamin Moore");
    assert_eq!(body[0]["po_count"], 1);
    assert_eq!(body[0]["open_po_count"], 1);
    assert!(body[0]["last_po_at"].is_string());
    assert_eq!(body[0]["contact_name"], serde_json::Value::Null);
    let vendor_id = body[0]["id"].as_str().unwrap().to_string();

    let (status, body) = get_json(router.clone(), &format!("/vendors/{vendor_id}"), manager).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["purchase_orders"].as_array().unwrap().len(), 1);
    assert_eq!(body["purchase_orders"][0]["vendor_id"], "Benjamin Moore");

    // Create + update through the vendor endpoints.
    let (status, body) = post_json(
        router.clone(),
        "/vendors",
        serde_json::json!({"name": "Coast Distributors"}),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["po_count"], 0);
    assert_eq!(body["last_po_at"], serde_json::Value::Null);
    let coast_id = body["id"].as_str().unwrap().to_string();
    let (status, _) = post_json(
        router.clone(),
        "/vendors",
        serde_json::json!({"name": "Coast Distributors"}),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = post_json(
        router.clone(),
        &format!("/vendors/{coast_id}"),
        serde_json::json!({"display_name": "Coast", "edi_enabled": true}),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Coast Distributors");
    assert_eq!(body["display_name"], "Coast");
    assert_eq!(body["edi_enabled"], true);

    // Dashboard on an empty day: zeros + one daily row, and the
    // KPI endpoint still answers on the same range.
    let (status, body) = get_json(
        router.clone(),
        &format!("/bi/dashboard?store_id={store_id}&start={today}&end={today}"),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sales"]["sale_count"], 0);
    assert_eq!(body["sales"]["avg_ticket"], serde_json::Value::Null);
    assert_eq!(body["daily"].as_array().unwrap().len(), 1);
    assert_eq!(body["daily"][0]["date"], today.to_string());
    assert_eq!(body["by_category"], serde_json::json!([]));
    assert_eq!(body["tenders"], serde_json::json!([]));
    assert_eq!(body["top_products"], serde_json::json!([]));
    assert_eq!(body["low_stock"], serde_json::json!([]));
    let (status, body) = get_json(
        router.clone(),
        &format!("/bi/kpi?store_id={store_id}&start={today}&end={today}"),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sales"]["sale_count"], 0);
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
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws/v1/store/{store_id}?token={token}"))
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

// --- counter-UI wiring (spec §6.1) ---------------------------------

async fn seed_counter_customer(
    edge: &Edge,
    id: u128,
    account: &str,
    company: &str,
    price_level_id: Option<Uuid>,
) -> Uuid {
    let customer_id = Uuid::from_u128(id);
    df_db::insert_customer(
        edge.pool(),
        &Customer {
            id: customer_id,
            org_id: Uuid::from_u128(0x2000),
            account_number: account.to_string(),
            customer_type: "contractor".to_string(),
            company_name: Some(company.to_string()),
            first_name: None,
            last_name: None,
            email: Some("shop@acme.example".to_string()),
            phone: Some("604-555-0100".to_string()),
            price_level_id,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: false,
            credit_limit: rust_decimal::Decimal::from(1000),
            payment_terms: "NET30".to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
        },
    )
    .await
    .unwrap();
    customer_id
}

/// Import the real Gennex black colorant SKU from the Benjamin
/// Moore price book (10005510, July 17 2026): quart, NET $39.34.
async fn import_gennex_black(edge: &Edge) {
    df_db::import_products(
        edge.pool(),
        Uuid::from_u128(0x2000),
        None,
        &[df_db::ImportProductRow {
            row: 2,
            sku: "0229S1-004".to_string(),
            name: "GENNEX WB CLRNT -BLACK".to_string(),
            category: "COLORANT".to_string(),
            line: "00229".to_string(),
            color: "".to_string(),
            size: "004".to_string(),
            upc: "".to_string(),
            msrp: "$45.00".to_string(),
            map: "$41.00".to_string(),
            net_price: "$39.34".to_string(),
            list_price: "$39.64".to_string(),
            brand_code: "BMC".to_string(),
        }],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn counter_product_search_returns_imported_real_sku() {
    let edge = test_edge().await;
    import_gennex_black(&edge).await;
    let users = seeded_users(&edge).await;
    let router = edge.router();
    let cashier = Some(users.cashier.token.as_str());

    let (status, body) = get_json(router.clone(), "/pos/products/search?q=0229S1", cashier).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 1);
    let row = &body[0];
    assert_eq!(row["sku"], "0229S1-004");
    assert_eq!(row["name"], "GENNEX WB CLRNT -BLACK");
    assert_eq!(row["list_price"], "39.64");
    assert_eq!(row["standard_cost"], "39.34");
    assert_eq!(row["msrp"], "45.00");
    assert_eq!(row["map"], "41.00");
    assert_eq!(row["container_volume_ml"], 946);
    assert_eq!(row["on_hand"], "0");

    // Exact sku and exact barcode (a barcode is a SKU) agree.
    for query in ["sku=0229S1-004", "barcode=0229S1-004"] {
        let (status, body) = get_json(
            router.clone(),
            &format!("/pos/products/search?{query}"),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["sku"], "0229S1-004");
    }

    // Case-insensitive substring over the name.
    let (status, body) = get_json(router.clone(), "/pos/products/search?q=gennex", cashier).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 1);

    // No match is an empty array, not a 404.
    let (status, body) = get_json(
        router.clone(),
        "/pos/products/search?q=does-not-exist",
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn counter_price_levels_preview_applies_gold() {
    let edge = test_edge().await;
    let product_id = seed_product(&edge).await;
    let users = seeded_users(&edge).await;
    let router = edge.router();
    let cashier = Some(users.cashier.token.as_str());
    let gold = df_db::PRICE_LEVEL_GOLD_ID.to_string();

    // The four seeded levels, with the fixed UUIDs.
    let (status, body) = get_json(router.clone(), "/pricing/levels", cashier).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 4);
    let by_code = |code: &str| {
        body.as_array()
            .unwrap()
            .iter()
            .find(|level| level["code"] == code)
            .unwrap()
            .clone()
    };
    assert_eq!(
        by_code("retail")["id"],
        df_db::PRICE_LEVEL_RETAIL_ID.to_string()
    );
    assert_eq!(by_code("retail")["discount_pct"], "0");
    assert_eq!(by_code("gold")["id"], gold);
    assert_eq!(by_code("gold")["discount_pct"], "0.20");
    assert_eq!(by_code("ws")["discount_pct"], "0.25");
    assert_eq!(by_code("platinum")["discount_pct"], "0.30");

    // Explicit Gold on the preview: 2 x $100 x 0.8 = $160
    // subtotal; eco $1.50; tax 7.25% on $161.50 = $11.7088.
    let mut preview = sale_body(product_id, "Cash");
    preview["price_level_id"] = serde_json::json!(gold);
    let (status, body) =
        post_json(router.clone(), "/pos/carts/price-preview", preview, cashier).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["subtotal"]["amount"], "160.0000");
    assert_eq!(body["eco_fee_total"]["amount"], "1.5000");
    assert_eq!(body["tax_total"]["amount"], "11.7088");
    assert_eq!(body["grand_total"]["amount"], "173.2088");

    // The customer's stored level applies when the sale names
    // no level of its own.
    let customer_id = seed_counter_customer(
        &edge,
        0x4000,
        "ACME-001",
        "Acme Painting",
        Some(df_db::PRICE_LEVEL_GOLD_ID),
    )
    .await;
    let mut preview = sale_body(product_id, "Cash");
    preview["customer_id"] = serde_json::json!(customer_id.to_string());
    let (status, body) =
        post_json(router.clone(), "/pos/carts/price-preview", preview, cashier).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["subtotal"]["amount"], "160.0000");
    assert_eq!(body["grand_total"]["amount"], "173.2088");
}

#[tokio::test]
async fn counter_visa_tender_records_brand_and_etransfer_has_no_surcharge() {
    let edge = test_edge().await;
    let product_id = seed_product(&edge).await;
    edge.set_payment_provider(Arc::new(MockTerminal::new()));
    let users = seeded_users(&edge).await;
    let router = edge.router();
    let cashier = Some(users.cashier.token.as_str());

    // Visa authorizes as a card (same 3% surcharge) but the
    // recorded tender stays Visa, never generic Card.
    let (status, body) = post_json(
        router.clone(),
        "/pos/transactions",
        sale_body(product_id, "Visa"),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["grand_total"]["amount"], "222.5921");
    assert_eq!(body["pos_fee_total"]["amount"], "6.4833");
    assert_eq!(body["payments"][0]["tender"], "Visa");
    let tx_id = body["transaction_id"].as_str().unwrap().to_string();

    let (status, body) = get_json(
        router.clone(),
        &format!("/pos/transactions/{tx_id}"),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["payments"][0]["tender"], "visa");
    assert_eq!(body["payments"][0]["amount"], "222.5921");
    assert!(body["payments"][0]["reference"].as_str().is_some());
    assert_eq!(
        body["lines"][0]["description"],
        "Regal Select Interior Gallon"
    );

    // E-Transfer: no provider call, no surcharge, the supplied
    // reference is recorded.
    let mut etransfer = sale_body(product_id, "ETransfer");
    etransfer["tenders"] = serde_json::json!([{
        "kind": "ETransfer",
        "amount": null,
        "reference": "REF-2026-001",
    }]);
    let (status, body) = post_json(router.clone(), "/pos/transactions", etransfer, cashier).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["grand_total"]["amount"], "216.1088");
    assert_eq!(body["pos_fee_total"]["amount"], "0.0000");
    assert_eq!(body["payments"][0]["tender"], "ETransfer");
    assert_eq!(body["payments"][0]["reference"], "REF-2026-001");
    let tx_id = body["transaction_id"].as_str().unwrap().to_string();
    let (status, body) = get_json(
        router.clone(),
        &format!("/pos/transactions/{tx_id}"),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["payments"][0]["tender"], "etransfer");
    assert_eq!(body["payments"][0]["reference"], "REF-2026-001");

    // Unknown transaction: 404.
    let (status, _) = get_json(
        router.clone(),
        &format!("/pos/transactions/{}", Uuid::now_v7()),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn counter_invoice_returns_header_lines_and_totals() {
    let edge = test_edge().await;
    let product_id = seed_product(&edge).await;
    edge.set_payment_provider(Arc::new(MockTerminal::new()));
    let users = seeded_users(&edge).await;
    let router = edge.router();
    let cashier = Some(users.cashier.token.as_str());

    let mut sale = sale_body(product_id, "Visa");
    sale["job_name"] = serde_json::json!("Kitchen repaint");
    sale["po_number"] = serde_json::json!("PO-77");
    let (status, body) = post_json(router.clone(), "/pos/transactions", sale, cashier).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let tx_id = body["transaction_id"].as_str().unwrap().to_string();
    let doc_number = body["doc_number"].as_str().unwrap().to_string();

    let (status, body) = get_json(
        router.clone(),
        &format!("/pos/transactions/{tx_id}/invoice"),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["store_name"], "Test Paint Co");
    assert_eq!(body["doc_number"], doc_number);
    assert_eq!(body["invoice_number"], doc_number);
    assert_eq!(body["job_name"], "Kitchen repaint");
    assert_eq!(body["po_number"], "PO-77");
    assert_eq!(body["fulfilment_mode"], "counter");
    assert_eq!(body["lines"].as_array().unwrap().len(), 1);
    assert_eq!(body["lines"][0]["sku"], "BM-REGAL-1G");
    assert_eq!(body["lines"][0]["size_label"], "Gallon");
    assert_eq!(body["lines"][0]["line_total"], "200.0000");
    assert_eq!(body["tenders"][0]["label"], "Visa");
    assert_eq!(body["tenders"][0]["amount"], "222.5921");
    // US/CA store: no GST/PST split; the stored tax stands alone.
    assert_eq!(body["totals"]["subtotal"], "200.0000");
    assert_eq!(body["totals"]["eco_fee_total"], "1.5000");
    assert_eq!(body["totals"]["tax_total"], "14.6088");
    assert_eq!(body["totals"]["gst"], "0");
    assert_eq!(body["totals"]["pst"], "0");
    assert_eq!(body["totals"]["grand_total"], "222.5921");
    assert_eq!(body["totals"]["change_due"], "0");
    assert!(body["totals"]["charge"].is_null());
}

#[tokio::test]
async fn counter_customer_search_price_level_and_transactions() {
    let edge = test_edge().await;
    let product_id = seed_product(&edge).await;
    edge.set_payment_provider(Arc::new(MockTerminal::new()));
    let users = seeded_users(&edge).await;
    let router = edge.router();
    let cashier = Some(users.cashier.token.as_str());
    let customer_id = seed_counter_customer(&edge, 0x4000, "ACME-001", "Acme Painting", None).await;

    // Substring search over account, company and email.
    for query in ["acme", "ACME-001", "shop@acme"] {
        let (status, body) = get_json(
            router.clone(),
            &format!("/pos/customers/search?q={query}"),
            cashier,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert_eq!(body[0]["account_number"], "ACME-001");
        assert_eq!(body[0]["company_name"], "Acme Painting");
        assert!(body[0]["price_level_id"].is_null());
    }

    // Set the price level, then clear it again.
    let gold = df_db::PRICE_LEVEL_GOLD_ID.to_string();
    let (status, body) = post_json(
        router.clone(),
        &format!("/pos/customers/{customer_id}/price-level"),
        serde_json::json!({"price_level_id": gold}),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["price_level_id"], gold);
    let (status, body) = post_json(
        router.clone(),
        &format!("/pos/customers/{customer_id}/price-level"),
        serde_json::json!({"price_level_id": null}),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["price_level_id"].is_null());

    // An unknown level is a 404, and so is an unknown customer.
    let (status, _) = post_json(
        router.clone(),
        &format!("/pos/customers/{customer_id}/price-level"),
        serde_json::json!({"price_level_id": Uuid::now_v7().to_string()}),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get_json(
        router.clone(),
        &format!("/pos/customers/{}", Uuid::now_v7()),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A sale for this customer shows up in their history, newest
    // first, with lines and payments embedded.
    let mut sale = sale_body(product_id, "Cash");
    sale["customer_id"] = serde_json::json!(customer_id.to_string());
    sale["job_name"] = serde_json::json!("Kitchen repaint");
    let (status, _) = post_json(router.clone(), "/pos/transactions", sale, cashier).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = get_json(
        router.clone(),
        &format!("/pos/customers/{customer_id}/transactions"),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["job_name"], "Kitchen repaint");
    assert_eq!(body[0]["lines"].as_array().unwrap().len(), 1);
    assert_eq!(
        body[0]["lines"][0]["description"],
        "Regal Select Interior Gallon"
    );
    assert_eq!(body[0]["payments"].as_array().unwrap().len(), 1);
    assert_eq!(body[0]["payments"][0]["tender"], "cash");
}

#[tokio::test]
async fn counter_tint_formula_create_validates_colorants() {
    let edge = test_edge().await;
    let product_id = seed_product(&edge).await;
    import_gennex_black(&edge).await;
    let users = seeded_users(&edge).await;
    let router = edge.router();
    let cashier = Some(users.cashier.token.as_str());

    let formula_body = |code: &str| {
        serde_json::json!({
            "customer_id": null,
            "job_id": null,
            "color_code": "CC-100",
            "color_name": "Test Blue",
            "base_product_id": product_id.to_string(),
            "container_volume_ml": 3785,
            "sheen": "Eggshell",
            "colorants": [{"code": code, "oz": "0.5", "shots": 0}],
        })
    };
    let (status, body) = post_json(
        router.clone(),
        "/pos/tinting/formulas",
        formula_body("S1"),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["color_code_snapshot"], "CC-100");
    assert_eq!(body["color_name_snapshot"], "Test Blue");
    // 0.5 oz x $1.229375/fl oz, rounded to 4dp.
    assert_eq!(body["total_tint_cost"], "0.6147");

    // An unknown colorant code is a 400 naming the code.
    let (status, body) = post_json(
        router.clone(),
        "/pos/tinting/formulas",
        formula_body("ZZ"),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("ZZ"), "{body}");
}
