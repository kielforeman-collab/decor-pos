//! `df-server` — unified CLI binary (`standalone`, `hq`, `edge`, `worker`, `migrate`, `reports`, `jobs`).
//!
//! Phase 2 (v0.3):
//!
//! * the `edge` mode is live — it opens a [`df_edge::Edge`] on a
//!   local SQLite database and serves its HTTP/WS API, including the
//!   Phase 2 tinting + report endpoints (`POST
//!   /pos/tinting/dispense`, `GET /pos/tinting/formulas`,
//!   `GET /pos/tinting/colorants`, `GET /pos/reports/daily-usage`,
//!   `GET /pos/reports/low-stock`), with the mock terminal as its
//!   payment provider and an in-process in-memory HQ sink, so `POST
//!   /pos/sync/run` works out of the box (the in-memory HQ is lost
//!   on restart; a PostgreSQL HQ sink replaces it in a later phase).
//! * `reports` prints a report as JSON for a store + date from any
//!   database URL the `df-db` repos can open (SQLite edge file or a
//!   PostgreSQL HQ URL — the live-Postgres path is compile-checked,
//!   not tested; see the README). It runs no migrations: the
//!   database must already exist.
//! * `jobs run` runs the Phase 2 daily jobs (`daily-usage`,
//!   `low-stock`, `shortage-alerts`, or all of them) once, right
//!   now, and prints the recorded runs as JSON. This is the
//!   retrievable form of the scheduled jobs — there is no daemon
//!   worker yet (`worker` mode is still a stub); schedule it with
//!   cron/systemd calling `jobs run` daily, or run it by hand.
//!
//! The other modes remain Phase 0 stubs (exit 2) until their phases
//! land.
//!
//! ```text
//! df-server edge --db sqlite:edge.db --store-id <uuid> --listen 127.0.0.1:8080
//! df-server reports --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --kind daily-usage
//! df-server jobs run --db sqlite:edge.db --org-id <uuid> --store-id <uuid>
//! ```

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use df_domain::Currency;
use df_edge::{Edge, EdgeConfig, MockTerminal};
use df_jobs::{JobContext, JobRunner, register_phase2_jobs};
use df_sync::MemoryHq;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "df-server",
    version,
    about = "decor-pos unified server binary (edge, reports and jobs run live in Phase 2, v0.3)"
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
    /// Print a report as JSON (no server, no migrations — the
    /// database must already exist).
    Reports {
        /// sqlx database URL, e.g. `sqlite:edge.db` or a
        /// PostgreSQL URL.
        #[arg(long)]
        db: String,
        /// Organization UUID the report scopes to.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store to report on.
        #[arg(long)]
        store_id: Uuid,
        /// Report kind: `daily-usage`, `low-stock`, `shortage`.
        #[arg(long)]
        kind: String,
        /// Date to report on (`YYYY-MM-DD`; defaults to today, UTC).
        #[arg(long)]
        date: Option<String>,
        /// Currency (`USD` or `CAD`). Defaults to the store row's
        /// currency; required when the database has no store row
        /// (e.g. a database created by `df-server edge`, which seeds
        /// no store row).
        #[arg(long)]
        currency: Option<String>,
    },
    /// Scheduled jobs.
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum JobsCommand {
    /// Run the Phase 2 daily jobs once, right now, and print the
    /// recorded runs as JSON. `--job` runs one job
    /// (`daily-usage`, `low-stock`, `shortage-alerts`); without it
    /// all three run. Exits 1 if any run failed.
    Run {
        /// sqlx database URL, e.g. `sqlite:edge.db` or a
        /// PostgreSQL URL.
        #[arg(long)]
        db: String,
        /// Organization UUID the jobs scope to.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the jobs run for.
        #[arg(long)]
        store_id: Uuid,
        /// One job to run (default: all three).
        #[arg(long)]
        job: Option<String>,
        /// Date the daily jobs report on (`YYYY-MM-DD`; defaults to
        /// today, UTC).
        #[arg(long)]
        date: Option<String>,
        /// Currency (`USD` or `CAD`). Defaults to the store row's
        /// currency; required when the database has no store row.
        #[arg(long)]
        currency: Option<String>,
    },
}

fn not_implemented(mode: &str) -> ! {
    eprintln!("df-server {mode}: not implemented yet — see spec §7");
    std::process::exit(2);
}

fn parse_date(text: Option<String>) -> Result<NaiveDate, String> {
    match text {
        None => Ok(chrono::Utc::now().date_naive()),
        Some(text) => {
            NaiveDate::from_str(&text).map_err(|_| format!("date must be YYYY-MM-DD, got {text:?}"))
        }
    }
}

async fn open_pool(db: &str) -> df_db::AnyPool {
    match df_db::connect(db).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("df-server: failed to connect to {db}: {err}");
            std::process::exit(1);
        }
    }
}

/// The store's currency: `--currency` wins when given, otherwise
/// the store row's currency. The edge daemon never inserts a store
/// row (`Edge::open` seeds nothing), so an edge database usually has
/// no store row — pass `--currency USD|CAD` for those, or seed the
/// store first.
async fn store_currency(pool: &df_db::AnyPool, store_id: Uuid, flag: Option<String>) -> Currency {
    if let Some(text) = flag {
        return match Currency::from_str(&text) {
            Ok(currency) => currency,
            Err(err) => {
                eprintln!("df-server: --currency {text:?} is not USD/CAD: {err}");
                std::process::exit(2);
            }
        };
    }
    match df_db::get_store(pool, store_id).await {
        Ok(store) => match Currency::from_str(&store.currency) {
            Ok(currency) => currency,
            Err(err) => {
                eprintln!("df-server: store {store_id} has an unsupported currency: {err}");
                std::process::exit(1);
            }
        },
        Err(err) => {
            eprintln!(
                "df-server: cannot read store {store_id} ({err}); pass --currency USD|CAD or seed the store row"
            );
            std::process::exit(1);
        }
    }
}

async fn run_reports(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    kind: String,
    date: Option<String>,
    currency_flag: Option<String>,
) -> ! {
    let date = match parse_date(date) {
        Ok(date) => date,
        Err(err) => {
            eprintln!("df-server reports: {err}");
            std::process::exit(2);
        }
    };
    let pool = open_pool(&db).await;
    let value = match kind.as_str() {
        "daily-usage" => {
            let currency = store_currency(&pool, store_id, currency_flag).await;
            match df_reports::daily_usage_report(&pool, org_id, store_id, date, currency).await {
                Ok(report) => serde_json::to_value(report).expect("report serialises"),
                Err(err) => {
                    eprintln!("df-server reports: {err}");
                    std::process::exit(1);
                }
            }
        }
        "low-stock" => {
            // No product thresholds on the CLI path: there is no
            // min-level table yet (see `df-reports` docs), so the
            // product half is empty and the colorant half carries
            // the report. Use the edge HTTP endpoint with
            // operator-configured thresholds for products.
            eprintln!(
                "df-server reports: low-stock without product thresholds — products will be empty (no min-level table yet); colorants are reported"
            );
            match df_reports::low_stock_report(&pool, store_id, date, &[]).await {
                Ok(report) => serde_json::to_value(report).expect("report serialises"),
                Err(err) => {
                    eprintln!("df-server reports: {err}");
                    std::process::exit(1);
                }
            }
        }
        "shortage" => match df_reports::colorant_shortage_alerts(&pool, store_id).await {
            Ok(alerts) => serde_json::json!({ "alerts": alerts }),
            Err(err) => {
                eprintln!("df-server reports: {err}");
                std::process::exit(1);
            }
        },
        other => {
            eprintln!(
                "df-server reports: unknown --kind {other:?} (daily-usage | low-stock | shortage)"
            );
            std::process::exit(2);
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("report serialises")
    );
    std::process::exit(0);
}

async fn run_jobs(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    job: Option<String>,
    date: Option<String>,
    currency_flag: Option<String>,
) -> ! {
    let report_date = match parse_date(date) {
        Ok(date) => date,
        Err(err) => {
            eprintln!("df-server jobs run: {err}");
            std::process::exit(2);
        }
    };
    let pool = open_pool(&db).await;
    let currency = store_currency(&pool, store_id, currency_flag).await;
    let mut ctx = JobContext::new(pool, org_id, store_id, currency);
    ctx.report_date = Some(report_date);
    let runner = JobRunner::new();
    register_phase2_jobs(&runner);
    let runs = match job.as_deref() {
        Some(name) => match runner.run_now(&ctx, name).await {
            Ok(run) => vec![run],
            Err(err) => {
                eprintln!("df-server jobs run: {err}");
                std::process::exit(2);
            }
        },
        None => runner.run_all(&ctx).await,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&runs).expect("runs serialise")
    );
    if runs.iter().any(|run| !run.success) {
        std::process::exit(1);
    }
    std::process::exit(0);
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
        Command::Reports {
            db,
            org_id,
            store_id,
            kind,
            date,
            currency,
        } => run_reports(db, org_id, store_id, kind, date, currency).await,
        Command::Jobs { command } => match command {
            JobsCommand::Run {
                db,
                org_id,
                store_id,
                job,
                date,
                currency,
            } => run_jobs(db, org_id, store_id, job, date, currency).await,
        },
    }
}
