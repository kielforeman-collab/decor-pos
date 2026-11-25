//! Tint job lifecycle and dispense request/ack types (Phase 2, spec §6.2).
//!
//! Lifecycle: `Queued -> Dispensing -> Done`, or `Queued/Dispensing ->
//! Failed`. Terminal states (`Done`, `Failed`) accept no further
//! transition. A job always carries its formula id, the sale /
//! transaction line reference (when the tint belongs to a sale), the
//! exact amounts dispensed, and the error when it failed — so a tinted
//! sale line can be traced to its dispense record.

use chrono::{DateTime, Utc};
use df_domain::new_id;
use df_domain::tint::TintColorant;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::TinterError;

/// Tint job status. Stored as text (`queued`, `dispensing`, `done`,
/// `failed`) in `df-db::tint_jobs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TintJobStatus {
    Queued,
    Dispensing,
    Done,
    Failed,
}

impl TintJobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TintJobStatus::Queued => "queued",
            TintJobStatus::Dispensing => "dispensing",
            TintJobStatus::Done => "done",
            TintJobStatus::Failed => "failed",
        }
    }

    pub fn from_db_str(text: &str) -> Option<Self> {
        match text {
            "queued" => Some(TintJobStatus::Queued),
            "dispensing" => Some(TintJobStatus::Dispensing),
            "done" => Some(TintJobStatus::Done),
            "failed" => Some(TintJobStatus::Failed),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, TintJobStatus::Done | TintJobStatus::Failed)
    }
}

impl std::fmt::Display for TintJobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One colorant actually dispensed, in exact fluid ounces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispensedShot {
    pub code: String,
    pub fl_oz: Decimal,
}

/// One tint job: the record of a single dispense.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TintJob {
    pub id: Uuid,
    pub formula_id: Uuid,
    pub transaction_id: Option<Uuid>,
    /// `transaction_lines.line_no` the tint belongs to, if any.
    pub line_no: Option<i32>,
    pub status: TintJobStatus,
    pub dispensed: Vec<DispensedShot>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl TintJob {
    pub fn new(formula_id: Uuid, transaction_id: Option<Uuid>, line_no: Option<i32>) -> Self {
        TintJob {
            id: new_id(),
            formula_id,
            transaction_id,
            line_no,
            status: TintJobStatus::Queued,
            dispensed: Vec::new(),
            error: None,
            created_at: Utc::now(),
            completed_at: None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    fn transition(&mut self, to: TintJobStatus) -> Result<(), TinterError> {
        let allowed = matches!(
            (self.status, to),
            (TintJobStatus::Queued, TintJobStatus::Dispensing)
                | (TintJobStatus::Queued, TintJobStatus::Failed)
                | (TintJobStatus::Dispensing, TintJobStatus::Done)
                | (TintJobStatus::Dispensing, TintJobStatus::Failed)
        );
        if !allowed {
            return Err(TinterError::InvalidTransition {
                from: self.status.as_str().to_string(),
                to: to.as_str().to_string(),
            });
        }
        self.status = to;
        if to.is_terminal() {
            self.completed_at = Some(Utc::now());
        }
        Ok(())
    }

    /// `Queued -> Dispensing`.
    pub fn mark_dispensing(&mut self) -> Result<(), TinterError> {
        self.transition(TintJobStatus::Dispensing)
    }

    /// `Dispensing -> Done`, recording the exact amounts dispensed.
    pub fn mark_done(&mut self, dispensed: Vec<DispensedShot>) -> Result<(), TinterError> {
        self.transition(TintJobStatus::Done)?;
        self.dispensed = dispensed;
        Ok(())
    }

    /// `Queued/Dispensing -> Failed`, recording the reason.
    pub fn mark_failed(&mut self, error: impl Into<String>) -> Result<(), TinterError> {
        self.transition(TintJobStatus::Failed)?;
        self.error = Some(error.into());
        Ok(())
    }
}

/// A request to dispense one tint formula (spec §6.2 `DispenseRequest`).
/// Colorant shots reuse `df-domain::TintColorant` — the same type the
/// formula-cost math (`df_domain::tint_cost`) runs on.
#[derive(Debug, Clone, PartialEq)]
pub struct DispenseRequest {
    pub formula_id: Uuid,
    pub base_product_id: Uuid,
    pub base_product_name: String,
    pub container_volume_ml: i32,
    pub colorants: Vec<TintColorant>,
    /// Sale the tint belongs to, if any.
    pub transaction_id: Option<Uuid>,
    /// `transaction_lines.line_no` the tint belongs to, if any.
    pub line_no: Option<i32>,
    /// Requested dispenser; `None` means the tinter's default.
    pub dispenser: Option<String>,
}

/// Acknowledgement returned when a formula is accepted for dispensing
/// (spec §6.2 `JobAck`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobAck {
    pub job_id: Uuid,
    pub status: TintJobStatus,
    pub dispenser: String,
}

/// A completed dispense, as observed through
/// [`crate::TinterPlugin::completed_dispenses`] /
/// [`crate::TinterPlugin::drain_completed`] (the in-process equivalent
/// of spec §6.2 `stream_dispensed_formulas()`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispensedFormula {
    pub job_id: Uuid,
    pub formula_id: Uuid,
    pub dispensed: Vec<DispensedShot>,
    pub dispensed_at: DateTime<Utc>,
}

/// On-hand colorant level for one colorant code (fluid ounces, `Decimal`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorantLevel {
    pub code: String,
    pub on_hand_fl_oz: Decimal,
    pub low_threshold_fl_oz: Decimal,
}

impl ColorantLevel {
    pub fn new(
        code: impl Into<String>,
        on_hand_fl_oz: Decimal,
        low_threshold_fl_oz: Decimal,
    ) -> Self {
        ColorantLevel {
            code: code.into(),
            on_hand_fl_oz,
            low_threshold_fl_oz,
        }
    }

    /// At or below the configured low-level threshold.
    pub fn is_low(&self) -> bool {
        self.on_hand_fl_oz <= self.low_threshold_fl_oz
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::{Currency, Money};

    fn colorant(code: &str) -> TintColorant {
        TintColorant {
            code: code.to_string(),
            ounces: Decimal::from_str_exact("0").unwrap(),
            shots: 14,
            shots_per_fl_oz: 48,
            cost_per_fl_oz: Money::from_str_amount("0.4200", Currency::Usd).unwrap(),
        }
    }

    #[test]
    fn job_lifecycle_happy_path() {
        let mut job = TintJob::new(Uuid::now_v7(), None, None);
        assert_eq!(job.status, TintJobStatus::Queued);
        assert!(!job.is_terminal());
        job.mark_dispensing().unwrap();
        assert_eq!(job.status, TintJobStatus::Dispensing);
        job.mark_done(vec![DispensedShot {
            code: "Y3".to_string(),
            fl_oz: colorant("Y3").total_fl_oz().unwrap(),
        }])
        .unwrap();
        assert_eq!(job.status, TintJobStatus::Done);
        assert!(job.is_terminal());
        assert!(job.completed_at.is_some());
        assert_eq!(job.dispensed.len(), 1);
    }

    #[test]
    fn job_lifecycle_failed_from_queued() {
        let mut job = TintJob::new(Uuid::now_v7(), None, None);
        job.mark_failed("short Y3").unwrap();
        assert_eq!(job.status, TintJobStatus::Failed);
        assert_eq!(job.error.as_deref(), Some("short Y3"));
        assert!(job.is_terminal());
    }

    #[test]
    fn terminal_jobs_reject_further_transitions() {
        let mut job = TintJob::new(Uuid::now_v7(), None, None);
        job.mark_dispensing().unwrap();
        job.mark_done(vec![]).unwrap();
        assert!(matches!(
            job.mark_dispensing(),
            Err(TinterError::InvalidTransition { .. })
        ));
        assert!(matches!(
            job.mark_failed("late"),
            Err(TinterError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn queued_job_cannot_finish_without_dispensing() {
        let mut job = TintJob::new(Uuid::now_v7(), None, None);
        assert!(matches!(
            job.mark_done(vec![]),
            Err(TinterError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn status_round_trips_through_text() {
        for status in [
            TintJobStatus::Queued,
            TintJobStatus::Dispensing,
            TintJobStatus::Done,
            TintJobStatus::Failed,
        ] {
            assert_eq!(TintJobStatus::from_db_str(status.as_str()), Some(status));
        }
        assert_eq!(TintJobStatus::from_db_str("bogus"), None);
    }

    #[test]
    fn low_threshold_is_inclusive() {
        let level = ColorantLevel::new("Y3", Decimal::from(2), Decimal::from(2));
        assert!(level.is_low());
        let level = ColorantLevel::new("Y3", Decimal::from(3), Decimal::from(2));
        assert!(!level.is_low());
    }
}
