//! `df-api` binary — serves the Phase 0 Axum skeleton on 127.0.0.1:3000.
//!
//! Phase 0 only: `/health` and `/api-docs/openapi.json` work; auth routes
//! return 501. No database is wired yet.

use df_api::{ApiState, create_router};

#[tokio::main]
async fn main() {
    let router = create_router(ApiState::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("bind 127.0.0.1:3000");
    println!("df-api 0.1.0 listening on http://127.0.0.1:3000");
    println!("OpenAPI: http://127.0.0.1:3000/api-docs/openapi.json");
    axum::serve(listener, router).await.expect("serve df-api");
}
