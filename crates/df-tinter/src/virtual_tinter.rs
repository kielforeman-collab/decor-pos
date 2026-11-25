//! `VirtualTinter`: deterministic in-memory `TinterPlugin` simulator
//! (Phase 2, spec §7).
//!
//! Dispensing a formula computes each colorant's shot volumes with
//! `df-domain::TintColorant::total_fl_oz`, decrements the on-hand
//! levels, and records a `Done` job. If any colorant is unknown or
//! short the whole dispense fails *before* anything is decremented
//! and a `Failed` job is recorded instead. There is no randomness
//! and no wall-clock dependence in the dispense math — timestamps
//! only stamp the job record.

use std::collections::{HashMap, VecDeque};

use rust_decimal::Decimal;
use uuid::Uuid;

use crate::error::TinterError;
use crate::job::{
    ColorantLevel, DispenseRequest, DispensedFormula, DispensedShot, JobAck, TintJob,
};
use crate::plugin::TinterPlugin;

/// Deterministic in-memory tinter simulator.
#[derive(Debug, Default)]
pub struct VirtualTinter {
    name: String,
    dispensers: Vec<String>,
    /// Keyed by upper-cased colorant code.
    levels: HashMap<String, ColorantLevel>,
    jobs: Vec<TintJob>,
    completed: VecDeque<DispensedFormula>,
}

impl VirtualTinter {
    /// A simulator with a single dispenser named `dispenser_name`.
    pub fn new(dispenser_name: impl Into<String>) -> Self {
        let name = dispenser_name.into();
        VirtualTinter {
            name: name.clone(),
            dispensers: vec![name],
            levels: HashMap::new(),
            jobs: Vec::new(),
            completed: VecDeque::new(),
        }
    }

    /// A simulator with several dispensers (first is the default).
    pub fn with_dispensers(dispensers: Vec<String>) -> Self {
        let name = dispensers
            .first()
            .cloned()
            .unwrap_or_else(|| "virtual-simulator".to_string());
        VirtualTinter {
            name,
            dispensers,
            levels: HashMap::new(),
            jobs: Vec::new(),
            completed: VecDeque::new(),
        }
    }

    fn key(code: &str) -> String {
        code.to_ascii_uppercase()
    }

    /// Set (or replace) the on-hand level and low threshold for a
    /// colorant code. Codes are matched case-insensitively.
    pub fn set_level(
        &mut self,
        code: impl Into<String>,
        on_hand_fl_oz: Decimal,
        low_threshold_fl_oz: Decimal,
    ) {
        let level = ColorantLevel::new(code.into(), on_hand_fl_oz, low_threshold_fl_oz);
        self.levels.insert(Self::key(&level.code), level);
    }

    /// On-hand level for a colorant code, if configured.
    pub fn level(&self, code: &str) -> Option<&ColorantLevel> {
        self.levels.get(&Self::key(code))
    }

    /// All configured levels, sorted by code (deterministic order).
    pub fn levels(&self) -> Vec<&ColorantLevel> {
        let mut levels: Vec<&ColorantLevel> = self.levels.values().collect();
        levels.sort_by(|a, b| a.code.cmp(&b.code));
        levels
    }

    /// Colorants at or below their low-level threshold, sorted by
    /// code. Empty means no shortage alert is due.
    pub fn shortage_alerts(&self) -> Vec<&ColorantLevel> {
        self.levels()
            .into_iter()
            .filter(|level| level.is_low())
            .collect()
    }

    fn job_mut(&mut self, job_id: Uuid) -> Option<&mut TintJob> {
        self.jobs.iter_mut().find(|job| job.id == job_id)
    }

    /// Fail a recorded job with `error`, ignoring transition errors
    /// (a job that is already terminal stays as it is).
    fn fail_job(&mut self, job_id: Uuid, error: &str) {
        if let Some(job) = self.job_mut(job_id) {
            let _ = job.mark_failed(error);
        }
    }
}

impl TinterPlugin for VirtualTinter {
    fn name(&self) -> &str {
        &self.name
    }

    fn list_dispensers(&self) -> Vec<String> {
        self.dispensers.clone()
    }

    fn send_formula(&mut self, request: DispenseRequest) -> Result<JobAck, TinterError> {
        let dispenser = request
            .dispenser
            .clone()
            .unwrap_or_else(|| self.dispensers.first().cloned().unwrap_or_default());
        let job = TintJob::new(request.formula_id, request.transaction_id, request.line_no);
        let job_id = job.id;
        self.jobs.push(job);

        // Validate + aggregate first: duplicate lines for one code are
        // summed, so the shortage check and the decrement see the same
        // total and a dispense can never partially decrement.
        if request.colorants.is_empty() {
            let error = "formula has no colorants".to_string();
            self.fail_job(job_id, &error);
            return Err(TinterError::InvalidRequest(error));
        }
        let mut aggregated: Vec<(String, Decimal)> = Vec::new();
        for line in &request.colorants {
            let total = line.total_fl_oz()?;
            match aggregated.iter_mut().find(|(code, _)| *code == line.code) {
                Some((_, sum)) => *sum += total,
                None => aggregated.push((line.code.clone(), total)),
            }
        }
        for (code, needed) in &aggregated {
            let Some(on_hand) = self
                .levels
                .get(&Self::key(code))
                .map(|level| level.on_hand_fl_oz)
            else {
                let error = format!("unknown colorant code: {code}");
                self.fail_job(job_id, &error);
                return Err(TinterError::UnknownColorant(code.clone()));
            };
            if *needed > on_hand {
                let error = format!(
                    "insufficient colorant {code}: need {needed} fl oz, have {on_hand} fl oz"
                );
                self.fail_job(job_id, &error);
                return Err(TinterError::InsufficientColorant {
                    code: code.clone(),
                    needed_fl_oz: *needed,
                    on_hand_fl_oz: on_hand,
                });
            }
        }

        // All checks passed: decrement, then run the lifecycle.
        for (code, needed) in &aggregated {
            if let Some(level) = self.levels.get_mut(&Self::key(code)) {
                level.on_hand_fl_oz -= *needed;
            }
        }
        let dispensed: Vec<DispensedShot> = aggregated
            .iter()
            .map(|(code, fl_oz)| DispensedShot {
                code: code.clone(),
                fl_oz: *fl_oz,
            })
            .collect();
        let completed_at = {
            let stored = self.job_mut(job_id).expect("job just pushed");
            stored.mark_dispensing().expect("queued -> dispensing");
            stored
                .mark_done(dispensed.clone())
                .expect("dispensing -> done");
            stored.completed_at.expect("done stamps completed_at")
        };
        self.completed.push_back(DispensedFormula {
            job_id,
            formula_id: request.formula_id,
            dispensed,
            dispensed_at: completed_at,
        });
        Ok(JobAck {
            job_id,
            status: crate::job::TintJobStatus::Done,
            dispenser,
        })
    }

    fn job(&self, job_id: Uuid) -> Option<TintJob> {
        self.jobs.iter().find(|job| job.id == job_id).cloned()
    }

    fn jobs(&self) -> Vec<TintJob> {
        self.jobs.clone()
    }

    fn completed_dispenses(&self) -> Vec<DispensedFormula> {
        self.completed.iter().cloned().collect()
    }

    fn drain_completed(&mut self) -> Vec<DispensedFormula> {
        self.completed.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::tint::TintColorant;
    use df_domain::{Currency, Money};
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn colorant(code: &str, oz: &str, shots: u32) -> TintColorant {
        TintColorant {
            code: code.to_string(),
            ounces: dec(oz),
            shots,
            shots_per_fl_oz: 48,
            cost_per_fl_oz: Money::from_str_amount("0.4200", Currency::Usd).unwrap(),
        }
    }

    fn request(colorants: Vec<TintColorant>) -> DispenseRequest {
        DispenseRequest {
            formula_id: Uuid::now_v7(),
            base_product_id: Uuid::now_v7(),
            base_product_name: "Regal Select Interior Gallon".to_string(),
            container_volume_ml: 3785,
            colorants,
            transaction_id: Some(Uuid::now_v7()),
            line_no: Some(1),
            dispenser: None,
        }
    }

    fn tinter() -> VirtualTinter {
        let mut tinter = VirtualTinter::new("virtual-simulator");
        tinter.set_level("Y3", dec("10"), dec("2"));
        tinter.set_level("R1", dec("5"), dec("1"));
        tinter
    }

    #[test]
    fn dispense_decrements_levels_exactly() {
        let mut tinter = tinter();
        let req = request(vec![
            colorant("Y3", "0", 14), // 14/48 fl oz
            colorant("R1", "1", 0),  // 1 fl oz
        ]);
        let ack = tinter.send_formula(req).unwrap();
        assert_eq!(ack.status, crate::job::TintJobStatus::Done);
        assert_eq!(ack.dispenser, "virtual-simulator");
        // 10 - 14/48 = 461/48 = 9.604166... exact decimal.
        assert_eq!(
            tinter.level("Y3").unwrap().on_hand_fl_oz,
            dec("10") - dec("14") / dec("48")
        );
        assert_eq!(tinter.level("R1").unwrap().on_hand_fl_oz, dec("4"));
        let job = tinter.job(ack.job_id).unwrap();
        assert_eq!(job.status, crate::job::TintJobStatus::Done);
        assert_eq!(job.transaction_id, job.transaction_id);
        assert_eq!(job.line_no, Some(1));
        assert_eq!(job.dispensed.len(), 2);
        assert_eq!(job.dispensed[0].fl_oz, dec("14") / dec("48"));
    }

    #[test]
    fn dispense_is_case_insensitive_on_colorant_code() {
        let mut tinter = tinter();
        let req = request(vec![colorant("y3", "0", 48)]); // 1 fl oz
        tinter.send_formula(req).unwrap();
        assert_eq!(tinter.level("Y3").unwrap().on_hand_fl_oz, dec("9"));
    }

    #[test]
    fn duplicate_lines_for_one_code_sum_before_decrement() {
        let mut tinter = tinter();
        let req = request(vec![colorant("Y3", "0", 24), colorant("Y3", "0", 24)]);
        tinter.send_formula(req).unwrap();
        assert_eq!(tinter.level("Y3").unwrap().on_hand_fl_oz, dec("9"));
    }

    #[test]
    fn shortage_fails_without_decrementing_anything() {
        let mut tinter = tinter();
        let req = request(vec![
            colorant("Y3", "0", 14), // fine on its own
            colorant("R1", "6", 0),  // short: only 5 on hand
        ]);
        let err = tinter.send_formula(req).unwrap_err();
        assert!(matches!(
            err,
            TinterError::InsufficientColorant { ref code, .. } if code == "R1"
        ));
        // Atomic: Y3 untouched too.
        assert_eq!(tinter.level("Y3").unwrap().on_hand_fl_oz, dec("10"));
        assert_eq!(tinter.level("R1").unwrap().on_hand_fl_oz, dec("5"));
        let jobs = tinter.jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, crate::job::TintJobStatus::Failed);
        assert!(jobs[0].error.as_ref().unwrap().contains("R1"));
        assert!(tinter.completed_dispenses().is_empty());
    }

    #[test]
    fn unknown_colorant_fails_without_decrementing() {
        let mut tinter = tinter();
        let req = request(vec![colorant("ZZ", "1", 0)]);
        assert!(matches!(
            tinter.send_formula(req).unwrap_err(),
            TinterError::UnknownColorant(_)
        ));
        assert_eq!(tinter.level("Y3").unwrap().on_hand_fl_oz, dec("10"));
        assert_eq!(tinter.jobs()[0].status, crate::job::TintJobStatus::Failed);
    }

    #[test]
    fn empty_formula_fails() {
        let mut tinter = tinter();
        let req = request(vec![]);
        assert!(matches!(
            tinter.send_formula(req).unwrap_err(),
            TinterError::InvalidRequest(_)
        ));
        assert_eq!(tinter.jobs()[0].status, crate::job::TintJobStatus::Failed);
    }

    #[test]
    fn shortage_alerts_list_at_or_below_threshold() {
        let mut tinter = tinter();
        assert!(tinter.shortage_alerts().is_empty());
        tinter.set_level("BK", dec("1"), dec("1"));
        tinter.set_level("W1", dec("0.5"), dec("2"));
        let alerts = tinter.shortage_alerts();
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].code, "BK");
        assert_eq!(alerts[1].code, "W1");
        // Dispensing down to exactly the threshold trips the alert.
        let req = request(vec![colorant("Y3", "8", 0)]);
        tinter.send_formula(req).unwrap();
        assert_eq!(tinter.level("Y3").unwrap().on_hand_fl_oz, dec("2"));
        assert!(
            tinter
                .shortage_alerts()
                .iter()
                .any(|level| level.code == "Y3")
        );
    }

    #[test]
    fn completed_dispenses_drain_exactly_once() {
        let mut tinter = tinter();
        let req = request(vec![colorant("Y3", "0", 14)]);
        let ack = tinter.send_formula(req).unwrap();
        assert_eq!(tinter.completed_dispenses().len(), 1);
        assert_eq!(tinter.completed_dispenses()[0].job_id, ack.job_id);
        let drained = tinter.drain_completed();
        assert_eq!(drained.len(), 1);
        assert!(tinter.drain_completed().is_empty());
        // The snapshot reads the same pending queue, so after a drain
        // it is empty too — drain is the exactly-once observation.
        assert!(tinter.completed_dispenses().is_empty());
    }

    #[test]
    fn list_dispensers_and_name() {
        let tinter = VirtualTinter::with_dispensers(vec!["sim-a".to_string(), "sim-b".to_string()]);
        assert_eq!(tinter.name(), "sim-a");
        assert_eq!(tinter.list_dispensers(), vec!["sim-a", "sim-b"]);
    }
}
