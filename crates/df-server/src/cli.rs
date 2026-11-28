//! Command-line definitions for `df-server`.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "df-server",
    version,
    about = "decor-pos unified server binary (hq live in Phase 8, v1.2; edge, reports and jobs live since v0.3)"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
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
        /// Country code for the store's jurisdiction (`US` or
        /// `CA`), used when seeding the store row on first boot —
        /// tax and eco fees price from it. Omit it (or pass an
        /// empty value) to seed a blank country, which prices tax
        /// as 0 until the store row is fixed.
        #[arg(long)]
        country: Option<String>,
        /// Region / province code for the store's jurisdiction
        /// (e.g. `BC`), used with `--country` when seeding the
        /// store row on first boot. Matched case-insensitively
        /// against the tax presets (`CA`/`BC` = GST 5% + PST 7%).
        #[arg(long)]
        region: Option<String>,
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
pub(crate) enum MigrateCommand {
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
pub(crate) enum JobsCommand {
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
pub(crate) enum StatementsCommand {
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
pub(crate) enum AccountingCommand {
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
pub(crate) enum EdiCommand {
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
pub(crate) enum UsersCommand {
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
