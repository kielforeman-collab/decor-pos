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

mod cli;
mod db;
mod edge;
mod edi;
mod hq;
mod ops;
mod tokens;
mod users;

use clap::Parser;

use cli::{
    AccountingCommand, Cli, Command, EdiCommand, JobsCommand, MigrateCommand, StatementsCommand,
    TokensCommand, UsersCommand,
};
use db::{not_implemented, run_migrate_import, run_migrate_schema};
use edge::{EdgeRunArgs, run_edge};
use edi::{EdiCtx, run_edi_catalog, run_edi_intake, run_edi_match, run_edi_send};
use hq::{HardeningFlags, run_hq};
use ops::{JobsRunArgs, run_accounting_export, run_jobs, run_reports, run_statements};
use tokens::{run_tokens_create, run_tokens_list, run_tokens_revoke};
use users::{
    CreateUserArgs, run_users_create, run_users_create_admin, run_users_list,
    run_users_revoke_sessions,
};

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
            cors_origin,
            auth_rate_limit_max,
            auth_rate_limit_window_secs,
            trust_proxy_headers,
        } => {
            run_hq(
                db,
                listen,
                org_id,
                store_id,
                HardeningFlags {
                    cors_origins: cors_origin,
                    auth_rate_limit_max,
                    auth_rate_limit_window_secs,
                    trust_proxy_headers,
                },
            )
            .await
        }
        Command::Edge {
            db,
            store_id,
            listen,
            org_id,
            store_name,
            currency,
            country,
            region,
            email_outbox,
            hq_url,
            hq_token_env,
            sync_interval_secs,
            cors_origin,
            auth_rate_limit_max,
            auth_rate_limit_window_secs,
            trust_proxy_headers,
        } => {
            run_edge(EdgeRunArgs {
                db,
                store_id,
                listen,
                org_id,
                store_name,
                currency,
                country,
                region,
                email_outbox,
                hq_url,
                hq_token_env,
                sync_interval_secs,
                hardening: HardeningFlags {
                    cors_origins: cors_origin,
                    auth_rate_limit_max,
                    auth_rate_limit_window_secs,
                    trust_proxy_headers,
                },
            })
            .await
        }
        Command::Tokens { command } => match command {
            TokensCommand::Create {
                db,
                org_id,
                store_id,
                label,
                ttl_days,
            } => run_tokens_create(db, org_id, store_id, label, ttl_days).await,
            TokensCommand::List {
                db,
                org_id,
                store_id,
            } => run_tokens_list(db, org_id, store_id).await,
            TokensCommand::Revoke { db, id } => run_tokens_revoke(db, id).await,
        },
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
    use crate::db::{applied_migration_versions, dialect_for_url, resolve_db_url};

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
    /// applies 0001–0015 (0008 does not exist), and a second run
    /// changes nothing.
    #[tokio::test]
    async fn migration_report_is_idempotent() {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        assert!(applied_migration_versions(&pool).await.is_empty());
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
        let applied = applied_migration_versions(&pool).await;
        assert_eq!(
            applied,
            vec![
                1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19
            ]
        );
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
        assert_eq!(applied_migration_versions(&pool).await, applied);
    }
}
