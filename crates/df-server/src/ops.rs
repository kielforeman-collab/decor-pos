//! Operational CLI subcommands: reports, scheduled jobs, monthly statements,
//! and accounting exports.

use std::path::PathBuf;
use std::str::FromStr;

use chrono::{Datelike, NaiveDate};
use df_jobs::{
    JobContext, JobRunner, register_phase2_jobs, register_phase3_jobs, register_phase6_jobs,
};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::db::{open_pool, parse_date, store_currency};

pub(crate) async fn run_reports(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    kind: String,
    date: Option<String>,
    currency_flag: Option<String>,
) -> ! {
    let date = match parse_date(date) {
        Ok(date) => date,
        Err(err) => {
            eprintln!("df-server reports: {err}");
            std::process::exit(2);
        }
    };
    let pool = open_pool(&db).await;
    let value = match kind.as_str() {
        "daily-usage" => {
            let currency = store_currency(&pool, store_id, currency_flag).await;
            match df_reports::daily_usage_report(&pool, org_id, store_id, date, currency).await {
                Ok(report) => serde_json::to_value(report).expect("report serialises"),
                Err(err) => {
                    eprintln!("df-server reports: {err}");
                    std::process::exit(1);
                }
            }
        }
        "low-stock" => {
            eprintln!(
                "df-server reports: low-stock without product thresholds — products will be empty (no min-level table yet)"
            );
            match df_reports::low_stock_report(&pool, store_id, date, &[]).await {
                Ok(report) => serde_json::to_value(report).expect("report serialises"),
                Err(err) => {
                    eprintln!("df-server reports: {err}");
                    std::process::exit(1);
                }
            }
        }
        "kpi" => {
            let currency = store_currency(&pool, store_id, currency_flag).await;
            match df_reports::kpi_dashboard(&pool, org_id, store_id, date, date, date, currency)
                .await
            {
                Ok(report) => serde_json::to_value(report).expect("report serialises"),
                Err(err) => {
                    eprintln!("df-server reports: {err}");
                    std::process::exit(1);
                }
            }
        }
        "eco" => {
            let currency = store_currency(&pool, store_id, currency_flag).await;
            let month_start =
                chrono::NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap();
            match df_reports::eco_remittance_report(&pool, org_id, month_start, date, currency)
                .await
            {
                Ok(report) => serde_json::to_value(report).expect("report serialises"),
                Err(err) => {
                    eprintln!("df-server reports: {err}");
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!(
                "df-server reports: unknown --kind {other:?} (daily-usage | low-stock | kpi | eco)"
            );
            std::process::exit(2);
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("report serialises")
    );
    std::process::exit(0);
}

/// Arguments for `jobs run` (a struct so `run_jobs` stays under
/// clippy's argument limit).
pub(crate) struct JobsRunArgs {
    pub(crate) db: String,
    pub(crate) org_id: Uuid,
    pub(crate) store_id: Uuid,
    pub(crate) job: Option<String>,
    pub(crate) date: Option<String>,
    pub(crate) currency_flag: Option<String>,
    pub(crate) typst_bin: Option<PathBuf>,
    pub(crate) finance_rate: Option<String>,
    pub(crate) email_outbox: Option<PathBuf>,
    pub(crate) email_recipients: Vec<String>,
}

pub(crate) async fn run_jobs(args: JobsRunArgs) -> ! {
    let JobsRunArgs {
        db,
        org_id,
        store_id,
        job,
        date,
        currency_flag,
        typst_bin,
        finance_rate,
        email_outbox,
        email_recipients,
    } = args;
    let report_date = match parse_date(date) {
        Ok(date) => date,
        Err(err) => {
            eprintln!("df-server jobs run: {err}");
            std::process::exit(2);
        }
    };
    let finance_rate = match finance_rate {
        Some(text) => match Decimal::from_str(&text) {
            Ok(rate) => rate,
            Err(_) => {
                eprintln!(
                    "df-server jobs run: --finance-rate must be a decimal percent, got {text:?}"
                );
                std::process::exit(2);
            }
        },
        None => Decimal::from_str("1.5").unwrap(),
    };
    let pool = open_pool(&db).await;
    let currency = store_currency(&pool, store_id, currency_flag).await;
    let mut ctx = JobContext::new(pool, org_id, store_id, currency);
    ctx.report_date = Some(report_date);
    ctx.typst_bin = typst_bin;
    ctx.finance_charge_rate_pct = finance_rate;
    ctx.email_outbox_dir = email_outbox;
    ctx.email_recipients = email_recipients;
    let runner = JobRunner::new();
    register_phase2_jobs(&runner);
    register_phase3_jobs(&runner);
    register_phase6_jobs(&runner);
    let runs = match job.as_deref() {
        Some(name) => match runner.run_now(&ctx, name).await {
            Ok(run) => vec![run],
            Err(err) => {
                eprintln!("df-server jobs run: {err}");
                std::process::exit(2);
            }
        },
        None => runner.run_all(&ctx).await,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&runs).expect("runs serialise")
    );
    if runs.iter().any(|run| !run.success) {
        std::process::exit(1);
    }
    std::process::exit(0);
}

pub(crate) fn default_period() -> String {
    let today = chrono::Utc::now().date_naive();
    let (year, month) = if today.month() == 1 {
        (today.year() - 1, 12)
    } else {
        (today.year(), today.month() - 1)
    };
    format!("{year:04}-{month:02}")
}

pub(crate) async fn run_statements(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    period: Option<String>,
    out: Option<PathBuf>,
    typst_bin: Option<PathBuf>,
) -> ! {
    let period = period.unwrap_or_else(default_period);
    if NaiveDate::parse_from_str(&format!("{period}-01"), "%Y-%m-%d").is_err() {
        eprintln!("df-server statements run: --period must be YYYY-MM, got {period:?}");
        std::process::exit(2);
    }
    let pool = open_pool(&db).await;
    let currency = store_currency(&pool, store_id, None).await;
    let (org_name, store_name) = match (
        df_db::get_organization(&pool, org_id).await,
        df_db::get_store(&pool, store_id).await,
    ) {
        (Ok(org), Ok(store)) => (org.name, store.name),
        (Err(err), _) | (_, Err(err)) => {
            eprintln!("df-server statements run: cannot read org/store: {err}");
            std::process::exit(1);
        }
    };
    let statements =
        match df_reports::build_statements(&pool, org_id, store_id, &period, currency).await {
            Ok(statements) => statements,
            Err(err) => {
                eprintln!("df-server statements run: {err}");
                std::process::exit(1);
            }
        };
    let out_dir = out.unwrap_or_else(|| PathBuf::from(format!("statements-{period}")));
    if let Err(err) = std::fs::create_dir_all(&out_dir) {
        eprintln!(
            "df-server statements run: cannot create {}: {err}",
            out_dir.display()
        );
        std::process::exit(1);
    }
    let mut summaries = Vec::new();
    let mut failed = false;
    for statement in &statements {
        // Freeze the statement row first (idempotent on re-run),
        // then render the PDF and record its hash on the row.
        let row_id = match df_reports::persist_statement(&pool, statement).await {
            Ok(id) => id,
            Err(err) => {
                eprintln!(
                    "df-server statements run: cannot persist statement {}: {err}",
                    statement.account_number
                );
                failed = true;
                continue;
            }
        };
        let customer = match df_db::get_customer(&pool, statement.customer_id).await {
            Ok(customer) => customer,
            Err(err) => {
                eprintln!(
                    "df-server statements run: cannot read customer {}: {err}",
                    statement.customer_id
                );
                failed = true;
                continue;
            }
        };
        let header =
            df_reports::header_for(&customer, &org_name, &store_name, statement.period_end);
        let file_name = format!("{}-{}.pdf", statement.account_number, period);
        let path = out_dir.join(&file_name);
        match df_reports::render_statement_pdf(statement, &header, typst_bin.as_deref()) {
            Ok(pdf) => {
                let sha256 = df_reports::statement_pdf_sha256(&pdf);
                if let Err(err) = std::fs::write(&path, &pdf) {
                    eprintln!(
                        "df-server statements run: cannot write {}: {err}",
                        path.display()
                    );
                    failed = true;
                    continue;
                }
                if let Err(err) = df_db::set_statement_pdf_sha256(&pool, row_id, &sha256).await {
                    eprintln!(
                        "df-server statements run: cannot record PDF hash for {}: {err}",
                        statement.account_number
                    );
                    failed = true;
                    continue;
                }
                summaries.push(serde_json::json!({
                    "account_number": statement.account_number,
                    "customer_name": header.customer_name,
                    "closing_balance": statement.closing_balance.amount().to_string(),
                    "statement_row_id": row_id,
                    "pdf_path": path.to_string_lossy(),
                    "pdf_bytes": pdf.len(),
                    "pdf_sha256": sha256,
                }));
            }
            Err(err) => {
                eprintln!(
                    "df-server statements run: statement {} failed to render: {err}",
                    statement.account_number
                );
                failed = true;
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "period": period,
            "statements": summaries,
        }))
        .expect("summaries serialise")
    );
    std::process::exit(i32::from(failed));
}

pub(crate) async fn run_accounting_export(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    kind: String,
    ledger: String,
    out: PathBuf,
) -> ! {
    let _ = org_id;
    let pool = open_pool(&db).await;
    let summary = match (kind.as_str(), ledger.as_str()) {
        ("csv", "ar") => match df_accounting::load_ar_entries_for_store(&pool, store_id).await {
            Ok(entries) => {
                let plugin = df_accounting::CsvAccountingPlugin;
                match df_accounting::AccountingPlugin::export_ar_ledger(&plugin, &entries, &out) {
                    Ok(summary) => summary,
                    Err(err) => {
                        eprintln!("df-server accounting export: {err}");
                        std::process::exit(1);
                    }
                }
            }
            Err(err) => {
                eprintln!("df-server accounting export: {err}");
                std::process::exit(1);
            }
        },
        ("csv", "inventory") => {
            match df_accounting::load_inventory_entries_for_store(&pool, store_id).await {
                Ok(entries) => {
                    let plugin = df_accounting::CsvAccountingPlugin;
                    match df_accounting::AccountingPlugin::export_inventory_ledger(
                        &plugin, &entries, &out,
                    ) {
                        Ok(summary) => summary,
                        Err(err) => {
                            eprintln!("df-server accounting export: {err}");
                            std::process::exit(1);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("df-server accounting export: {err}");
                    std::process::exit(1);
                }
            }
        }
        ("iif", "ar") => match df_accounting::load_ar_entries_for_store(&pool, store_id).await {
            Ok(entries) => {
                let plugin = df_accounting::IifAccountingPlugin::default();
                match df_accounting::AccountingPlugin::export_ar_ledger(&plugin, &entries, &out) {
                    Ok(summary) => summary,
                    Err(err) => {
                        eprintln!("df-server accounting export: {err}");
                        std::process::exit(1);
                    }
                }
            }
            Err(err) => {
                eprintln!("df-server accounting export: {err}");
                std::process::exit(1);
            }
        },
        ("iif", "inventory") => {
            match df_accounting::load_inventory_entries_for_store(&pool, store_id).await {
                Ok(entries) => {
                    let plugin = df_accounting::IifAccountingPlugin::default();
                    match df_accounting::AccountingPlugin::export_inventory_ledger(
                        &plugin, &entries, &out,
                    ) {
                        Ok(summary) => summary,
                        Err(err) => {
                            eprintln!("df-server accounting export: {err}");
                            std::process::exit(1);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("df-server accounting export: {err}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!(
                "df-server accounting export: unknown --kind {kind:?} / --ledger {ledger:?} (csv|iif, ar|inventory)"
            );
            std::process::exit(2);
        }
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "kind": kind,
            "ledger": ledger,
            "rows": summary.rows,
            "bytes": summary.bytes,
            "path": summary.path.to_string_lossy(),
        }))
        .expect("summary serialises")
    );
    std::process::exit(0);
}
