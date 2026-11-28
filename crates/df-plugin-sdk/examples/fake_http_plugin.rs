//! Fake HTTP plugin for `df-plugin-sdk` host integration tests.
//!
//! A tiny real HTTP server (axum) implementing the generic HTTP
//! plugin protocol the SDK speaks: `GET /health` and `POST /echo`.
//! Run as:
//!
//! ```sh
//! cargo run --example fake_http_plugin -- --port 48001
//! ```
//!
//! Test support only — not shipped.

use axum::Json;
use axum::routing::{get, post};

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn echo(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    Json(body)
}

#[tokio::main]
async fn main() {
    let mut port: u16 = 48001;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--port"
            && let Some(value) = args.next()
        {
            port = value.parse().expect("--port must be a number");
        }
    }
    let app = axum::Router::new()
        .route("/health", get(health))
        .route("/echo", post(echo));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind fake http plugin");
    axum::serve(listener, app)
        .await
        .expect("serve fake http plugin");
}
