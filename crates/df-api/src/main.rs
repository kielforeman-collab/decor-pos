//! `df-api` binary — serves the Axum API (default 127.0.0.1:3000;
//! `--listen 0.0.0.0:3000` for Docker — see `docs/self-hosting.md`).
//!
//! Phase 2 (v0.3): `/health` and `/api-docs/openapi.json` work, auth
//! routes return 501, and the tinting-admin + report endpoints work
//! when a database is configured:
//!
//! ```text
//! df-api --db sqlite:edge.db --org-id <uuid> --store-id <uuid>
//! ```
//!
//! Without `--db` the admin endpoints return 503 (see `df_api` docs).
//! The pool is opened read/write but this binary runs no migrations —
//! the database must already exist (created by `df-server edge` or a
//! migration run); a missing table surfaces as a 500 from the
//! handler, not a startup failure. Phase 7 (v1.1): every route
//! except `/health`, `/api-docs/openapi.json` and the auth login
//! endpoints requires a `Bearer dfp_...` session token.

use clap::Parser;
use df_api::{ApiState, create_router};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "df-api",
    version,
    about = "decor-pos HQ API (Phase 2: tinting admin + reports need --db/--org-id)"
)]
struct Cli {
    /// sqlx database URL, e.g. `sqlite:edge.db` or a PostgreSQL URL.
    #[arg(long)]
    db: Option<String>,
    /// Organization UUID the admin endpoints scope to.
    #[arg(long)]
    org_id: Option<Uuid>,
    /// Default store UUID (query ?store_id= overrides it per call).
    #[arg(long)]
    store_id: Option<Uuid>,
    /// Address to listen on. Defaults to loopback; pass
    /// `0.0.0.0:3000` when running inside Docker (see
    /// `docs/self-hosting.md`). Phase 7 (v1.1): every route except
    /// `/health`, `/api-docs/openapi.json` and the auth login
    /// endpoints requires a `Bearer dfp_...` session token — see
    /// `docs/security-review-v1.0.md` and `docs/auth.md`.
    #[arg(long, default_value = "127.0.0.1:3000")]
    listen: std::net::SocketAddr,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mut state = ApiState {
        org_id: cli.org_id,
        store_id: cli.store_id,
        ..ApiState::default()
    };
    if let Some(db) = &cli.db {
        match df_db::connect(db).await {
            Ok(pool) => {
                state.pool = Some(pool);
                println!("df-api: database connected ({})", df_db::redact_db_url(db));
            }
            Err(err) => {
                eprintln!(
                    "df-api: failed to connect to {}: {err}",
                    df_db::redact_db_url(db)
                );
                std::process::exit(1);
            }
        }
    } else {
        println!("df-api: no --db given; admin endpoints will return 503");
    }
    let router = create_router(state);
    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .expect("bind listen address");
    println!(
        "df-api {} listening on http://{}",
        env!("CARGO_PKG_VERSION"),
        cli.listen
    );
    println!("OpenAPI: http://{}/api-docs/openapi.json", cli.listen);
    axum::serve(listener, router).await.expect("serve df-api");
}
