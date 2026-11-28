//! HQ HTTP server mode (`df-server hq`).

use std::net::SocketAddr;

use uuid::Uuid;

use crate::db::{migrate_and_report, require_db_url};

/// `df-server hq`: connect, migrate, then serve the `df-api`
/// router (Phase 7 auth gate included — `create_router` applies
/// it, so HQ routes are authenticated exactly like `df-api`'s).
pub(crate) async fn run_hq(
    db_flag: Option<String>,
    listen: SocketAddr,
    org_id: Uuid,
    store_id: Option<Uuid>,
) -> ! {
    let db = require_db_url(db_flag, "hq");
    let (pool, report) = migrate_and_report(&db, "hq").await;
    println!(
        "df-server hq: database {} — {} migration(s) applied in total ({} newly applied this start)",
        df_db::redact_db_url(&db),
        report.applied.len(),
        report.newly_applied.len()
    );
    if !report.newly_applied.is_empty() {
        println!(
            "df-server hq: newly applied migrations: {:?}",
            report.newly_applied
        );
    }
    let state = df_api::ApiState {
        pool: Some(pool),
        org_id: Some(org_id),
        store_id,
        ..df_api::ApiState::default()
    };
    let router = df_api::create_router(state);
    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("df-server hq: cannot bind {listen}: {err}");
            std::process::exit(1);
        }
    };
    println!("df-server hq listening on http://{listen} (org {org_id})");
    println!("OpenAPI: http://{listen}/api-docs/openapi.json");
    if let Err(err) = axum::serve(listener, router).await {
        eprintln!("df-server hq: serve failed: {err}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
