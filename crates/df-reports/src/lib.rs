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
//! * [`kpi_dashboard`] — Phase 6: one store's KPI dashboard
//!   (sales, margin, inventory turns, AR aging, formula usage
//!   tracked only) plus [`run_inquiry`] ad-hoc inquiries.
//! * [`eco_remittance_report`] — Phase 6: PaintCare/eco-fee
//!   remittance by jurisdiction (JSON + [`render_eco_csv`]).
//! * [`deliver_to_outbox`] — Phase 6: scheduled email delivery
//!   as a file-based outbox (rendered file + `manifest.json`);
//!   no live SMTP send.
//! * [`render_kpi_dashboard_pdf`] — Phase 6: the KPI dashboard
//!   as a real Typst PDF (same binary resolution as statements).
//!
//! Every report has a structured type (JSON-serializable) and a
//! plain-text rendering (`render_text` / `render_shortage_alerts_text`).
//! All money and volumes are `rust_decimal::Decimal` — never floats.
//!
//! No Excel output: that is Phase 6 (spec §7) — and Phase 6 has
//! no `.xlsx` writer available offline (no xlsx crate is vendored
//! in the cargo cache), so scheduled Excel delivery is a real CSV
//! (which Excel opens natively) plus a Typst PDF; see
//! [`outbox`] and the Phase 6 job in `df-jobs`.
//!
//! Known gap, stated plainly: the schema has **no min-level table**
//! for products (spec §5 defines no `product_min_levels` table), so
//! the low-stock report takes its product thresholds from the caller
//! (`ProductThreshold`). The caller — today the edge daemon's
//! operator-configured thresholds — owns them until a back-office
//! min/max table lands in Phase 3.

pub mod daily;
pub mod eco;
pub mod error;
pub mod kpi;
pub mod low_stock;
pub mod outbox;
pub mod shortage;
pub mod statement;

pub use daily::{ColorUsage, ColorantUsage, DailyUsageReport, SalesTotals, daily_usage_report};
pub use eco::{EcoJurisdictionRow, EcoRemittanceReport, eco_remittance_report, render_eco_csv};
pub use error::ReportError;
pub use kpi::{
    AdHocInquiry, FormulaColorUsage, FormulaUsage, InquiryRow, KpiDashboard, KpiInventory,
    KpiMargin, KpiSales, kpi_dashboard, render_kpi_dashboard_pdf, render_kpi_dashboard_typst,
    run_inquiry,
};
pub use low_stock::{LowStockProduct, LowStockReport, ProductThreshold, low_stock_report};
pub use outbox::{EmailDelivery, deliver_to_outbox, file_sha256, read_outbox_manifest};
pub use shortage::{ColorantShortageAlert, colorant_shortage_alerts, render_shortage_alerts_text};
pub use statement::{
    StatementHeader, build_statements, format_money, header_for, period_label, persist_statement,
    render_statement_pdf, render_statement_typst, resolve_typst_binary, statement_pdf_sha256,
    to_statement_row,
};

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
