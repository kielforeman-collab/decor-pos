//! End-to-end HTTP integration tests for HQ security & sync hardening:
//!   * Store-scoped `dfs_` service tokens (create / list without hash / ingest /
//!     cross-store 403 / non-ingest route 403 / revoke -> 401)
//!   * F8 HQ `sync_conflicts` triage (`GET /api/v1/sync/conflicts` +
//!     `POST /api/v1/sync/conflicts/{entry_id}/resolve`)
//!   * F10 auth rate limiting (`429` + `Retry-After`), strict CORS allowlist,
//!     and defensive security response headers.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::Utc;
use df_api::{ApiState, create_router_with_hardening};
use df_auth_http::test_support::seed_test_users;
use df_auth_http::{AuthRateLimitConfig, HttpHardeningConfig};
use df_db::{Dialect, Organization, Store, connect, insert_organization, insert_store, migrate};
use df_sync::{DeltaBatch, OutboxEntry};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn setup_hq(
    hardening: HttpHardeningConfig,
) -> (axum::Router, df_db::AnyPool, Uuid, Uuid, Uuid) {
    let pool = connect("sqlite::memory:").await.unwrap();
    migrate(&pool, Dialect::Sqlite).await.unwrap();

    let org_id = Uuid::from_u128(0x2000);
    let store_a = Uuid::from_u128(0x1001);
    let store_b = Uuid::from_u128(0x1002);

    insert_organization(
        &pool,
        &Organization {
            id: org_id,
            name: "Hardening HQ".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: json!({}),
            settings_json: json!({}),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    for (id, code, name) in [(store_a, "ST-A", "Store A"), (store_b, "ST-B", "Store B")] {
        insert_store(
            &pool,
            &Store {
                id,
                org_id,
                code: code.to_string(),
                name: name.to_string(),
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
    }

    let state = ApiState {
        pool: Some(pool.clone()),
        org_id: Some(org_id),
        store_id: Some(store_a),
        ..ApiState::default()
    };
    let router = create_router_with_hardening(state, hardening);
    (router, pool, org_id, store_a, store_b)
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn tx_entry(store_id: Uuid, tx_id: Uuid, total: &str) -> OutboxEntry {
    OutboxEntry::new(
        store_id,
        "transaction",
        tx_id,
        json!({
            "id": tx_id.to_string(),
            "doc_number": "INV-100",
            "doc_type": "sale",
            "status": "completed",
            "subtotal": total,
            "tax_total": "0.0000",
            "eco_fee_total": "0.0000",
            "pos_fee_total": "0.0000",
            "grand_total": total,
        }),
    )
}

#[tokio::test]
async fn service_token_lifecycle_store_binding_and_hq_conflict_triage() {
    let (router, pool, org_id, store_a, store_b) = setup_hq(HttpHardeningConfig::default()).await;

    let users = seed_test_users(&pool, org_id).await;
    let admin_tok = users.admin.token.clone();
    let mgr_tok = users.manager.token.clone();

    // Manager cannot issue service tokens (Admin UserManage required -> 403).
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/service-tokens")
                .header("authorization", format!("Bearer {mgr_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "store_id": store_a, "label": "edge-a" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // Admin issues a service token scoped to store_a.
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/auth/service-tokens")
                .header("authorization", format!("Bearer {admin_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "store_id": store_a, "label": "edge-a", "ttl_days": 365 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let created = body_json(res).await;
    let svc_token = created["token"].as_str().unwrap().to_string();
    let svc_id = created["id"].as_str().unwrap().to_string();
    assert!(svc_token.starts_with("dfs_"));

    // Listing tokens never returns raw token or token_hash.
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/auth/service-tokens")
                .header("authorization", format!("Bearer {admin_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list = body_json(res).await;
    let items = list.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].get("token").is_none());
    assert!(items[0].get("token_hash").is_none());

    // Service token ingests a valid batch for its bound store_a.
    let tx_id = Uuid::now_v7();
    let batch_a = DeltaBatch::new(store_a, vec![tx_entry(store_a, tx_id, "100.0000")]);
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/sync/ingest")
                .header("authorization", format!("Bearer {svc_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&batch_a).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let report = body_json(res).await;
    assert_eq!(report["applied"], 1);
    assert_eq!(report["conflicts"], 0);

    // Cross-store push with store_a's token -> 403 Forbidden.
    let batch_b = DeltaBatch::new(store_b, vec![tx_entry(store_b, Uuid::now_v7(), "50.0000")]);
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/sync/ingest")
                .header("authorization", format!("Bearer {svc_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&batch_b).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // Service token is rejected (403) on non-ingest routes (least privilege).
    for uri in [
        format!("/api/v1/inventory/balances?store_id={store_a}"),
        "/api/v1/sync/conflicts".to_string(),
        "/api/v1/auth/service-tokens".to_string(),
    ] {
        let res = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(&uri)
                    .header("authorization", format!("Bearer {svc_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "uri={uri}");
    }

    // Conflicting re-send of tx_id records HQ conflict + returns conflict_ids.
    let bad_entry = tx_entry(store_a, tx_id, "999.0000");
    let conflict_batch = DeltaBatch::new(store_a, vec![bad_entry.clone()]);
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/sync/ingest")
                .header("authorization", format!("Bearer {svc_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&conflict_batch).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let report = body_json(res).await;
    assert_eq!(report["conflicts"], 1);
    assert_eq!(report["conflict_ids"], json!([bad_entry.id.to_string()]));

    // Manager lists open HQ conflicts and resolves the conflict.
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v1/sync/conflicts?store_id={store_a}"))
                .header("authorization", format!("Bearer {mgr_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let conflicts = body_json(res).await;
    assert_eq!(conflicts.as_array().unwrap().len(), 1);
    assert_eq!(conflicts[0]["entry_id"], bad_entry.id.to_string());

    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/sync/conflicts/{}/resolve", bad_entry.id))
                .header("authorization", format!("Bearer {mgr_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "resolution_note": "posted reversing tx INV-101" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Open conflicts list is now empty; include_resolved=true shows it.
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v1/sync/conflicts?store_id={store_a}"))
                .header("authorization", format!("Bearer {mgr_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_json(res).await.as_array().unwrap().is_empty());

    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/v1/sync/conflicts?store_id={store_a}&include_resolved=true"
                ))
                .header("authorization", format!("Bearer {mgr_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let all_conflicts = body_json(res).await;
    assert_eq!(all_conflicts.as_array().unwrap().len(), 1);
    assert_eq!(
        all_conflicts[0]["resolution_note"],
        "posted reversing tx INV-101"
    );

    // Revoking the service token immediately yields 401 on subsequent ingest.
    let res = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/auth/service-tokens/{svc_id}/revoke"))
                .header("authorization", format!("Bearer {admin_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/sync/ingest")
                .header("authorization", format!("Bearer {svc_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&batch_a).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn hq_auth_rate_limiting_and_strict_cors() {
    let hardening = HttpHardeningConfig {
        cors_origins: vec!["https://hq-ui.example.com".to_string()],
        auth_rate_limit: AuthRateLimitConfig {
            max_requests: 2,
            window: Duration::from_secs(60),
            trust_proxy_headers: false,
        },
        ..HttpHardeningConfig::default()
    };
    let (router, _pool, _org, _store_a, _store_b) = setup_hq(hardening).await;

    // Allowed origin gets CORS header; unlisted origin gets none.
    let ok_cors = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health")
                .header("origin", "https://hq-ui.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        ok_cors
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("https://hq-ui.example.com")
    );
    assert_eq!(
        ok_cors
            .headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );

    let denied_cors = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health")
                .header("origin", "https://attacker.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        denied_cors
            .headers()
            .get("access-control-allow-origin")
            .is_none()
    );

    // 3rd login attempt hits 429 + Retry-After before Argon2id runs.
    let login_payload =
        json!({ "email": "nobody@hq.test", "password": "wrongpassword" }).to_string();
    for expected in [
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::TOO_MANY_REQUESTS,
    ] {
        let res = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(login_payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), expected);
        assert_eq!(
            res.headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
        if expected == StatusCode::TOO_MANY_REQUESTS {
            assert!(res.headers().get("retry-after").is_some());
        }
    }
}
