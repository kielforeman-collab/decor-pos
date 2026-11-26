//! `df-server` — unified CLI binary (`standalone`, `hq`, `edge`, `worker`, `migrate`, `reports`, `jobs`, `statements`, `accounting`, `edi`).
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
//!   `low-stock`, `shortage-alerts`), the Phase 3 monthly jobs
//!   (`monthly-statements`, `finance-charges`) and the Phase 6
//!   daily jobs (`scheduled-report-email`, needs `--email-outbox`
//!   and at least one `--email-recipient`;
//!   `labor-forecast-refresh`, the 7-day sales-driven labor
//!   forecast), or all of them, once,
//!   right now, and prints the recorded runs as JSON. This is the
//!   retrievable form of the scheduled jobs — there is no daemon
//!   worker yet (`worker` mode is still a stub); schedule it with
//!   cron/systemd calling `jobs run` daily, or run it by hand.
//!
//! Phase 6 (v1.0):
//!
//! * `migrate import` migrates the database, imports legacy Decor
//!   Fusion CSVs from a directory (customers, products, formulas,
//!   AR balances, inventory) and prints the reconciliation
//!   manifest as JSON. Exit code 1 when any row or file failed.
//!
//! Phase 3 (v0.4):
//!
//! * `statements run` builds a month's statements from the AR ledger
//!   and renders each to a real Typst PDF in an output directory.
//! * `accounting export` writes a CSV or QuickBooks IIF export of
//!   the AR or inventory ledger to a file (file-based only — no live
//!   QuickBooks/Xero sync; that is Phase 4+).
//!
//! Phase 4 (v0.5):
//!
//! * `edi send` / `edi intake` / `edi match` / `edi catalog` run the
//!   edge's vendor-EDI wiring against an existing edge database
//!   (SQLite) and print the result as JSON. No migrations run on
//!   this path beyond what `Edge::open` already applies, and the
//!   database file must already exist — a missing file is exit 2,
//!   not a fresh database (SQLite would otherwise create one
//!   silently).
//!
//! The remaining modes (`standalone`, `worker`) are still Phase 0
//! stubs (exit 2) until their phases land.
//!
//! ```text
//! df-server edge --db sqlite:edge.db --store-id <uuid> --listen 127.0.0.1:8080
//! df-server reports --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --kind daily-usage
//! df-server jobs run --db sqlite:edge.db --org-id <uuid> --store-id <uuid>
//! df-server statements run --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --period 2026-11 --out ./statements-2026-11
//! df-server accounting export --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --kind csv --ledger ar --out ar.csv
//! df-server edi send --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --po-id <uuid> --out-dir ./edi-drop
//! df-server edi intake --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --doc-type 855 --file ./vendor.855.x12 --vendor-id VENDOR1
//! df-server edi match --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --po-id <uuid>
//! df-server edi catalog --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --file ./catalog.csv --vendor-id VENDOR1
//! df-server migrate import --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --dir ./legacy [--out manifest.json]
//! ```
//!
//! Phase 7 (v1.1) — auth core:
//!
//! ```text
//! df-server users create-admin --db sqlite:edge.db --org-id <uuid> --email admin@example.invalid --display-name "Admin"
//! df-server users create --db sqlite:edge.db --org-id <uuid> --email cashier@example.invalid --display-name "Cashier" --role Cashier --pin 4821
//! df-server users list --db sqlite:edge.db --org-id <uuid>
//! df-server users revoke-sessions --db sqlite:edge.db --email cashier@example.invalid
//! ```
//!
//! The password comes from the `DF_USERS_PASSWORD` env var (or the
//! var named by `--password-env`) when set, otherwise it is read
//! from stdin at a prompt. It is never logged and never printed.
//! `create-admin` refuses when an Admin already exists for the org
//! unless `--force` is passed AND the operator types `YES` at the
//! confirmation prompt. See `docs/auth.md` for the full bootstrap
//! procedure.
//!
//! Phase 8 (v1.2) — PostgreSQL HQ mode:
//!
//! * `hq` is live: it connects to the database named by `--db`
//!   (or the `DATABASE_URL` env var when `--db` is absent — the
//!   flag wins), applies the pending migrations for the URL's
//!   dialect, then serves the `df-api` router on `--listen`
//!   (default `127.0.0.1:3000`) with the pool in `ApiState`. The
//!   Phase 7 auth gate applies exactly as on the `df-api` binary:
//!   every route except `/health`, `/api-docs/openapi.json` and
//!   the `/api/v1/auth/*` login endpoints requires a `Bearer
//!   dfp_...` session token, with per-route RBAC. HQ works against
//!   PostgreSQL or SQLite — the dialect follows the URL.
//! * `migrate schema` applies pending migrations to the database
//!   named by `--db` / `DATABASE_URL` (PostgreSQL or SQLite) and
//!   prints which migrations it applied as JSON; a second run is
//!   a no-op (`"status": "already_up_to_date"`). The applied list
//!   is read from sqlx's own `_sqlx_migrations` tracking table
//!   before and after the run — no parallel tracking exists.
//! * The database URL is never logged: startup logs show the
//!   redacted URL (`postgres://***@host:5432/db`).
//!
//! ```text
//! df-server migrate schema --db postgres://decor:<password>@127.0.0.1:5432/decorpos
//! df-server hq --db postgres://decor:<password>@127.0.0.1:5432/decorpos --org-id <uuid> [--store-id <uuid>] [--listen 127.0.0.1:3000]
//! df-server users create-admin --db postgres://decor:<password>@127.0.0.1:5432/decorpos --org-id <uuid> --email admin@example.invalid --display-name "Admin"
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use clap::{Parser, Subcommand};
use df_auth::{Role, hash_password, hash_pin};
use df_domain::Currency;
use df_edge::{Edge, EdgeConfig, MockTerminal};
use df_jobs::{
    JobContext, JobRunner, register_phase2_jobs, register_phase3_jobs, register_phase6_jobs,
};
use df_sync::MemoryHq;
use rust_decimal::Decimal;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "df-server",
    version,
    about = "decor-pos unified server binary (hq live in Phase 8, v1.2; edge, reports and jobs live since v0.3)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// All-in-one single-binary mode (df-api + df-edge + df-jobs).
    Standalone,
    /// Central HQ server mode (Phase 8, v1.2): connects to the
    /// database, applies pending migrations, then serves the
    /// `df-api` router (with the Phase 7 auth gate) on `--listen`.
    Hq {
        /// sqlx database URL, e.g.
        /// `postgres://decor:<password>@127.0.0.1:5432/decorpos`
        /// or `sqlite:edge.db`. When absent, the `DATABASE_URL`
        /// env var is used; `--db` wins when both are set.
        #[arg(long)]
        db: Option<String>,
        /// Address to listen on, e.g. `127.0.0.1:3000`.
        #[arg(long, default_value = "127.0.0.1:3000")]
        listen: SocketAddr,
        /// Organization UUID the HQ API scopes to (required:
        /// the admin endpoints 503 without one).
        #[arg(long)]
        org_id: Uuid,
        /// Default store UUID (query `?store_id=` overrides it
        /// per call).
        #[arg(long)]
        store_id: Option<Uuid>,
    },
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
        /// Directory scheduled report emails queue into (the
        /// `/bi/email-outbox` endpoint reads the same directory's
        /// `manifest.json`). No SMTP send happens.
        #[arg(long)]
        email_outbox: Option<PathBuf>,
    },
    /// Background worker mode.
    Worker,
    /// Database migrations and legacy data import (Phase 6, v1.0).
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
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
        /// Report kind: `daily-usage`, `low-stock`, `shortage`,
        /// `kpi` (single-day KPI dashboard for `--date`) or `eco`
        /// (eco-fee remittance for the month to `--date`).
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
    /// Monthly AR statements (Phase 3, v0.4).
    Statements {
        #[command(subcommand)]
        command: StatementsCommand,
    },
    /// File-based accounting exports (Phase 3, v0.4).
    Accounting {
        #[command(subcommand)]
        command: AccountingCommand,
    },
    /// Vendor EDI (Phase 4, v0.5): send a PO, intake a vendor
    /// document, run the 3-way match, import a vendor catalog.
    Edi {
        #[command(subcommand)]
        command: EdiCommand,
    },
    /// User administration (Phase 7, v1.1): create the first admin,
    /// create users, list users, revoke sessions. Runs migrations
    /// first, so it works on a fresh database.
    Users {
        #[command(subcommand)]
        command: UsersCommand,
    },
}

#[derive(Debug, Subcommand)]
enum MigrateCommand {
    /// Apply pending schema migrations to the database named by
    /// `--db` (or `DATABASE_URL` when `--db` is absent) and print
    /// the result as JSON: the dialect, the versions newly applied
    /// this run, and all applied versions. A second run is a
    /// no-op (`"status": "already_up_to_date"`, exit 0).
    Schema {
        /// sqlx database URL, e.g. a PostgreSQL URL or
        /// `sqlite:edge.db`. When absent, the `DATABASE_URL` env
        /// var is used; `--db` wins when both are set.
        #[arg(long)]
        db: Option<String>,
    },
    /// Import legacy Decor Fusion CSVs from a directory
    /// (customers, products, formulas, AR balances, inventory)
    /// and print the reconciliation manifest as JSON. The
    /// database is migrated first (SQLite or PostgreSQL by URL).
    /// Exit code 1 when any row or file failed — the manifest
    /// still lists what happened to each row.
    Import {
        /// sqlx database URL, e.g. `sqlite:edge.db` or a
        /// PostgreSQL URL.
        #[arg(long)]
        db: String,
        /// Organization UUID the import scopes to.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the import scopes to.
        #[arg(long)]
        store_id: Uuid,
        /// Directory holding the legacy CSV files.
        #[arg(long)]
        dir: PathBuf,
        /// Write the manifest JSON to this file instead of
        /// printing it.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum JobsCommand {
    /// Run the Phase 2 daily jobs, the Phase 3 monthly jobs and the
    /// Phase 6 scheduled-report-email job once, right now, and print
    /// the recorded runs as JSON. `--job` runs one job
    /// (`daily-usage`, `low-stock`, `shortage-alerts`,
    /// `monthly-statements`, `finance-charges`,
    /// `scheduled-report-email`); without it all run.
    /// Exits 1 if any run failed.
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
        /// One job to run (default: all).
        #[arg(long)]
        job: Option<String>,
        /// Date the jobs report on (`YYYY-MM-DD`; defaults to
        /// today, UTC). Monthly jobs report on the previous month.
        #[arg(long)]
        date: Option<String>,
        /// Currency (`USD` or `CAD`). Defaults to the store row's
        /// currency; required when the database has no store row.
        #[arg(long)]
        currency: Option<String>,
        /// Path to the `typst` binary for the monthly-statements job
        /// (default: `DF_TYPST_BIN` env, then `typst` on `PATH`).
        #[arg(long)]
        typst_bin: Option<PathBuf>,
        /// Monthly finance-charge rate in percent for the
        /// finance-charges job (default 1.5).
        #[arg(long)]
        finance_rate: Option<String>,
        /// Directory the `scheduled-report-email` job writes
        /// rendered reports and `manifest.json` into. No SMTP send
        /// happens; the outbox is the delivery record.
        #[arg(long)]
        email_outbox: Option<PathBuf>,
        /// Email address the `scheduled-report-email` job queues
        /// reports for (repeatable).
        #[arg(long)]
        email_recipient: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum StatementsCommand {
    /// Build a month's statements from the AR ledger and render each
    /// to a real Typst PDF in the output directory. Prints a JSON
    /// summary per statement (account, closing balance, PDF path,
    /// SHA-256). Exits 1 if any statement fails to render.
    Run {
        /// sqlx database URL, e.g. `sqlite:edge.db` or a
        /// PostgreSQL URL.
        #[arg(long)]
        db: String,
        /// Organization UUID the statements scope to.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the statements run for.
        #[arg(long)]
        store_id: Uuid,
        /// Period to statement (`YYYY-MM`; defaults to last month).
        #[arg(long)]
        period: Option<String>,
        /// Output directory for the PDFs (default
        /// `./statements-<period>`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Path to the `typst` binary (default: `DF_TYPST_BIN` env,
        /// then `typst` on `PATH`).
        #[arg(long)]
        typst_bin: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum AccountingCommand {
    /// Write a file-based accounting export of a ledger to `--out`.
    /// `--kind csv` writes CSV; `--kind iif` writes QuickBooks IIF.
    /// File-based only — no live QuickBooks/Xero sync (Phase 4+).
    Export {
        /// sqlx database URL, e.g. `sqlite:edge.db` or a
        /// PostgreSQL URL.
        #[arg(long)]
        db: String,
        /// Organization UUID the export scopes to.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the export runs for.
        #[arg(long)]
        store_id: Uuid,
        /// Export kind: `csv` or `iif`.
        #[arg(long)]
        kind: String,
        /// Ledger to export: `ar` or `inventory`.
        #[arg(long)]
        ledger: String,
        /// Output file path.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum EdiCommand {
    /// Send a draft PO as X12 850 and print the send receipt as
    /// JSON. `--out-dir` stages the 850 through the local
    /// transport stand-in (`{out-dir}/outbox/`); without it the
    /// 850 is stored + printed but no file is staged.
    Send {
        /// sqlx database URL (must already exist — see the module
        /// docs; a missing SQLite file is exit 2).
        #[arg(long)]
        db: String,
        /// Organization UUID the edge serves.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the edge serves.
        #[arg(long)]
        store_id: Uuid,
        /// UUID of the draft purchase order to send.
        #[arg(long)]
        po_id: Uuid,
        /// Root folder for the transport stand-in.
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Currency (`USD` or `CAD`). Defaults to the store row's
        /// currency, then USD.
        #[arg(long)]
        currency: Option<String>,
    },
    /// Intake one vendor document from a file and print the intake
    /// result as JSON (`--doc-type 856|810|855|832|csv`). An 855
    /// stores the acknowledgment and reports the ack application;
    /// an 832 stores a price-apply report in `pending_review`.
    Intake {
        /// sqlx database URL (must already exist).
        #[arg(long)]
        db: String,
        /// Organization UUID the edge serves.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the edge serves.
        #[arg(long)]
        store_id: Uuid,
        /// Document type: `856`, `810`, `855`, `832` or `csv`.
        #[arg(long)]
        doc_type: String,
        /// File holding the raw document text.
        #[arg(long)]
        file: PathBuf,
        /// Vendor id the document came from.
        #[arg(long)]
        vendor_id: String,
        /// PO to link the document to (otherwise linked by the PO
        /// number the vendor echoes).
        #[arg(long)]
        po_id: Option<Uuid>,
        /// Currency (`USD` or `CAD`). Defaults to the store row's
        /// currency, then USD.
        #[arg(long)]
        currency: Option<String>,
    },
    /// Run the 3-way match for a PO and print the report as JSON.
    /// When both the ASN and the invoice are stored the report is
    /// persisted for later review.
    Match {
        /// sqlx database URL (must already exist).
        #[arg(long)]
        db: String,
        /// Organization UUID the edge serves.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the edge serves.
        #[arg(long)]
        store_id: Uuid,
        /// UUID of the purchase order to match.
        #[arg(long)]
        po_id: Uuid,
        /// Currency (`USD` or `CAD`). Defaults to the store row's
        /// currency, then USD.
        #[arg(long)]
        currency: Option<String>,
    },
    /// Import a vendor catalog CSV file and print the import
    /// result (valid items + per-row errors) as JSON.
    Catalog {
        /// sqlx database URL (must already exist).
        #[arg(long)]
        db: String,
        /// Organization UUID the edge serves.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the edge serves.
        #[arg(long)]
        store_id: Uuid,
        /// CSV file to import.
        #[arg(long)]
        file: PathBuf,
        /// Vendor id the catalog came from.
        #[arg(long)]
        vendor_id: String,
        /// Currency (`USD` or `CAD`). Defaults to the store row's
        /// currency, then USD.
        #[arg(long)]
        currency: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum UsersCommand {
    /// Create the first Admin for an org. Refuses when an Admin
    /// already exists unless `--force` is passed AND the operator
    /// confirms by typing `YES` at the prompt. The password comes
    /// from the env var named by `--password-env` (default
    /// `DF_USERS_PASSWORD`) when set, otherwise from stdin at a
    /// prompt; it is never logged. No default password exists.
    CreateAdmin {
        /// sqlx database URL, e.g. `sqlite:edge.db`.
        #[arg(long)]
        db: String,
        /// Organization UUID the admin belongs to.
        #[arg(long)]
        org_id: Uuid,
        /// Name for the org row when it does not exist yet
        /// (default `Decor POS`).
        #[arg(long)]
        org_name: Option<String>,
        /// Admin email (login name).
        #[arg(long)]
        email: String,
        /// Admin display name.
        #[arg(long)]
        display_name: String,
        /// Allow creating a second admin for the org (still needs
        /// the `YES` confirmation at the prompt).
        #[arg(long)]
        force: bool,
        /// Env var to read the password from (default
        /// `DF_USERS_PASSWORD`).
        #[arg(long, default_value = "DF_USERS_PASSWORD")]
        password_env: String,
    },
    /// Create a user with any role. At least one of a password or
    /// `--pin` is required; `--no-password` creates a PIN-only user
    /// (cashier counter login). `--customer-id` is required for
    /// `--role PortalContractor` and rejected for other roles.
    Create {
        /// sqlx database URL, e.g. `sqlite:edge.db`.
        #[arg(long)]
        db: String,
        /// Organization UUID the user belongs to.
        #[arg(long)]
        org_id: Uuid,
        /// Name for the org row when it does not exist yet
        /// (default `Decor POS`).
        #[arg(long)]
        org_name: Option<String>,
        /// User email (login name).
        #[arg(long)]
        email: String,
        /// User display name.
        #[arg(long)]
        display_name: String,
        /// Role: Cashier | SalesAssociate | Warehouse | Driver |
        /// Manager | Admin | PortalContractor.
        #[arg(long)]
        role: String,
        /// Counter PIN (4-6 digits) for counter switching and
        /// manager overrides.
        #[arg(long)]
        pin: Option<String>,
        /// Contractor customer id this portal user may access
        /// (required for PortalContractor).
        #[arg(long)]
        customer_id: Option<Uuid>,
        /// Do not set a password (PIN-only user). Without this the
        /// password is read from `--password-env` or stdin.
        #[arg(long)]
        no_password: bool,
        /// Env var to read the password from (default
        /// `DF_USERS_PASSWORD`).
        #[arg(long, default_value = "DF_USERS_PASSWORD")]
        password_env: String,
    },
    /// List the users of an org as JSON (id, email, display name,
    /// role, portal customer, active). Hashes are never printed.
    List {
        /// sqlx database URL, e.g. `sqlite:edge.db`.
        #[arg(long)]
        db: String,
        /// Organization UUID to list.
        #[arg(long)]
        org_id: Uuid,
    },
    /// Revoke every live session of a user (logout everywhere).
    RevokeSessions {
        /// sqlx database URL, e.g. `sqlite:edge.db`.
        #[arg(long)]
        db: String,
        /// Email of the user to log out everywhere.
        #[arg(long)]
        email: String,
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
            eprintln!(
                "df-server: failed to connect to {}: {err}",
                df_db::redact_db_url(db)
            );
            std::process::exit(1);
        }
    }
}

/// Resolve a database URL: `--db` wins when given (and non-empty),
/// otherwise the `DATABASE_URL` env var, otherwise `None` — the
/// caller exits 2 with a usage error. The URL value is never logged
/// by this function.
fn resolve_db_url(flag: Option<String>) -> Option<String> {
    if let Some(db) = flag.filter(|db| !db.is_empty()) {
        return Some(db);
    }
    std::env::var("DATABASE_URL")
        .ok()
        .filter(|db| !db.is_empty())
}

/// Require a database URL for a subcommand: `--db` wins, else
/// `DATABASE_URL`; exit 2 when neither is set.
fn require_db_url(flag: Option<String>, what: &str) -> String {
    match resolve_db_url(flag) {
        Some(db) => db,
        None => {
            eprintln!("df-server {what}: no database — pass --db <url> or set DATABASE_URL");
            std::process::exit(2);
        }
    }
}

/// The migration set for a database URL: PostgreSQL URLs take the
/// `postgres` migrations, anything else the SQLite set.
fn dialect_for_url(db: &str) -> df_db::Dialect {
    if db.starts_with("postgres") {
        df_db::Dialect::Postgres
    } else {
        df_db::Dialect::Sqlite
    }
}

/// Versions sqlx has recorded in its own `_sqlx_migrations`
/// tracking table, in ascending order. Empty when the table does
/// not exist yet (a fresh database) — this reads sqlx's tracking,
/// it does not re-implement it. sqlx 0.8.6's public `Migrator`
/// exposes no applied-migration list, so the table read is the
/// only way to report what a run applied.
async fn applied_migration_versions(pool: &df_db::AnyPool) -> Vec<i64> {
    sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

/// Outcome of a migration run: which versions this run applied
/// (`newly_applied`) and every version applied in total
/// (`applied`), both ascending.
struct MigrationReport {
    newly_applied: Vec<i64>,
    applied: Vec<i64>,
}

/// Open a pool, apply the pending migrations for the URL's
/// dialect, and report what changed. The pool is returned so the
/// caller (e.g. `hq`) can keep serving on it.
async fn migrate_and_report(db: &str, what: &str) -> (df_db::AnyPool, MigrationReport) {
    let pool = open_pool(db).await;
    let before = applied_migration_versions(&pool).await;
    if let Err(err) = df_db::migrate(&pool, dialect_for_url(db)).await {
        eprintln!("df-server {what}: migration failed: {err}");
        std::process::exit(1);
    }
    let applied = applied_migration_versions(&pool).await;
    let newly_applied = applied
        .iter()
        .filter(|version| !before.contains(version))
        .copied()
        .collect();
    (
        pool,
        MigrationReport {
            newly_applied,
            applied,
        },
    )
}

/// `df-server migrate schema`: apply pending migrations and print
/// the result as JSON. Exit 0 whether or not anything was applied.
async fn run_migrate_schema(db_flag: Option<String>) -> ! {
    let db = require_db_url(db_flag, "migrate schema");
    let dialect = dialect_for_url(&db);
    let (_pool, report) = migrate_and_report(&db, "migrate schema").await;
    let status = if report.newly_applied.is_empty() {
        "already_up_to_date"
    } else {
        "migrated"
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "database": df_db::redact_db_url(&db),
            "dialect": match dialect {
                df_db::Dialect::Postgres => "postgres",
                df_db::Dialect::Sqlite => "sqlite",
            },
            "status": status,
            "newly_applied": report.newly_applied,
            "applied": report.applied,
        }))
        .expect("report serialises")
    );
    std::process::exit(0);
}

/// `df-server hq`: connect, migrate, then serve the `df-api`
/// router (Phase 7 auth gate included — `create_router` applies
/// it, so HQ routes are authenticated exactly like `df-api`'s).
async fn run_hq(
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
        "kpi" => {
            let currency = store_currency(&pool, store_id, currency_flag).await;
            match df_reports::kpi_dashboard(&pool, org_id, store_id, date, date, date, currency)
                .await
            {
                Ok(report) => serde_json::to_value(report).expect("report serialises"),
                Err(err) => {
                    eprintln!("df-server reports: {err}");
                    std::process::exit(1);
                }
            }
        }
        "eco" => {
            let currency = store_currency(&pool, store_id, currency_flag).await;
            let month_start =
                chrono::NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap();
            match df_reports::eco_remittance_report(&pool, org_id, month_start, date, currency)
                .await
            {
                Ok(report) => serde_json::to_value(report).expect("report serialises"),
                Err(err) => {
                    eprintln!("df-server reports: {err}");
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!(
                "df-server reports: unknown --kind {other:?} (daily-usage | low-stock | shortage | kpi | eco)"
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

/// Arguments for `jobs run` (a struct so `run_jobs` stays under
/// clippy's argument limit).
struct JobsRunArgs {
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    job: Option<String>,
    date: Option<String>,
    currency_flag: Option<String>,
    typst_bin: Option<PathBuf>,
    finance_rate: Option<String>,
    email_outbox: Option<PathBuf>,
    email_recipients: Vec<String>,
}

async fn run_jobs(args: JobsRunArgs) -> ! {
    let JobsRunArgs {
        db,
        org_id,
        store_id,
        job,
        date,
        currency_flag,
        typst_bin,
        finance_rate,
        email_outbox,
        email_recipients,
    } = args;
    let report_date = match parse_date(date) {
        Ok(date) => date,
        Err(err) => {
            eprintln!("df-server jobs run: {err}");
            std::process::exit(2);
        }
    };
    let finance_rate = match finance_rate {
        Some(text) => match Decimal::from_str(&text) {
            Ok(rate) => rate,
            Err(_) => {
                eprintln!(
                    "df-server jobs run: --finance-rate must be a decimal percent, got {text:?}"
                );
                std::process::exit(2);
            }
        },
        None => Decimal::from_str("1.5").unwrap(),
    };
    let pool = open_pool(&db).await;
    let currency = store_currency(&pool, store_id, currency_flag).await;
    let mut ctx = JobContext::new(pool, org_id, store_id, currency);
    ctx.report_date = Some(report_date);
    ctx.typst_bin = typst_bin;
    ctx.finance_charge_rate_pct = finance_rate;
    ctx.email_outbox_dir = email_outbox;
    ctx.email_recipients = email_recipients;
    let runner = JobRunner::new();
    register_phase2_jobs(&runner);
    register_phase3_jobs(&runner);
    register_phase6_jobs(&runner);
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

fn default_period() -> String {
    let today = chrono::Utc::now().date_naive();
    let (year, month) = if today.month() == 1 {
        (today.year() - 1, 12)
    } else {
        (today.year(), today.month() - 1)
    };
    format!("{year:04}-{month:02}")
}

async fn run_statements(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    period: Option<String>,
    out: Option<PathBuf>,
    typst_bin: Option<PathBuf>,
) -> ! {
    let period = period.unwrap_or_else(default_period);
    if NaiveDate::parse_from_str(&format!("{period}-01"), "%Y-%m-%d").is_err() {
        eprintln!("df-server statements run: --period must be YYYY-MM, got {period:?}");
        std::process::exit(2);
    }
    let pool = open_pool(&db).await;
    let currency = store_currency(&pool, store_id, None).await;
    let (org_name, store_name) = match (
        df_db::get_organization(&pool, org_id).await,
        df_db::get_store(&pool, store_id).await,
    ) {
        (Ok(org), Ok(store)) => (org.name, store.name),
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("df-server statements run: cannot read org/store: {err}");
            std::process::exit(1);
        }
    };
    let statements =
        match df_reports::build_statements(&pool, org_id, store_id, &period, currency).await {
            Ok(statements) => statements,
            Err(err) => {
                eprintln!("df-server statements run: {err}");
                std::process::exit(1);
            }
        };
    let out_dir = out.unwrap_or_else(|| PathBuf::from(format!("statements-{period}")));
    if let Err(err) = std::fs::create_dir_all(&out_dir) {
        eprintln!(
            "df-server statements run: cannot create {}: {err}",
            out_dir.display()
        );
        std::process::exit(1);
    }
    let mut summaries = Vec::new();
    let mut failed = false;
    for statement in &statements {
        // Freeze the statement row first (idempotent on re-run),
        // then render the PDF and record its hash on the row.
        let row_id = match df_reports::persist_statement(&pool, statement).await {
            Ok(id) => id,
            Err(err) => {
                eprintln!(
                    "df-server statements run: cannot persist statement {}: {err}",
                    statement.account_number
                );
                failed = true;
                continue;
            }
        };
        let customer = match df_db::get_customer(&pool, statement.customer_id).await {
            Ok(customer) => customer,
            Err(err) => {
                eprintln!(
                    "df-server statements run: cannot read customer {}: {err}",
                    statement.customer_id
                );
                failed = true;
                continue;
            }
        };
        let header =
            df_reports::header_for(&customer, &org_name, &store_name, statement.period_end);
        let file_name = format!("{}-{}.pdf", statement.account_number, period);
        let path = out_dir.join(&file_name);
        match df_reports::render_statement_pdf(statement, &header, typst_bin.as_deref()) {
            Ok(pdf) => {
                let sha256 = df_reports::statement_pdf_sha256(&pdf);
                if let Err(err) = std::fs::write(&path, &pdf) {
                    eprintln!(
                        "df-server statements run: cannot write {}: {err}",
                        path.display()
                    );
                    failed = true;
                    continue;
                }
                if let Err(err) = df_db::set_statement_pdf_sha256(&pool, row_id, &sha256).await {
                    eprintln!(
                        "df-server statements run: cannot record PDF hash for {}: {err}",
                        statement.account_number
                    );
                    failed = true;
                    continue;
                }
                summaries.push(serde_json::json!({
                    "account_number": statement.account_number,
                    "customer_name": header.customer_name,
                    "closing_balance": statement.closing_balance.amount().to_string(),
                    "statement_row_id": row_id,
                    "pdf_path": path.to_string_lossy(),
                    "pdf_bytes": pdf.len(),
                    "pdf_sha256": sha256,
                }));
            }
            Err(err) => {
                eprintln!(
                    "df-server statements run: statement {} failed to render: {err}",
                    statement.account_number
                );
                failed = true;
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "period": period,
            "statements": summaries,
        }))
        .expect("summaries serialise")
    );
    std::process::exit(i32::from(failed));
}

async fn run_accounting_export(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    kind: String,
    ledger: String,
    out: PathBuf,
) -> ! {
    let _ = org_id;
    let pool = open_pool(&db).await;
    let summary = match (kind.as_str(), ledger.as_str()) {
        ("csv", "ar") => match df_accounting::load_ar_entries_for_store(&pool, store_id).await {
            Ok(entries) => {
                let plugin = df_accounting::CsvAccountingPlugin;
                match df_accounting::AccountingPlugin::export_ar_ledger(&plugin, &entries, &out) {
                    Ok(summary) => summary,
                    Err(err) => {
                        eprintln!("df-server accounting export: {err}");
                        std::process::exit(1);
                    }
                }
            }
            Err(err) => {
                eprintln!("df-server accounting export: {err}");
                std::process::exit(1);
            }
        },
        ("csv", "inventory") => {
            match df_accounting::load_inventory_entries_for_store(&pool, store_id).await {
                Ok(entries) => {
                    let plugin = df_accounting::CsvAccountingPlugin;
                    match df_accounting::AccountingPlugin::export_inventory_ledger(
                        &plugin, &entries, &out,
                    ) {
                        Ok(summary) => summary,
                        Err(err) => {
                            eprintln!("df-server accounting export: {err}");
                            std::process::exit(1);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("df-server accounting export: {err}");
                    std::process::exit(1);
                }
            }
        }
        ("iif", "ar") => match df_accounting::load_ar_entries_for_store(&pool, store_id).await {
            Ok(entries) => {
                let plugin = df_accounting::IifAccountingPlugin::default();
                match df_accounting::AccountingPlugin::export_ar_ledger(&plugin, &entries, &out) {
                    Ok(summary) => summary,
                    Err(err) => {
                        eprintln!("df-server accounting export: {err}");
                        std::process::exit(1);
                    }
                }
            }
            Err(err) => {
                eprintln!("df-server accounting export: {err}");
                std::process::exit(1);
            }
        },
        ("iif", "inventory") => {
            match df_accounting::load_inventory_entries_for_store(&pool, store_id).await {
                Ok(entries) => {
                    let plugin = df_accounting::IifAccountingPlugin::default();
                    match df_accounting::AccountingPlugin::export_inventory_ledger(
                        &plugin, &entries, &out,
                    ) {
                        Ok(summary) => summary,
                        Err(err) => {
                            eprintln!("df-server accounting export: {err}");
                            std::process::exit(1);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("df-server accounting export: {err}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!(
                "df-server accounting export: unknown --kind {kind:?} / --ledger {ledger:?} (csv|iif, ar|inventory)"
            );
            std::process::exit(2);
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "kind": kind,
            "ledger": ledger,
            "rows": summary.rows,
            "bytes": summary.bytes,
            "path": summary.path.to_string_lossy(),
        }))
        .expect("summary serialises")
    );
    std::process::exit(0);
}

/// The database + tenant every `edi` subcommand needs. Bundled so
/// the `run_edi_*` helpers stay under clippy's argument limit.
struct EdiCtx {
    db: String,
    org_id: Uuid,
    store_id: Uuid,
}

/// Open the edge for an `edi` subcommand. The database file must
/// already exist: SQLite would otherwise create an empty one
/// silently, so a missing file is exit 2. The currency is only
/// needed for the `EdgeConfig` (no EDI result carries money in a
/// currency), so it resolves flag → store row → USD.
async fn open_edge_for_edi(ctx: &EdiCtx, currency_flag: Option<String>) -> Edge {
    if let Some(path) = ctx.db.strip_prefix("sqlite:")
        && path != ":memory:"
        && !PathBuf::from(path).exists()
    {
        eprintln!(
            "df-server edi: database file {path} does not exist — run `df-server edge` first (or pass an existing --db)"
        );
        std::process::exit(2);
    }
    let pool = open_pool(&ctx.db).await;
    let currency = match currency_flag {
        Some(text) => match Currency::from_str(&text) {
            Ok(currency) => currency,
            Err(err) => {
                eprintln!("df-server edi: --currency {text:?} is not USD/CAD: {err}");
                std::process::exit(2);
            }
        },
        None => match df_db::get_store(&pool, ctx.store_id).await {
            Ok(store) => match Currency::from_str(&store.currency) {
                Ok(currency) => currency,
                Err(err) => {
                    eprintln!(
                        "df-server edi: store {} has an unsupported currency: {err}",
                        ctx.store_id
                    );
                    std::process::exit(1);
                }
            },
            Err(_) => Currency::Usd,
        },
    };
    let config = EdgeConfig {
        store_id: ctx.store_id,
        org_id: ctx.org_id,
        db_url: ctx.db.clone(),
        store_name: "Decor POS Store".to_string(),
        currency,
        surcharge_rules: EdgeConfig::default_surcharge_rules(currency),
    };
    match Edge::open(config).await {
        Ok(edge) => edge,
        Err(err) => {
            eprintln!("df-server edi: failed to open edge: {err}");
            std::process::exit(1);
        }
    }
}

fn print_json(value: serde_json::Value) -> ! {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("value serialises")
    );
    std::process::exit(0);
}

/// `df-server migrate import`: migrate the database, import the
/// legacy CSVs in `dir`, and print (or write) the reconciliation
/// manifest. Exits 1 when any row or file failed; the manifest
/// still records every row's outcome.
async fn run_migrate_import(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    dir: PathBuf,
    out: Option<PathBuf>,
) -> ! {
    let pool = open_pool(&db).await;
    let dialect = if db.starts_with("postgres") {
        df_db::Dialect::Postgres
    } else {
        df_db::Dialect::Sqlite
    };
    if let Err(err) = df_db::migrate(&pool, dialect).await {
        eprintln!("df-server migrate import: migration failed: {err}");
        std::process::exit(1);
    }
    let manifest = match df_migrator::import_directory(&pool, org_id, store_id, &dir).await {
        Ok(manifest) => manifest,
        Err(err) => {
            eprintln!("df-server migrate import: {err}");
            std::process::exit(1);
        }
    };
    let json = serde_json::to_string_pretty(&manifest).expect("manifest serialises");
    match out {
        Some(path) => {
            if let Err(err) = std::fs::write(&path, &json) {
                eprintln!(
                    "df-server migrate import: cannot write {}: {err}",
                    path.display()
                );
                std::process::exit(1);
            }
            println!(
                "manifest written to {}: {} read / {} imported / {} skipped / {} failed",
                path.display(),
                manifest.total_rows_read,
                manifest.total_imported,
                manifest.total_skipped,
                manifest.total_failed
            );
        }
        None => println!("{json}"),
    }
    if !manifest.is_clean() {
        eprintln!(
            "df-server migrate import: finished with {} skipped / {} failed row(s) — see the manifest for reasons",
            manifest.total_skipped, manifest.total_failed
        );
        std::process::exit(1);
    }
    std::process::exit(0);
}

async fn run_edi_send(
    ctx: EdiCtx,
    po_id: Uuid,
    out_dir: Option<PathBuf>,
    currency: Option<String>,
) -> ! {
    let edge = open_edge_for_edi(&ctx, currency).await;
    let out_dir = out_dir.map(|path| path.to_string_lossy().to_string());
    match edge.send_purchase_order(po_id, out_dir).await {
        Ok(response) => print_json(serde_json::to_value(response).expect("response serialises")),
        Err(err) => {
            eprintln!("df-server edi send: {err}");
            std::process::exit(1);
        }
    }
}

async fn run_edi_intake(
    ctx: EdiCtx,
    doc_type: String,
    file: PathBuf,
    vendor_id: String,
    po_id: Option<Uuid>,
    currency: Option<String>,
) -> ! {
    let raw_text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) => {
            eprintln!(
                "df-server edi intake: cannot read {}: {err}",
                file.display()
            );
            std::process::exit(2);
        }
    };
    let edge = open_edge_for_edi(&ctx, currency).await;
    let request = df_edge::IntakeRequest {
        doc_type,
        raw_text,
        vendor_id,
        related_po_id: po_id,
    };
    match edge.intake_edi(request).await {
        Ok(response) => print_json(serde_json::to_value(response).expect("response serialises")),
        Err(err) => {
            eprintln!("df-server edi intake: {err}");
            std::process::exit(1);
        }
    }
}

async fn run_edi_match(ctx: EdiCtx, po_id: Uuid, currency: Option<String>) -> ! {
    let edge = open_edge_for_edi(&ctx, currency).await;
    match edge.three_way_match(po_id).await {
        Ok(report) => print_json(serde_json::to_value(report).expect("report serialises")),
        Err(err) => {
            eprintln!("df-server edi match: {err}");
            std::process::exit(1);
        }
    }
}

async fn run_edi_catalog(
    ctx: EdiCtx,
    file: PathBuf,
    vendor_id: String,
    currency: Option<String>,
) -> ! {
    let csv_text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) => {
            eprintln!(
                "df-server edi catalog: cannot read {}: {err}",
                file.display()
            );
            std::process::exit(2);
        }
    };
    let edge = open_edge_for_edi(&ctx, currency).await;
    match edge.import_vendor_catalog(&csv_text, &vendor_id).await {
        Ok(response) => print_json(serde_json::to_value(response).expect("response serialises")),
        Err(err) => {
            eprintln!("df-server edi catalog: {err}");
            std::process::exit(1);
        }
    }
}

/// Open a pool and run the Phase 7 migrations on it, so the `users`
/// subcommands work on a fresh database. The database file must
/// already exist for SQLite (a missing file is created by `connect`
/// with `mode=rwc` — the same behaviour as `df-server edge`).
async fn open_migrated_pool(db: &str) -> df_db::AnyPool {
    let pool = open_pool(db).await;
    let dialect = if db.starts_with("postgres") {
        df_db::Dialect::Postgres
    } else {
        df_db::Dialect::Sqlite
    };
    if let Err(err) = df_db::migrate(&pool, dialect).await {
        eprintln!("df-server users: migration failed: {err}");
        std::process::exit(1);
    }
    pool
}

/// Insert the org row when it does not exist yet (bootstrap case:
/// the first admin on a fresh database). The name is only a label;
/// pass `--org-name` to set it.
async fn ensure_org(pool: &df_db::AnyPool, org_id: Uuid, org_name: Option<String>) {
    if df_db::get_organization(pool, org_id).await.is_ok() {
        return;
    }
    let org = df_db::Organization {
        id: org_id,
        name: org_name.unwrap_or_else(|| "Decor POS".to_string()),
        default_currency: "USD".to_string(),
        enabled_modules: serde_json::json!({}),
        settings_json: serde_json::json!({}),
        created_at: chrono::Utc::now(),
    };
    if let Err(err) = df_db::insert_organization(pool, &org).await {
        eprintln!("df-server users: cannot create organization {org_id}: {err}");
        std::process::exit(1);
    }
}

/// Read a password: the env var first (read once, never logged),
/// otherwise a stdin prompt. Returns `None` when the prompt gets an
/// empty line — the caller decides whether that is an error (a
/// password is required) or fine (a PIN-only user).
fn read_password(password_env: &str) -> Option<String> {
    if let Ok(password) = std::env::var(password_env)
        && !password.is_empty()
    {
        return Some(password);
    }
    eprint!("Password (min 8 characters): ");
    use std::io::Write;
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => {
            let password = line.trim_end_matches(['\r', '\n']).to_string();
            if password.is_empty() {
                None
            } else {
                Some(password)
            }
        }
        Err(err) => {
            eprintln!("df-server users: cannot read password from stdin: {err}");
            std::process::exit(2);
        }
    }
}

/// Ask the operator to type `YES` (exactly) to confirm a dangerous
/// step. Anything else aborts.
fn confirm_yes(prompt: &str) -> bool {
    eprint!("{prompt}");
    use std::io::Write;
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) => line.trim() == "YES",
        Err(_) => false,
    }
}

fn hash_or_exit(password: &str, what: &str) -> String {
    match hash_password(password) {
        Ok(hash) => hash,
        Err(err) => {
            eprintln!("df-server users: {what}: {err}");
            std::process::exit(2);
        }
    }
}

async fn run_users_create_admin(
    db: String,
    org_id: Uuid,
    org_name: Option<String>,
    email: String,
    display_name: String,
    force: bool,
    password_env: String,
) -> ! {
    let pool = open_migrated_pool(&db).await;
    ensure_org(&pool, org_id, org_name).await;
    let existing = match df_db::count_users_with_role(&pool, org_id, Role::Admin).await {
        Ok(n) => n,
        Err(err) => {
            eprintln!("df-server users create-admin: cannot count admins: {err}");
            std::process::exit(1);
        }
    };
    if existing > 0 && !force {
        eprintln!(
            "df-server users create-admin: org {org_id} already has {existing} admin(s) — pass --force and confirm with YES to add another"
        );
        std::process::exit(2);
    }
    if existing > 0
        && !confirm_yes(&format!(
            "org {org_id} already has {existing} admin(s); type YES to create another admin: "
        ))
    {
        eprintln!("df-server users create-admin: aborted");
        std::process::exit(2);
    }
    let Some(password) = read_password(&password_env) else {
        eprintln!(
            "df-server users create-admin: empty password — set the env var or type a password"
        );
        std::process::exit(2);
    };
    let hash = hash_or_exit(&password, "invalid password");
    let user = match df_db::create_user(
        &pool,
        &df_db::NewUser {
            org_id,
            email: email.clone(),
            display_name,
            password_hash: Some(hash),
            counter_pin_hash: None,
            badge_token_hash: None,
            role: Role::Admin,
            portal_customer_id: None,
            active: true,
        },
    )
    .await
    {
        Ok(user) => user,
        Err(err) => {
            eprintln!("df-server users create-admin: cannot create admin {email}: {err}");
            std::process::exit(1);
        }
    };
    print_json(serde_json::json!({
        "id": user.id,
        "email": user.email,
        "display_name": user.display_name,
        "role": "Admin",
        "org_id": org_id,
    }));
}

/// Arguments for `df-server users create`, bundled so the
/// function stays under clippy's too-many-arguments lint.
struct CreateUserArgs {
    db: String,
    org_id: Uuid,
    org_name: Option<String>,
    email: String,
    display_name: String,
    role_text: String,
    pin: Option<String>,
    customer_id: Option<Uuid>,
    no_password: bool,
    password_env: String,
}

async fn run_users_create(args: CreateUserArgs) -> ! {
    let CreateUserArgs {
        db,
        org_id,
        org_name,
        email,
        display_name,
        role_text,
        pin,
        customer_id,
        no_password,
        password_env,
    } = args;
    let role = match Role::from_str(&role_text) {
        Ok(role) => role,
        Err(err) => {
            eprintln!("df-server users create: {err}");
            std::process::exit(2);
        }
    };
    if role == Role::PortalContractor && customer_id.is_none() {
        eprintln!(
            "df-server users create: --customer-id is required for --role PortalContractor (the customer the portal user may access)"
        );
        std::process::exit(2);
    }
    if role != Role::PortalContractor && customer_id.is_some() {
        eprintln!("df-server users create: --customer-id is only for --role PortalContractor");
        std::process::exit(2);
    }
    let pool = open_migrated_pool(&db).await;
    ensure_org(&pool, org_id, org_name).await;
    if let Some(customer) = customer_id
        && let Err(err) = df_db::get_customer(&pool, customer).await
    {
        eprintln!("df-server users create: cannot read customer {customer}: {err}");
        std::process::exit(1);
    }
    let pin_hash = match pin {
        Some(pin) => match hash_pin(&pin) {
            Ok(hash) => Some(hash),
            Err(err) => {
                eprintln!("df-server users create: invalid --pin: {err}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    let password_hash = if no_password {
        None
    } else {
        match read_password(&password_env) {
            Some(password) => Some(hash_or_exit(&password, "invalid password")),
            None => {
                if pin_hash.is_none() {
                    eprintln!(
                        "df-server users create: empty password — set the env var, type a password, pass --pin, or use --no-password for a PIN-only user"
                    );
                    std::process::exit(2);
                }
                None
            }
        }
    };
    if password_hash.is_none() && pin_hash.is_none() {
        eprintln!(
            "df-server users create: a user needs a password or a --pin (pass --no-password with --pin for a PIN-only user)"
        );
        std::process::exit(2);
    }
    let user = match df_db::create_user(
        &pool,
        &df_db::NewUser {
            org_id,
            email: email.clone(),
            display_name,
            password_hash,
            counter_pin_hash: pin_hash,
            badge_token_hash: None,
            role,
            portal_customer_id: customer_id,
            active: true,
        },
    )
    .await
    {
        Ok(user) => user,
        Err(err) => {
            eprintln!("df-server users create: cannot create user {email}: {err}");
            std::process::exit(1);
        }
    };
    print_json(serde_json::json!({
        "id": user.id,
        "email": user.email,
        "display_name": user.display_name,
        "role": role.as_str(),
        "org_id": org_id,
        "portal_customer_id": customer_id,
    }));
}

async fn run_users_list(db: String, org_id: Uuid) -> ! {
    let pool = open_migrated_pool(&db).await;
    let users = match df_db::list_users_for_org(&pool, org_id).await {
        Ok(users) => users,
        Err(err) => {
            eprintln!("df-server users list: cannot list users for org {org_id}: {err}");
            std::process::exit(1);
        }
    };
    let value: Vec<serde_json::Value> = users
        .iter()
        .map(|user| {
            serde_json::json!({
                "id": user.id,
                "email": user.email,
                "display_name": user.display_name,
                "role": df_auth::role_from_id(user.role_id)
                    .map(|role| role.as_str().to_string())
                    .unwrap_or_else(|| format!("unknown({})", user.role_id)),
                "portal_customer_id": user.portal_customer_id,
                "active": user.active,
            })
        })
        .collect();
    print_json(serde_json::Value::Array(value));
}

async fn run_users_revoke_sessions(db: String, email: String) -> ! {
    let pool = open_migrated_pool(&db).await;
    let user = match df_db::get_user_by_email(&pool, &email).await {
        Ok(user) => user,
        Err(err) => {
            eprintln!("df-server users revoke-sessions: cannot find user {email}: {err}");
            std::process::exit(1);
        }
    };
    let revoked = match df_db::revoke_sessions_for_user(&pool, user.id).await {
        Ok(n) => n,
        Err(err) => {
            eprintln!("df-server users revoke-sessions: cannot revoke sessions: {err}");
            std::process::exit(1);
        }
    };
    print_json(serde_json::json!({
        "email": email,
        "user_id": user.id,
        "sessions_revoked": revoked,
    }));
}

async fn run_edge(
    db: String,
    store_id: Uuid,
    listen: SocketAddr,
    org_id: Option<Uuid>,
    store_name: String,
    currency: String,
    email_outbox: Option<PathBuf>,
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
    // First boot seeds the configured org + store rows (a no-op when
    // they already exist). Without this, a fresh database has an empty
    // `stores` table and store-validating flows — e.g. the GUI product
    // import — fail with "row not found".
    match edge.ensure_configured_store().await {
        Ok(true) => println!("df-server edge: seeded store {store_id} (org {org_id}) — first boot"),
        Ok(false) => {}
        Err(err) => {
            eprintln!("df-server edge: failed to seed store: {err}");
            std::process::exit(1);
        }
    };
    // Standalone/mock terminal: card tenders approve locally so the
    // POS API works out of the box with no real terminal attached.
    edge.set_payment_provider(Arc::new(MockTerminal::new()));
    if let Some(dir) = email_outbox {
        edge.set_email_outbox_dir(dir);
    }
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
        Command::Hq {
            db,
            listen,
            org_id,
            store_id,
        } => run_hq(db, listen, org_id, store_id).await,
        Command::Edge {
            db,
            store_id,
            listen,
            org_id,
            store_name,
            currency,
            email_outbox,
        } => {
            run_edge(
                db,
                store_id,
                listen,
                org_id,
                store_name,
                currency,
                email_outbox,
            )
            .await
        }
        Command::Worker => not_implemented("worker"),
        Command::Migrate { command } => match command {
            MigrateCommand::Schema { db } => run_migrate_schema(db).await,
            MigrateCommand::Import {
                db,
                org_id,
                store_id,
                dir,
                out,
            } => run_migrate_import(db, org_id, store_id, dir, out).await,
        },
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
                typst_bin,
                finance_rate,
                email_outbox,
                email_recipient,
            } => {
                run_jobs(JobsRunArgs {
                    db,
                    org_id,
                    store_id,
                    job,
                    date,
                    currency_flag: currency,
                    typst_bin,
                    finance_rate,
                    email_outbox,
                    email_recipients: email_recipient,
                })
                .await
            }
        },
        Command::Statements { command } => match command {
            StatementsCommand::Run {
                db,
                org_id,
                store_id,
                period,
                out,
                typst_bin,
            } => run_statements(db, org_id, store_id, period, out, typst_bin).await,
        },
        Command::Accounting { command } => match command {
            AccountingCommand::Export {
                db,
                org_id,
                store_id,
                kind,
                ledger,
                out,
            } => run_accounting_export(db, org_id, store_id, kind, ledger, out).await,
        },
        Command::Users { command } => match command {
            UsersCommand::CreateAdmin {
                db,
                org_id,
                org_name,
                email,
                display_name,
                force,
                password_env,
            } => {
                run_users_create_admin(
                    db,
                    org_id,
                    org_name,
                    email,
                    display_name,
                    force,
                    password_env,
                )
                .await
            }
            UsersCommand::Create {
                db,
                org_id,
                org_name,
                email,
                display_name,
                role,
                pin,
                customer_id,
                no_password,
                password_env,
            } => {
                run_users_create(CreateUserArgs {
                    db,
                    org_id,
                    org_name,
                    email,
                    display_name,
                    role_text: role,
                    pin,
                    customer_id,
                    no_password,
                    password_env,
                })
                .await
            }
            UsersCommand::List { db, org_id } => run_users_list(db, org_id).await,
            UsersCommand::RevokeSessions { db, email } => {
                run_users_revoke_sessions(db, email).await
            }
        },
        Command::Edi { command } => match command {
            EdiCommand::Send {
                db,
                org_id,
                store_id,
                po_id,
                out_dir,
                currency,
            } => {
                run_edi_send(
                    EdiCtx {
                        db,
                        org_id,
                        store_id,
                    },
                    po_id,
                    out_dir,
                    currency,
                )
                .await
            }
            EdiCommand::Intake {
                db,
                org_id,
                store_id,
                doc_type,
                file,
                vendor_id,
                po_id,
                currency,
            } => {
                run_edi_intake(
                    EdiCtx {
                        db,
                        org_id,
                        store_id,
                    },
                    doc_type,
                    file,
                    vendor_id,
                    po_id,
                    currency,
                )
                .await
            }
            EdiCommand::Match {
                db,
                org_id,
                store_id,
                po_id,
                currency,
            } => {
                run_edi_match(
                    EdiCtx {
                        db,
                        org_id,
                        store_id,
                    },
                    po_id,
                    currency,
                )
                .await
            }
            EdiCommand::Catalog {
                db,
                org_id,
                store_id,
                file,
                vendor_id,
                currency,
            } => {
                run_edi_catalog(
                    EdiCtx {
                        db,
                        org_id,
                        store_id,
                    },
                    file,
                    vendor_id,
                    currency,
                )
                .await
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialect_for_url_prefers_postgres_prefix() {
        assert_eq!(
            dialect_for_url("postgres://decor:secret@127.0.0.1:5432/decorpos"),
            df_db::Dialect::Postgres
        );
        assert_eq!(
            dialect_for_url("postgresql://decor@db/decorpos"),
            df_db::Dialect::Postgres
        );
        assert_eq!(dialect_for_url("sqlite:edge.db"), df_db::Dialect::Sqlite);
        assert_eq!(dialect_for_url("sqlite::memory:"), df_db::Dialect::Sqlite);
    }

    #[test]
    fn resolve_db_url_flag_wins_over_env() {
        let saved = std::env::var("DATABASE_URL").ok();
        unsafe { std::env::set_var("DATABASE_URL", "sqlite:from-env.db") };
        let resolved = resolve_db_url(Some("sqlite:from-flag.db".to_string()));
        match saved {
            Some(value) => unsafe { std::env::set_var("DATABASE_URL", value) },
            None => unsafe { std::env::remove_var("DATABASE_URL") },
        }
        assert_eq!(resolved.as_deref(), Some("sqlite:from-flag.db"));
    }

    #[test]
    fn resolve_db_url_falls_back_to_env_then_none() {
        let saved = std::env::var("DATABASE_URL").ok();
        unsafe { std::env::set_var("DATABASE_URL", "sqlite:from-env.db") };
        let from_env = resolve_db_url(None);
        unsafe { std::env::remove_var("DATABASE_URL") };
        let neither = resolve_db_url(None);
        match saved {
            Some(value) => unsafe { std::env::set_var("DATABASE_URL", value) },
            None => unsafe { std::env::remove_var("DATABASE_URL") },
        }
        assert_eq!(from_env.as_deref(), Some("sqlite:from-env.db"));
        assert_eq!(neither, None);
    }

    /// The reporting primitive `migrate schema` and `hq` share: a
    /// fresh database reports no applied versions, `migrate`
    /// applies 0001–0013 (0008 does not exist), and a second run
    /// changes nothing.
    #[tokio::test]
    async fn migration_report_is_idempotent() {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        assert!(applied_migration_versions(&pool).await.is_empty());
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
        let applied = applied_migration_versions(&pool).await;
        assert_eq!(applied, vec![1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13]);
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
        assert_eq!(applied_migration_versions(&pool).await, applied);
    }
}
