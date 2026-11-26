//! CRM rules that sit on top of the wiring worker's `0006`
//! repositories (Phase 4, v0.5; spec §4 `crm_marketing`, §7
//! Phase 4; domain rules in `df-crm`, migrations `0006_phase4` /
//! `0007_phase4_crm_delivery`).
//!
//! * Reminder tickler ([`list_reminders_due_for_store`],
//!   [`list_overdue_reminders_for_store`],
//!   [`complete_reminder`], [`snooze_reminder`]) — due / overdue
//!   lists and state changes over the activities that carry a
//!   due date, using `df-crm::reminder` so the rule lives in one
//!   place.
//! * [`insert_appointment_checked`] — appointment insert with
//!   the `df-crm` overlap check (same contractor or same staff
//!   member, scheduled appointments only); a clash is
//!   [`DbError::AppointmentOverlap`] and nothing is inserted.
//! * [`lapsed_contractor_report`] — the lapsed-contractor report
//!   (exit test (c)): contractors with no completed sale in N
//!   days, with never-purchased contractors as a distinct status.
//!   Purchases are completed `sale` transactions; trailing spend
//!   is the completed-sale total over the trailing 365 days.
//! * [`evaluate_saved_view`] — run a saved view's
//!   `CustomerFilter` over the org's customers. A view whose
//!   `filter_json` is in the *activity*-filter dialect
//!   (`activity_type` / `completed` / `due_before`, the dialect
//!   `df-edge::run_saved_view` uses) is rejected with
//!   [`DbError::InvalidData`] rather than silently misread — the
//!   two dialects are different types in `df-crm::saved_view`
//!   and a view is one or the other.

use std::collections::HashMap;

use chrono::{DateTime, TimeDelta, Utc};
use df_crm::{
    ActivityType, Appointment as DomainAppointment, AppointmentStatus,
    CrmActivity as DomainActivity, CustomerFilter, CustomerSnapshot, lapsed_report,
};
use rust_decimal::Decimal;
use sqlx::AnyPool;
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{Appointment, CrmActivity, CrmSavedView, Customer};

// --- row <-> domain conversions -----------------------------------------------

/// Row `CrmActivity` → domain `CrmActivity` (activity-type string
/// parsed; unknown types fail loudly).
fn activity_to_domain(row: &CrmActivity) -> Result<DomainActivity, DbError> {
    let activity_type = ActivityType::from_str(&row.activity_type).ok_or_else(|| {
        DbError::InvalidData(format!("unknown activity_type: {}", row.activity_type))
    })?;
    Ok(DomainActivity {
        id: row.id,
        org_id: row.org_id,
        store_id: row.store_id,
        customer_id: row.customer_id,
        activity_type,
        subject: row.subject.clone(),
        notes: row.notes.clone(),
        created_by: row.created_by,
        due_at: row.due_at,
        completed_at: row.completed_at,
        snoozed_until: row.snoozed_until,
        created_at: row.created_at,
    })
}

/// Domain `CrmActivity` → row `CrmActivity`.
fn activity_from_domain(domain: &DomainActivity) -> CrmActivity {
    CrmActivity {
        id: domain.id,
        org_id: domain.org_id,
        store_id: domain.store_id,
        customer_id: domain.customer_id,
        activity_type: domain.activity_type.as_str().to_string(),
        subject: domain.subject.clone(),
        notes: domain.notes.clone(),
        created_by: domain.created_by,
        due_at: domain.due_at,
        completed_at: domain.completed_at,
        snoozed_until: domain.snoozed_until,
        created_at: domain.created_at,
    }
}

// --- reminder tickler -------------------------------------------------------

/// Open reminders (activities with a due date) for a store that
/// are due at or before `as_of`, earliest effective due first.
pub async fn list_reminders_due_for_store(
    pool: &AnyPool,
    store_id: Uuid,
    as_of: DateTime<Utc>,
) -> Result<Vec<CrmActivity>, DbError> {
    let activities = crate::crm::list_crm_activities_for_store(pool, store_id).await?;
    let domain: Vec<DomainActivity> = activities
        .iter()
        .map(activity_to_domain)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(df_crm::reminder::due_reminders(&domain, as_of)
        .into_iter()
        .map(activity_from_domain)
        .collect())
}

/// Open reminders for a store that are strictly past their
/// effective due time at `as_of`, most overdue first.
pub async fn list_overdue_reminders_for_store(
    pool: &AnyPool,
    store_id: Uuid,
    as_of: DateTime<Utc>,
) -> Result<Vec<CrmActivity>, DbError> {
    let activities = crate::crm::list_crm_activities_for_store(pool, store_id).await?;
    let domain: Vec<DomainActivity> = activities
        .iter()
        .map(activity_to_domain)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(df_crm::reminder::overdue_reminders(&domain, as_of)
        .into_iter()
        .map(activity_from_domain)
        .collect())
}

/// Mark a reminder done at `at`. Returns the updated activity.
pub async fn complete_reminder(
    pool: &AnyPool,
    activity_id: Uuid,
    at: DateTime<Utc>,
) -> Result<CrmActivity, DbError> {
    let row = crate::crm::get_crm_activity(pool, activity_id).await?;
    let mut activity = activity_to_domain(&row)?;
    df_crm::reminder::complete(&mut activity, at)?;
    let row = activity_from_domain(&activity);
    crate::crm::update_crm_activity(pool, &row).await?;
    Ok(row)
}

/// Snooze a reminder to `until` (must be after `now`). Returns
/// the updated activity.
pub async fn snooze_reminder(
    pool: &AnyPool,
    activity_id: Uuid,
    until: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<CrmActivity, DbError> {
    let row = crate::crm::get_crm_activity(pool, activity_id).await?;
    let mut activity = activity_to_domain(&row)?;
    df_crm::reminder::snooze(&mut activity, until, now)?;
    let row = activity_from_domain(&activity);
    crate::crm::update_crm_activity(pool, &row).await?;
    Ok(row)
}

// --- appointments with overlap check ----------------------------------------

fn appointment_to_domain(row: &Appointment) -> Result<DomainAppointment, DbError> {
    let status = AppointmentStatus::from_str(&row.status).ok_or_else(|| {
        DbError::InvalidData(format!("unknown appointment status: {}", row.status))
    })?;
    Ok(DomainAppointment {
        id: row.id,
        org_id: row.org_id,
        store_id: row.store_id,
        customer_id: row.customer_id,
        staff_id: row.staff_id,
        title: row.title.clone(),
        starts_at: row.starts_at,
        ends_at: row.ends_at,
        status,
        notes: row.notes.clone(),
        created_at: row.created_at,
    })
}

/// Insert an appointment after the `df-crm` overlap check. The
/// appointment must be `scheduled`; a clash with another
/// scheduled appointment for the same contractor or staff member
/// returns [`DbError::AppointmentOverlap`] and inserts nothing.
pub async fn insert_appointment_checked(
    pool: &AnyPool,
    appointment: &Appointment,
) -> Result<(), DbError> {
    if appointment.status != AppointmentStatus::Scheduled.as_str() {
        return Err(DbError::InvalidData(format!(
            "only a scheduled appointment can be overlap-checked, got {}",
            appointment.status
        )));
    }
    let candidate = appointment_to_domain(appointment)?;
    let existing = crate::crm::list_appointments_for_store(pool, appointment.store_id).await?;
    let existing_domain: Vec<DomainAppointment> = existing
        .iter()
        .map(appointment_to_domain)
        .collect::<Result<Vec<_>, _>>()?;
    df_crm::check_overlap(&existing_domain, &candidate)?;
    crate::crm::insert_appointment(pool, appointment).await
}

// --- customer snapshots (balances + purchases) ------------------------------

/// Per-customer purchase facts, aggregated in Rust (the codebase
/// rule: no date arithmetic in SQL, so SQLite text timestamps are
/// never compared there).
struct PurchaseAggregate {
    last_purchase_at: Option<DateTime<Utc>>,
    trailing_spend: Decimal,
}

fn purchase_aggregates(
    rows: &[(Option<Uuid>, DateTime<Utc>, Decimal)],
    as_of: DateTime<Utc>,
) -> HashMap<Uuid, PurchaseAggregate> {
    let window_start = as_of - TimeDelta::days(365);
    let mut map: HashMap<Uuid, PurchaseAggregate> = HashMap::new();
    for (customer_id, created_at, grand_total) in rows {
        let Some(customer_id) = customer_id else {
            continue;
        };
        let entry = map.entry(*customer_id).or_insert(PurchaseAggregate {
            last_purchase_at: None,
            trailing_spend: Decimal::ZERO,
        });
        if entry
            .last_purchase_at
            .map(|last| *created_at > last)
            .unwrap_or(true)
        {
            entry.last_purchase_at = Some(*created_at);
        }
        if *created_at > window_start && *created_at <= as_of {
            entry.trailing_spend += *grand_total;
        }
    }
    map
}

/// Everything the customer-segment reports need, loaded once:
/// the org's customers, one snapshot each, trailing 365-day
/// spend per customer, and AR balance per customer.
struct CustomerFacts {
    customers: Vec<Customer>,
    snapshots: HashMap<Uuid, CustomerSnapshot>,
    trailing_spend: HashMap<Uuid, Decimal>,
    balances: HashMap<Uuid, Decimal>,
}

async fn load_customer_facts(
    pool: &AnyPool,
    org_id: Uuid,
    as_of: DateTime<Utc>,
) -> Result<CustomerFacts, DbError> {
    let customers = crate::list_customers_for_org(pool, org_id).await?;
    let mut balances: HashMap<Uuid, Decimal> = HashMap::new();
    let balance_rows = sqlx::query("SELECT customer_id, amount FROM ar_ledger WHERE org_id = ?")
        .bind(uuid_str(org_id))
        .fetch_all(pool)
        .await?;
    for row in &balance_rows {
        let customer_id = uuid_col(row, "customer_id")?;
        let amount = dec_col(row, "amount")?;
        *balances.entry(customer_id).or_insert(Decimal::ZERO) += amount;
    }
    let purchase_rows = sqlx::query(
        "SELECT customer_id, created_at, grand_total FROM transactions WHERE org_id = ? AND status = 'completed' AND doc_type = 'sale'",
    )
    .bind(uuid_str(org_id))
    .fetch_all(pool)
    .await?;
    let mut raw = Vec::new();
    for row in &purchase_rows {
        raw.push((
            opt_uuid_col(row, "customer_id")?,
            dt_col(row, "created_at")?,
            dec_col(row, "grand_total")?,
        ));
    }
    let purchases = purchase_aggregates(&raw, as_of);
    let mut snapshots = HashMap::new();
    let mut trailing_spend = HashMap::new();
    for c in &customers {
        let agg = purchases.get(&c.id);
        snapshots.insert(
            c.id,
            CustomerSnapshot {
                customer_id: c.id,
                customer_type: c.customer_type.clone(),
                charge_enabled: c.charge_enabled,
                credit_hold: c.credit_hold,
                balance: balances.get(&c.id).copied().unwrap_or(Decimal::ZERO),
                last_purchase_at: agg.and_then(|a| a.last_purchase_at),
            },
        );
        trailing_spend.insert(c.id, agg.map(|a| a.trailing_spend).unwrap_or(Decimal::ZERO));
    }
    Ok(CustomerFacts {
        customers,
        snapshots,
        trailing_spend,
        balances,
    })
}

/// One [`CustomerSnapshot`] per customer in the org: type, flags,
/// AR balance, and last completed purchase.
pub async fn customer_snapshots(
    pool: &AnyPool,
    org_id: Uuid,
    as_of: DateTime<Utc>,
) -> Result<Vec<CustomerSnapshot>, DbError> {
    let facts = load_customer_facts(pool, org_id, as_of).await?;
    Ok(facts
        .customers
        .iter()
        .filter_map(|c| facts.snapshots.get(&c.id).cloned())
        .collect())
}

// --- lapsed-contractor report -----------------------------------------------

/// Contractors with no completed sale in `lapsed_after_days`
/// (strictly more days since the last sale than the window), plus
/// — when `include_never_purchased` is set — contractors who
/// never bought, as a distinct `NeverPurchased` status. Rows come
/// back never-purchased first, then most-inactive first. See
/// `df_crm::lapsed_report` for the classification rules.
pub async fn lapsed_contractor_report(
    pool: &AnyPool,
    org_id: Uuid,
    as_of: DateTime<Utc>,
    lapsed_after_days: i64,
    include_never_purchased: bool,
) -> Result<Vec<df_crm::LapsedContractor>, DbError> {
    let facts = load_customer_facts(pool, org_id, as_of).await?;
    let summaries: Vec<df_crm::ContractorPurchaseSummary> = facts
        .customers
        .iter()
        .filter(|c| c.customer_type == "contractor")
        .map(|c| {
            let snap = facts.snapshots.get(&c.id);
            df_crm::ContractorPurchaseSummary {
                customer_id: c.id,
                account_number: c.account_number.clone(),
                company_name: c.company_name.clone(),
                last_purchase_at: snap.and_then(|s| s.last_purchase_at),
                trailing_spend: facts
                    .trailing_spend
                    .get(&c.id)
                    .copied()
                    .unwrap_or(Decimal::ZERO),
                ar_balance: facts.balances.get(&c.id).copied().unwrap_or(Decimal::ZERO),
            }
        })
        .collect();
    Ok(lapsed_report(
        &summaries,
        as_of,
        lapsed_after_days,
        include_never_purchased,
    ))
}

// --- saved-view evaluation --------------------------------------------------

/// Keys of the activity-filter dialect. A view whose JSON carries
/// any of these and none of the customer-filter keys is an
/// activity view, not a customer view.
const ACTIVITY_FILTER_KEYS: [&str; 3] = ["activity_type", "completed", "due_before"];
const CUSTOMER_FILTER_KEYS: [&str; 8] = [
    "customer_type",
    "charge_enabled",
    "credit_hold",
    "min_balance",
    "max_balance",
    "last_purchase_within_days",
    "last_purchase_before_days",
    "has_purchased",
];

/// Run a saved view's customer filter over the org's customers
/// and return the matching customer rows. The filter is evaluated
/// at now. A view stored in the activity-filter dialect is
/// rejected instead of silently misread (see the module docs).
pub async fn evaluate_saved_view(
    pool: &AnyPool,
    view: &CrmSavedView,
) -> Result<Vec<Customer>, DbError> {
    let json = &view.filter_json;
    if let Some(obj) = json.as_object() {
        let has_activity_key = ACTIVITY_FILTER_KEYS.iter().any(|k| obj.contains_key(*k));
        let has_customer_key = CUSTOMER_FILTER_KEYS.iter().any(|k| obj.contains_key(*k));
        if has_activity_key && !has_customer_key {
            return Err(DbError::InvalidData(format!(
                "saved view {} is stored in the activity-filter dialect (activity_type/completed/due_before); evaluate it over activities, not customers",
                view.id
            )));
        }
    }
    let filter: CustomerFilter = serde_json::from_value(json.clone()).map_err(|e| {
        DbError::InvalidData(format!(
            "saved view {} has an invalid customer filter: {e}",
            view.id
        ))
    })?;
    let as_of = Utc::now();
    let customers = crate::list_customers_for_org(pool, view.org_id).await?;
    let snapshots = customer_snapshots(pool, view.org_id, as_of).await?;
    let by_id: HashMap<Uuid, &CustomerSnapshot> =
        snapshots.iter().map(|s| (s.customer_id, s)).collect();
    Ok(customers
        .into_iter()
        .filter(|c| {
            by_id
                .get(&c.id)
                .map(|s| filter.matches(s, as_of))
                .unwrap_or(false)
        })
        .collect())
}
