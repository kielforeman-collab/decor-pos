//! `df-db` — sqlx repositories & migrations (Phase 1, v0.2).
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
//!
//! Phase 1 adds the spec §5.5 tables (`transactions`,
//! `transaction_lines`, `inventory_ledger`, `till_sessions`,
//! `payments`, `sync_outbox`, `sync_state`) in migration `0002_phase1`
//! and the repositories in [`transactions`] and [`inventory`]. Write
//! functions there are generic over any sqlx executor, so a caller can
//! run them inside one transaction (`&mut *tx`) or standalone
//! (`&pool`).
//!
//! Phase 2 adds the spec §5.3/§7 tint tables (`tint_jobs`,
//! `colorant_levels` in migration `0003_phase2`) and the repositories
//! in [`tint`]: color search, colorant CRUD, formula lookup by id /
//! color / customer history, tint-job persistence, and the atomic
//! colorant-level decrement a dispense runs.
//!
//! SQLite (edge) FK note: the edge schema deliberately omits FOREIGN
//! KEY references to HQ reference data (stores, users, customers,
//! jobs, products) — the edge must keep selling when that data has
//! not synced down yet (sqlx enables SQLite FK enforcement by
//! default, so a reference would block the sale). PostgreSQL (HQ)
//! enforces the full FK graph from spec §5. Edge-owned chains
//! (`transaction_lines` / `payments` → `transactions`) are still
//! enforced in SQLite.

pub mod codec;
pub mod error;
pub mod inventory;
pub mod models;
pub mod repos;
pub mod tint;
pub mod transactions;

pub use error::DbError;
pub use inventory::*;
pub use models::*;
pub use repos::*;
pub use sqlx::AnyPool;
pub use tint::*;
pub use transactions::*;

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
    // sqlx's SQLite driver opens an existing file but does not create
    // a missing one unless `mode=rwc` is set — so a fresh
    // `sqlite:edge.db` (the form `df-server edge --help` documents)
    // failed with "unable to open database file". Append the mode for
    // file URLs that do not already set one.
    let owned;
    let url = if url.starts_with("sqlite:") && !url.contains(":memory:") && !url.contains("mode=") {
        let sep = if url.contains('?') { '&' } else { '?' };
        owned = format!("{url}{sep}mode=rwc");
        &owned
    } else {
        url
    };
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
