//! Delivery dispatch-board transitions, audit trail, POD-gated
//! close-out and packing-slip persistence (Phase 4, v0.5; spec §4
//! `delivery_logistics`, §7 Phase 4; domain rules in
//! `df-delivery`, migrations `0006_phase4` /
//! `0007_phase4_crm_delivery`).
//!
//! Basic delivery / POD CRUD lives in [`crate::crm`] (the wiring
//! worker's `0006` repos). This module adds the rules that sit on
//! top of those rows:
//!
//! * [`transition_delivery`] — a status change validated by the
//!   `df-delivery` state machine, written together with its
//!   [`DeliveryEvent`] audit row in one transaction, so the trail
//!   can never disagree with the row. A direct jump to
//!   `Delivered` is refused here — delivery closes out only
//!   through [`close_out_delivery`].
//! * [`close_out_delivery`] — the POD gate. The delivery must be
//!   `out_for_delivery` and its POD complete (signature AND
//!   photo). On an incomplete POD this returns
//!   [`DbError::PodIncomplete`] and writes **nothing**: no status
//!   change, no audit event, no order close-out. On success it
//!   marks the delivery `delivered`, writes the audit event, and
//!   auto-closes the linked order (`transactions` → `completed`)
//!   in the same transaction.
//! * [`list_dispatch_board`] — open deliveries for a store,
//!   highest priority first, then oldest first.
//! * Packing slips ([`render_and_store_packing_slip`]) — rendered
//!   from the linked sale's lines with `df-delivery`'s
//!   `PackingSlip` (plain text + ZPL) and persisted so the exact
//!   printed slip can be re-printed.

use chrono::{DateTime, Utc};
use df_delivery::{
    Delivery as DomainDelivery, DeliveryEvent as DomainDeliveryEvent, DeliveryStatus, PackingSlip,
    PackingSlipLine, PodPhoto, PodRecord, PodSignature,
};
use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{Delivery, DeliveryEvent, DeliveryPod, PackingSlipRow};

// --- row decoding -----------------------------------------------------------

fn delivery_event_from_row(row: &sqlx::any::AnyRow) -> Result<DeliveryEvent, DbError> {
    Ok(DeliveryEvent {
        id: uuid_col(row, "id")?,
        delivery_id: uuid_col(row, "delivery_id")?,
        from_status: text_col(row, "from_status")?,
        to_status: text_col(row, "to_status")?,
        actor_user_id: uuid_col(row, "actor_user_id")?,
        note: opt_text_col(row, "note")?,
        created_at: dt_col(row, "created_at")?,
    })
}

fn packing_slip_from_row(row: &sqlx::any::AnyRow) -> Result<PackingSlipRow, DbError> {
    Ok(PackingSlipRow {
        id: uuid_col(row, "id")?,
        delivery_id: uuid_col(row, "delivery_id")?,
        rendered_text: text_col(row, "rendered_text")?,
        rendered_zpl: text_col(row, "rendered_zpl")?,
        created_at: dt_col(row, "created_at")?,
    })
}

// --- domain conversions -----------------------------------------------------

/// Row `Delivery` → domain `Delivery` (status string parsed;
/// unknown statuses fail loudly).
fn delivery_to_domain(row: &Delivery) -> Result<DomainDelivery, DbError> {
    let status = DeliveryStatus::from_str(&row.status)
        .ok_or_else(|| DbError::InvalidData(format!("unknown delivery status: {}", row.status)))?;
    Ok(DomainDelivery {
        id: row.id,
        org_id: row.org_id,
        store_id: row.store_id,
        transaction_id: row.transaction_id,
        customer_id: row.customer_id,
        priority: row.priority,
        driver_id: row.driver_id,
        vehicle: row.vehicle.clone(),
        address_json: row.address_json.clone(),
        scheduled_date: row.scheduled_date,
        status,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

/// Row `DeliveryPod` → domain `PodRecord`. The stored base64
/// signature is decoded and checked against its SHA-256, so a
/// corrupt row fails here. A missing photo reference means "no
/// photo yet" — the record is simply incomplete.
fn pod_to_domain(pod: &DeliveryPod) -> Result<PodRecord, DbError> {
    let mut record = PodRecord::new(pod.delivery_id);
    record.id = pod.id;
    let signature = PodSignature::from_b64(
        &pod.signature_b64,
        &pod.signature_sha256,
        &pod.received_by,
        pod.signed_at,
    )?;
    record.record_signature(signature, pod.signed_at);
    if let Some(path) = &pod.photo_ref {
        let at = pod.photo_at.unwrap_or(pod.signed_at);
        let photo = PodPhoto::record(
            path.clone(),
            pod.photo_sha256.clone().unwrap_or_default(),
            at,
        )?;
        record.record_photo(photo, at);
    }
    Ok(record)
}

fn domain_event_to_row(event: &DomainDeliveryEvent, note: Option<String>) -> DeliveryEvent {
    DeliveryEvent {
        id: event.id,
        delivery_id: event.delivery_id,
        from_status: event.from_status.as_str().to_string(),
        to_status: event.to_status.as_str().to_string(),
        actor_user_id: event.actor_user_id,
        note,
        created_at: event.created_at,
    }
}

// --- audit trail ------------------------------------------------------------

/// Insert one delivery audit event. Executor-generic so the
/// transition / close-out functions can run it inside their
/// transaction.
pub async fn insert_delivery_event<'e, E>(executor: E, event: &DeliveryEvent) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO delivery_events (id, delivery_id, from_status, to_status, actor_user_id, note, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(uuid_str(event.id))
    .bind(uuid_str(event.delivery_id))
    .bind(&event.from_status)
    .bind(&event.to_status)
    .bind(uuid_str(event.actor_user_id))
    .bind(&event.note)
    .bind(dt_str(event.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

/// All audit events for a delivery, oldest first.
pub async fn list_delivery_events_for_delivery(
    pool: &AnyPool,
    delivery_id: Uuid,
) -> Result<Vec<DeliveryEvent>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM delivery_events WHERE delivery_id = $1 ORDER BY created_at ASC")
            .bind(uuid_str(delivery_id))
            .fetch_all(pool)
            .await?;
    rows.iter().map(delivery_event_from_row).collect()
}

// --- transitions + close-out ------------------------------------------------

/// Assign a driver (and optionally a vehicle) to a draft
/// delivery, enforcing the `df-delivery` state machine. The
/// driver + vehicle + status update and the audit event commit
/// in one transaction. Returns the audit event.
pub async fn assign_delivery(
    pool: &AnyPool,
    id: Uuid,
    driver_id: Uuid,
    vehicle: Option<String>,
    actor_user_id: Uuid,
    note: Option<String>,
    at: DateTime<Utc>,
) -> Result<DeliveryEvent, DbError> {
    let row = crate::crm::get_delivery(pool, id).await?;
    let mut domain = delivery_to_domain(&row)?;
    let domain_event =
        domain.assign(driver_id, vehicle.clone(), actor_user_id, at, note.clone())?;
    let event = domain_event_to_row(&domain_event, note);
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE deliveries SET driver_id = $1, vehicle = $2, status = $3, updated_at = $4 WHERE id = $5",
    )
    .bind(uuid_str(driver_id))
    .bind(&vehicle)
    .bind(domain.status.as_str())
    .bind(dt_str(at))
    .bind(uuid_str(id))
    .execute(&mut *tx)
    .await?;
    insert_delivery_event(&mut *tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Move a delivery to `to`, enforcing the `df-delivery` state
/// machine. The status update and the audit event commit in one
/// transaction. A direct jump to `Delivered` is refused — use
/// [`close_out_delivery`], which additionally requires a
/// complete POD. Returns the audit event.
pub async fn transition_delivery(
    pool: &AnyPool,
    id: Uuid,
    to: DeliveryStatus,
    actor_user_id: Uuid,
    note: Option<String>,
    at: DateTime<Utc>,
) -> Result<DeliveryEvent, DbError> {
    let row = crate::crm::get_delivery(pool, id).await?;
    let mut domain = delivery_to_domain(&row)?;
    let domain_event = domain.transition(to, actor_user_id, at, note.clone())?;
    let event = domain_event_to_row(&domain_event, note);
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE deliveries SET status = $1, updated_at = $2 WHERE id = $3")
        .bind(domain.status.as_str())
        .bind(dt_str(at))
        .bind(uuid_str(id))
        .execute(&mut *tx)
        .await?;
    insert_delivery_event(&mut *tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Close out a delivery as `Delivered`, gated on a complete POD
/// (signature AND photo). The delivery must be
/// `out_for_delivery`. On an incomplete POD — including no POD
/// row at all — this returns [`DbError::PodIncomplete`] and
/// writes nothing: no status change, no audit event, no order
/// close-out. On success the delivery becomes `delivered`, the
/// audit event is written, and the linked order (if any) is
/// auto-closed (`transactions` → `completed`), all in one
/// transaction. Returns the audit event.
pub async fn close_out_delivery(
    pool: &AnyPool,
    id: Uuid,
    actor_user_id: Uuid,
    at: DateTime<Utc>,
) -> Result<DeliveryEvent, DbError> {
    let row = crate::crm::get_delivery(pool, id).await?;
    let pod_row = crate::crm::get_delivery_pod_for_delivery(pool, id).await?;
    let pod = match &pod_row {
        Some(pod_row) => pod_to_domain(pod_row)?,
        None => PodRecord::new(id),
    };
    let mut domain = delivery_to_domain(&row)?;
    let domain_event = domain.close_out(&pod, actor_user_id, at)?;
    let event = domain_event_to_row(
        &domain_event,
        Some(format!(
            "POD complete: signature by {} + photo",
            pod.signature
                .as_ref()
                .map(|s| s.signer_name.as_str())
                .unwrap_or("?")
        )),
    );
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE deliveries SET status = $1, updated_at = $2 WHERE id = $3")
        .bind(domain.status.as_str())
        .bind(dt_str(at))
        .bind(uuid_str(id))
        .execute(&mut *tx)
        .await?;
    insert_delivery_event(&mut *tx, &event).await?;
    // Auto-close the linked order (only when it is still open).
    if let Some(transaction_id) = row.transaction_id {
        sqlx::query(
            "UPDATE transactions SET status = 'completed', completed_at = $1 WHERE id = $2 AND status IN ('open', 'parked')",
        )
        .bind(dt_str(at))
        .bind(uuid_str(transaction_id))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(event)
}

/// The dispatch board for a store: open deliveries only
/// (`draft` / `assigned` / `out_for_delivery`), highest priority
/// first, then oldest first.
pub async fn list_dispatch_board(pool: &AnyPool, store_id: Uuid) -> Result<Vec<Delivery>, DbError> {
    let mut rows = crate::crm::list_deliveries_for_store(pool, store_id).await?;
    rows.retain(|d| matches!(d.status.as_str(), "draft" | "assigned" | "out_for_delivery"));
    rows.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    Ok(rows)
}

// --- packing slips ----------------------------------------------------------

/// Insert one rendered packing slip row.
pub async fn insert_packing_slip(pool: &AnyPool, slip: &PackingSlipRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO packing_slips (id, delivery_id, rendered_text, rendered_zpl, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid_str(slip.id))
    .bind(uuid_str(slip.delivery_id))
    .bind(&slip.rendered_text)
    .bind(&slip.rendered_zpl)
    .bind(dt_str(slip.created_at))
    .execute(pool)
    .await?;
    Ok(())
}

/// The stored packing slip for a delivery, if one was rendered.
pub async fn get_packing_slip_for_delivery(
    pool: &AnyPool,
    delivery_id: Uuid,
) -> Result<Option<PackingSlipRow>, DbError> {
    let row = sqlx::query(
        "SELECT * FROM packing_slips WHERE delivery_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(uuid_str(delivery_id))
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(row) => packing_slip_from_row(&row).map(Some),
    }
}

/// Render a packing slip for a delivery from the linked sale's
/// lines (empty when the delivery names no transaction) and
/// persist it. `store_name` is printed in the slip header.
/// Returns the stored row.
pub async fn render_and_store_packing_slip(
    pool: &AnyPool,
    delivery_id: Uuid,
    store_name: &str,
) -> Result<PackingSlipRow, DbError> {
    let delivery = crate::crm::get_delivery(pool, delivery_id).await?;
    let mut doc_number = String::new();
    let mut lines = Vec::new();
    if let Some(transaction_id) = delivery.transaction_id {
        let transaction = crate::get_transaction(pool, transaction_id).await?;
        doc_number = transaction.doc_number;
        for line in crate::list_transaction_lines(pool, transaction_id).await? {
            lines.push(PackingSlipLine {
                description: line.description,
                qty: line.qty,
                uom_code: line.uom_code,
            });
        }
    }
    let customer_name = match delivery.customer_id {
        Some(customer_id) => match crate::ar::get_customer(pool, customer_id).await {
            Ok(customer) => customer_display_name(&customer),
            Err(_) => "Customer".to_string(),
        },
        None => "Customer".to_string(),
    };
    let address_lines = address_lines_from_json(&delivery.address_json);
    let mut slip = PackingSlip::new(
        delivery_id,
        store_name,
        doc_number,
        customer_name,
        address_lines,
        lines,
        delivery.priority,
    );
    slip.vehicle = delivery.vehicle.clone();
    let row = PackingSlipRow {
        id: slip.id,
        delivery_id,
        rendered_text: slip.render_text(),
        rendered_zpl: slip.render_zpl(),
        created_at: Utc::now(),
    };
    insert_packing_slip(pool, &row).await?;
    Ok(row)
}

fn customer_display_name(customer: &crate::models::Customer) -> String {
    if let Some(company) = &customer.company_name {
        if !company.trim().is_empty() {
            return company.clone();
        }
    }
    let parts: Vec<&str> = customer
        .first_name
        .iter()
        .chain(customer.last_name.iter())
        .map(|s| s.as_str())
        .collect();
    if parts.is_empty() {
        customer.account_number.clone()
    } else {
        parts.join(" ")
    }
}

fn address_lines_from_json(address_json: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    let get = |key: &str| {
        address_json
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    if let Some(street) = get("street") {
        lines.push(street);
    }
    let city_line = match (get("city"), get("region"), get("postal_code")) {
        (Some(city), Some(region), Some(postal)) => format!("{city}, {region} {postal}"),
        (Some(city), None, Some(postal)) => format!("{city} {postal}"),
        (Some(city), _, _) => city,
        _ => String::new(),
    };
    if !city_line.is_empty() {
        lines.push(city_line);
    }
    lines
}
