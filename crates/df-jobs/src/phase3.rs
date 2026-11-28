//! The Phase 3 monthly jobs (spec §7, v0.4).
//!
//! * `monthly-statements` — runs on the 1st at 06:00 UTC: builds the
//!   previous month's statements for every charge account
//!   ([`df_reports::build_statements`]) and renders each one to a
//!   real Typst PDF, recording the SHA-256 of every PDF in the run's
//!   JSON output. The PDFs themselves are written to disk by the
//!   `df-server statements run` CLI (the job has no output
//!   directory); the job proves the batch renders and reports the
//!   hashes the `statements` row stores (persisted by `df-ar`'s
//!   repo — Worker A — not here).
//! * `finance-charges` — runs on the 1st at 06:05 UTC: assesses one
//!   month's finance charge on every charge account's overdue
//!   balance using `df-ar`'s assessment
//!   ([`df_ar::overdue_balance`], [`df_ar::finance_charge_for`]) and
//!   posts the charge to `ar_ledger` — idempotently, keyed by
//!   [`df_ar::statement_period_key`] (`reference_type =
//!   'statement_run'`), so a re-run of the same period posts
//!   nothing ([`df_db::find_ar_entry_by_reference`]).
//!
//! Both jobs report on the previous month relative to the context's
//! `report_date` (today, UTC, unless overridden) — the hook that
//! makes them testable without waiting for the 1st.

use std::sync::Arc;

use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde_json::json;

use crate::runner::{JobContext, JobDefinition, JobError, JobRunner, Schedule};

/// Job names (stable — the `df-server jobs run` CLI takes these).
pub const MONTHLY_STATEMENTS_JOB: &str = "monthly-statements";
pub const FINANCE_CHARGES_JOB: &str = "finance-charges";

/// The month before `date`, as `"YYYY-MM"`.
fn previous_month(date: NaiveDate) -> String {
    let (year, month) = if date.month() == 1 {
        (date.year() - 1, 12)
    } else {
        (date.year(), date.month() - 1)
    };
    format!("{year:04}-{month:02}")
}

/// Register the two Phase 3 monthly jobs on a runner.
pub fn register_phase3_jobs(runner: &JobRunner) {
    runner.register(
        JobDefinition {
            name: MONTHLY_STATEMENTS_JOB,
            description: "Monthly statements: build last month's statements and render each to a Typst PDF",
            schedule: Schedule::Monthly { day: 1, hour: 6, minute: 0 },
        },
        Arc::new(|ctx: &JobContext| {
            Box::pin(async move {
                let period = previous_month(ctx.report_date());
                let org = df_db::get_organization(&ctx.pool, ctx.org_id)
                    .await
                    .map_err(JobError::Db)?;
                let store = df_db::get_store(&ctx.pool, ctx.store_id)
                    .await
                    .map_err(JobError::Db)?;
                let statements = df_reports::build_statements(
                    &ctx.pool,
                    ctx.org_id,
                    ctx.store_id,
                    &period,
                    ctx.currency,
                )
                .await
                .map_err(JobError::Report)?;
                let mut rendered = Vec::new();
                let mut total_due = Decimal::ZERO;
                for statement in &statements {
                    let customer = df_db::get_customer(&ctx.pool, statement.customer_id)
                        .await
                        .map_err(JobError::Db)?;
                    let header = df_reports::header_for(
                        &customer,
                        &org.name,
                        &store.name,
                        statement.period_end,
                    );
                    let pdf = df_reports::render_statement_pdf(
                        statement,
                        &header,
                        ctx.typst_bin.as_deref(),
                    )
                    .map_err(JobError::Report)?;
                    total_due += statement.closing_balance.amount();
                    rendered.push(json!({
                        "account_number": statement.account_number,
                        "customer_name": header.customer_name,
                        "closing_balance": statement.closing_balance.amount().to_string(),
                        "pdf_bytes": pdf.len(),
                        "pdf_sha256": df_reports::statement_pdf_sha256(&pdf),
                    }));
                }
                let currency = ctx.currency;
                let summary = format!(
                    "monthly statements {period}: {} statement(s), total due {} {}",
                    statements.len(),
                    total_due,
                    currency.code()
                );
                Ok((
                    summary,
                    json!({ "period": period, "statements": rendered }),
                ))
            })
        }),
    );
    runner.register(
        JobDefinition {
            name: FINANCE_CHARGES_JOB,
            description: "Finance charges: assess one month's charge on overdue charge-account balances via df-ar and post it idempotently to ar_ledger",
            schedule: Schedule::Monthly { day: 1, hour: 6, minute: 5 },
        },
        Arc::new(|ctx: &JobContext| {
            Box::pin(async move {
                // Assess the month just ended: the report date is
                // the 1st, so the period is the previous calendar
                // month and overdue is measured as of its last day.
                let as_of = ctx.report_date() - chrono::Duration::days(1);
                let (period_start, period_end) = df_ar::period_for(as_of);
                let customers = df_db::list_charge_customers(&ctx.pool, ctx.org_id)
                    .await
                    .map_err(JobError::Db)?;
                let mut assessments = Vec::new();
                let mut total_posted = Decimal::ZERO;
                let mut total_skipped = 0u32;
                for customer in customers {
                    let entries =
                        df_db::list_ar_entries_for_customer(&ctx.pool, customer.id).await?;
                    let allocations =
                        df_db::list_allocations_for_customer(&ctx.pool, customer.id).await?;
                    let domain_entries: Vec<df_ar::LedgerEntry> = entries
                        .iter()
                        .map(df_db::ledger_entry_to_domain)
                        .collect();
                    let domain_allocations: Vec<df_ar::Allocation> = allocations
                        .iter()
                        .map(df_db::allocation_to_domain)
                        .collect();
                    let open = df_ar::open_charges(&domain_entries, &domain_allocations);
                    let overdue = df_ar::overdue_balance(&open, period_end, ctx.currency);
                    if overdue.is_zero() {
                        continue;
                    }
                    let rate = customer
                        .finance_rate_pct_monthly
                        .unwrap_or(ctx.finance_charge_rate_pct);
                    let charge = df_ar::finance_charge_for(overdue, rate);
                    if charge.is_zero() {
                        continue;
                    }
                    let key = df_ar::statement_period_key(customer.id, period_start);
                    let already = df_db::find_ar_entry_by_reference(
                        &ctx.pool,
                        customer.id,
                        "finance_charge",
                        "statement_run",
                        key,
                    )
                    .await?;
                    if already.is_some() {
                        total_skipped += 1;
                        assessments.push(json!({
                            "customer_id": customer.id,
                            "account_number": customer.account_number,
                            "overdue_balance": overdue.amount().to_string(),
                            "finance_charge": charge.amount().to_string(),
                            "posted": false,
                            "reason": "already posted for this period",
                        }));
                        continue;
                    }
                    let entry = df_db::models::ArLedgerEntry {
                        id: uuid::Uuid::now_v7(),
                        org_id: ctx.org_id,
                        store_id: ctx.store_id,
                        customer_id: customer.id,
                        job_id: None,
                        entry_type: "finance_charge".to_string(),
                        reference_type: "statement_run".to_string(),
                        reference_id: key,
                        amount: charge.amount(),
                        due_date: Some(period_end),
                        statement_id: None,
                        created_at: chrono::Utc::now(),
                    };
                    df_db::insert_ar_entry(&ctx.pool, &entry).await?;
                    total_posted += charge.amount();
                    assessments.push(json!({
                        "customer_id": customer.id,
                        "account_number": customer.account_number,
                        "overdue_balance": overdue.amount().to_string(),
                        "finance_charge": charge.amount().to_string(),
                        "posted": true,
                        "entry_id": entry.id,
                    }));
                }
                let summary = format!(
                    "finance charges for {}–{}: {} account(s) assessed, {} posted ({}), {} skipped as already posted",
                    period_start,
                    period_end,
                    assessments.len(),
                    total_posted,
                    ctx.currency.code(),
                    total_skipped
                );
                Ok((
                    summary,
                    json!({
                        "period_start": period_start.to_string(),
                        "period_end": period_end.to_string(),
                        "assessments": assessments,
                    }),
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
    use df_db::models::{ArLedgerEntry, Customer, Organization, Store};
    use df_domain::Currency;
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use uuid::Uuid;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

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
        let customer_id = Uuid::from_u128(0x4000);
        df_db::insert_customer(
            &pool,
            &Customer {
                id: customer_id,
                org_id,
                account_number: "C-1042".to_string(),
                customer_type: "contractor".to_string(),
                company_name: Some("Harbourview Painting Ltd.".to_string()),
                first_name: None,
                last_name: None,
                email: None,
                phone: None,
                price_level_id: None,
                salesperson_id: None,
                tax_exempt_cert: None,
                tax_exempt_expires: None,
                charge_enabled: true,
                credit_limit: dec("5000.0000"),
                payment_terms: "NET30".to_string(),
                terms_days: None,
                finance_rate_pct_monthly: None,
                credit_hold: false,
                require_po_number: false,
                require_job_name: false,
                portal_enabled: false,
                address_line1: None,
                address_line2: None,
                city: None,
                province: None,
                postal_code: None,
                notes: None,
            },
        )
        .await
        .unwrap();
        // An October charge due Oct 15 (overdue on Nov 30: 46 days
        // past due) and a November charge due Dec 5 (not overdue).
        for (amount, due, created) in [
            ("500.0000", "2026-10-15", "2026-10-01T10:00:00Z"),
            ("1200.0000", "2026-12-05", "2026-11-05T10:00:00Z"),
        ] {
            df_db::insert_ar_entry(
                &pool,
                &ArLedgerEntry {
                    id: Uuid::now_v7(),
                    org_id,
                    store_id,
                    customer_id,
                    job_id: None,
                    entry_type: "charge".to_string(),
                    reference_type: "transaction".to_string(),
                    reference_id: Uuid::now_v7(),
                    amount: dec(amount),
                    due_date: Some(NaiveDate::from_str(due).unwrap()),
                    statement_id: None,
                    created_at: chrono::DateTime::parse_from_rfc3339(created)
                        .unwrap()
                        .with_timezone(&Utc),
                },
            )
            .await
            .unwrap();
        }
        let mut ctx = JobContext::new(pool, org_id, store_id, Currency::Usd);
        // Report on Dec 1: the previous month is November.
        ctx.report_date = Some(NaiveDate::from_str("2026-12-01").unwrap());
        ctx
    }

    #[tokio::test]
    async fn phase3_jobs_assess_and_render() {
        let runner = JobRunner::new();
        register_phase3_jobs(&runner);
        assert_eq!(runner.definitions().len(), 2);
        let ctx = seeded_ctx().await;
        // Run in this order on purpose: the finance charge the
        // second job posts is dated `Utc::now()` (Nov 26 on this
        // VM's clock), which falls inside the November statement
        // period — running statements first keeps the two jobs'
        // assertions independent. In production both run on the
        // 1st, so the posted charge lands in the *next* period.
        let statements = runner.run_now(&ctx, MONTHLY_STATEMENTS_JOB).await.unwrap();
        assert!(statements.success, "{statements:?}");
        assert!(statements.summary.contains("1 statement(s)"));
        let rendered = statements.output.as_ref().unwrap()["statements"]
            .as_array()
            .unwrap();
        assert_eq!(rendered.len(), 1);
        assert_eq!(rendered[0]["account_number"], "C-1042");
        assert_eq!(rendered[0]["closing_balance"], "1700.0000");
        assert!(rendered[0]["pdf_bytes"].as_u64().unwrap() > 5_000);
        assert_eq!(rendered[0]["pdf_sha256"].as_str().unwrap().len(), 64);
        let charges = runner.run_now(&ctx, FINANCE_CHARGES_JOB).await.unwrap();
        assert!(charges.success, "{charges:?}");
        // Only the $500 October charge (due Oct 15) is overdue on
        // Nov 30; at the default 1.5%/month that is a $7.50 charge,
        // posted to the ledger.
        assert!(charges.summary.contains("1 account(s) assessed"));
        let assessed = charges.output.as_ref().unwrap()["assessments"]
            .as_array()
            .unwrap();
        assert_eq!(assessed.len(), 1);
        assert_eq!(assessed[0]["overdue_balance"], "500.0000");
        assert_eq!(assessed[0]["finance_charge"], "7.5000");
        assert_eq!(assessed[0]["posted"], true);
        // The charge really landed in the ledger…
        let entries = df_db::list_ar_entries_for_customer(&ctx.pool, Uuid::from_u128(0x4000))
            .await
            .unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].entry_type, "finance_charge");
        assert_eq!(entries[2].amount, dec("7.5000"));
        // …and a second run of the same period posts nothing.
        let rerun = runner.run_now(&ctx, FINANCE_CHARGES_JOB).await.unwrap();
        assert!(rerun.success);
        let reassessed = rerun.output.as_ref().unwrap()["assessments"]
            .as_array()
            .unwrap();
        assert_eq!(reassessed.len(), 1);
        assert_eq!(reassessed[0]["posted"], false);
        let entries = df_db::list_ar_entries_for_customer(&ctx.pool, Uuid::from_u128(0x4000))
            .await
            .unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(runner.runs().len(), 3);
    }

    #[test]
    fn previous_month_wraps_the_year() {
        assert_eq!(
            previous_month(NaiveDate::from_str("2026-01-15").unwrap()),
            "2025-12"
        );
        assert_eq!(
            previous_month(NaiveDate::from_str("2026-12-01").unwrap()),
            "2026-11"
        );
    }
}
