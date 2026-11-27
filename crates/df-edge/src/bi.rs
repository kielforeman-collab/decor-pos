//! BI on the edge (Phase 6, v1.0; spec §7).
//!
//! The Rust API the HTTP endpoints call — no HTTP here. Every
//! method delegates to `df-reports` (the single implementation);
//! this module is wiring only (pool/config lookup + the outbox
//! directory from [`EdgeConfig`](crate::types::EdgeConfig)).
//!
//! * [`Edge::kpi_dashboard`] — one store's KPI dashboard
//!   (sales, margin, inventory turns, AR aging, formula usage
//!   tracked only).
//! * [`Edge::dashboard_aggregate`] — one store's dashboard
//!   aggregate for the Products / Vendors / Dashboards screens
//!   (sales, daily, by-category, tenders, top products /
//!   customers, tint, low stock).
//! * [`Edge::eco_remittance`] — org-wide PaintCare/eco-fee
//!   remittance by jurisdiction.
//! * [`Edge::ad_hoc_inquiry`] — completed transactions filtered
//!   by date range + optional customer / product / doc type.
//! * [`Edge::email_outbox`] — the queued scheduled-email
//!   deliveries from `manifest.json` in the configured outbox
//!   directory. Refused (`NotFound`) when no outbox directory is
//!   configured — the edge never invents a delivery record.

use chrono::NaiveDate;
use df_reports::{
    AdHocInquiry, DashboardAggregate, EcoRemittanceReport, EmailDelivery, InquiryRow, KpiDashboard,
};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

impl Edge {
    /// One store's KPI dashboard for `[start, end]`, AR aging as
    /// of `as_of`.
    pub async fn kpi_dashboard(
        &self,
        store_id: Uuid,
        start: NaiveDate,
        end: NaiveDate,
        as_of: NaiveDate,
    ) -> Result<KpiDashboard, EdgeError> {
        Ok(df_reports::kpi_dashboard(
            self.pool(),
            self.config().org_id,
            store_id,
            start,
            end,
            as_of,
            self.config().currency,
        )
        .await?)
    }

    /// One store's dashboard aggregate for `[start, end]`
    /// (sales, daily, by-category, tenders, top products /
    /// customers, tint, low stock).
    pub async fn dashboard_aggregate(
        &self,
        store_id: Uuid,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<DashboardAggregate, EdgeError> {
        Ok(df_reports::dashboard_aggregate(
            self.pool(),
            self.config().org_id,
            store_id,
            start,
            end,
            self.config().currency,
        )
        .await?)
    }

    /// Org-wide eco-fee remittance for `[start, end]`.
    pub async fn eco_remittance(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<EcoRemittanceReport, EdgeError> {
        Ok(df_reports::eco_remittance_report(
            self.pool(),
            self.config().org_id,
            start,
            end,
            self.config().currency,
        )
        .await?)
    }

    /// Ad-hoc inquiry over one store's completed transactions.
    pub async fn ad_hoc_inquiry(
        &self,
        store_id: Uuid,
        inquiry: &AdHocInquiry,
    ) -> Result<Vec<InquiryRow>, EdgeError> {
        Ok(df_reports::run_inquiry(self.pool(), store_id, inquiry, self.config().currency).await?)
    }

    /// Queued scheduled-email deliveries from the configured
    /// outbox directory's `manifest.json`. An absent manifest
    /// reads as an empty outbox; an unconfigured outbox directory
    /// is an error, not an empty list.
    pub async fn email_outbox(&self) -> Result<Vec<EmailDelivery>, EdgeError> {
        let Some(dir) = self.email_outbox_dir() else {
            return Err(EdgeError::NotFound(
                "no email outbox directory configured for this edge (call Edge::set_email_outbox_dir or df-server edge --email-outbox)"
                    .to_string(),
            ));
        };
        Ok(df_reports::read_outbox_manifest(&dir)?)
    }
}
