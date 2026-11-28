//! End-to-end HTTP integration tests for Edge Admin & Sync UI endpoints:
//!   - RBAC matrix across Cashier / Manager / Admin on `/admin/store`,
//!     `/admin/users`, `/admin/service-tokens`, and `/pos/sync/dead-letters`.
//!   - Store settings update (`POST /admin/store`) normalizes jurisdiction
//!     codes, surfaces active tax/eco presets, and immediately drives live
//!     `POST /pos/carts/price-preview` tax computation.
//!   - Staff/user CRUD, duplicate-email 409, password + PIN verification via
//!     real login / counter-switch, self-lockout guardrails, and session
//!     revocation on role change / explicit revoke.
//!   - Store service token issuance (`dfs_`), metadata-only listing, and
//!     revocation.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use df_auth_http::test_support::seed_test_users;
use df_db::models::Product;
use df_domain::{Currency, Money};
use df_edge::{Edge, EdgeConfig, SaleInput, SaleLineInput};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const STORE_ID: u128 = 0x5100;
const ORG_ID: u128 = 0x5200;
const PRODUCT_ID: u128 = 0x5300;

fn dec(s: &str) -> Decimal {
    Decimal::from_str_exact(s).unwrap()
}

fn usd(s: &str) -> Money {
    Money::from_str_amount(s, Currency::Usd).unwrap()
}

fn edge_config() -> EdgeConfig {
    EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Admin Test Store".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    }
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn req(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(tok) = token {
        builder = builder.header("authorization", format!("Bearer {tok}"));
    }
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn admin_rbac_gates_store_users_tokens_and_dead_letters() {
    let edge = Edge::open(edge_config()).await.unwrap();
    edge.ensure_configured_store(None, None).await.unwrap();
    let users = seed_test_users(edge.pool(), Uuid::from_u128(ORG_ID)).await;
    let app = df_edge::http::router(edge);

    // Unauthenticated -> 401
    for (method, path) in [
        (Method::GET, "/admin/store"),
        (Method::POST, "/admin/store"),
        (Method::GET, "/admin/users"),
        (Method::POST, "/admin/users"),
        (Method::GET, "/admin/service-tokens"),
        (Method::POST, "/admin/service-tokens"),
        (Method::GET, "/pos/sync/dead-letters"),
    ] {
        let res = req(&app, method, path, None, None).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "anon {path}");
    }

    // Cashier -> 403 on all admin & dead-letter routes
    for (method, path, payload) in [
        (Method::GET, "/admin/store", None),
        (Method::POST, "/admin/store", Some(json!({"name": "X"}))),
        (Method::GET, "/admin/users", None),
        (Method::GET, "/admin/service-tokens", None),
        (Method::GET, "/pos/sync/dead-letters", None),
    ] {
        let res = req(&app, method, path, Some(&users.cashier.token), payload).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "cashier {path}");
    }

    // Manager -> 200 on read store + dead-letters, 403 on admin writes/users/tokens
    let res = req(
        &app,
        Method::GET,
        "/admin/store",
        Some(&users.manager.token),
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let res = req(
        &app,
        Method::GET,
        "/pos/sync/dead-letters",
        Some(&users.manager.token),
        None,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    for (method, path, payload) in [
        (Method::POST, "/admin/store", Some(json!({"name": "Nope"}))),
        (Method::GET, "/admin/users", None),
        (Method::GET, "/admin/service-tokens", None),
    ] {
        let res = req(&app, method, path, Some(&users.manager.token), payload).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "manager {path}");
    }

    // Admin -> 200 on all admin reads
    for path in ["/admin/store", "/admin/users", "/admin/service-tokens"] {
        let res = req(&app, Method::GET, path, Some(&users.admin.token), None).await;
        assert_eq!(res.status(), StatusCode::OK, "admin {path}");
    }
}

#[tokio::test]
async fn admin_store_settings_updates_jurisdiction_and_drives_live_tax_pricing() {
    let edge = Edge::open(edge_config()).await.unwrap();
    edge.ensure_configured_store(None, None).await.unwrap();
    let users = seed_test_users(edge.pool(), Uuid::from_u128(ORG_ID)).await;

    df_db::insert_product(
        edge.pool(),
        &Product {
            id: Uuid::from_u128(PRODUCT_ID),
            org_id: Uuid::from_u128(ORG_ID),
            sku: "BM-ADMIN-1G".to_string(),
            manufacturer_code: Some("BM".to_string()),
            name: "Admin Test Paint 1G".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: dec("100.0000"),
            standard_cost: dec("50.0000"),
            avg_cost: dec("50.0000"),
            msrp: None,
            map: None,
            min_margin_pct: None,
            sheen: None,
            paint_base_code: None,
            container_volume_ml: None,
            eco_fee_category: None,
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            is_active: true,
        },
    )
    .await
    .unwrap();

    let app = df_edge::http::router(edge);

    let sale = SaleInput {
        customer_id: None,
        job_id: None,
        lines: vec![SaleLineInput {
            product_id: Uuid::from_u128(PRODUCT_ID),
            sku: "BM-ADMIN-1G".to_string(),
            description: "Admin Test Paint 1G".to_string(),
            qty: dec("1"),
            uom: "gal".to_string(),
            unit_price: usd("100.0000"),
            unit_cost: usd("50.0000"),
        }],
        tenders: vec![],
        po_number: None,
        price_level_id: None,
        job_name: None,
    };

    // Initially blank jurisdiction -> zero tax.
    let res = req(
        &app,
        Method::POST,
        "/pos/carts/price-preview",
        Some(&users.admin.token),
        Some(serde_json::to_value(&sale).unwrap()),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let preview = body_json(res).await;
    assert_eq!(preview["tax_total"]["amount"], "0.0000");

    // Admin updates store jurisdiction to CA / BC (5% GST + 7% PST = 12%).
    let res = req(
        &app,
        Method::POST,
        "/admin/store",
        Some(&users.admin.token),
        Some(json!({
            "name": "Vancouver Paint Hub",
            "code": "yvr1",
            "country_code": "ca",
            "region_code": "bc",
            "postal_code": "V6B 1A1",
            "currency": "cad",
            "timezone": "America/Vancouver",
            "is_warehouse": true
        })),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let updated = body_json(res).await;
    assert_eq!(updated["store"]["name"], "Vancouver Paint Hub");
    assert_eq!(updated["store"]["code"], "YVR1");
    assert_eq!(updated["store"]["country_code"], "CA");
    assert_eq!(updated["store"]["region_code"], "BC");
    assert_eq!(updated["store"]["currency"], "CAD");
    assert_eq!(updated["store"]["is_warehouse"], true);
    assert_eq!(updated["active_tax_preset"]["total_rate"], "0.12");
    assert!(updated["active_eco_fee_rule_count"].as_u64().unwrap() > 0);
    assert!(!updated["available_presets"].as_array().unwrap().is_empty());

    // Live price preview now reflects 12% BC tax immediately.
    let res = req(
        &app,
        Method::POST,
        "/pos/carts/price-preview",
        Some(&users.admin.token),
        Some(serde_json::to_value(&sale).unwrap()),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let preview = body_json(res).await;
    assert_eq!(preview["tax_total"]["amount"], "12.0000");
}

#[tokio::test]
async fn admin_users_crud_guardrails_and_session_revocation() {
    let edge = Edge::open(edge_config()).await.unwrap();
    edge.ensure_configured_store(None, None).await.unwrap();
    let users = seed_test_users(edge.pool(), Uuid::from_u128(ORG_ID)).await;
    let app = df_edge::http::router(edge);

    // 1. Create staff user with password + PIN.
    let res = req(
        &app,
        Method::POST,
        "/admin/users",
        Some(&users.admin.token),
        Some(json!({
            "email": "newclerk@store.test",
            "display_name": "New Clerk",
            "role": "Cashier",
            "password": "clerk-password-123",
            "pin": "2468"
        })),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let created = body_json(res).await;
    let clerk_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["role"], "Cashier");
    assert_eq!(created["has_password"], true);
    assert_eq!(created["has_pin"], true);
    assert!(created.get("password_hash").is_none());
    assert!(created.get("counter_pin_hash").is_none());

    // Duplicate email -> 409 Conflict.
    let dup = req(
        &app,
        Method::POST,
        "/admin/users",
        Some(&users.admin.token),
        Some(json!({
            "email": "newclerk@store.test",
            "role": "Cashier",
            "password": "another-password-123"
        })),
    )
    .await;
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    // 2. Created user can log in by password AND counter-switch by PIN.
    let login_res = req(
        &app,
        Method::POST,
        "/pos/auth/login",
        None,
        Some(json!({
            "email": "newclerk@store.test",
            "password": "clerk-password-123"
        })),
    )
    .await;
    assert_eq!(login_res.status(), StatusCode::OK);
    let clerk_token = body_json(login_res).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    let pin_res = req(
        &app,
        Method::POST,
        "/pos/auth/counter-switch",
        None,
        Some(json!({
            "email": "newclerk@store.test",
            "pin": "2468"
        })),
    )
    .await;
    assert_eq!(pin_res.status(), StatusCode::OK);

    // 3. Updating role revokes existing sessions immediately.
    let upd_res = req(
        &app,
        Method::POST,
        &format!("/admin/users/{clerk_id}"),
        Some(&users.admin.token),
        Some(json!({
            "display_name": "Lead Clerk",
            "role": "Manager"
        })),
    )
    .await;
    assert_eq!(upd_res.status(), StatusCode::OK);
    let upd = body_json(upd_res).await;
    assert_eq!(upd["display_name"], "Lead Clerk");
    assert_eq!(upd["role"], "Manager");

    // Old clerk token is now 401.
    let check_old = req(
        &app,
        Method::GET,
        "/pos/sync/state",
        Some(&clerk_token),
        None,
    )
    .await;
    assert_eq!(check_old.status(), StatusCode::UNAUTHORIZED);

    // 4. Explicit revoke-sessions endpoint invalidates fresh session.
    let relogin = req(
        &app,
        Method::POST,
        "/pos/auth/login",
        None,
        Some(json!({
            "email": "newclerk@store.test",
            "password": "clerk-password-123"
        })),
    )
    .await;
    let fresh_tok = body_json(relogin).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    let revoke_res = req(
        &app,
        Method::POST,
        &format!("/admin/users/{clerk_id}/revoke-sessions"),
        Some(&users.admin.token),
        Some(json!({})),
    )
    .await;
    assert_eq!(revoke_res.status(), StatusCode::OK);
    assert!(
        body_json(revoke_res).await["revoked_count"]
            .as_u64()
            .unwrap()
            >= 1
    );

    let check_revoked = req(&app, Method::GET, "/pos/sync/state", Some(&fresh_tok), None).await;
    assert_eq!(check_revoked.status(), StatusCode::UNAUTHORIZED);

    // 5. Self-lockout guardrails: admin cannot deactivate or demote themselves.
    let self_deact = req(
        &app,
        Method::POST,
        &format!("/admin/users/{}", users.admin.user.id),
        Some(&users.admin.token),
        Some(json!({ "active": false })),
    )
    .await;
    assert_eq!(self_deact.status(), StatusCode::BAD_REQUEST);

    let self_demote = req(
        &app,
        Method::POST,
        &format!("/admin/users/{}", users.admin.user.id),
        Some(&users.admin.token),
        Some(json!({ "role": "Cashier" })),
    )
    .await;
    assert_eq!(self_demote.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn admin_service_tokens_create_list_revoke_over_edge_http() {
    let edge = Edge::open(edge_config()).await.unwrap();
    edge.ensure_configured_store(None, None).await.unwrap();
    let users = seed_test_users(edge.pool(), Uuid::from_u128(ORG_ID)).await;
    let app = df_edge::http::router(edge);

    // Create a store-scoped dfs_ token (store_id defaults to configured store).
    let res = req(
        &app,
        Method::POST,
        "/admin/service-tokens",
        Some(&users.admin.token),
        Some(json!({
            "label": "Front Counter Edge",
            "ttl_days": 90
        })),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let created = body_json(res).await;
    let raw_token = created["token"].as_str().unwrap();
    assert!(raw_token.starts_with("dfs_"));
    assert_eq!(created["label"], "Front Counter Edge");
    assert_eq!(created["store_id"], Uuid::from_u128(STORE_ID).to_string());
    assert!(created["expires_at"].is_string());
    let token_id = created["id"].as_str().unwrap().to_string();

    // List returns metadata only — no raw token, no token_hash.
    let list_res = req(
        &app,
        Method::GET,
        "/admin/service-tokens",
        Some(&users.admin.token),
        None,
    )
    .await;
    assert_eq!(list_res.status(), StatusCode::OK);
    let list = body_json(list_res).await;
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], token_id);
    assert!(arr[0].get("token").is_none());
    assert!(arr[0].get("token_hash").is_none());

    // Revoke succeeds once, then 404 on second revoke.
    let rev_res = req(
        &app,
        Method::POST,
        &format!("/admin/service-tokens/{token_id}/revoke"),
        Some(&users.admin.token),
        Some(json!({})),
    )
    .await;
    assert_eq!(rev_res.status(), StatusCode::OK);
    assert_eq!(body_json(rev_res).await["revoked"], true);

    let rev_again = req(
        &app,
        Method::POST,
        &format!("/admin/service-tokens/{token_id}/revoke"),
        Some(&users.admin.token),
        Some(json!({})),
    )
    .await;
    assert_eq!(rev_again.status(), StatusCode::NOT_FOUND);
}
