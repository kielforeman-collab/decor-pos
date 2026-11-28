//! End-to-end HTTP tests for customer setup (`POST /pos/customers`
//! and `POST /pos/customers/{id}`) over the real `Edge::router()`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use df_auth_http::test_support::{SeededUsers, seed_test_users};
use df_db::models::{Organization, Store};
use df_domain::Currency;
use df_edge::{Edge, EdgeConfig};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

const STORE_ID: u128 = 0x1100;
const ORG_ID: u128 = 0x2200;

async fn post_json(
    edge: &Edge,
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
    let response = edge
        .router()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn get_json(edge: &Edge, uri: &str, token: Option<&str>) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().uri(uri).method("GET");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = edge
        .router()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn seeded_edge() -> (Edge, SeededUsers) {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Cad,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Cad),
    };
    let edge = Edge::open(config).await.unwrap();
    let pool = edge.pool();
    df_db::insert_organization(
        pool,
        &Organization {
            id: Uuid::from_u128(ORG_ID),
            name: "Test Paint Co".to_string(),
            default_currency: "CAD".to_string(),
            enabled_modules: serde_json::json!({}),
            settings_json: serde_json::json!({}),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    df_db::insert_store(
        pool,
        &Store {
            id: Uuid::from_u128(STORE_ID),
            org_id: Uuid::from_u128(ORG_ID),
            code: "S01".to_string(),
            name: "Main Street".to_string(),
            country_code: "CA".to_string(),
            region_code: "BC".to_string(),
            postal_code: "V6B 1A1".to_string(),
            currency: "CAD".to_string(),
            tax_zone_id: Uuid::now_v7(),
            timezone: "America/Vancouver".to_string(),
            is_warehouse: false,
        },
    )
    .await
    .unwrap();
    let users = seed_test_users(pool, Uuid::from_u128(ORG_ID)).await;
    (edge, users)
}

#[tokio::test]
async fn rbac_gates_customer_create_and_credit_controls() {
    let (edge, users) = seeded_edge().await;

    // 401 unauthenticated.
    let (status, _) = post_json(&edge, "/pos/customers", json!({"first_name": "Jane"}), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // 403 Warehouse (no CustomerManage).
    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({"first_name": "Jane"}),
        Some(&users.warehouse.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Cashier cannot enable a charge account or set credit limit.
    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({
            "company_name": "Sneaky Painters",
            "charge_enabled": true,
            "credit_limit": "5000.00"
        }),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Cashier CAN create a retail customer with contact, address, notes.
    let (status, created) = post_json(
        &edge,
        "/pos/customers",
        json!({
            "customer_type": "retail",
            "first_name": "Jane",
            "last_name": "Doe",
            "email": "jane.doe@example.com",
            "phone": "604-555-0199",
            "address_line1": "123 Main St",
            "city": "Vancouver",
            "province": "BC",
            "postal_code": "v6b 1a1",
            "notes": "Prefers eggshell sheen"
        }),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {created}");
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["account_number"], "C-1001");
    assert_eq!(created["postal_code"], "V6B 1A1");
    assert_eq!(created["notes"], "Prefers eggshell sheen");
    assert_eq!(created["charge_enabled"], false);

    // Search finds the new customer by phone as well as name.
    let (status, found) = get_json(
        &edge,
        "/pos/customers/search?q=555-0199",
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let arr = found.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);
}

#[tokio::test]
async fn manager_creates_and_edits_charge_customer_and_cashier_can_edit_contact_only() {
    let (edge, users) = seeded_edge().await;
    let price_levels = df_db::list_price_levels(edge.pool(), Uuid::from_u128(ORG_ID))
        .await
        .unwrap();
    let gold_id = price_levels[0].id;

    // Manager creates a contractor charge account.
    let (status, created) = post_json(
        &edge,
        "/pos/customers",
        json!({
            "account_number": "C-2050",
            "customer_type": "contractor",
            "company_name": "Harbourview Painting Ltd.",
            "email": "ap@harbourview.example",
            "phone": "604-555-0101",
            "price_level_id": gold_id,
            "tax_exempt_cert": "PST-998877",
            "tax_exempt_expires": "2027-12-31",
            "require_po_number": true,
            "require_job_name": true,
            "charge_enabled": true,
            "credit_limit": "7500.00",
            "payment_terms": "net45",
            "terms_days": 45,
            "finance_rate_pct_monthly": "1.5",
            "portal_enabled": true
        }),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {created}");
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["account_number"], "C-2050");
    assert_eq!(created["payment_terms"], "NET45");
    assert_eq!(created["terms_days"], 45);
    assert_eq!(created["charge_enabled"], true);
    assert_eq!(created["portal_enabled"], true);
    assert_eq!(created["require_po_number"], true);

    // Next auto-assigned account number skips past C-2050 → C-2051.
    let (status, auto_next) = post_json(
        &edge,
        "/pos/customers",
        json!({"company_name": "Next Auto Co"}),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(auto_next["account_number"], "C-2051");

    // Cashier can edit contact/address on the charge customer as long as
    // credit controls stay identical.
    let (status, edited_by_cashier) = post_json(
        &edge,
        &format!("/pos/customers/{id}"),
        json!({
            "account_number": "C-2050",
            "customer_type": "contractor",
            "company_name": "Harbourview Painting Ltd.",
            "email": "billing@harbourview.example",
            "phone": "604-555-0102",
            "address_line1": "800 Marine Dr",
            "city": "North Vancouver",
            "province": "BC",
            "price_level_id": gold_id,
            "tax_exempt_cert": "PST-998877",
            "tax_exempt_expires": "2027-12-31",
            "require_po_number": true,
            "require_job_name": true,
            "charge_enabled": true,
            "credit_limit": "7500.00",
            "payment_terms": "NET45",
            "terms_days": 45,
            "finance_rate_pct_monthly": "1.5",
            "credit_hold": false,
            "portal_enabled": true
        }),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {edited_by_cashier}");
    assert_eq!(edited_by_cashier["email"], "billing@harbourview.example");
    assert_eq!(edited_by_cashier["city"], "North Vancouver");

    // Cashier trying to raise credit limit on edit → 403.
    let (status, _) = post_json(
        &edge,
        &format!("/pos/customers/{id}"),
        json!({
            "account_number": "C-2050",
            "customer_type": "contractor",
            "company_name": "Harbourview Painting Ltd.",
            "charge_enabled": true,
            "credit_limit": "99999.00",
            "payment_terms": "NET45",
            "terms_days": 45,
            "finance_rate_pct_monthly": "1.5",
            "credit_hold": false,
            "portal_enabled": true
        }),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Manager puts account on credit hold and clears notes/address_line2.
    let (status, held) = post_json(
        &edge,
        &format!("/pos/customers/{id}"),
        json!({
            "account_number": "C-2050",
            "customer_type": "commercial",
            "company_name": "Harbourview Coatings Inc.",
            "charge_enabled": true,
            "credit_limit": "10000.00",
            "payment_terms": "NET30",
            "credit_hold": true,
            "portal_enabled": false
        }),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {held}");
    assert_eq!(held["customer_type"], "commercial");
    assert_eq!(held["company_name"], "Harbourview Coatings Inc.");
    assert_eq!(held["credit_hold"], true);
    assert_eq!(held["portal_enabled"], false);
    assert!(held["address_line1"].is_null());
}

#[tokio::test]
async fn validation_rejects_bad_inputs_and_duplicate_accounts() {
    let (edge, users) = seeded_edge().await;

    // Missing name.
    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({"customer_type": "retail"}),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Invalid customer type.
    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({"company_name": "Acme", "customer_type": "vip"}),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Invalid email.
    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({"company_name": "Acme", "email": "not-an-email"}),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Negative credit limit.
    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({"company_name": "Acme", "credit_limit": "-50.00"}),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Unknown price level.
    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({"company_name": "Acme", "price_level_id": Uuid::now_v7()}),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Duplicate account number.
    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({"account_number": "C-DUP", "company_name": "First Co"}),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = post_json(
        &edge,
        "/pos/customers",
        json!({"account_number": "C-DUP", "company_name": "Second Co"}),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Update unknown id → 404.
    let (status, _) = post_json(
        &edge,
        &format!("/pos/customers/{}", Uuid::now_v7()),
        json!({"company_name": "Ghost Co"}),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
