//! CRM on the edge (Phase 4, v0.5; spec §6.1/§7): activity log,
//! reminder tickler, appointments, saved search views and the
//! lapsed-contractor report.
//!
//! The rules live in `df-crm` (pure domain: activity types,
//! reminder due/complete/snooze, appointment window + overlap,
//! saved-view filter dialects, lapsed classification) and in
//! `df-db::crm_reports` (the repositories that apply those rules
//! to the migration `0006_phase4` / `0007_phase4_crm_delivery`
//! tables). This module is the thin wiring layer: request
//! validation, row ↔ domain conversion at the boundary, and
//! persistence calls. It holds no CRM rules of its own —
//! duplicating them here would be a second implementation of the
//! sibling crates' domain logic.
//!
//! Two saved-view dialects exist (see `df-crm::saved_view`):
//! the **activity** dialect (`activity_type`, `customer_id`,
//! `completed`, `due_before`) filters the store's activity log,
//! and the **customer** dialect (`customer_type`,
//! `charge_enabled`, `min_balance`, `last_purchase_before_days`,
//! …) segments customers. `run_saved_view` runs the activity
//! dialect; `run_saved_view_customers` runs the customer dialect
//! (a view stored in the activity dialect is rejected there
//! rather than silently misread).
//!
//! All money is `rust_decimal::Decimal` — never floats.

use chrono::{DateTime, NaiveDate, Utc};
use df_crm::{
    ActivityType, Appointment as DomainAppointment, AppointmentStatus, LapsedContractor,
    saved_view::{ActivityFilter, ActivitySnapshot},
};
use df_db::models::{Appointment, CrmActivity, CrmSavedView};
use df_domain::Money;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

// --- request types ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateActivityRequest {
    pub customer_id: Option<Uuid>,
    pub activity_type: String,
    pub subject: String,
    pub notes: Option<String>,
    pub created_by: Option<Uuid>,
    pub due_at: Option<DateTime<Utc>>,
}

/// Partial activity update. `None` means "leave unchanged";
/// `Some(None)` on `notes` / `due_at` clears the field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateActivityRequest {
    #[serde(default)]
    pub customer_id: Option<Uuid>,
    #[serde(default)]
    pub activity_type: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub notes: Option<Option<String>>,
    #[serde(default)]
    pub due_at: Option<Option<DateTime<Utc>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnoozeActivityRequest {
    pub snoozed_until: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateAppointmentRequest {
    pub customer_id: Option<Uuid>,
    pub staff_id: Option<Uuid>,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub notes: Option<String>,
}

/// Partial appointment update. `None` means "leave unchanged";
/// `Some(None)` on `notes` clears it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateAppointmentRequest {
    #[serde(default)]
    pub customer_id: Option<Uuid>,
    #[serde(default)]
    pub staff_id: Option<Uuid>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ends_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub notes: Option<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateSavedViewRequest {
    pub name: String,
    pub filter_json: Value,
}

/// One customer matching a saved customer-segment view, with the
/// facts the filter evaluated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedViewCustomer {
    pub customer_id: Uuid,
    pub account_number: String,
    pub company_name: Option<String>,
    pub customer_type: String,
    pub charge_enabled: bool,
    pub credit_hold: bool,
    pub balance: Money,
    pub last_purchase_at: Option<DateTime<Utc>>,
}

/// The lapsed-contractor report: charge contractors with no
/// completed sale in the last `inactive_days` days, plus — when
/// `include_never_purchased` is set — contractors who never
/// bought, as a distinct status (see `df-crm::lapsed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapsedReport {
    pub store_id: Uuid,
    pub as_of: NaiveDate,
    pub inactive_days: i64,
    pub include_never_purchased: bool,
    pub customers: Vec<LapsedContractor>,
}

// --- helpers ----------------------------------------------------------------

fn validate_subject(subject: &str) -> Result<(), EdgeError> {
    if subject.trim().is_empty() {
        Err(EdgeError::InvalidInput(
            "subject must not be empty".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn activity_type_from_str(value: &str) -> Result<ActivityType, EdgeError> {
    ActivityType::from_str(value).ok_or_else(|| {
        EdgeError::InvalidInput(format!(
            "activity_type must be one of call, email, visit, quote, note, follow_up — got {value:?}"
        ))
    })
}

fn appointment_status_from_str(value: &str) -> Result<AppointmentStatus, EdgeError> {
    AppointmentStatus::from_str(value).ok_or_else(|| {
        EdgeError::InvalidInput(format!(
            "appointment status must be scheduled, completed, cancelled or no_show — got {value:?}"
        ))
    })
}

/// Row `Appointment` → domain `Appointment` (status string
/// parsed; unknown statuses fail loudly).
fn appointment_to_domain(row: &Appointment) -> Result<DomainAppointment, EdgeError> {
    Ok(DomainAppointment {
        id: row.id,
        org_id: row.org_id,
        store_id: row.store_id,
        customer_id: row.customer_id,
        staff_id: row.staff_id,
        title: row.title.clone(),
        starts_at: row.starts_at,
        ends_at: row.ends_at,
        status: appointment_status_from_str(&row.status)?,
        notes: row.notes.clone(),
        created_at: row.created_at,
    })
}

/// Domain `Appointment` → row `Appointment`.
fn appointment_from_domain(domain: &DomainAppointment) -> Appointment {
    Appointment {
        id: domain.id,
        org_id: domain.org_id,
        store_id: domain.store_id,
        customer_id: domain.customer_id,
        staff_id: domain.staff_id,
        title: domain.title.clone(),
        starts_at: domain.starts_at,
        ends_at: domain.ends_at,
        status: domain.status.as_str().to_string(),
        notes: domain.notes.clone(),
        created_at: domain.created_at,
    }
}

impl Edge {
    fn org_id(&self) -> Uuid {
        self.config().org_id
    }

    fn store_id(&self) -> Uuid {
        self.config().store_id
    }

    // --- activities ---------------------------------------------------------

    /// Log a CRM activity (optionally a reminder when `due_at` is
    /// set). The activity type comes from the `df-crm` closed
    /// list; the customer, when given, must exist.
    pub async fn create_activity(
        &self,
        body: CreateActivityRequest,
    ) -> Result<CrmActivity, EdgeError> {
        let activity_type = activity_type_from_str(&body.activity_type)?;
        validate_subject(&body.subject)?;
        if let Some(customer_id) = body.customer_id {
            df_db::get_customer(self.pool(), customer_id).await?;
        }
        let domain = df_crm::activity::CrmActivity::new(
            self.org_id(),
            self.store_id(),
            body.customer_id,
            activity_type,
            body.subject,
        );
        let activity = CrmActivity {
            id: domain.id,
            org_id: domain.org_id,
            store_id: domain.store_id,
            customer_id: domain.customer_id,
            activity_type: domain.activity_type.as_str().to_string(),
            subject: domain.subject,
            notes: body.notes,
            created_by: body.created_by,
            due_at: body.due_at,
            completed_at: None,
            snoozed_until: None,
            created_at: domain.created_at,
        };
        df_db::insert_crm_activity(self.pool(), &activity).await?;
        Ok(activity)
    }

    /// Fetch one activity by id.
    pub async fn get_activity(&self, id: Uuid) -> Result<CrmActivity, EdgeError> {
        df_db::get_crm_activity(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_db::DbError::NotFound(_) => EdgeError::NotFound(format!("CRM activity {id}")),
                other => EdgeError::from(other),
            })
    }

    /// List activities for this store, newest first, optionally
    /// filtered by customer, type and completion.
    pub async fn list_activities(
        &self,
        customer_id: Option<Uuid>,
        activity_type: Option<&str>,
        completed: Option<bool>,
    ) -> Result<Vec<CrmActivity>, EdgeError> {
        if let Some(activity_type) = activity_type {
            activity_type_from_str(activity_type)?;
        }
        let mut activities =
            df_db::list_crm_activities_for_store(self.pool(), self.store_id()).await?;
        if let Some(customer_id) = customer_id {
            activities.retain(|a| a.customer_id == Some(customer_id));
        }
        if let Some(activity_type) = activity_type {
            activities.retain(|a| a.activity_type == activity_type);
        }
        if let Some(completed) = completed {
            activities.retain(|a| a.completed_at.is_some() == completed);
        }
        Ok(activities)
    }

    /// Apply a partial activity update.
    pub async fn update_activity(
        &self,
        id: Uuid,
        body: UpdateActivityRequest,
    ) -> Result<CrmActivity, EdgeError> {
        let mut activity = self.get_activity(id).await?;
        if let Some(customer_id) = body.customer_id {
            df_db::get_customer(self.pool(), customer_id).await?;
            activity.customer_id = Some(customer_id);
        }
        if let Some(activity_type) = body.activity_type {
            activity_type_from_str(&activity_type)?;
            activity.activity_type = activity_type;
        }
        if let Some(subject) = body.subject {
            validate_subject(&subject)?;
            activity.subject = subject;
        }
        if let Some(notes) = body.notes {
            activity.notes = notes;
        }
        if let Some(due_at) = body.due_at {
            activity.due_at = due_at;
        }
        df_db::update_crm_activity(self.pool(), &activity).await?;
        Ok(activity)
    }

    /// Delete one activity. Unknown ids are a 404.
    pub async fn delete_activity(&self, id: Uuid) -> Result<(), EdgeError> {
        self.get_activity(id).await?;
        df_db::delete_crm_activity(self.pool(), id).await?;
        Ok(())
    }

    /// Mark a reminder done, via the `df-crm` tickler rule. A
    /// reminder that is already complete is an error
    /// (`CrmError::AlreadyCompleted` → 400).
    pub async fn complete_activity(&self, id: Uuid) -> Result<CrmActivity, EdgeError> {
        self.get_activity(id).await?;
        Ok(df_db::complete_reminder(self.pool(), id, Utc::now()).await?)
    }

    /// Snooze a reminder, via the `df-crm` tickler rule. The
    /// activity must carry a due date and the snooze target must
    /// be in the future; violations are 400s.
    pub async fn snooze_activity(
        &self,
        id: Uuid,
        snoozed_until: DateTime<Utc>,
    ) -> Result<CrmActivity, EdgeError> {
        self.get_activity(id).await?;
        Ok(df_db::snooze_reminder(self.pool(), id, snoozed_until, Utc::now()).await?)
    }

    /// Reminders due at `as_of`, earliest effective due first
    /// (`df-db::crm_reports`, `df-crm::reminder`).
    pub async fn due_reminders(&self, as_of: DateTime<Utc>) -> Result<Vec<CrmActivity>, EdgeError> {
        Ok(df_db::list_reminders_due_for_store(self.pool(), self.store_id(), as_of).await?)
    }

    /// Reminders strictly past their effective due time at
    /// `as_of`, most overdue first.
    pub async fn overdue_reminders(
        &self,
        as_of: DateTime<Utc>,
    ) -> Result<Vec<CrmActivity>, EdgeError> {
        Ok(df_db::list_overdue_reminders_for_store(self.pool(), self.store_id(), as_of).await?)
    }

    // --- appointments -------------------------------------------------------

    /// Book an appointment. The window is validated by the
    /// `df-crm` constructor and the overlap check
    /// (`df-db::insert_appointment_checked`) rejects a clash with
    /// another scheduled appointment for the same contractor or
    /// staff member.
    pub async fn create_appointment(
        &self,
        body: CreateAppointmentRequest,
    ) -> Result<Appointment, EdgeError> {
        validate_subject(&body.title)?;
        if let Some(customer_id) = body.customer_id {
            df_db::get_customer(self.pool(), customer_id).await?;
        }
        let mut domain = DomainAppointment::new(
            self.org_id(),
            self.store_id(),
            body.customer_id,
            body.staff_id,
            body.title,
            body.starts_at,
            body.ends_at,
        )?;
        if let Some(notes) = body.notes {
            domain = domain.with_notes(notes);
        }
        let appointment = appointment_from_domain(&domain);
        df_db::insert_appointment_checked(self.pool(), &appointment).await?;
        Ok(appointment)
    }

    /// Fetch one appointment by id.
    pub async fn get_appointment(&self, id: Uuid) -> Result<Appointment, EdgeError> {
        df_db::get_appointment(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_db::DbError::NotFound(_) => EdgeError::NotFound(format!("appointment {id}")),
                other => EdgeError::from(other),
            })
    }

    /// List appointments for this store, earliest first,
    /// optionally filtered by day (`YYYY-MM-DD`, UTC calendar
    /// day), customer or staff. Day / customer filtering uses
    /// the `df-crm` list helpers.
    pub async fn list_appointments(
        &self,
        day: Option<NaiveDate>,
        customer_id: Option<Uuid>,
        staff_id: Option<Uuid>,
    ) -> Result<Vec<Appointment>, EdgeError> {
        let rows = df_db::list_appointments_for_store(self.pool(), self.store_id()).await?;
        let domain: Vec<DomainAppointment> = rows
            .iter()
            .map(appointment_to_domain)
            .collect::<Result<Vec<_>, _>>()?;
        let mut kept: Vec<Uuid> = domain.iter().map(|a| a.id).collect();
        if let Some(day) = day {
            let on_day: Vec<Uuid> = df_crm::appointments_for_day(&domain, day)
                .iter()
                .map(|a| a.id)
                .collect();
            kept.retain(|id| on_day.contains(id));
        }
        if let Some(customer_id) = customer_id {
            let for_customer: Vec<Uuid> = df_crm::appointments_for_contractor(&domain, customer_id)
                .iter()
                .map(|a| a.id)
                .collect();
            kept.retain(|id| for_customer.contains(id));
        }
        if let Some(staff_id) = staff_id {
            kept.retain(|id| {
                domain
                    .iter()
                    .find(|a| a.id == *id)
                    .map(|a| a.staff_id == Some(staff_id))
                    .unwrap_or(false)
            });
        }
        let mut out: Vec<Appointment> = rows
            .into_iter()
            .filter(|row| kept.contains(&row.id))
            .collect();
        out.sort_by_key(|row| row.starts_at);
        Ok(out)
    }

    /// Apply a partial appointment update. The merged window is
    /// re-validated by the `df-crm` constructor and the overlap
    /// check re-run; a non-scheduled appointment cannot be
    /// rescheduled (cancel it instead).
    pub async fn update_appointment(
        &self,
        id: Uuid,
        body: UpdateAppointmentRequest,
    ) -> Result<Appointment, EdgeError> {
        let current = self.get_appointment(id).await?;
        if current.status != AppointmentStatus::Scheduled.as_str()
            && (body.starts_at.is_some() || body.ends_at.is_some() || body.staff_id.is_some())
        {
            return Err(EdgeError::InvalidInput(format!(
                "appointment {id} is {} and cannot be rescheduled",
                current.status
            )));
        }
        let mut merged = current.clone();
        if let Some(customer_id) = body.customer_id {
            df_db::get_customer(self.pool(), customer_id).await?;
            merged.customer_id = Some(customer_id);
        }
        if let Some(staff_id) = body.staff_id {
            merged.staff_id = Some(staff_id);
        }
        if let Some(title) = body.title {
            validate_subject(&title)?;
            merged.title = title;
        }
        if let Some(starts_at) = body.starts_at {
            merged.starts_at = starts_at;
        }
        if let Some(ends_at) = body.ends_at {
            merged.ends_at = ends_at;
        }
        if let Some(status) = body.status {
            appointment_status_from_str(&status)?;
            merged.status = status;
        }
        if let Some(notes) = body.notes {
            merged.notes = notes;
        }
        // Re-validate the window + overlap through the domain.
        let mut candidate = DomainAppointment::new(
            merged.org_id,
            merged.store_id,
            merged.customer_id,
            merged.staff_id,
            merged.title.clone(),
            merged.starts_at,
            merged.ends_at,
        )?;
        candidate.id = merged.id;
        candidate.status = appointment_status_from_str(&merged.status)?;
        candidate.notes = merged.notes.clone();
        candidate.created_at = merged.created_at;
        if candidate.status == AppointmentStatus::Scheduled {
            let existing = df_db::list_appointments_for_store(self.pool(), self.store_id()).await?;
            let existing_domain: Vec<DomainAppointment> = existing
                .iter()
                .map(appointment_to_domain)
                .collect::<Result<Vec<_>, _>>()?;
            df_crm::check_overlap(&existing_domain, &candidate)?;
        }
        let updated = appointment_from_domain(&candidate);
        df_db::update_appointment(self.pool(), &updated).await?;
        Ok(updated)
    }

    /// Cancel an appointment (status → `cancelled`).
    pub async fn cancel_appointment(&self, id: Uuid) -> Result<Appointment, EdgeError> {
        self.update_appointment(
            id,
            UpdateAppointmentRequest {
                status: Some(AppointmentStatus::Cancelled.as_str().to_string()),
                ..Default::default()
            },
        )
        .await
    }

    /// Delete one appointment. Unknown ids are a 404.
    pub async fn delete_appointment(&self, id: Uuid) -> Result<(), EdgeError> {
        self.get_appointment(id).await?;
        df_db::delete_appointment(self.pool(), id).await?;
        Ok(())
    }

    // --- saved views --------------------------------------------------------

    /// Save a named filter. The name is unique per store (schema
    /// constraint); the filter must be a JSON object. Either
    /// dialect may be stored — the run endpoints decide which
    /// dialect a view is in.
    pub async fn create_saved_view(
        &self,
        body: CreateSavedViewRequest,
    ) -> Result<CrmSavedView, EdgeError> {
        if body.name.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "saved view name must not be empty".to_string(),
            ));
        }
        if !body.filter_json.is_object() {
            return Err(EdgeError::InvalidInput(
                "filter_json must be a JSON object".to_string(),
            ));
        }
        let view = CrmSavedView {
            id: Uuid::now_v7(),
            org_id: self.org_id(),
            store_id: self.store_id(),
            name: body.name,
            filter_json: body.filter_json,
            created_at: Utc::now(),
        };
        df_db::insert_crm_saved_view(self.pool(), &view).await?;
        Ok(view)
    }

    /// List saved views for this store, by name.
    pub async fn list_saved_views(&self) -> Result<Vec<CrmSavedView>, EdgeError> {
        Ok(df_db::list_crm_saved_views_for_store(self.pool(), self.store_id()).await?)
    }

    /// Run a saved view over the activity log (the activity
    /// dialect: `activity_type`, `customer_id`, `completed`,
    /// `due_before`), via `df-crm::saved_view::ActivityFilter`.
    /// A view stored in the customer dialect is *not* rejected
    /// here: the activity filter is lenient (`#[serde(default)]`,
    /// unknown keys ignored), so a customer-dialect view parses
    /// as an unconstrained activity filter and matches every
    /// activity. Use `run_saved_view_customers` for customer
    /// views — that is the endpoint that knows the dialect.
    pub async fn run_saved_view(&self, id: Uuid) -> Result<Vec<CrmActivity>, EdgeError> {
        let view = df_db::get_crm_saved_view(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_db::DbError::NotFound(_) => EdgeError::NotFound(format!("saved view {id}")),
                other => EdgeError::from(other),
            })?;
        let filter: ActivityFilter =
            serde_json::from_value(view.filter_json.clone()).map_err(|err| {
                EdgeError::InvalidInput(format!(
                    "saved view {id} filter_json is not a valid activity filter: {err}"
                ))
            })?;
        let activities = df_db::list_crm_activities_for_store(self.pool(), self.store_id()).await?;
        Ok(activities
            .into_iter()
            .filter(|a| {
                filter.matches(&ActivitySnapshot {
                    activity_type: a.activity_type.clone(),
                    customer_id: a.customer_id,
                    completed: a.completed_at.is_some(),
                    due_at: a.due_at,
                })
            })
            .collect())
    }

    /// Run a saved view over customers (the customer dialect),
    /// via `df-db::evaluate_saved_view`. A view stored in the
    /// activity dialect is rejected with a 400 instead of
    /// silently misread.
    pub async fn run_saved_view_customers(
        &self,
        id: Uuid,
    ) -> Result<Vec<SavedViewCustomer>, EdgeError> {
        let view = df_db::get_crm_saved_view(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_db::DbError::NotFound(_) => EdgeError::NotFound(format!("saved view {id}")),
                other => EdgeError::from(other),
            })?;
        let customers = df_db::evaluate_saved_view(self.pool(), &view).await?;
        let snapshots = df_db::customer_snapshots(self.pool(), self.org_id(), Utc::now()).await?;
        let currency = self.config().currency;
        Ok(customers
            .into_iter()
            .map(|c| {
                let snap = snapshots.iter().find(|s| s.customer_id == c.id);
                SavedViewCustomer {
                    customer_id: c.id,
                    account_number: c.account_number,
                    company_name: c.company_name,
                    customer_type: c.customer_type,
                    charge_enabled: c.charge_enabled,
                    credit_hold: c.credit_hold,
                    balance: Money::new(snap.map(|s| s.balance).unwrap_or_default(), currency),
                    last_purchase_at: snap.and_then(|s| s.last_purchase_at),
                }
            })
            .collect())
    }

    /// Delete one saved view. Unknown ids are a 404.
    pub async fn delete_saved_view(&self, id: Uuid) -> Result<(), EdgeError> {
        df_db::get_crm_saved_view(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_db::DbError::NotFound(_) => EdgeError::NotFound(format!("saved view {id}")),
                other => EdgeError::from(other),
            })?;
        df_db::delete_crm_saved_view(self.pool(), id).await?;
        Ok(())
    }

    // --- lapsed-contractor report -------------------------------------------

    /// Contractors with no completed sale in `inactive_days`
    /// (strictly more days since the last sale), plus — when
    /// `include_never_purchased` is set — contractors who never
    /// bought, as a distinct status. Classification and ordering
    /// come from `df-crm::lapsed_report` via
    /// `df-db::lapsed_contractor_report`; `as_of` is the report
    /// date at the start of the day (UTC).
    pub async fn lapsed_contractors(
        &self,
        as_of: NaiveDate,
        inactive_days: i64,
        include_never_purchased: bool,
    ) -> Result<LapsedReport, EdgeError> {
        if inactive_days < 0 {
            return Err(EdgeError::InvalidInput(
                "inactive_days must not be negative".to_string(),
            ));
        }
        let as_of_dt = as_of
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| EdgeError::InvalidInput(format!("bad as_of date {as_of}")))?
            .and_utc();
        let customers = df_db::lapsed_contractor_report(
            self.pool(),
            self.org_id(),
            as_of_dt,
            inactive_days,
            include_never_purchased,
        )
        .await?;
        Ok(LapsedReport {
            store_id: self.store_id(),
            as_of,
            inactive_days,
            include_never_purchased,
            customers,
        })
    }
}
