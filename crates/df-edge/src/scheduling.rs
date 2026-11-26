//! Scheduling+ on the edge (Phase 6, v1.0; spec §6.1/§7):
//! shift templates, staff shifts, timeclock and the sales-driven
//! labor forecast + coverage check.
//!
//! The rules live in `df-scheduling` (template validation, shift
//! overlap, clock-in/out guards, forecasting, coverage) and the
//! persistence in `df-db::scheduling` (migration `0011_phase6`).
//! This module is the thin wiring layer: request validation, row
//! ↔ domain conversion at the boundary, and persistence calls.
//! It holds no scheduling rules of its own — duplicating them
//! here would be a second implementation of the sibling crate's
//! domain logic.
//!
//! All times are UTC in v1.0: shift spans, timeclock punches and
//! forecast hour buckets are recorded and compared in UTC (the
//! same convention as the mobile KPIs' UTC calendar day). Store-
//! local timezone rendering is a follow-up, not a silent
//! conversion.
//!
//! All hours are `rust_decimal::Decimal` — never floats.

use chrono::{DateTime, NaiveDate, Utc};
use df_scheduling::{
    CoverageCheck, ForecastReport, ShiftStatus, ShiftTemplate as DomainTemplate,
    StaffShift as DomainShift, TimeclockEntry as DomainEntry,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;
use df_db::models::{ShiftTemplate, StaffShift, TimeclockEntry};

// --- request types ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateShiftTemplateRequest {
    pub name: String,
    pub weekday: u8,
    pub starts_at: String,
    pub ends_at: String,
    pub role: Option<String>,
    pub staff_id: Option<Uuid>,
    pub required_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateStaffShiftRequest {
    pub staff_id: Uuid,
    pub shift_date: NaiveDate,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub template_id: Option<Uuid>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetShiftStatusRequest {
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockRequest {
    pub staff_id: Uuid,
}

// --- row ↔ domain conversion --------------------------------------------------

fn shift_status_from_str(value: &str) -> Result<ShiftStatus, EdgeError> {
    ShiftStatus::from_str(value).map_err(EdgeError::Scheduling)
}

fn shift_to_domain(row: &StaffShift) -> Result<DomainShift, EdgeError> {
    Ok(DomainShift {
        id: row.id,
        org_id: row.org_id,
        store_id: row.store_id,
        template_id: row.template_id,
        staff_id: row.staff_id,
        shift_date: row.shift_date,
        starts_at: row.starts_at,
        ends_at: row.ends_at,
        status: shift_status_from_str(&row.status)?,
        notes: row.notes.clone(),
    })
}

fn shift_to_row(domain: &DomainShift) -> StaffShift {
    StaffShift {
        id: domain.id,
        org_id: domain.org_id,
        store_id: domain.store_id,
        template_id: domain.template_id,
        staff_id: domain.staff_id,
        shift_date: domain.shift_date,
        starts_at: domain.starts_at,
        ends_at: domain.ends_at,
        status: domain.status.as_str().to_string(),
        notes: domain.notes.clone(),
    }
}

fn timeclock_to_domain(row: &TimeclockEntry) -> DomainEntry {
    DomainEntry {
        id: row.id,
        org_id: row.org_id,
        store_id: row.store_id,
        staff_id: row.staff_id,
        clock_in: row.clock_in,
        clock_out: row.clock_out,
        source: row.source.clone(),
    }
}

impl Edge {
    fn sched_org_id(&self) -> Uuid {
        self.config().org_id
    }

    fn sched_store_id(&self) -> Uuid {
        self.config().store_id
    }

    // --- shift templates ------------------------------------------------------

    /// Create a shift template. The template is validated by the
    /// `df-scheduling` rule (weekday, parsable times, end after
    /// start, at least one person required).
    pub async fn create_shift_template(
        &self,
        body: CreateShiftTemplateRequest,
    ) -> Result<ShiftTemplate, EdgeError> {
        if body.name.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "template name must not be empty".to_string(),
            ));
        }
        let domain = DomainTemplate {
            id: Uuid::now_v7(),
            org_id: self.sched_org_id(),
            store_id: self.sched_store_id(),
            name: body.name,
            weekday: body.weekday,
            starts_at: body.starts_at,
            ends_at: body.ends_at,
            role: body.role,
            staff_id: body.staff_id,
            required_count: body.required_count,
            is_active: true,
        };
        domain.validate()?;
        let row = ShiftTemplate {
            id: domain.id,
            org_id: domain.org_id,
            store_id: domain.store_id,
            name: domain.name,
            weekday: i32::from(domain.weekday),
            starts_at: domain.starts_at,
            ends_at: domain.ends_at,
            role: domain.role,
            staff_id: domain.staff_id,
            required_count: i32::try_from(domain.required_count)
                .map_err(|_| EdgeError::InvalidInput("required_count is too large".to_string()))?,
            is_active: domain.is_active,
        };
        df_db::insert_shift_template(self.pool(), &row).await?;
        Ok(row)
    }

    /// List this store's shift templates, weekday then start time.
    pub async fn list_shift_templates(&self) -> Result<Vec<ShiftTemplate>, EdgeError> {
        Ok(df_db::list_shift_templates_for_store(self.pool(), self.sched_store_id()).await?)
    }

    // --- staff shifts ---------------------------------------------------------

    /// Create a concrete shift. The window is validated and the
    /// overlap check (`df-scheduling`) refuses a clash with
    /// another active shift for the same staff member.
    pub async fn create_staff_shift(
        &self,
        body: CreateStaffShiftRequest,
    ) -> Result<StaffShift, EdgeError> {
        let domain = DomainShift {
            id: Uuid::now_v7(),
            org_id: self.sched_org_id(),
            store_id: self.sched_store_id(),
            template_id: body.template_id,
            staff_id: body.staff_id,
            shift_date: body.shift_date,
            starts_at: body.starts_at,
            ends_at: body.ends_at,
            status: ShiftStatus::Scheduled,
            notes: body.notes,
        };
        let existing = df_db::list_staff_shifts_for_store_date(
            self.pool(),
            self.sched_store_id(),
            body.shift_date,
        )
        .await?;
        let existing_domain: Vec<DomainShift> = existing
            .iter()
            .map(shift_to_domain)
            .collect::<Result<_, _>>()?;
        df_scheduling::check_overlap(&domain, &existing_domain)?;
        let row = shift_to_row(&domain);
        df_db::insert_staff_shift(self.pool(), &row).await?;
        Ok(row)
    }

    /// List this store's shifts for one date, start time first.
    pub async fn list_staff_shifts(&self, date: NaiveDate) -> Result<Vec<StaffShift>, EdgeError> {
        Ok(
            df_db::list_staff_shifts_for_store_date(self.pool(), self.sched_store_id(), date)
                .await?,
        )
    }

    /// Change a shift's status. Moving a cancelled shift back to
    /// active re-runs the overlap check, so a reactivated shift
    /// cannot silently double-book its staff member.
    pub async fn set_shift_status(&self, id: Uuid, status: &str) -> Result<StaffShift, EdgeError> {
        let parsed = shift_status_from_str(status)?;
        let mut row = df_db::get_staff_shift(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_db::DbError::NotFound(_) => EdgeError::NotFound(format!("staff shift {id}")),
                other => EdgeError::from(other),
            })?;
        if parsed.is_active() {
            let domain = DomainShift {
                status: parsed,
                ..shift_to_domain(&row)?
            };
            let existing = df_db::list_staff_shifts_for_store_date(
                self.pool(),
                self.sched_store_id(),
                row.shift_date,
            )
            .await?;
            let existing_domain: Vec<DomainShift> = existing
                .iter()
                .map(shift_to_domain)
                .collect::<Result<_, _>>()?;
            df_scheduling::check_overlap(&domain, &existing_domain)?;
        }
        df_db::set_staff_shift_status(self.pool(), id, parsed.as_str()).await?;
        row.status = parsed.as_str().to_string();
        Ok(row)
    }

    // --- timeclock ------------------------------------------------------------

    /// Clock in. Refuses when the staff member already has an open
    /// entry (`df-scheduling` guard).
    pub async fn clock_in(&self, staff_id: Uuid) -> Result<TimeclockEntry, EdgeError> {
        let open = df_db::get_open_timeclock_entry(self.pool(), staff_id).await?;
        let existing: Vec<DomainEntry> = open.iter().map(timeclock_to_domain).collect();
        let domain = df_scheduling::clock_in(
            self.sched_org_id(),
            self.sched_store_id(),
            staff_id,
            Utc::now(),
            "counter",
            &existing,
        )?;
        let row = TimeclockEntry {
            id: domain.id,
            org_id: domain.org_id,
            store_id: domain.store_id,
            staff_id: domain.staff_id,
            clock_in: domain.clock_in,
            clock_out: domain.clock_out,
            source: domain.source,
        };
        df_db::insert_timeclock_entry(self.pool(), &row).await?;
        Ok(row)
    }

    /// Clock out. Refuses with no open entry, or when now is at or
    /// before the clock-in (`df-scheduling` guard).
    pub async fn clock_out(&self, staff_id: Uuid) -> Result<TimeclockEntry, EdgeError> {
        let open = df_db::get_open_timeclock_entry(self.pool(), staff_id).await?;
        let mut existing: Vec<DomainEntry> = open.iter().map(timeclock_to_domain).collect();
        let closed = df_scheduling::clock_out(staff_id, Utc::now(), &mut existing)?;
        df_db::close_timeclock_entry(self.pool(), closed.id, closed.clock_out.unwrap()).await?;
        let mut row = open.expect("open entry existed for the domain close");
        row.clock_out = closed.clock_out;
        Ok(row)
    }

    /// Timeclock entries for one UTC date, newest first.
    pub async fn list_timeclock_entries(
        &self,
        date: NaiveDate,
    ) -> Result<Vec<TimeclockEntry>, EdgeError> {
        Ok(
            df_db::list_timeclock_entries_for_store_date(self.pool(), self.sched_store_id(), date)
                .await?,
        )
    }

    // --- forecast + coverage ----------------------------------------------------

    /// Labor forecast for `days` dates from `from`, from the last
    /// 28 days of completed sales + closed timeclock entries.
    /// `ratio_override` replaces the history-derived sales-per-
    /// labor-hour ratio when given. Insufficient history is a
    /// 400 with the reason, not an invented forecast.
    pub async fn labor_forecast(
        &self,
        from: NaiveDate,
        days: i64,
        ratio_override: Option<Decimal>,
    ) -> Result<ForecastReport, EdgeError> {
        if !(1..=31).contains(&days) {
            return Err(EdgeError::InvalidInput(
                "forecast days must be between 1 and 31".to_string(),
            ));
        }
        let window_end = from
            .and_hms_opt(0, 0, 0)
            .expect("midnight exists")
            .and_utc();
        let window_start = window_end - chrono::Duration::days(28);
        let transactions =
            df_db::list_transactions_for_store(self.pool(), self.sched_store_id()).await?;
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
            self.pool(),
            self.sched_store_id(),
            window_start,
            window_end,
        )
        .await?;
        let labor: Vec<df_scheduling::LaborPoint> = entries
            .iter()
            .filter_map(|entry| {
                let hours = timeclock_to_domain(entry).worked_hours()?;
                Some(df_scheduling::LaborPoint {
                    at: entry.clock_in,
                    hours,
                })
            })
            .collect();
        let targets: Vec<NaiveDate> = (0..days)
            .map(|offset| from + chrono::Duration::days(offset))
            .collect();
        Ok(df_scheduling::build_forecast(
            self.sched_store_id(),
            &sales,
            &labor,
            &targets,
            ratio_override,
        )?)
    }

    /// Coverage for one date: scheduled (active-shift) hours vs
    /// the forecast required hours. When history cannot support a
    /// forecast the check reports `Unknown` with the scheduled
    /// hours it could still measure.
    pub async fn labor_coverage(&self, date: NaiveDate) -> Result<CoverageCheck, EdgeError> {
        let forecast_hours = match self.labor_forecast(date, 1, None).await {
            Ok(report) => Some(report.days[0].required_hours),
            Err(EdgeError::Scheduling(df_scheduling::SchedulingError::InsufficientHistory(_))) => {
                None
            }
            Err(err) => return Err(err),
        };
        let rows = self.list_staff_shifts(date).await?;
        let domain: Vec<DomainShift> =
            rows.iter().map(shift_to_domain).collect::<Result<_, _>>()?;
        let scheduled = df_scheduling::scheduled_hours(&domain);
        Ok(df_scheduling::coverage_check(
            date,
            forecast_hours,
            scheduled,
        ))
    }
}

/// Parse a `Decimal` query value (the sales-per-labor-hour
/// override). A malformed value is an edge error, not a silent
/// default.
pub fn parse_ratio(value: &str) -> Result<Decimal, EdgeError> {
    Decimal::from_str(value).map_err(|_| {
        EdgeError::InvalidInput(format!(
            "sales_per_labor_hour must be a decimal, got {value:?}"
        ))
    })
}
