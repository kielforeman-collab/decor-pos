use super::*;
use axum::body::Body;
use axum::http::Request;
use chrono::Utc;
use df_db::models::{ColorCatalogEntry, Colorant, Organization, Store};
use rust_decimal::Decimal;
use std::str::FromStr;
use tower::ServiceExt;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn seeded_state() -> (ApiState, df_auth_http::test_support::SeededUsers) {
    let pool = df_db::connect("sqlite::memory:").await.unwrap();
    df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
    let org_id = Uuid::from_u128(0x2000);
    let store_id = Uuid::from_u128(0x1000);
    df_db::insert_organization(
        &pool,
        &Organization {
            id: org_id,
            name: "Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({}),
            settings_json: serde_json::json!({}),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    df_db::insert_store(
        &pool,
        &Store {
            id: store_id,
            org_id,
            code: "MAIN".to_string(),
            name: "Main Store".to_string(),
            country_code: "US".to_string(),
            region_code: "CA".to_string(),
            postal_code: "90210".to_string(),
            currency: "USD".to_string(),
            tax_zone_id: Uuid::now_v7(),
            timezone: "America/Los_Angeles".to_string(),
            is_warehouse: false,
        },
    )
    .await
    .unwrap();
    df_db::insert_color(
        &pool,
        &ColorCatalogEntry {
            id: Uuid::from_u128(0x6000),
            org_id,
            manufacturer: "Benjamin Moore".to_string(),
            color_code: "OC-17".to_string(),
            color_name: "White Dove".to_string(),
            collection: None,
            rgb_hex: None,
            recommended_base: None,
        },
    )
    .await
    .unwrap();
    // The formula's base product must exist (SQLite enforces
    // the `tint_formulas.base_product_id -> products(id)` FK).
    df_db::insert_product(
        &pool,
        &df_db::models::Product {
            id: Uuid::from_u128(0x3000),
            org_id,
            sku: "BM-REGAL-1G".to_string(),
            manufacturer_code: Some("BM".to_string()),
            name: "Regal Select Interior Gallon".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: dec("100.0000"),
            standard_cost: dec("50.0000"),
            avg_cost: dec("50.0000"),
            min_margin_pct: None,
            sheen: Some("Eggshell".to_string()),
            paint_base_code: Some("1X".to_string()),
            container_volume_ml: Some(3785),
            eco_fee_category: Some("architectural_paint".to_string()),
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            upc: None,
            category: None,
            line_code: None,
            color_name: None,
            size_code: None,
            source_metadata: None,
            is_active: true,
            map: None,
            msrp: None,
        },
    )
    .await
    .unwrap();
    let y3 = Uuid::from_u128(0x5001);
    df_db::insert_colorant(
        &pool,
        &Colorant {
            id: y3,
            org_id,
            system_name: "Gennex".to_string(),
            code: "Y3".to_string(),
            name: "Yellow Oxide".to_string(),
            cost_per_fl_oz: dec("0.4200"),
            shots_per_fl_oz: 48,
        },
    )
    .await
    .unwrap();
    let users = df_auth_http::test_support::seed_test_users(&pool, org_id).await;
    (
        ApiState {
            version: "1.2.5",
            pool: Some(pool),
            org_id: Some(org_id),
            store_id: Some(store_id),
        },
        users,
    )
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
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
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
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn health_returns_ok() {
    let router = create_router(ApiState::default());
    let response = router
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["status"], "ok");
    assert_eq!(value["version"], "1.2.5");
}

#[tokio::test]
async fn openapi_document_is_3_1() {
    let router = create_router(ApiState::default());
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api-docs/openapi.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["openapi"], "3.1.0");
    assert!(value["paths"]["/health"].is_object());
    assert!(value["paths"]["/api/v1/auth/login"].is_object());
    assert!(value["paths"]["/api/v1/tinting/formulas"].is_object());
    assert!(value["paths"]["/api/v1/reports/daily-usage"].is_object());
    // The public document describes shapes only: it must not
    // contain any data rows (no product SKU, no user email, no
    // token, no seeded account number).
    let text = serde_json::to_string(&value).unwrap();
    for leak in [
        "BM-REGAL-1G",
        "example.invalid",
        "dfp_",
        "C-PORTAL-A",
        "C-100",
    ] {
        assert!(!text.contains(leak), "openapi doc leaks {leak}");
    }
}

#[tokio::test]
async fn inventory_and_ar_reads() {
    use df_db::models::{ArLedgerEntry, Customer, InventoryLedgerEntry};

    let (state, users) = seeded_state().await;
    let pool = state.pool.clone().unwrap();
    let store_id = state.store_id.unwrap();
    let org_id = state.org_id.unwrap();
    let router = create_router(state);
    let admin = Some(users.admin.token.as_str());

    // 10 units received against the seeded Regal product.
    df_db::insert_inventory_delta(
        &pool,
        &InventoryLedgerEntry {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            product_id: Uuid::from_u128(0x3000),
            lot_number: None,
            delta_on_hand: dec("10"),
            delta_committed: Decimal::ZERO,
            delta_on_order: Decimal::ZERO,
            delta_in_transit: Decimal::ZERO,
            unit_cost: dec("50.0000"),
            reason_type: "po_receive".to_string(),
            reference_id: Uuid::now_v7(),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    let (status, body) = get_json(
        router.clone(),
        &format!("/api/v1/inventory/balances?store_id={store_id}"),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["sku"], "BM-REGAL-1G");
    assert_eq!(body[0]["on_hand"], "10");

    // One charge customer: $100 due Nov 1 (current on Nov 25),
    // $50 due Oct 1 (31-60), $20 payment (reduces current).
    let customer_id = Uuid::from_u128(0x4000);
    df_db::insert_customer(
        &pool,
        &Customer {
            id: customer_id,
            org_id,
            account_number: "C-100".to_string(),
            customer_type: "contractor".to_string(),
            company_name: Some("Acme Painting".to_string()),
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: true,
            credit_limit: dec("1000.0000"),
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
    for (amount, due) in [
        ("100.0000", Some("2026-11-01")),
        ("50.0000", Some("2026-10-01")),
        ("-20.0000", None),
    ] {
        df_db::insert_ar_entry(
            &pool,
            &ArLedgerEntry {
                id: Uuid::now_v7(),
                org_id,
                store_id,
                customer_id,
                job_id: None,
                entry_type: if amount.starts_with('-') {
                    "payment".to_string()
                } else {
                    "charge".to_string()
                },
                reference_type: "transaction".to_string(),
                reference_id: Uuid::now_v7(),
                amount: dec(amount),
                due_date: due.map(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap()),
                statement_id: None,
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    }
    let (status, body) = get_json(
        router.clone(),
        &format!("/api/v1/ar/customers/{customer_id}/summary"),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["balance"], "130.0000");
    assert_eq!(body["entry_count"], 3);
    let (status, body) = get_json(
        router.clone(),
        &format!("/api/v1/ar/aging?store_id={store_id}&as_of=2026-11-25"),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["totals"],
        serde_json::json!(["80.0000", "50.0000", "0.0000", "0.0000", "0.0000"])
    );
    assert_eq!(body["customers"][0]["account_number"], "C-100");
}

#[tokio::test]
async fn hq_login_logout_flow() {
    use df_auth_http::test_support::{TEST_PASSWORD, TEST_PIN};

    let (state, users) = seeded_state().await;
    let store_id = state.store_id.unwrap();
    let router = create_router(state);

    // Wrong password: generic 401.
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"email": "admin@example.invalid", "password": "wrong password"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "invalid credentials");

    // Right password: a dfp_ token, role + expiry in the body.
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"email": "admin@example.invalid", "password": TEST_PASSWORD}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = body["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("dfp_"));
    assert_eq!(body["role"], "Admin");
    assert!(body["expires_at"].is_string());

    // The token works on a protected route…
    let (status, _) = get_json(
        router.clone(),
        &format!("/api/v1/inventory/balances?store_id={store_id}"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // …logout revokes it…
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/logout",
        serde_json::json!({}),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revoked"], true);

    // …and the revoked token is a 401 from then on.
    let (status, _) = get_json(
        router.clone(),
        &format!("/api/v1/inventory/balances?store_id={store_id}"),
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Counter-switch by PIN for the cashier, and a manager
    // override approval by the manager's PIN.
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/counter-switch",
        serde_json::json!({"email": "cashier@example.invalid", "pin": TEST_PIN}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["role"], "Cashier");

    let (status, body) = post_json(
        router.clone(),
        "/api/v1/auth/override-verify",
        serde_json::json!({"user_id": users.manager.id().to_string(), "pin": TEST_PIN, "reason_code": "price_below_floor"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["approved"], true);
    assert_eq!(body["approver_role"], "Manager");
}

#[tokio::test]
async fn hq_unauthenticated_is_401() {
    let (state, _) = seeded_state().await;
    let store_id = state.store_id.unwrap();
    let router = create_router(state);
    // No token on a protected HQ route: 401, never 403.
    let (status, _) = get_json(
        router.clone(),
        &format!("/api/v1/inventory/balances?store_id={store_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = get_json(router.clone(), "/api/v1/crm/activities", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Health and the OpenAPI document stay public.
    let (status, _) = get_json(router.clone(), "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = get_json(router, "/api-docs/openapi.json", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn hq_cashier_cannot_create_formula_but_manager_can() {
    let (state, users) = seeded_state().await;
    let org_id = state.org_id.unwrap();
    let store_id = state.store_id.unwrap();
    let router = create_router(state);
    let body = serde_json::json!({
        "id": Uuid::now_v7().to_string(),
        "org_id": org_id.to_string(),
        "store_id": store_id.to_string(),
        "customer_id": null,
        "job_id": null,
        "color_id": null,
        "color_code_snapshot": "OC-17",
        "color_name_snapshot": "White Dove",
        "base_product_id": Uuid::from_u128(0x3000).to_string(),
        "container_volume_ml": 3785,
        "sheen": "Eggshell",
        "room_or_usage": null,
        "is_custom_match": false,
        "colorants_json": [{"code": "Y3", "oz": 0, "shots": 14}],
        "total_tint_cost": "0.1225",
        "tint_surcharge": "0.0000",
        "dispenser_name": null,
        "external_formula_ref": null,
        "dispensed_at": Utc::now().to_rfc3339(),
    });
    // Creating a formula needs InventoryAdjust: the cashier gets
    // 403, the manager succeeds with 201.
    let (status, _) = post_json(
        router.clone(),
        "/api/v1/tinting/formulas",
        body.clone(),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/tinting/formulas",
        body,
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["color_code_snapshot"], "OC-17");
}

#[tokio::test]
async fn admin_endpoints_503_without_a_database() {
    let router = create_router(ApiState::default());
    let (status, body) = get_json(
        router,
        "/api/v1/reports/daily-usage?store_id=00000000-0000-7000-8000-000000000001",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["error"].as_str().unwrap().contains("--db"));
}

#[tokio::test]
async fn tinting_admin_round_trip() {
    let (state, users) = seeded_state().await;
    let store_id = state.store_id.unwrap();
    let org_id = state.org_id.unwrap();
    let router = create_router(state);
    let _ = org_id;
    let admin = Some(users.admin.token.as_str());

    // Color search finds White Dove.
    let (status, body) = get_json(
        router.clone(),
        "/api/v1/colors/search?manufacturer=Benjamin%20Moore&query=dove",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["color_code"], "OC-17");

    // Create a formula, then find it by customer + by color.
    let customer_id = Uuid::from_u128(0x4000);
    let formula_id = Uuid::from_u128(0x7000);
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/tinting/formulas",
        serde_json::json!({
            "id": formula_id.to_string(),
            "org_id": Uuid::from_u128(0x2000).to_string(),
            "store_id": store_id.to_string(),
            "customer_id": customer_id.to_string(),
            "job_id": null,
            "color_id": Uuid::from_u128(0x6000).to_string(),
            "color_code_snapshot": "OC-17",
            "color_name_snapshot": "White Dove",
            "base_product_id": Uuid::from_u128(0x3000).to_string(),
            "container_volume_ml": 3785,
            "sheen": "Eggshell",
            "room_or_usage": null,
            "is_custom_match": false,
            "colorants_json": [{"code": "Y3", "oz": 0, "shots": 14}],
            "total_tint_cost": "0.1225",
            "tint_surcharge": "0.0000",
            "dispenser_name": null,
            "external_formula_ref": null,
            "dispensed_at": Utc::now().to_rfc3339(),
        }),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["id"], formula_id.to_string());
    let (status, body) = get_json(
        router.clone(),
        &format!("/api/v1/tinting/formulas?customer_id={customer_id}"),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 1);
    let (status, body) = get_json(
        router.clone(),
        "/api/v1/tinting/formulas?query=white%20dove",
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 1);

    // Colorant catalog lists Y3 for the org.
    let (status, body) = get_json(router.clone(), "/api/v1/tinting/colorants", admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["code"], "Y3");

    // Reports reflect the formula recorded today and empty low-stock products.
    let (status, body) = get_json(
        router.clone(),
        &format!("/api/v1/reports/daily-usage?store_id={store_id}"),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["formulas_recorded"], 1);
    let (status, body) = get_json(
        router.clone(),
        &format!("/api/v1/reports/low-stock?store_id={store_id}"),
        admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["products"].as_array().unwrap().is_empty());
}

fn sync_sale_delta_batch(store_id: Uuid, customer_id: Uuid, tx_id: Uuid) -> df_sync::DeltaBatch {
    use df_sync::{DeltaBatch, OutboxEntry};
    DeltaBatch::new(
        store_id,
        vec![
            OutboxEntry::new(
                store_id,
                "transaction",
                tx_id,
                serde_json::json!({
                    "id": tx_id.to_string(),
                    "doc_number": "TX-000001",
                    "doc_type": "sale",
                    "status": "completed",
                    "customer_id": customer_id.to_string(),
                    "subtotal": "200.0000",
                    "tax_total": "16.1088",
                    "eco_fee_total": "0.0000",
                    "pos_fee_total": "0.0000",
                    "grand_total": "216.1088"
                }),
            ),
            OutboxEntry::new(
                store_id,
                "inventory_ledger",
                Uuid::now_v7(),
                serde_json::json!({
                    "product_id": Uuid::from_u128(0x3000).to_string(),
                    "delta_on_hand": "-2",
                    "delta_committed": "0",
                    "delta_on_order": "0",
                    "delta_in_transit": "0",
                    "unit_cost": "50.0000",
                    "lot_number": null,
                    "reason_type": "sale",
                    "reference_id": tx_id.to_string(),
                    "created_at": Utc::now().to_rfc3339()
                }),
            ),
            OutboxEntry::new(
                store_id,
                "ar_ledger",
                Uuid::now_v7(),
                serde_json::json!({
                    "customer_id": customer_id.to_string(),
                    "amount": "216.1088",
                    "entry_type": "charge",
                    "reference_type": "transaction",
                    "reference_id": tx_id.to_string(),
                    "due_date": "2026-12-26",
                    "created_at": Utc::now().to_rfc3339()
                }),
            ),
        ],
    )
}

fn sync_sale_batch(store_id: Uuid, customer_id: Uuid, tx_id: Uuid) -> serde_json::Value {
    serde_json::to_value(sync_sale_delta_batch(store_id, customer_id, tx_id)).unwrap()
}

#[tokio::test]
async fn sync_ingest_applies_sale_and_replay_is_noop() {
    let (state, users) = seeded_state().await;
    let store_id = Uuid::from_u128(0x1000);
    let router = create_router(state);
    let tx_id = Uuid::now_v7();
    let batch = sync_sale_batch(store_id, users.customer_a, tx_id);
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/sync/ingest",
        batch.clone(),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], 3);
    assert_eq!(body["duplicates"], 0);
    assert_eq!(body["conflicts"], 0);
    assert_eq!(body["applied_ids"].as_array().unwrap().len(), 3);

    // Replay: the same entry UUIDs are a no-op.
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/sync/ingest",
        batch,
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], 0);
    assert_eq!(body["duplicates"], 3);
    assert_eq!(body["conflicts"], 0);
}

#[tokio::test]
async fn sync_ingest_auth() {
    let (state, users) = seeded_state().await;
    let router = create_router(state);
    let empty = serde_json::json!({
        "batch_id": Uuid::now_v7().to_string(),
        "store_id": Uuid::from_u128(0x1000).to_string(),
        "entries": []
    });
    // No token -> 401.
    let (status, _) = post_json(router.clone(), "/api/v1/sync/ingest", empty.clone(), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Cashier -> 403 (SyncIngest is Manager/Admin only).
    let (status, _) = post_json(
        router.clone(),
        "/api/v1/sync/ingest",
        empty.clone(),
        Some(&users.cashier.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Portal contractor -> 403 (no staff permissions at all).
    let (status, _) = post_json(
        router.clone(),
        "/api/v1/sync/ingest",
        empty.clone(),
        Some(&users.contractor_a.token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Manager -> 200 on an empty batch.
    let (status, body) = post_json(
        router.clone(),
        "/api/v1/sync/ingest",
        empty.clone(),
        Some(&users.manager.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], 0);
    // Admin -> 200 too.
    let (status, body) = post_json(
        router,
        "/api/v1/sync/ingest",
        empty,
        Some(&users.admin.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn sync_ingest_remote_hq_end_to_end() {
    use df_sync::{HqSink, RemoteHq};
    let (state, users) = seeded_state().await;
    let store_id = Uuid::from_u128(0x1000);
    let router = create_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let remote = RemoteHq::new(format!("http://{addr}"), users.manager.token.clone());
    let batch = sync_sale_delta_batch(store_id, users.customer_a, Uuid::now_v7());
    let report = remote.apply_batch(&batch).await.unwrap();
    assert_eq!(report.applied, 3);
    assert_eq!(report.duplicates, 0);
    assert_eq!(report.conflicts, 0);
    // Replay through the remote client is a duplicate, not a
    // double post.
    let replay = remote.apply_batch(&batch).await.unwrap();
    assert_eq!(replay.applied, 0);
    assert_eq!(replay.duplicates, 3);
}
