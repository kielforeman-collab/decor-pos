//! Scheduling+ repositories (Phase 6, v1.0; spec §4
//! `scheduling_plus`, §5.5): shift templates, staff shifts and
//! timeclock entries on the migration `0011_phase6` tables.
//!
//! The domain rules live in `df-scheduling` (template validation,
//! shift overlap, clock-in/out guards, forecasting, coverage);
//! this module is persistence only. Write functions are generic
//! over any sqlx executor so the edge can run them inside one
//! local SQLite transaction; read functions take `&AnyPool`.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{ShiftTemplate, StaffShift, TimeclockEntry};

fn shift_template_from_row(row: &sqlx::any::AnyRow) -> Result<ShiftTemplate, DbError> {
    Ok(ShiftTemplate {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        name: text_col(row, "name")?,
        weekday: i32_col(row, "weekday")?,
        starts_at: text_col(row, "starts_at")?,
        ends_at: text_col(row, "ends_at")?,
        role: opt_text_col(row, "role")?,
        staff_id: opt_uuid_col(row, "staff_id")?,
        required_count: i32_col(row, "required_count")?,
        is_active: bool_col(row, "is_active")?,
    })
}

fn staff_shift_from_row(row: &sqlx::any::AnyRow) -> Result<StaffShift, DbError> {
    Ok(StaffShift {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        template_id: opt_uuid_col(row, "template_id")?,
        staff_id: uuid_col(row, "staff_id")?,
        shift_date: date_col(row, "shift_date")?,
        starts_at: dt_col(row, "starts_at")?,
        ends_at: dt_col(row, "ends_at")?,
        status: text_col(row, "status")?,
        notes: opt_text_col(row, "notes")?,
    })
}

fn timeclock_entry_from_row(row: &sqlx::any::AnyRow) -> Result<TimeclockEntry, DbError> {
    Ok(TimeclockEntry {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        store_id: uuid_col(row, "store_id")?,
        staff_id: uuid_col(row, "staff_id")?,
        clock_in: dt_col(row, "clock_in")?,
        clock_out: opt_dt_col(row, "clock_out")?,
        source: text_col(row, "source")?,
    })
}

// --- shift templates ---------------------------------------------------------

pub async fn insert_shift_template<'e, E>(
    executor: E,
    template: &ShiftTemplate,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO shift_templates (id, org_id, store_id, name, weekday, starts_at, ends_at, role, staff_id, required_count, is_active) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(template.id))
    .bind(uuid_str(template.org_id))
    .bind(uuid_str(template.store_id))
    .bind(&template.name)
    .bind(template.weekday)
    .bind(&template.starts_at)
    .bind(&template.ends_at)
    .bind(&template.role)
    .bind(opt_uuid_str(template.staff_id))
    .bind(template.required_count)
    .bind(template.is_active)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn list_shift_templates_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<ShiftTemplate>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM shift_templates WHERE store_id = ? ORDER BY weekday ASC, starts_at ASC",
    )
    .bind(uuid_str(store_id))
    .fetch_all(pool)
    .await?;
    rows.iter().map(shift_template_from_row).collect()
}

// --- staff shifts ------------------------------------------------------------

pub async fn insert_staff_shift<'e, E>(executor: E, shift: &StaffShift) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO staff_shifts (id, org_id, store_id, template_id, staff_id, shift_date, starts_at, ends_at, status, notes) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(shift.id))
    .bind(uuid_str(shift.org_id))
    .bind(uuid_str(shift.store_id))
    .bind(opt_uuid_str(shift.template_id))
    .bind(uuid_str(shift.staff_id))
    .bind(date_str(shift.shift_date))
    .bind(dt_str(shift.starts_at))
    .bind(dt_str(shift.ends_at))
    .bind(&shift.status)
    .bind(&shift.notes)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn get_staff_shift(pool: &AnyPool, id: Uuid) -> Result<StaffShift, DbError> {
    let row = sqlx::query("SELECT * FROM staff_shifts WHERE id = ?")
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("staff shift {id}")))?;
    staff_shift_from_row(&row)
}

pub async fn list_staff_shifts_for_store_date(
    pool: &AnyPool,
    store_id: Uuid,
    date: NaiveDate,
) -> Result<Vec<StaffShift>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM staff_shifts WHERE store_id = ? AND shift_date = ? ORDER BY starts_at ASC",
    )
    .bind(uuid_str(store_id))
    .bind(date_str(date))
    .fetch_all(pool)
    .await?;
    rows.iter().map(staff_shift_from_row).collect()
}

/// Every shift for a store in a date range (inclusive), oldest
/// first — the coverage check reads a whole week this way.
pub async fn list_staff_shifts_for_store_range(
    pool: &AnyPool,
    store_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<StaffShift>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM staff_shifts WHERE store_id = ? AND shift_date >= ? AND shift_date <= ? ORDER BY shift_date ASC, starts_at ASC",
    )
    .bind(uuid_str(store_id))
    .bind(date_str(from))
    .bind(date_str(to))
    .fetch_all(pool)
    .await?;
    rows.iter().map(staff_shift_from_row).collect()
}

pub async fn set_staff_shift_status(pool: &AnyPool, id: Uuid, status: &str) -> Result<(), DbError> {
    sqlx::query("UPDATE staff_shifts SET status = ? WHERE id = ?")
        .bind(status)
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

// --- timeclock entries ---------------------------------------------------------

pub async fn insert_timeclock_entry<'e, E>(
    executor: E,
    entry: &TimeclockEntry,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO timeclock_entries (id, org_id, store_id, staff_id, clock_in, clock_out, source) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid_str(entry.id))
    .bind(uuid_str(entry.org_id))
    .bind(uuid_str(entry.store_id))
    .bind(uuid_str(entry.staff_id))
    .bind(dt_str(entry.clock_in))
    .bind(opt_dt_str(entry.clock_out))
    .bind(&entry.source)
    .execute(executor)
    .await?;
    Ok(())
}

/// The staff member's open entry (clocked in, not yet out), if
/// any. At most one can exist — the domain guards it, and the
/// edge checks before inserting.
pub async fn get_open_timeclock_entry(
    pool: &AnyPool,
    staff_id: Uuid,
) -> Result<Option<TimeclockEntry>, DbError> {
    let row = sqlx::query(
        "SELECT * FROM timeclock_entries WHERE staff_id = ? AND clock_out IS NULL ORDER BY clock_in DESC",
    )
    .bind(uuid_str(staff_id))
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(timeclock_entry_from_row(&row)?)),
        None => Ok(None),
    }
}

pub async fn close_timeclock_entry(
    pool: &AnyPool,
    id: Uuid,
    clock_out: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE timeclock_entries SET clock_out = ? WHERE id = ? AND clock_out IS NULL")
        .bind(dt_str(clock_out))
        .bind(uuid_str(id))
        .execute(pool)
        .await?;
    Ok(())
}

/// Closed timeclock entries for a store whose clock-in falls in
/// the window — the labor history the forecast reads.
pub async fn list_closed_timeclock_entries_for_store_range(
    pool: &AnyPool,
    store_id: Uuid,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<TimeclockEntry>, DbError> {
    let rows = sqlx::query(
        "SELECT * FROM timeclock_entries WHERE store_id = ? AND clock_in >= ? AND clock_in < ? AND clock_out IS NOT NULL ORDER BY clock_in ASC",
    )
    .bind(uuid_str(store_id))
    .bind(dt_str(from))
    .bind(dt_str(to))
    .fetch_all(pool)
    .await?;
    rows.iter().map(timeclock_entry_from_row).collect()
}

/// All timeclock entries for a store on one UTC date, newest
/// first — the timeclock board in the back office.
pub async fn list_timeclock_entries_for_store_date(
    pool: &AnyPool,
    store_id: Uuid,
    date: NaiveDate,
) -> Result<Vec<TimeclockEntry>, DbError> {
    let start = date
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc();
    let end = start + chrono::Duration::days(1);
    let rows = sqlx::query(
        "SELECT * FROM timeclock_entries WHERE store_id = ? AND clock_in >= ? AND clock_in < ? ORDER BY clock_in DESC",
    )
    .bind(uuid_str(store_id))
    .bind(dt_str(start))
    .bind(dt_str(end))
    .fetch_all(pool)
    .await?;
    rows.iter().map(timeclock_entry_from_row).collect()
}
