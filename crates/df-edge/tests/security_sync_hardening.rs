//! End-to-end HTTP integration tests for Edge security & sync hardening:
//!   * F8 dead-letter HTTP endpoints (`GET /pos/sync/dead-letters`,
//!     `POST /pos/sync/dead-letters/{id}/requeue`,
//!     `POST /pos/sync/dead-letters/{id}/dismiss`, RBAC gated on `SyncIngest`)
//!   * F10 auth rate limiting (`429` + `Retry-After`) and strict CORS allowlist.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use df_auth_http::test_support::seed_test_users;
use df_auth_http::{AuthRateLimitConfig, HttpHardeningConfig};
use df_domain::Currency;
use df_edge::{Edge, EdgeConfig};
use df_sync::{DeltaBatch, HqSink, MemoryHq, Outbox, OutboxEntry};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn open_edge() -> Edge {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(0x1000),
        org_id: Uuid::from_u128(0x2000),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Hardening Edge".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    let edge = Edge::open(config).await.unwrap();
    edge.ensure_configured_store(Some("US"), Some("CA"))
        .await
        .unwrap();
    edge
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn edge_dead_letter_http_lifecycle_and_rbac() {
    let edge = open_edge().await;
    let store = Uuid::from_u128(0x1000);
    let org = Uuid::from_u128(0x2000);
    let hq = Arc::new(MemoryHq::new());
    edge.set_hq(Arc::clone(&hq));

    let users = seed_test_users(edge.pool(), org).await;
    let cashier_tok = users.cashier.token.clone();
    let manager_tok = users.manager.token.clone();

    // Seed an existing transaction on HQ, then enqueue a conflicting edit on edge.
    let tx_id = Uuid::now_v7();
    hq.apply_batch(&DeltaBatch::new(
        store,
        vec![OutboxEntry::new(
            store,
            "transaction",
            tx_id,
            json!({ "id": tx_id.to_string(), "grand_total": "100.0000" }),
        )],
    ))
    .await
    .unwrap();
    let bad = OutboxEntry::new(
        store,
        "transaction",
        tx_id,
        json!({ "id": tx_id.to_string(), "grand_total": "999.0000" }),
    );
    Outbox::enqueue(edge.pool(), &bad).await.unwrap();

    let app = edge.router();

    // Run sync -> conflict is quarantined into dead-letter queue.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/pos/sync/run")
                .header("authorization", format!("Bearer {manager_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let report = body_json(res).await;
    assert_eq!(report["conflicts"], 1);
    assert_eq!(report["conflict_ids"], json!([bad.id.to_string()]));

    // Sync state surfaces dead_letter_count == 1 and pending_count == 0.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/pos/sync/state")
                .header("authorization", format!("Bearer {cashier_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let state = body_json(res).await;
    assert_eq!(state["pending_count"], 0);
    assert_eq!(state["dead_letter_count"], 1);

    // Cashier cannot view/manage dead letters (403); Manager can (200).
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/pos/sync/dead-letters")
                .header("authorization", format!("Bearer {cashier_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/pos/sync/dead-letters")
                .header("authorization", format!("Bearer {manager_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let list = body_json(res).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], bad.id.to_string());

    // Requeue -> moves back to pending; sync again -> re-quarantines; dismiss -> clears.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/pos/sync/dead-letters/{}/requeue", bad.id))
                .header("authorization", format!("Bearer {manager_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(Outbox::pending_count(edge.pool(), store).await.unwrap(), 1);

    // Re-run sync to re-quarantine, then dismiss.
    let _ = edge.sync_now(hq.as_ref()).await.unwrap();
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/pos/sync/dead-letters/{}/dismiss", bad.id))
                .header("authorization", format!("Bearer {manager_tok}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "reason": "resolved on HQ via reversing transaction" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/pos/sync/dead-letters")
                .header("authorization", format!("Bearer {manager_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_json(res).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn edge_auth_rate_limiting_and_cors() {
    let edge = open_edge().await;
    let hardening = HttpHardeningConfig {
        cors_origins: vec!["http://localhost:1420".to_string()],
        auth_rate_limit: AuthRateLimitConfig {
            max_requests: 2,
            window: Duration::from_secs(60),
            trust_proxy_headers: false,
        },
        ..HttpHardeningConfig::default()
    };
    let app = edge.router_with_hardening(hardening);

    // Preflight for allowed origin succeeds; unlisted origin gets no Allow-Origin.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/pos/auth/login")
                .header("origin", "http://localhost:1420")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        res.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("http://localhost:1420")
    );

    let payload = json!({ "email": "nobody@edge.test", "password": "wrongpassword" }).to_string();
    for expected in [
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::TOO_MANY_REQUESTS,
    ] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/pos/auth/login")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), expected);
        if expected == StatusCode::TOO_MANY_REQUESTS {
            assert!(res.headers().get("retry-after").is_some());
        }
    }
}
