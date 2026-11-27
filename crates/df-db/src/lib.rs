//! `df-db` — sqlx repositories & migrations (Phase 1, v0.2).
//!
//! Two dialects, one repository implementation, one SQL text:
//!
//! * PostgreSQL HQ — `migrations/postgres/` (same tables as SQLite;
//!   ids, money, timestamps, dates and JSON are stored as TEXT, the
//!   same encoding SQLite uses).
//! * SQLite edge — `migrations/sqlite/` (same tables; UUID/JSON as TEXT,
//!   timestamps as RFC 3339 text, booleans as INTEGER 0/1).
//!
//! Repositories run over [`sqlx::AnyPool`] with `$N` placeholders.
//! sqlx's `Any` driver does NOT rewrite `?` placeholders — they reach
//! PostgreSQL literally, where `?` is the JSONB operator, so every
//! query syntax-errors there (proven live 2026-11-26). `$N` works on
//! both dialects: natively on PostgreSQL, and as named parameters
//! bound positionally on SQLite. No `query!` macros are
//! used, so the workspace builds with `SQLX_OFFLINE=true` and no live
//! database (see `.cargo/config.toml`).
//!
//! Why TEXT columns on PostgreSQL? The `Any` driver can only decode
//! Bool, integers, floats, Bytea and Text from Postgres, and it sends
//! text binds typed as TEXT, which Postgres refuses to assign into
//! uuid / numeric / timestamptz / date / jsonb columns. Native PG
//! types would need per-parameter `CAST`s and per-column CAST reads,
//! which SQLite destroys (`CAST(uuid-text AS UUID)` yields 0).
//! TEXT keeps one SQL text for both dialects; money stays exact
//! (decimal strings), UUIDs stay hyphenated, timestamps stay
//! RFC 3339. Native PG types are a future data-layer phase, not a
//! Phase 8 item.
//!
//! Phase 0 tests exercise the SQLite path end-to-end. The PostgreSQL
//! path is covered live since Phase 8 (`tests/pg_repos.rs`, gated on
//! a running PG 16+ via `DECOR_POS_TEST_PG_URL` or
//! `/tmp/decor-pos-pg-url.txt`).
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
//! Phase 3 adds the contractor AR repositories in [`ar`]:
//! charge-account credit fields, customer jobs, AR allocations,
//! statements, inventory transfers, physical counts and reorder
//! points (migration `0005_phase3`).
//!
//! Phase 4 adds the CRM + delivery repositories in [`crm`]:
//! activity log, appointments, saved views, deliveries and
//! proof-of-delivery rows (migration `0006_phase4`), plus the
//! domain-rule repositories in [`delivery`] (validated
//! transitions with an audit trail, POD-gated close-out,
//! dispatch board, packing slips) and [`crm_reports`]
//! (reminder tickler, overlap-checked appointments,
//! lapsed-contractor report, saved-view evaluation) on migration
//! `0007_phase4_crm_delivery` (delivery events, packing slips,
//! dispatch priority / vehicle, POD photo time).
//!
//! Phase 6 adds the Scheduling+ repositories in [`scheduling`]:
//! shift templates, staff shifts and timeclock entries
//! (migration `0011_phase6`).
//!
//! Phase 7 adds the auth-core repositories in [`auth`]: users
//! (create / lookup / credential-hash setters / portal-customer
//! link) and sessions (insert / lookup by token hash / revoke /
//! list), plus the DB-backed login services (`login_with_password`,
//! `login_with_pin`, `login_with_badge`, `login_portal`,
//! `verify_manager_override`, `validate_session_token`) built on
//! the pure verification in `df-auth` (migration `0012_phase7`:
//! `roles` seed, `users.portal_customer_id`, `sessions`).
//!
//! SQLite (edge) FK note: the edge schema deliberately omits FOREIGN
//! KEY references to HQ reference data (stores, users, customers,
//! jobs, products) — the edge must keep selling when that data has
//! not synced down yet (sqlx enables SQLite FK enforcement by
//! default, so a reference would block the sale). PostgreSQL (HQ)
//! enforces the full FK graph from spec §5. Edge-owned chains
//! (`transaction_lines` / `payments` → `transactions`,
//! `transfer_lines` → `inventory_transfers`, `count_lines` →
//! `physical_counts`) are still enforced in SQLite.

pub mod ar;
pub mod auth;
pub mod codec;
pub mod crm;
pub mod crm_reports;
pub mod delivery;
pub mod error;
pub mod inventory;
pub mod models;
pub mod portal;
pub mod product_import;
pub mod purchase_orders;
pub mod repos;
pub mod scheduling;
pub mod tint;
pub mod transactions;
pub mod vendors;

pub use ar::*;
pub use auth::*;
pub use crm::*;
pub use crm_reports::*;
pub use delivery::*;
pub use error::DbError;
pub use inventory::*;
pub use models::*;
pub use portal::*;
pub use product_import::*;
pub use purchase_orders::*;
pub use repos::*;
pub use scheduling::*;
pub use sqlx::AnyPool;
pub use tint::*;
pub use transactions::*;
pub use vendors::*;

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

/// Redact the userinfo (username/password) from a database URL for
/// safe logging: `postgres://user:secret@host/db` becomes
/// `postgres://***@host/db`. SQLite URLs and URLs without userinfo
/// are returned unchanged. Callers must use this — never log a raw
/// `--db` URL — because a PostgreSQL URL carries the password.
pub fn redact_db_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after_scheme = &url[scheme_end + 3..];
    // Userinfo, if any, ends at the last '@' before the first '/'.
    let path_start = after_scheme.find('/').unwrap_or(after_scheme.len());
    let Some(at) = after_scheme[..path_start].rfind('@') else {
        return url.to_string();
    };
    format!("{}://***@{}", &url[..scheme_end], &after_scheme[at + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_db_url_hides_postgres_password() {
        assert_eq!(
            redact_db_url("postgres://decor:secret123@db.internal:5432/decorpos"),
            "postgres://***@db.internal:5432/decorpos"
        );
        // Password containing '@': the userinfo runs to the LAST '@'.
        assert_eq!(
            redact_db_url("postgres://decor:p@ss@db.internal/decorpos"),
            "postgres://***@db.internal/decorpos"
        );
        // No userinfo: unchanged.
        assert_eq!(
            redact_db_url("postgres://db.internal/decorpos"),
            "postgres://db.internal/decorpos"
        );
        // SQLite URLs: unchanged.
        assert_eq!(redact_db_url("sqlite:edge.db"), "sqlite:edge.db");
        assert_eq!(redact_db_url("sqlite::memory:"), "sqlite::memory:");
    }
}
