//! `df-server` — unified CLI binary (`standalone`, `hq`, `edge`, `worker`, `migrate`).
//!
//! Phase 1 (v0.2): the `edge` mode is live — it opens a [`df_edge::Edge`]
//! on a local SQLite database and serves its HTTP/WS API, with the
//! mock terminal as its payment provider and an in-process
//! in-memory HQ sink, so `POST /pos/sync/run` works out of the box
//! (the in-memory HQ is lost on restart; a PostgreSQL HQ sink
//! replaces it in a later phase). The other modes remain Phase 0
//! stubs (exit 2) until their phases land.
//!
//! ```text
//! df-server edge --db sqlite:edge.db --store-id <uuid> --listen 127.0.0.1:8080
//! ```

use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use df_domain::Currency;
use df_edge::{Edge, EdgeConfig, MockTerminal};
use df_sync::MemoryHq;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "df-server",
    version,
    about = "decor-pos unified server binary (edge mode live in Phase 1, v0.2)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// All-in-one single-binary mode (df-api + df-edge + df-jobs).
    Standalone,
    /// Central HQ server mode.
    Hq,
    /// Store edge daemon mode: local SQLite + POS HTTP/WS API.
    Edge {
        /// sqlx database URL, e.g. `sqlite:edge.db` or `sqlite::memory:`.
        #[arg(long)]
        db: String,
        /// UUID of the store this edge serves.
        #[arg(long)]
        store_id: Uuid,
        /// Address to listen on, e.g. `127.0.0.1:8080`.
        #[arg(long)]
        listen: SocketAddr,
        /// Organization UUID (defaults to a fresh UUIDv7; pricing
        /// rules are looked up by org, so pass the real org id when
        /// the edge database has pricing data).
        #[arg(long)]
        org_id: Option<Uuid>,
        /// Store name shown on receipts.
        #[arg(long, default_value = "Decor POS Store")]
        store_name: String,
        /// Currency code (`USD` or `CAD`).
        #[arg(long, default_value = "USD")]
        currency: String,
    },
    /// Background worker mode.
    Worker,
    /// Run database migrations.
    Migrate,
}

fn not_implemented(mode: &str) -> ! {
    eprintln!("df-server {mode}: not implemented yet — see spec §7");
    std::process::exit(2);
}

async fn run_edge(
    db: String,
    store_id: Uuid,
    listen: SocketAddr,
    org_id: Option<Uuid>,
    store_name: String,
    currency: String,
) -> ! {
    let currency: Currency = match currency.parse() {
        Ok(currency) => currency,
        Err(err) => {
            eprintln!("df-server edge: invalid --currency: {err}");
            std::process::exit(2);
        }
    };
    let org_id = org_id.unwrap_or_else(Uuid::now_v7);
    let config = EdgeConfig {
        store_id,
        org_id,
        db_url: db,
        store_name,
        currency,
        surcharge_rules: EdgeConfig::default_surcharge_rules(currency),
    };
    let edge = match Edge::open(config).await {
        Ok(edge) => edge,
        Err(err) => {
            eprintln!("df-server edge: failed to open edge: {err}");
            std::process::exit(1);
        }
    };
    // Standalone/mock terminal: card tenders approve locally so the
    // POS API works out of the box with no real terminal attached.
    edge.set_payment_provider(Arc::new(MockTerminal::new()));
    // In-process in-memory HQ sink, so `/pos/sync/run` works out of
    // the box in the dev path (lost on restart; a real HQ sink lands
    // in a later phase).
    edge.set_hq(Arc::new(MemoryHq::new()));
    println!("df-server edge listening on http://{listen} (store {store_id}, org {org_id})");
    if let Err(err) = edge.serve(listen).await {
        eprintln!("df-server edge: serve failed: {err}");
        std::process::exit(1);
    }
    std::process::exit(0);
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Standalone => not_implemented("standalone"),
        Command::Hq => not_implemented("hq"),
        Command::Edge {
            db,
            store_id,
            listen,
            org_id,
            store_name,
            currency,
        } => run_edge(db, store_id, listen, org_id, store_name, currency).await,
        Command::Worker => not_implemented("worker"),
        Command::Migrate => not_implemented("migrate"),
    }
}
