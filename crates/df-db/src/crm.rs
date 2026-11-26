//! CRM activity log, appointments, saved views, delivery dispatch
//! and proof-of-delivery repositories (Phase 4, spec §5.5/§7).
//!
//! Follows the `df-db` conventions — `?` placeholders (rewritten
//! to `$N` for PostgreSQL), ids / timestamps crossing the driver as
//! text (UUID as hyphenated text, timestamps as RFC 3339, dates as
//! `YYYY-MM-DD`). No `query!` macros, so the workspace builds with
//! `SQLX_OFFLINE=true` and no live database.
//!
//! List functions return a store's rows and leave filtering
//! (due-date, day, status) to the caller in Rust — the same rule as
//! `list_transactions_for_store`, so SQLite text timestamps are
//! never compared in SQL.

use chrono::{DateTime, Utc};
use sqlx::AnyPool;
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{Appointment, CrmActivity, CrmSavedView, Delivery, DeliveryPod};

// --- row decoding -----------------------------------------------------------

fn activity_from_row(row: &sqlx::any::AnyRow) -> Result<CrmActivity, DbError> {
    Ok(CrmActivity {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        customer_id: opt_uuid_col(row, "customer_id")?,
        activity_type: text_col(row, "activity_type")?,
        subject: text_col(row, "subject")?,
        notes: opt_text_col(row, "notes")?,
        created_by: opt_uuid_col(row, "created_by")?,
        due_at: opt_dt_col(row, "due_at")?,
        completed_at: opt_dt_col(row, "completed_at")?,
        snoozed_until: opt_dt_col(row, "snoozed_until")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn appointment_from_row(row: &sqlx::any::AnyRow) -> Result<Appointment, DbError> {
    Ok(Appointment {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        customer_id: opt_uuid_col(row, "customer_id")?,
        staff_id: opt_uuid_col(row, "staff_id")?,
        title: text_col(row, "title")?,
        starts_at: dt_col(row, "starts_at")?,
        ends_at: dt_col(row, "ends_at")?,
        status: text_col(row, "status")?,
        notes: opt_text_col(row, "notes")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn saved_view_from_row(row: &sqlx::any::AnyRow) -> Result<CrmSavedView, DbError> {
    Ok(CrmSavedView {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        name: text_col(row, "name")?,
        filter_json: json_col(row, "filter_json")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn delivery_from_row(row: &sqlx::any::AnyRow) -> Result<Delivery, DbError> {
    Ok(Delivery {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        transaction_id: opt_uuid_col(row, "transaction_id")?,
        customer_id: opt_uuid_col(row, "customer_id")?,
        driver_id: opt_uuid_col(row, "driver_id")?,
        status: text_col(row, "status")?,
        address_json: json_col(row, "address_json")?,
        scheduled_date: opt_date_col(row, "scheduled_date")?,
        priority: i32_col(row, "priority")?,
        vehicle: opt_text_col(row, "vehicle")?,
        created_at: dt_col(row, "created_at")?,
        updated_at: dt_col(row, "updated_at")?,
    })
}

fn pod_from_row(row: &sqlx::any::AnyRow) -> Result<DeliveryPod, DbError> {
    Ok(DeliveryPod {
        id: uuid_col(row, "id")?,
        delivery_id: uuid_col(row, "delivery_id")?,
        signature_sha256: text_col(row, "signature_sha256")?,
        signature_b64: text_col(row, "signature_b64")?,
        photo_ref: opt_text_col(row, "photo_ref")?,
        photo_sha256: opt_text_col(row, "photo_sha256")?,
        received_by: text_col(row, "received_by")?,
        signed_at: dt_col(row, "signed_at")?,
        photo_at: opt_dt_col(row, "photo_at")?,
        created_at: dt_col(row, "created_at")?,
    })
}

// --- activities -------------------------------------------------------------

/// Insert one CRM activity row.
pub async fn insert_crm_activity(pool: &AnyPool, activity: &CrmActivity) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO crm_activities (id, org_id, store_id, customer_id, activity_type, subject, notes, created_by, due_at, completed_at, snoozed_until, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(activity.id))
    .bind(uuid_str(activity.org_id))
    .bind(uuid_str(activity.store_id))
    .bind(opt_uuid_str(activity.customer_id))
    .bind(&activity.activity_type)
    .bind(&activity.subject)
    .bind(&activity.notes)
    .bind(opt_uuid_str(activity.created_by))
    .bind(opt_dt_str(activity.due_at))
    .bind(opt_dt_str(activity.completed_at))
    .bind(opt_dt_str(activity.snoozed_until))
    .bind(dt_str(activity.created_at))
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch one activity by id.
pub async fn get_crm_activity(pool: &AnyPool, id: Uuid) -> Result<CrmActivity, DbError> {
    let row = sqlx::query("SELECT * FROM crm_activities WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("CRM activity {id}")))?;
    activity_from_row(&row)
}

/// All activities for a store, newest first. Callers filter by
/// customer / type / completion in Rust.
pub async fn list_crm_activities_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<CrmActivity>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM crm_activities WHERE store_id = ? ORDER BY created_at DESC")
            .bind(uuid_str(store_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(activity_from_row).collect()
}

/// Update the mutable fields of an activity (type, subject, notes,
/// due/completed/snoozed timestamps, customer link).
pub async fn update_crm_activity(pool: &AnyPool, activity: &CrmActivity) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE crm_activities SET customer_id = ?, activity_type = ?, subject = ?, notes = ?, due_at = ?, completed_at = ?, snoozed_until = ? WHERE id = ?",
    )
    .bind(opt_uuid_str(activity.customer_id))
    .bind(&activity.activity_type)
    .bind(&activity.subject)
    .bind(&activity.notes)
    .bind(opt_dt_str(activity.due_at))
    .bind(opt_dt_str(activity.completed_at))
    .bind(opt_dt_str(activity.snoozed_until))
    .bind(uuid_str(activity.id))
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete one activity. No-op when the id does not exist.
pub async fn delete_crm_activity(pool: &AnyPool, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM crm_activities WHERE id = ?")
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- appointments -----------------------------------------------------------

/// Insert one appointment row.
pub async fn insert_appointment(pool: &AnyPool, appointment: &Appointment) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO appointments (id, org_id, store_id, customer_id, staff_id, title, starts_at, ends_at, status, notes, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(appointment.id))
    .bind(uuid_str(appointment.org_id))
    .bind(uuid_str(appointment.store_id))
    .bind(opt_uuid_str(appointment.customer_id))
    .bind(opt_uuid_str(appointment.staff_id))
    .bind(&appointment.title)
    .bind(dt_str(appointment.starts_at))
    .bind(dt_str(appointment.ends_at))
    .bind(&appointment.status)
    .bind(&appointment.notes)
    .bind(dt_str(appointment.created_at))
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch one appointment by id.
pub async fn get_appointment(pool: &AnyPool, id: Uuid) -> Result<Appointment, DbError> {
    let row = sqlx::query("SELECT * FROM appointments WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("appointment {id}")))?;
    appointment_from_row(&row)
}

/// All appointments for a store, earliest first. Callers filter by
/// day / customer / staff in Rust.
pub async fn list_appointments_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<Appointment>, DbError> {
    let rows = sqlx::query("SELECT * FROM appointments WHERE store_id = ? ORDER BY starts_at ASC")
        .bind(uuid_str(store_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(appointment_from_row).collect()
}

/// Update the mutable fields of an appointment.
pub async fn update_appointment(pool: &AnyPool, appointment: &Appointment) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE appointments SET customer_id = ?, staff_id = ?, title = ?, starts_at = ?, ends_at = ?, status = ?, notes = ? WHERE id = ?",
    )
    .bind(opt_uuid_str(appointment.customer_id))
    .bind(opt_uuid_str(appointment.staff_id))
    .bind(&appointment.title)
    .bind(dt_str(appointment.starts_at))
    .bind(dt_str(appointment.ends_at))
    .bind(&appointment.status)
    .bind(&appointment.notes)
    .bind(uuid_str(appointment.id))
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete one appointment. No-op when the id does not exist.
pub async fn delete_appointment(pool: &AnyPool, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM appointments WHERE id = ?")
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- saved views ------------------------------------------------------------

/// Insert one saved view. The (store, name) unique constraint is
/// enforced by the schema; a duplicate name is a `DbError::Sqlx`.
pub async fn insert_crm_saved_view(pool: &AnyPool, view: &CrmSavedView) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO crm_saved_views (id, org_id, store_id, name, filter_json, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(view.id))
    .bind(uuid_str(view.org_id))
    .bind(uuid_str(view.store_id))
    .bind(&view.name)
    .bind(view.filter_json.to_string())
    .bind(dt_str(view.created_at))
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch one saved view by id.
pub async fn get_crm_saved_view(pool: &AnyPool, id: Uuid) -> Result<CrmSavedView, DbError> {
    let row = sqlx::query("SELECT * FROM crm_saved_views WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("saved view {id}")))?;
    saved_view_from_row(&row)
}

/// All saved views for a store, by name.
pub async fn list_crm_saved_views_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<CrmSavedView>, DbError> {
    let rows = sqlx::query("SELECT * FROM crm_saved_views WHERE store_id = ? ORDER BY name ASC")
        .bind(uuid_str(store_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(saved_view_from_row).collect()
}

/// Delete one saved view. No-op when the id does not exist.
pub async fn delete_crm_saved_view(pool: &AnyPool, id: Uuid) -> Result<(), DbError> {
    sqlx::query("DELETE FROM crm_saved_views WHERE id = ?")
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- deliveries -------------------------------------------------------------

/// Insert one delivery row.
pub async fn insert_delivery(pool: &AnyPool, delivery: &Delivery) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO deliveries (id, org_id, store_id, transaction_id, customer_id, driver_id, status, address_json, scheduled_date, priority, vehicle, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(delivery.id))
    .bind(uuid_str(delivery.org_id))
    .bind(uuid_str(delivery.store_id))
    .bind(opt_uuid_str(delivery.transaction_id))
    .bind(opt_uuid_str(delivery.customer_id))
    .bind(opt_uuid_str(delivery.driver_id))
    .bind(&delivery.status)
    .bind(delivery.address_json.to_string())
    .bind(opt_date_str(delivery.scheduled_date))
    .bind(delivery.priority)
    .bind(&delivery.vehicle)
    .bind(dt_str(delivery.created_at))
    .bind(dt_str(delivery.updated_at))
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch one delivery by id.
pub async fn get_delivery(pool: &AnyPool, id: Uuid) -> Result<Delivery, DbError> {
    let row = sqlx::query("SELECT * FROM deliveries WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("delivery {id}")))?;
    delivery_from_row(&row)
}

/// All deliveries for a store, newest first. Callers filter by
/// status / driver / date in Rust (dispatch board).
pub async fn list_deliveries_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<Delivery>, DbError> {
    let rows = sqlx::query("SELECT * FROM deliveries WHERE store_id = ? ORDER BY created_at DESC")
        .bind(uuid_str(store_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(delivery_from_row).collect()
}

/// Set a delivery's status, stamping `updated_at` to now. The
/// caller validates the transition (see `df-edge::delivery`).
pub async fn set_delivery_status(
    pool: &AnyPool,
    id: Uuid,
    status: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE deliveries SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status)
        .bind(dt_str(now))
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- proof of delivery ------------------------------------------------------

/// Insert one POD row.
pub async fn insert_delivery_pod(pool: &AnyPool, pod: &DeliveryPod) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO delivery_pods (id, delivery_id, signature_sha256, signature_b64, photo_ref, photo_sha256, received_by, signed_at, photo_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(pod.id))
    .bind(uuid_str(pod.delivery_id))
    .bind(&pod.signature_sha256)
    .bind(&pod.signature_b64)
    .bind(&pod.photo_ref)
    .bind(&pod.photo_sha256)
    .bind(&pod.received_by)
    .bind(dt_str(pod.signed_at))
    .bind(opt_dt_str(pod.photo_at))
    .bind(dt_str(pod.created_at))
    .execute(pool)
    .await?;
    Ok(())
}

/// The latest POD for a delivery, if one was submitted.
pub async fn get_delivery_pod_for_delivery(
    pool: &AnyPool,
    delivery_id: Uuid,
) -> Result<Option<DeliveryPod>, DbError> {
    let row = sqlx::query(
        "SELECT * FROM delivery_pods WHERE delivery_id = ? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(uuid_str(delivery_id))
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(row) => pod_from_row(&row).map(Some),
    }
}
