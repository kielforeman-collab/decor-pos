//! Job registry, schedules and the runner.
//!
//! A job is a named async function over a [`JobContext`] that
//! returns `(summary, output_json)` on success. The runner records
//! every execution as a [`JobRun`] and keeps the history in memory
//! (see the crate docs for why it is not yet persisted).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use df_domain::Currency;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Errors a job run can return.
#[derive(Debug, Error)]
pub enum JobError {
    #[error("database error: {0}")]
    Db(#[from] df_db::DbError),
    #[error("report error: {0}")]
    Report(#[from] df_reports::ReportError),
    #[error("unknown job: {0}")]
    UnknownJob(String),
    #[error("job failed: {0}")]
    Failed(String),
}

/// What a job runs against: the database pool plus the store/org it
/// reports on. `report_date` overrides the date daily jobs report
/// on (`None` means today, UTC) — the hook that makes the daily jobs
/// testable without waiting for tomorrow.
#[derive(Debug, Clone)]
pub struct JobContext {
    pub pool: df_db::AnyPool,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub currency: Currency,
    pub report_date: Option<NaiveDate>,
    /// Caller-supplied product thresholds for the low-stock job
    /// (see `df_reports::ProductThreshold` — there is no min-level
    /// table yet, so the operator supplies them).
    pub product_thresholds: Vec<df_reports::ProductThreshold>,
    /// Path to the `typst` binary the monthly-statements job renders
    /// PDFs with (`None` means `df-reports` resolves it: explicit
    /// path → `DF_TYPST_BIN` → `typst` on `PATH` → the developer
    /// install).
    pub typst_bin: Option<std::path::PathBuf>,
    /// Monthly finance-charge rate in percent (e.g. `1.5` for
    /// 1.5%/month) used by the `finance-charges` job. Operator-
    /// configured; the 1.5% default matches the rate the spec's
    /// finance-charge example uses.
    pub finance_charge_rate_pct: Decimal,
    /// Directory the `scheduled-report-email` job (Phase 6) writes
    /// rendered reports and `manifest.json` into (`None` means the
    /// job fails with a clear message — there is no default outbox).
    pub email_outbox_dir: Option<std::path::PathBuf>,
    /// Recipients the `scheduled-report-email` job queues reports
    /// for (one manifest entry per recipient per file). Empty
    /// means the job fails with a clear message.
    pub email_recipients: Vec<String>,
}

impl JobContext {
    pub fn new(pool: df_db::AnyPool, org_id: Uuid, store_id: Uuid, currency: Currency) -> Self {
        JobContext {
            pool,
            org_id,
            store_id,
            currency,
            report_date: None,
            product_thresholds: Vec::new(),
            typst_bin: None,
            finance_charge_rate_pct: Decimal::from_str_exact("1.5").unwrap(),
            email_outbox_dir: None,
            email_recipients: Vec::new(),
        }
    }

    /// The date a daily job reports on.
    pub fn report_date(&self) -> NaiveDate {
        self.report_date.unwrap_or_else(|| Utc::now().date_naive())
    }
}

/// When a job runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Schedule {
    /// Once a day at `hour:minute` UTC.
    Daily { hour: u8, minute: u8 },
    /// Once a month on `day` at `hour:minute` UTC. When the month is
    /// shorter than `day` (e.g. day 31 in February) the job runs on
    /// the month's last day.
    Monthly { day: u8, hour: u8, minute: u8 },
    /// Only when run explicitly (`run_now` / `run_all`).
    Manual,
}

/// A registered job's identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub schedule: Schedule,
}

/// The recorded outcome of one job execution. `output` is the job's
/// JSON result (a report document for the Phase 2 jobs); `error`
/// carries the failure message when `success` is false.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRun {
    pub id: Uuid,
    pub job_name: String,
    pub ran_at: DateTime<Utc>,
    pub success: bool,
    pub summary: String,
    pub error: Option<String>,
    pub output: Option<serde_json::Value>,
}

/// A job function: `(summary, output_json)` on success. The
/// returned future borrows the context (higher-ranked lifetime), so
/// a job can hold `&JobContext` across awaits without cloning the
/// pool handle into the closure.
pub type JobFn = Arc<
    dyn for<'a> Fn(
            &'a JobContext,
        ) -> Pin<
            Box<dyn Future<Output = Result<(String, serde_json::Value), JobError>> + Send + 'a>,
        > + Send
        + Sync,
>;

struct RegisteredJob {
    definition: JobDefinition,
    run: JobFn,
}

/// The job runner: registry + run history. Cheaply cloneable via
/// `Arc` internally? No — the runner itself is cheap to share by
/// reference; `start_periodic` takes an `Arc<JobRunner>`.
#[derive(Default)]
pub struct JobRunner {
    jobs: Mutex<HashMap<String, RegisteredJob>>,
    runs: Mutex<Vec<JobRun>>,
    last_run_at: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl JobRunner {
    pub fn new() -> Self {
        JobRunner::default()
    }

    /// Register a job (replaces any previous job of the same name).
    pub fn register(&self, definition: JobDefinition, run: JobFn) {
        self.jobs.lock().expect("jobs lock").insert(
            definition.name.to_string(),
            RegisteredJob { definition, run },
        );
    }

    /// Registered job definitions, sorted by name.
    pub fn definitions(&self) -> Vec<JobDefinition> {
        let jobs = self.jobs.lock().expect("jobs lock");
        let mut definitions: Vec<JobDefinition> =
            jobs.values().map(|job| job.definition.clone()).collect();
        definitions.sort_by(|a, b| a.name.cmp(b.name));
        definitions
    }

    /// All recorded runs, oldest first.
    pub fn runs(&self) -> Vec<JobRun> {
        self.runs.lock().expect("runs lock").clone()
    }

    /// The most recent run of one job, if any.
    pub fn last_run(&self, job_name: &str) -> Option<JobRun> {
        self.runs
            .lock()
            .expect("runs lock")
            .iter()
            .rev()
            .find(|run| run.job_name == job_name)
            .cloned()
    }

    fn record(&self, run: JobRun) {
        self.last_run_at
            .lock()
            .expect("last-run lock")
            .insert(run.job_name.clone(), run.ran_at);
        self.runs.lock().expect("runs lock").push(run);
    }

    /// Run one job by name, now, and record the run. Returns the
    /// recorded [`JobRun`] (success or failure — a failing job
    /// records a failed run rather than erroring the caller, so one
    /// bad job never takes down a batch; `UnknownJob` is the only
    /// error, for a name that was never registered).
    pub async fn run_now(&self, ctx: &JobContext, job_name: &str) -> Result<JobRun, JobError> {
        let run_fn = {
            let jobs = self.jobs.lock().expect("jobs lock");
            match jobs.get(job_name) {
                Some(job) => job.run.clone(),
                None => return Err(JobError::UnknownJob(job_name.to_string())),
            }
        };
        let ran_at = Utc::now();
        let run = match run_fn(ctx).await {
            Ok((summary, output)) => JobRun {
                id: Uuid::now_v7(),
                job_name: job_name.to_string(),
                ran_at,
                success: true,
                summary,
                error: None,
                output: Some(output),
            },
            Err(err) => JobRun {
                id: Uuid::now_v7(),
                job_name: job_name.to_string(),
                ran_at,
                success: false,
                summary: format!("{job_name} failed"),
                error: Some(err.to_string()),
                output: None,
            },
        };
        self.record(run.clone());
        Ok(run)
    }

    /// Run every registered job, in name order, and return the runs.
    pub async fn run_all(&self, ctx: &JobContext) -> Vec<JobRun> {
        let mut runs = Vec::new();
        for definition in self.definitions() {
            // Names come from the registry, so `UnknownJob` cannot
            // happen here.
            if let Ok(run) = self.run_now(ctx, definition.name).await {
                runs.push(run);
            }
        }
        runs
    }

    /// Run the jobs that are due at `now`: a `Daily` job is due when
    /// `now` is at or past today's scheduled time and the job has not
    /// run since that time (or has never run); a `Monthly` job is due
    /// when `now` is at or past this month's scheduled day/time and
    /// has not run since then. `Manual` jobs are never due. Pure
    /// function of `now` + run history — no wall-clock waiting, so
    /// tests drive it with fixed instants.
    pub async fn run_due(&self, ctx: &JobContext, now: DateTime<Utc>) -> Vec<JobRun> {
        let mut runs = Vec::new();
        for definition in self.definitions() {
            let scheduled = match definition.schedule {
                Schedule::Daily { hour, minute } => {
                    let Some(scheduled_today) = now
                        .date_naive()
                        .and_hms_opt(hour.into(), minute.into(), 0)
                        .map(|naive| naive.and_utc())
                    else {
                        continue;
                    };
                    scheduled_today
                }
                Schedule::Monthly { day, hour, minute } => {
                    let Some(scheduled_this_month) = monthly_scheduled(now, day, hour, minute)
                    else {
                        continue;
                    };
                    scheduled_this_month
                }
                Schedule::Manual => continue,
            };
            if now < scheduled {
                continue;
            }
            let last = self
                .last_run_at
                .lock()
                .expect("last-run lock")
                .get(definition.name)
                .copied();
            if last.is_some_and(|last| last >= scheduled) {
                continue;
            }
            if let Ok(run) = self.run_now(ctx, definition.name).await {
                runs.push(run);
            }
        }
        runs
    }
}

/// The scheduled instant for a monthly job in `now`'s month:
/// `day` at `hour:minute` UTC, clamped to the month's last day.
fn monthly_scheduled(now: DateTime<Utc>, day: u8, hour: u8, minute: u8) -> Option<DateTime<Utc>> {
    use chrono::Datelike;
    let date = now.date_naive();
    let days_in_month = {
        let (year, month) = (date.year(), date.month());
        let (next_year, next_month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        NaiveDate::from_ymd_opt(next_year, next_month, 1)
            .unwrap()
            .signed_duration_since(NaiveDate::from_ymd_opt(year, month, 1).unwrap())
            .num_days() as u8
    };
    let day = day.min(days_in_month);
    date.with_day(day.into())
        .and_then(|d| d.and_hms_opt(hour.into(), minute.into(), 0))
        .map(|naive| naive.and_utc())
}

/// Run `run_due(Utc::now())` every `check_every`, forever, on a
/// spawned task. Thin wrapper — all the logic is in `run_due`, which
/// is what tests exercise. Returns the task handle; abort it to stop.
pub fn start_periodic(
    runner: Arc<JobRunner>,
    ctx: JobContext,
    check_every: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(check_every);
        loop {
            ticker.tick().await;
            let _ = runner.run_due(&ctx, Utc::now()).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ctx() -> JobContext {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        JobContext::new(pool, Uuid::now_v7(), Uuid::now_v7(), Currency::Usd)
    }

    fn ok_job(summary: &'static str) -> JobFn {
        Arc::new(move |_ctx: &JobContext| {
            Box::pin(async move { Ok((summary.to_string(), serde_json::json!({"job": summary}))) })
        })
    }

    fn failing_job() -> JobFn {
        Arc::new(|_ctx: &JobContext| {
            Box::pin(async move {
                Err::<(String, serde_json::Value), JobError>(JobError::Failed("boom".to_string()))
            })
        })
    }

    #[tokio::test]
    async fn run_now_records_success_and_failure() {
        let runner = JobRunner::new();
        runner.register(
            JobDefinition {
                name: "ok-job",
                description: "succeeds",
                schedule: Schedule::Manual,
            },
            ok_job("did the thing"),
        );
        runner.register(
            JobDefinition {
                name: "bad-job",
                description: "fails",
                schedule: Schedule::Manual,
            },
            failing_job(),
        );
        let ctx = ctx().await;
        let run = runner.run_now(&ctx, "ok-job").await.unwrap();
        assert!(run.success);
        assert_eq!(run.summary, "did the thing");
        assert_eq!(
            run.output,
            Some(serde_json::json!({"job": "did the thing"}))
        );
        assert!(run.error.is_none());
        let run = runner.run_now(&ctx, "bad-job").await.unwrap();
        assert!(!run.success);
        assert_eq!(run.error.as_deref(), Some("job failed: boom"));
        assert!(run.output.is_none());
        // Unknown names error instead of recording a run.
        assert!(matches!(
            runner.run_now(&ctx, "nope").await,
            Err(JobError::UnknownJob(_))
        ));
        assert_eq!(runner.runs().len(), 2);
        assert_eq!(runner.last_run("bad-job").unwrap().job_name, "bad-job");
    }

    #[tokio::test]
    async fn run_due_fires_once_per_day_at_the_scheduled_time() {
        let runner = JobRunner::new();
        runner.register(
            JobDefinition {
                name: "daily-job",
                description: "daily",
                schedule: Schedule::Daily { hour: 6, minute: 0 },
            },
            ok_job("daily ran"),
        );
        runner.register(
            JobDefinition {
                name: "manual-job",
                description: "manual",
                schedule: Schedule::Manual,
            },
            ok_job("manual ran"),
        );
        let ctx = ctx().await;
        let day = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        // Before 06:00 — nothing due.
        let before = day.and_hms_opt(5, 59, 0).unwrap().and_utc();
        assert!(runner.run_due(&ctx, before).await.is_empty());
        // At 06:00 — the daily job fires, the manual one never does.
        let at = day.and_hms_opt(6, 0, 0).unwrap().and_utc();
        let runs = runner.run_due(&ctx, at).await;
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].job_name, "daily-job");
        // Later the same day — already ran, not due again.
        let later = day.and_hms_opt(18, 0, 0).unwrap().and_utc();
        assert!(runner.run_due(&ctx, later).await.is_empty());
        // Next day at 06:00 — due again.
        let next = (day + chrono::Duration::days(1))
            .and_hms_opt(6, 0, 0)
            .unwrap()
            .and_utc();
        assert_eq!(runner.run_due(&ctx, next).await.len(), 1);
        assert_eq!(runner.runs().len(), 2);
    }

    #[tokio::test]
    async fn run_due_fires_monthly_job_once_per_month() {
        let runner = JobRunner::new();
        runner.register(
            JobDefinition {
                name: "monthly-job",
                description: "monthly",
                schedule: Schedule::Monthly {
                    day: 1,
                    hour: 6,
                    minute: 0,
                },
            },
            ok_job("monthly ran"),
        );
        let ctx = ctx().await;
        // Nov 30 — not due yet (this month's 1st already passed and
        // the job has never run, so it IS due: the schedule is "the
        // 1st of this month", and Nov 30 is past it).
        let nov30 = NaiveDate::from_ymd_opt(2026, 11, 30)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc();
        let runs = runner.run_due(&ctx, nov30).await;
        assert_eq!(runs.len(), 1);
        // Later in November — already ran this month.
        let later = NaiveDate::from_ymd_opt(2026, 11, 30)
            .unwrap()
            .and_hms_opt(18, 0, 0)
            .unwrap()
            .and_utc();
        assert!(runner.run_due(&ctx, later).await.is_empty());
        // Dec 1 before 06:00 — December's run is not due yet.
        let dec1_early = NaiveDate::from_ymd_opt(2026, 12, 1)
            .unwrap()
            .and_hms_opt(5, 59, 0)
            .unwrap()
            .and_utc();
        assert!(runner.run_due(&ctx, dec1_early).await.is_empty());
        // Dec 1 at 06:00 — due again.
        let dec1 = NaiveDate::from_ymd_opt(2026, 12, 1)
            .unwrap()
            .and_hms_opt(6, 0, 0)
            .unwrap()
            .and_utc();
        assert_eq!(runner.run_due(&ctx, dec1).await.len(), 1);
        assert_eq!(runner.runs().len(), 2);
    }

    #[tokio::test]
    async fn monthly_schedule_clamps_to_short_months() {
        let runner = JobRunner::new();
        runner.register(
            JobDefinition {
                name: "month-end-job",
                description: "day 31",
                schedule: Schedule::Monthly {
                    day: 31,
                    hour: 6,
                    minute: 0,
                },
            },
            ok_job("month-end ran"),
        );
        let ctx = ctx().await;
        // February 2027 has 28 days: the day-31 job is due on Feb 28.
        let feb27 = NaiveDate::from_ymd_opt(2027, 2, 27)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc();
        assert!(runner.run_due(&ctx, feb27).await.is_empty());
        let feb28 = NaiveDate::from_ymd_opt(2027, 2, 28)
            .unwrap()
            .and_hms_opt(6, 0, 0)
            .unwrap()
            .and_utc();
        assert_eq!(runner.run_due(&ctx, feb28).await.len(), 1);
    }

    #[tokio::test]
    async fn run_all_runs_every_job_in_name_order() {
        let runner = JobRunner::new();
        runner.register(
            JobDefinition {
                name: "b-job",
                description: "",
                schedule: Schedule::Manual,
            },
            ok_job("b"),
        );
        runner.register(
            JobDefinition {
                name: "a-job",
                description: "",
                schedule: Schedule::Manual,
            },
            ok_job("a"),
        );
        let runs = runner.run_all(&ctx().await).await;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].job_name, "a-job");
        assert_eq!(runs[1].job_name, "b-job");
    }
}
