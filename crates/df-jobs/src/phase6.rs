//! The Phase 6 scheduled-report-email job (spec §7, v1.0).
//!
//! * `scheduled-report-email` — runs daily at 07:00 UTC: renders
//!   the KPI dashboard for the context's `report_date` (a single
//!   day) to a real Typst PDF and the eco-fee remittance for the
//!   calendar month to date to CSV, then queues one email per
//!   recipient per file in the file-based outbox
//!   ([`df_reports::deliver_to_outbox`]).
//!
//! The job sends nothing: there is no SMTP here. The outbox
//! directory (`JobContext::email_outbox_dir`) plus `manifest.json`
//! is the honest record — a mail relay picks it up. The job fails
//! loudly (not silently empty) when no outbox directory or no
//! recipients are configured, so a misconfigured schedule is
//! visible in the run history instead of looking like a quiet day.
//!
//! Excel note, stated plainly: no `.xlsx` writer crate is vendored
//! in the offline cargo cache, so the spreadsheet artifact is a
//! real CSV (which Excel opens natively), not a native `.xlsx`.
//! The PDF leg is real (Typst, same binary resolution as the
//! monthly statements).
//!
//! The Phase 6 labor-forecast job (spec §7 Phase 6, §4
//! `scheduling_plus`) lives in this module too:
//!
//! * `labor-forecast-refresh` — runs daily at 05:30 UTC, before
//!   the store opens: builds a 7-day labor forecast from the last
//!   28 days of completed sales and closed timeclock entries
//!   ([`df_scheduling::build_forecast`]) and returns it as the
//!   run's JSON output, so the back office's Scheduling tab (and
//!   any operator reading the job history) sees the fresh numbers
//!   without opening the edge.
//!
//! When history cannot support a forecast (no completed sales or
//! no worked timeclock hours in the window) the job still
//! **succeeds** and reports `forecast: null` with the reason — a
//! fresh store is not a broken job, and a failed run would make
//! `df-server jobs run` exit 1 on every empty database. The
//! coverage check in the back office reports the same state as
//! `Unknown` rather than guessing.

use std::sync::Arc;

use chrono::{Datelike, Utc};
use serde_json::json;

use crate::runner::{JobContext, JobDefinition, JobError, JobRunner, Schedule};

/// Job name (stable — the `df-server jobs run` CLI takes it).
pub const SCHEDULED_REPORT_EMAIL_JOB: &str = "scheduled-report-email";

/// Job name (stable — the `df-server jobs run` CLI takes it).
pub const LABOR_FORECAST_REFRESH_JOB: &str = "labor-forecast-refresh";

/// Days of history the labor forecast reads.
pub const FORECAST_HISTORY_DAYS: i64 = 28;

/// Days ahead the labor forecast covers.
pub const FORECAST_HORIZON_DAYS: i64 = 7;

/// Build the store's labor forecast for the 7 days from
/// `from_date`, from the previous 28 days of completed sales +
/// closed timeclock entries. Returns `None` when history cannot
/// support a forecast (the caller reports that, it is not an
/// error).
pub async fn refresh_labor_forecast(
    pool: &df_db::AnyPool,
    store_id: uuid::Uuid,
    from_date: chrono::NaiveDate,
) -> Result<Option<df_scheduling::ForecastReport>, JobError> {
    let window_end = from_date
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc();
    let window_start = window_end - chrono::Duration::days(FORECAST_HISTORY_DAYS);
    let transactions = df_db::list_transactions_for_store(pool, store_id).await?;
    let sales: Vec<df_scheduling::SalePoint> = transactions
        .iter()
        .filter(|tx| tx.status == "completed")
        .filter(|tx| tx.created_at >= window_start && tx.created_at < window_end)
        .map(|tx| df_scheduling::SalePoint {
            at: tx.created_at,
            amount: tx.grand_total,
        })
        .collect();
    let entries = df_db::list_closed_timeclock_entries_for_store_range(
        pool,
        store_id,
        window_start,
        window_end,
    )
    .await?;
    let labor: Vec<df_scheduling::LaborPoint> = entries
        .iter()
        .filter_map(|entry| {
            let hours = df_scheduling::TimeclockEntry {
                id: entry.id,
                org_id: entry.org_id,
                store_id: entry.store_id,
                staff_id: entry.staff_id,
                clock_in: entry.clock_in,
                clock_out: entry.clock_out,
                source: entry.source.clone(),
            }
            .worked_hours()?;
            Some(df_scheduling::LaborPoint {
                at: entry.clock_in,
                hours,
            })
        })
        .collect();
    let targets: Vec<chrono::NaiveDate> = (0..FORECAST_HORIZON_DAYS)
        .map(|offset| from_date + chrono::Duration::days(offset))
        .collect();
    match df_scheduling::build_forecast(store_id, &sales, &labor, &targets, None) {
        Ok(report) => Ok(Some(report)),
        Err(df_scheduling::SchedulingError::InsufficientHistory(_)) => Ok(None),
        Err(err) => Err(JobError::Failed(err.to_string())),
    }
}

/// Register the Phase 6 scheduled-report-email job on a runner.
pub fn register_phase6_jobs(runner: &JobRunner) {
    runner.register(
        JobDefinition {
            name: SCHEDULED_REPORT_EMAIL_JOB,
            description: "Scheduled report email: render the KPI dashboard (PDF) + eco remittance (CSV) and queue them in the file-based email outbox (no SMTP send)",
            schedule: Schedule::Daily { hour: 7, minute: 0 },
        },
        Arc::new(|ctx: &JobContext| {
            Box::pin(async move {
                let outbox_dir = ctx.email_outbox_dir.clone().ok_or_else(|| {
                    JobError::Failed(
                        "scheduled-report-email: no email outbox configured \
                         (set JobContext::email_outbox_dir / df-server jobs run --email-outbox)"
                            .to_string(),
                    )
                })?;
                if ctx.email_recipients.is_empty() {
                    return Err(JobError::Failed(
                        "scheduled-report-email: no recipients configured \
                         (set JobContext::email_recipients / df-server jobs run --email-recipient)"
                            .to_string(),
                    ));
                }
                let date = ctx.report_date();
                let month_start =
                    chrono::NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap();
                let org = df_db::get_organization(&ctx.pool, ctx.org_id)
                    .await
                    .map_err(JobError::Db)?;
                let store = df_db::get_store(&ctx.pool, ctx.store_id)
                    .await
                    .map_err(JobError::Db)?;
                let dashboard = df_reports::kpi_dashboard(
                    &ctx.pool,
                    ctx.org_id,
                    ctx.store_id,
                    date,
                    date,
                    date,
                    ctx.currency,
                )
                .await
                .map_err(JobError::Report)?;
                let pdf = df_reports::render_kpi_dashboard_pdf(
                    &dashboard,
                    &org.name,
                    &store.name,
                    ctx.typst_bin.as_deref(),
                )
                .map_err(JobError::Report)?;
                let remittance = df_reports::eco_remittance_report(
                    &ctx.pool,
                    ctx.org_id,
                    month_start,
                    date,
                    ctx.currency,
                )
                .await
                .map_err(JobError::Report)?;
                let csv = df_reports::render_eco_csv(&remittance);
                let scheduled_at = Utc::now();
                let kpi_file = format!("kpi-{date}.pdf");
                let eco_file = format!("eco-{:04}-{:02}.csv", date.year(), date.month());
                let kpi_subject = format!("KPI dashboard {date} — {}", store.name);
                let eco_subject = format!(
                    "Eco-fee remittance {:04}-{:02} to date — {}",
                    date.year(),
                    date.month(),
                    store.name
                );
                let mut deliveries = Vec::new();
                for recipient in &ctx.email_recipients {
                    for (kind, subject, file, bytes) in [
                        (
                            "kpi-dashboard",
                            kpi_subject.as_str(),
                            kpi_file.as_str(),
                            pdf.as_slice(),
                        ),
                        (
                            "eco-remittance",
                            eco_subject.as_str(),
                            eco_file.as_str(),
                            csv.as_bytes(),
                        ),
                    ] {
                        let delivery = df_reports::deliver_to_outbox(
                            &outbox_dir,
                            kind,
                            recipient,
                            subject,
                            scheduled_at,
                            file,
                            bytes,
                        )
                        .map_err(JobError::Report)?;
                        deliveries.push(json!({
                            "recipient": delivery.recipient,
                            "report_kind": delivery.report_kind,
                            "file_name": delivery.file_name,
                            "file_sha256": delivery.file_sha256,
                            "status": delivery.status,
                        }));
                    }
                }
                let summary = format!(
                    "scheduled report email {date}: KPI dashboard PDF ({} bytes) + eco remittance CSV ({} bytes) queued for {} recipient(s) in {}",
                    pdf.len(),
                    csv.len(),
                    ctx.email_recipients.len(),
                    outbox_dir.display()
                );
                Ok((
                    summary,
                    json!({
                        "date": date.to_string(),
                        "month_start": month_start.to_string(),
                        "kpi_pdf_sha256": df_reports::file_sha256(&pdf),
                        "eco_csv_sha256": df_reports::file_sha256(csv.as_bytes()),
                        "deliveries": deliveries,
                    }),
                ))
            })
        }),
    );
    runner.register(
        JobDefinition {
            name: LABOR_FORECAST_REFRESH_JOB,
            description: "Labor forecast refresh: rebuild the 7-day sales-driven labor forecast from the last 28 days of sales + timeclock history",
            schedule: Schedule::Daily { hour: 5, minute: 30 },
        },
        Arc::new(|ctx: &JobContext| {
            Box::pin(async move {
                let from_date = ctx.report_date();
                let report =
                    refresh_labor_forecast(&ctx.pool, ctx.store_id, from_date).await?;
                match report {
                    Some(report) => {
                        let total_hours: rust_decimal::Decimal = report
                            .days
                            .iter()
                            .map(|day| day.required_hours)
                            .sum();
                        let summary = format!(
                            "labor forecast for {}–{}: {} required hours over 7 days at {} sales per labor hour ({} history days, {} sales)",
                            report.days.first().map(|day| day.date).unwrap_or(from_date),
                            report.days.last().map(|day| day.date).unwrap_or(from_date),
                            total_hours,
                            report.sales_per_labor_hour,
                            report.history_days,
                            report.history_txns
                        );
                        Ok((summary, serde_json::to_value(report).expect("report serialises")))
                    }
                    None => Ok((
                        format!(
                            "labor forecast for {from_date}: insufficient history (no completed sales or no worked timeclock hours in the last {FORECAST_HISTORY_DAYS} days) — no forecast built"
                        ),
                        json!({ "forecast": null, "reason": "insufficient history" }),
                    )),
                }
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_db::models::{Organization, Store};

    async fn test_ctx(pool: df_db::AnyPool, outbox_dir: Option<std::path::PathBuf>) -> JobContext {
        let org_id = uuid::Uuid::from_u128(0x2000);
        let store_id = uuid::Uuid::from_u128(0x1000);
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
                tax_zone_id: uuid::Uuid::now_v7(),
                timezone: "America/Los_Angeles".to_string(),
                is_warehouse: false,
            },
        )
        .await
        .unwrap();
        let mut ctx = JobContext::new(pool, org_id, store_id, df_domain::Currency::Usd);
        ctx.report_date = Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 10).unwrap());
        ctx.email_outbox_dir = outbox_dir;
        ctx
    }

    async fn test_pool() -> df_db::AnyPool {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn scheduled_email_job_writes_outbox_with_rendered_files() {
        let pool = test_pool().await;
        let dir = std::env::temp_dir().join(format!("df-email-job-{}", uuid::Uuid::now_v7()));
        let mut ctx = test_ctx(pool, Some(dir.clone())).await;
        ctx.email_recipients = vec!["manager@example.com".to_string()];
        let runner = JobRunner::new();
        register_phase6_jobs(&runner);
        let run = runner
            .run_now(&ctx, SCHEDULED_REPORT_EMAIL_JOB)
            .await
            .unwrap();
        assert!(run.success, "job failed: {:?}", run.error);
        assert!(run.summary.contains("queued for 1 recipient(s)"));
        // Both rendered files exist on disk; the PDF is real.
        let pdf = std::fs::read(dir.join("kpi-2026-01-10.pdf")).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        let csv = std::fs::read_to_string(dir.join("eco-2026-01.csv")).unwrap();
        assert!(csv.starts_with("country_code,region_code,store_code,eco_fee_category"));
        // The manifest records both deliveries, queued — not sent.
        let manifest = df_reports::read_outbox_manifest(&dir).unwrap();
        assert_eq!(manifest.len(), 2);
        assert!(manifest.iter().all(|d| d.status == "queued"));
        assert_eq!(
            manifest[0].file_sha256,
            df_reports::file_sha256(&pdf),
            "manifest hash must match the rendered PDF"
        );
        assert_eq!(manifest[0].recipient, "manager@example.com");
        let output = run.output.unwrap();
        assert_eq!(output["deliveries"].as_array().unwrap().len(), 2);
        assert_eq!(
            output["kpi_pdf_sha256"].as_str().unwrap(),
            df_reports::file_sha256(&pdf)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn scheduled_email_job_fails_without_outbox_or_recipients() {
        let pool = test_pool().await;
        let runner = JobRunner::new();
        register_phase6_jobs(&runner);
        // No outbox at all.
        let ctx = test_ctx(pool.clone(), None).await;
        let run = runner
            .run_now(&ctx, SCHEDULED_REPORT_EMAIL_JOB)
            .await
            .unwrap();
        assert!(!run.success);
        assert!(run.error.as_deref().unwrap().contains("no email outbox"));
        // Outbox but no recipients (fresh pool — the org/store
        // ids are fixed test ids, so a second seed needs a
        // second database).
        let pool2 = test_pool().await;
        let dir = std::env::temp_dir().join(format!("df-email-job-{}", uuid::Uuid::now_v7()));
        let ctx = test_ctx(pool2, Some(dir.clone())).await;
        let run = runner
            .run_now(&ctx, SCHEDULED_REPORT_EMAIL_JOB)
            .await
            .unwrap();
        assert!(!run.success);
        assert!(run.error.as_deref().unwrap().contains("no recipients"));
        assert!(!dir.join("manifest.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- labor-forecast-refresh (Phase 6, scheduling+) ------------------------

    async fn forecast_seeded_ctx() -> JobContext {
        let pool = test_pool().await;
        let org_id = uuid::Uuid::from_u128(0x2000);
        let store_id = uuid::Uuid::from_u128(0x1000);
        // test_ctx seeds org/store with these same fixed ids.
        let mut ctx = test_ctx(pool.clone(), None).await;
        // Four Mondays (Nov 2/9/16/23 2026), 10 completed sales
        // of $100 each at 10:00, and one 5-hour timeclock entry
        // (08:00–13:00) per day. Ratio: $4,000 / 20h = $200/hour.
        for day in [2, 9, 16, 23] {
            for n in 0..10 {
                df_db::insert_transaction(
                    &pool,
                    &df_db::models::Transaction {
                        id: uuid::Uuid::now_v7(),
                        org_id,
                        store_id,
                        till_session_id: None,
                        doc_number: format!("TX-202611{day:02}-{n:02}"),
                        doc_type: "sale".to_string(),
                        status: "completed".to_string(),
                        customer_id: None,
                        job_id: None,
                        po_number: None,
                        salesperson_id: None,
                        cashier_id: Some(uuid::Uuid::now_v7()),
                        currency: "USD".to_string(),
                        subtotal: rust_decimal::Decimal::from_str_exact("100").unwrap(),
                        tax_total: rust_decimal::Decimal::ZERO,
                        eco_fee_total: rust_decimal::Decimal::ZERO,
                        pos_fee_total: rust_decimal::Decimal::ZERO,
                        grand_total: rust_decimal::Decimal::from_str_exact("100").unwrap(),
                        deposit_paid: rust_decimal::Decimal::ZERO,
                        fulfillment_mode: "counter".to_string(),
                        scheduled_at: None,
                        completed_at: Some(
                            chrono::TimeZone::with_ymd_and_hms(
                                &chrono::Utc,
                                2026,
                                11,
                                day,
                                10,
                                0,
                                0,
                            )
                            .unwrap(),
                        ),
                        created_at: chrono::TimeZone::with_ymd_and_hms(
                            &chrono::Utc,
                            2026,
                            11,
                            day,
                            10,
                            0,
                            0,
                        )
                        .unwrap(),
                    },
                )
                .await
                .unwrap();
            }
            df_db::insert_timeclock_entry(
                &pool,
                &df_db::models::TimeclockEntry {
                    id: uuid::Uuid::now_v7(),
                    org_id,
                    store_id,
                    staff_id: uuid::Uuid::from_u128(0x5001),
                    clock_in: chrono::TimeZone::with_ymd_and_hms(
                        &chrono::Utc,
                        2026,
                        11,
                        day,
                        8,
                        0,
                        0,
                    )
                    .unwrap(),
                    clock_out: Some(
                        chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 11, day, 13, 0, 0)
                            .unwrap(),
                    ),
                    source: "counter".to_string(),
                },
            )
            .await
            .unwrap();
        }
        // Forecast from Monday Nov 30.
        ctx.report_date = Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 30).unwrap());
        ctx
    }

    #[tokio::test]
    async fn labor_forecast_refresh_builds_the_7_day_forecast() {
        let runner = JobRunner::new();
        register_phase6_jobs(&runner);
        assert_eq!(runner.definitions().len(), 2);
        let ctx = forecast_seeded_ctx().await;
        let run = runner
            .run_now(&ctx, LABOR_FORECAST_REFRESH_JOB)
            .await
            .unwrap();
        assert!(run.success, "{run:?}");
        // 7 days × 5.00 required hours = 35.00 total at $200/hour.
        assert!(run.summary.contains("35.00 required hours"));
        assert!(run.summary.contains("200 sales per labor hour"));
        let output = run.output.unwrap();
        assert_eq!(output["sales_per_labor_hour"], "200");
        assert_eq!(output["history_days"], 4);
        assert_eq!(output["history_txns"], 40);
        assert_eq!(output["history_sales"], "4000");
        assert_eq!(output["history_labor_hours"], "20");
        let days = output["days"].as_array().unwrap();
        assert_eq!(days.len(), 7);
        // Monday Nov 30 forecasts from Mondays: 10 txns, $1,000,
        // 5.00 hours, with the 10:00 bucket.
        assert_eq!(days[0]["date"], "2026-11-30");
        assert_eq!(days[0]["forecast_txns"], "10.00");
        assert_eq!(days[0]["forecast_sales"], "1000.00");
        assert_eq!(days[0]["required_hours"], "5.00");
        assert_eq!(days[0]["hourly"][0]["hour"], 10);
        // Tuesday Dec 1 has no history: overall daily average.
        assert_eq!(days[1]["date"], "2026-12-01");
        assert_eq!(days[1]["forecast_sales"], "1000.00");
        assert_eq!(days[1]["required_hours"], "5.00");
        assert!(days[1]["hourly"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn labor_forecast_refresh_reports_insufficient_history() {
        let pool = test_pool().await;
        let ctx = test_ctx(pool, None).await;
        let runner = JobRunner::new();
        register_phase6_jobs(&runner);
        let run = runner
            .run_now(&ctx, LABOR_FORECAST_REFRESH_JOB)
            .await
            .unwrap();
        assert!(run.success, "{run:?}");
        assert!(run.summary.contains("insufficient history"));
        assert_eq!(run.output.unwrap()["forecast"], serde_json::Value::Null);
    }
}
