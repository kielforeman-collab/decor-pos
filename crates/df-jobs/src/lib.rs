//! `df-jobs` — scheduled job runner (Phase 2, v0.3; spec §7).
//!
//! A [`JobRunner`] holds a registry of named jobs with schedules and
//! executes them, recording every run as a [`JobRun`] (job name,
//! `ran_at`, success/failure, summary, and the job's JSON output).
//! Runs are **returned and kept in the runner's in-memory history**
//! — never fire-and-forget with no observable output. Durable
//! persistence of run records needs a `job_runs` table, which does
//! not exist yet (Phase 3); until then the history lives as long as
//! the process does, and callers that need a durable copy should
//! persist the returned [`JobRun`] themselves.
//!
//! The runner is testable without wall-clock waiting:
//! [`JobRunner::run_now`] and [`JobRunner::run_all`] run jobs on
//! demand, [`JobRunner::run_due`] runs whatever is due at a caller
//! supplied instant, and the periodic tokio loop
//! ([`start_periodic`]) is a thin wrapper over `run_due`.
//!
//! Phase 2 daily jobs ([`register_phase2_jobs`], all `Daily` at
//! 06:00 UTC by default): the daily usage report, the low-stock
//! check, and the colorant shortage alerts. Each produces its report
//! via `df-reports` and returns it as the run's JSON output.
//!
//! Statements, finance charges, reminders and accounting sync are
//! Phase 3 jobs (spec §7) — not here.

pub mod phase2;
pub mod runner;

pub use phase2::register_phase2_jobs;
pub use runner::{
    JobContext, JobDefinition, JobError, JobRun, JobRunner, Schedule, start_periodic,
};
