//! `df-reports` — reports computed from `df-db` data (Phase 2, v0.3; spec §7).
//!
//! * [`daily_usage_report`] — one store, one date: tints dispensed
//!   (count + per-color breakdown), colorant consumed (per colorant,
//!   fl oz + cost via `df-domain` tint math) and sales totals from
//!   the completed transactions that day.
//! * [`low_stock_report`] — products below their minimum level (from
//!   the append-only inventory ledger) and colorants at/below
//!   threshold, with purchase-order suggestions from
//!   [`df_edi::generate_purchase_orders`].
//! * [`colorant_shortage_alerts`] — colorants below threshold with
//!   on-hand, threshold and shortfall; usable standalone and as a
//!   section of the low-stock report.
//!
//! Every report has a structured type (JSON-serializable) and a
//! plain-text rendering (`render_text` / `render_shortage_alerts_text`).
//! All money and volumes are `rust_decimal::Decimal` — never floats.
//!
//! No PDF / Typst / Excel output: that is Phase 3/6 (spec §7 —
//! `df-reports` gains Typst PDF statements and Excel/BI views there).
//! These reports are JSON + plain text only.
//!
//! Known gap, stated plainly: the schema has **no min-level table**
//! for products (spec §5 defines no `product_min_levels` table), so
//! the low-stock report takes its product thresholds from the caller
//! (`ProductThreshold`). The caller — today the edge daemon's
//! operator-configured thresholds — owns them until a back-office
//! min/max table lands in Phase 3.

pub mod daily;
pub mod error;
pub mod low_stock;
pub mod shortage;

pub use daily::{ColorUsage, ColorantUsage, DailyUsageReport, SalesTotals, daily_usage_report};
pub use error::ReportError;
pub use low_stock::{LowStockProduct, LowStockReport, ProductThreshold, low_stock_report};
pub use shortage::{ColorantShortageAlert, colorant_shortage_alerts, render_shortage_alerts_text};

#[cfg(test)]
pub(crate) mod test_support {
    use df_db::{AnyPool, Dialect};

    /// In-memory SQLite pool with all migrations applied.
    pub(crate) async fn test_pool() -> AnyPool {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        df_db::migrate(&pool, Dialect::Sqlite).await.unwrap();
        pool
    }
}
