//! Database connection, migration, and shared CLI helpers for `df-server`.

use std::path::PathBuf;
use std::str::FromStr;

use chrono::NaiveDate;
use df_domain::Currency;
use uuid::Uuid;

pub(crate) fn not_implemented(mode: &str) -> ! {
    eprintln!("df-server {mode}: not implemented yet — see spec §7");
    std::process::exit(2);
}

pub(crate) fn parse_date(text: Option<String>) -> Result<NaiveDate, String> {
    match text {
        None => Ok(chrono::Utc::now().date_naive()),
        Some(text) => {
            NaiveDate::from_str(&text).map_err(|_| format!("date must be YYYY-MM-DD, got {text:?}"))
        }
    }
}

pub(crate) async fn open_pool(db: &str) -> df_db::AnyPool {
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
pub(crate) fn resolve_db_url(flag: Option<String>) -> Option<String> {
    if let Some(db) = flag.filter(|db| !db.is_empty()) {
        return Some(db);
    }
    std::env::var("DATABASE_URL")
        .ok()
        .filter(|db| !db.is_empty())
}

/// Require a database URL for a subcommand: `--db` wins, else
/// `DATABASE_URL`; exit 2 when neither is set.
pub(crate) fn require_db_url(flag: Option<String>, what: &str) -> String {
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
pub(crate) fn dialect_for_url(db: &str) -> df_db::Dialect {
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
pub(crate) async fn applied_migration_versions(pool: &df_db::AnyPool) -> Vec<i64> {
    sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

/// Outcome of a migration run: which versions this run applied
/// (`newly_applied`) and every version applied in total
/// (`applied`), both ascending.
pub(crate) struct MigrationReport {
    pub(crate) newly_applied: Vec<i64>,
    pub(crate) applied: Vec<i64>,
}

/// Open a pool, apply the pending migrations for the URL's
/// dialect, and report what changed. The pool is returned so the
/// caller (e.g. `hq`) can keep serving on it.
pub(crate) async fn migrate_and_report(db: &str, what: &str) -> (df_db::AnyPool, MigrationReport) {
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
pub(crate) async fn run_migrate_schema(db_flag: Option<String>) -> ! {
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

/// The store's currency: `--currency` wins when given, otherwise
/// the store row's currency. The edge daemon never inserts a store
/// row (`Edge::open` seeds nothing), so an edge database usually has
/// no store row — pass `--currency USD|CAD` for those, or seed the
/// store first.
pub(crate) async fn store_currency(
    pool: &df_db::AnyPool,
    store_id: Uuid,
    flag: Option<String>,
) -> Currency {
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

pub(crate) fn print_json(value: serde_json::Value) -> ! {
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
pub(crate) async fn run_migrate_import(
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

/// Open a pool and run the Phase 7 migrations on it, so the `users`
/// subcommands work on a fresh database. The database file must
/// already exist for SQLite (a missing file is created by `connect`
/// with `mode=rwc` — the same behaviour as `df-server edge`).
pub(crate) async fn open_migrated_pool(db: &str) -> df_db::AnyPool {
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
