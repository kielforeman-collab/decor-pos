//! `df-db` — sqlx repositories & migrations (Phase 0, v0.1).
//!
//! Two dialects, one repository implementation:
//!
//! * PostgreSQL HQ — `migrations/postgres/` (NUMERIC(14,4), UUID, JSONB,
//!   TIMESTAMPTZ).
//! * SQLite edge — `migrations/sqlite/` (same tables; UUID/JSON as TEXT,
//!   timestamps as RFC 3339 text, booleans as INTEGER 0/1).
//!
//! Repositories run over [`sqlx::AnyPool`] with `?` placeholders,
//! which sqlx rewrites to `$N` for PostgreSQL. No `query!` macros are
//! used, so the workspace builds with `SQLX_OFFLINE=true` and no live
//! database (see `.cargo/config.toml`).
//!
//! Phase 0 tests exercise the SQLite path end-to-end. The PostgreSQL
//! path is migration-embedded and compile-checked; a live-Postgres
//! round-trip test is a Phase 1 item (needs a running PG 16+).
//! See `repos.rs` for why ids/money/timestamps cross the driver as text.

pub mod error;
pub mod models;
pub mod repos;

pub use error::DbError;
pub use models::*;
pub use repos::*;
pub use sqlx::AnyPool;

/// Which migration set to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    Sqlite,
}

/// Install the sqlx `Any` drivers (PostgreSQL + SQLite). Call once
/// before [`connect`].
pub fn install_drivers() {
    sqlx::any::install_default_drivers();
}

/// Open a pool. For `sqlite::memory:` URLs the pool is limited to a
/// single connection, because each SQLite in-memory connection is a
/// separate database.
pub async fn connect(url: &str) -> Result<AnyPool, DbError> {
    install_drivers();
    let mut options = sqlx::any::AnyPoolOptions::new();
    if url.contains(":memory:") {
        options = options.max_connections(1);
    }
    Ok(options.connect(url).await?)
}

/// Run the embedded migrations for a dialect against a pool.
pub async fn migrate(pool: &AnyPool, dialect: Dialect) -> Result<(), DbError> {
    match dialect {
        Dialect::Postgres => {
            sqlx::migrate!("migrations/postgres").run(pool).await?;
        }
        Dialect::Sqlite => {
            sqlx::migrate!("migrations/sqlite").run(pool).await?;
        }
    }
    Ok(())
}
