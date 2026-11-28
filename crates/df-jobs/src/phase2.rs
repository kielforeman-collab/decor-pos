//! The Phase 2 daily jobs (spec §7, v0.3 — formula tracking only).
//!
//! Both run daily at 06:00 UTC by default and produce their reports
//! via `df-reports`, returning the report document as the run's JSON
//! output:
//!
//! * `daily-usage` — the daily usage report for the context's
//!   `report_date` (today, UTC, unless overridden);
//! * `low-stock` — the product low-stock report from the context's
//!   thresholds, with draft PO suggestions.
//!
//! A job whose report is empty still succeeds — an empty report is a
//! result, not a failure.

use std::sync::Arc;

use df_reports::{daily_usage_report, low_stock_report};

use crate::runner::{JobContext, JobDefinition, JobError, JobRunner, Schedule};

/// Job names (stable — the `df-server jobs run` CLI takes these).
pub const DAILY_USAGE_JOB: &str = "daily-usage";
pub const LOW_STOCK_JOB: &str = "low-stock";

/// Register the Phase 2 daily jobs on a runner.
pub fn register_phase2_jobs(runner: &JobRunner) {
    runner.register(
        JobDefinition {
            name: DAILY_USAGE_JOB,
            description: "Daily usage report: tracked formulas recorded, colorant used, sales totals",
            schedule: Schedule::Daily { hour: 6, minute: 0 },
        },
        Arc::new(|ctx: &JobContext| {
            Box::pin(async move {
                let date = ctx.report_date();
                let report = daily_usage_report(
                    &ctx.pool,
                    ctx.org_id,
                    ctx.store_id,
                    date,
                    ctx.currency,
                )
                .await
                .map_err(JobError::Report)?;
                let summary = format!(
                    "daily usage {date}: {} formula(s) recorded, tint cost {}, {} sale(s) / {} return(s), grand total {}",
                    report.formulas_recorded,
                    report.total_tint_cost.amount(),
                    report.sales.sale_count,
                    report.sales.return_count,
                    report.sales.grand_total.amount()
                );
                Ok((summary, serde_json::to_value(&report).map_err(|e| {
                    JobError::Failed(format!("report did not serialise: {e}"))
                })?))
            })
        }),
    );
    runner.register(
        JobDefinition {
            name: LOW_STOCK_JOB,
            description: "Low-stock check: products below minimum, with draft PO suggestions",
            schedule: Schedule::Daily { hour: 6, minute: 0 },
        },
        Arc::new(|ctx: &JobContext| {
            Box::pin(async move {
                let date = ctx.report_date();
                let report =
                    low_stock_report(&ctx.pool, ctx.store_id, date, &ctx.product_thresholds)
                        .await
                        .map_err(JobError::Report)?;
                let summary = format!(
                    "low stock {date}: {} product(s) below minimum, {} draft PO(s) suggested",
                    report.products.len(),
                    report.suggested_purchase_orders.len()
                );
                Ok((
                    summary,
                    serde_json::to_value(&report)
                        .map_err(|e| JobError::Failed(format!("report did not serialise: {e}")))?,
                ))
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::JobRunner;
    use chrono::Utc;
    use df_db::models::{Organization, Store};
    use df_domain::Currency;
    use uuid::Uuid;

    async fn seeded_ctx() -> JobContext {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
        df_db::insert_organization(
            &pool,
            &Organization {
                id: org_id,
                name: "Test Paint Co".to_string(),
                default_currency: "USD".to_string(),
                enabled_modules: serde_json::json!({}),
                settings_json: serde_json::json!({}),
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        df_db::insert_store(
            &pool,
            &Store {
                id: store_id,
                org_id,
                code: "MAIN".to_string(),
                name: "Main Store".to_string(),
                country_code: "US".to_string(),
                region_code: "CA".to_string(),
                postal_code: "90210".to_string(),
                currency: "USD".to_string(),
                tax_zone_id: Uuid::now_v7(),
                timezone: "America/Los_Angeles".to_string(),
                is_warehouse: false,
            },
        )
        .await
        .unwrap();
        JobContext::new(pool, org_id, store_id, Currency::Usd)
    }

    #[tokio::test]
    async fn phase2_jobs_run_and_return_their_reports() {
        let runner = JobRunner::new();
        register_phase2_jobs(&runner);
        assert_eq!(runner.definitions().len(), 2);
        let ctx = seeded_ctx().await;
        let runs = runner.run_all(&ctx).await;
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|run| run.success));
        let usage = runs
            .iter()
            .find(|run| run.job_name == DAILY_USAGE_JOB)
            .unwrap();
        assert_eq!(usage.output.as_ref().unwrap()["formulas_recorded"], 0);
        assert!(usage.summary.contains("0 formula(s) recorded"));
        let low = runs
            .iter()
            .find(|run| run.job_name == LOW_STOCK_JOB)
            .unwrap();
        assert!(
            low.output.as_ref().unwrap()["products"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        // History is observable after the run.
        assert_eq!(runner.runs().len(), 2);
        assert_eq!(
            runner.last_run(DAILY_USAGE_JOB).unwrap().job_name,
            DAILY_USAGE_JOB
        );
    }
}
