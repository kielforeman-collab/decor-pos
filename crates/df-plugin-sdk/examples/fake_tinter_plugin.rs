//! Fake tinter plugin for `df-plugin-sdk` tests.
//!
//! A tiny real HTTP server (axum) implementing the HTTP plugin
//! protocol the SDK speaks: `GET /health`, `POST /echo`,
//! `POST /tinter/list_dispensers`, `POST /tinter/send_formula` and
//! `POST /tinter/dispense_result`. Run as:
//!
//! ```sh
//! cargo run --example fake_tinter_plugin -- --port 48001
//! ```
//!
//! This is test support only — it is not shipped, and it is not a
//! vendor plugin (no COLORx, spec §9).

use axum::Json;
use axum::routing::{get, post};
use std::sync::atomic::{AtomicU64, Ordering};

static JOB_SEQ: AtomicU64 = AtomicU64::new(1);

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn echo(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    Json(body)
}

async fn list_dispensers() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "dispensers": [
            { "id": "sim-1", "name": "Virtual Dispenser 1", "status": "ready" },
            { "id": "sim-2", "name": "Virtual Dispenser 2", "status": "offline" },
        ]
    }))
}

async fn send_formula(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    let seq = JOB_SEQ.fetch_add(1, Ordering::SeqCst);
    let color_code = body
        .get("color_code")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    Json(serde_json::json!({
        "job_id": format!("job-{seq:04}-{color_code}"),
        "status": "accepted",
        "dispenser_id": "sim-1",
    }))
}

async fn dispense_result(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    let job_id = body
        .get("job_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Json(serde_json::json!({
        "job_id": job_id,
        "status": "completed",
        "external_ref": format!("SIM-{job_id}"),
    }))
}

#[tokio::main]
async fn main() {
    let mut port: u16 = 48001;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--port" {
            if let Some(value) = args.next() {
                port = value.parse().expect("--port must be a number");
            }
        }
    }
    let app = axum::Router::new()
        .route("/health", get(health))
        .route("/echo", post(echo))
        .route("/tinter/list_dispensers", post(list_dispensers))
        .route("/tinter/send_formula", post(send_formula))
        .route("/tinter/dispense_result", post(dispense_result));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind fake tinter plugin");
    axum::serve(listener, app).await.expect("serve fake plugin");
}
