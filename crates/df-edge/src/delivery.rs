//! Delivery dispatch + packing slips + driver proof-of-delivery
//! on the edge (Phase 4, v0.5; spec §6.1/§7).
//!
//! The rules live in `df-delivery` (pure domain: the dispatch
//! state machine, POD completeness, packing-slip rendering) and
//! in `df-db::delivery` (the repositories that apply those rules
//! to the migration `0006_phase4` / `0007_phase4_crm_delivery`
//! tables — transitions with an audit trail, POD-gated
//! close-out, dispatch board, persisted packing slips). This
//! module is the thin wiring layer: request validation, row ↔
//! domain conversion at the boundary, and persistence calls. It
//! holds no delivery rules of its own.
//!
//! Two rules worth knowing before touching this file:
//!
//! * `delivered` is only reachable through a POD submission —
//!   the status endpoint refuses a direct jump, and
//!   `df-db::close_out_delivery` refuses an incomplete POD
//!   (signature AND photo) and writes nothing. A failed
//!   close-out leaves the delivery `out_for_delivery` with no
//!   audit event.
//! * Closing out a delivery auto-closes its linked order
//!   (`transactions` → `completed`, when still open) in the same
//!   transaction — see `df-db::delivery::close_out_delivery`.
//!
//! The POD photo is a file reference plus SHA-256; the photo
//! bytes are never stored and no camera hardware is involved.
//! All money is `rust_decimal::Decimal` — never floats.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{DateTime, NaiveDate, Utc};
use df_db::models::{Delivery, DeliveryEvent, DeliveryPod, PackingSlipRow};
use df_delivery::{Delivery as DomainDelivery, DeliveryStatus, PodPhoto, PodRecord, PodSignature};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

// --- request / view types ---------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateDeliveryRequest {
    pub transaction_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub address_json: Value,
    pub scheduled_date: Option<NaiveDate>,
    /// Dispatch urgency (higher = more urgent). Defaults to 0.
    pub priority: Option<i32>,
    pub vehicle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssignDeliveryRequest {
    pub driver_id: Uuid,
    pub vehicle: Option<String>,
    /// Dispatcher performing the assignment (audit trail). When
    /// absent, the nil UUID is recorded — the edge has no auth
    /// context yet, so pass the dispatcher's user id when known.
    pub actor_user_id: Option<Uuid>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetDeliveryStatusRequest {
    pub status: String,
    /// Dispatcher performing the change (audit trail). When
    /// absent, the nil UUID is recorded.
    pub actor_user_id: Option<Uuid>,
    pub note: Option<String>,
}

/// POD submission: base64 signature bytes + a photo reference.
/// `signed_at` defaults to now. The signature is decoded and
/// hashed by `df-delivery`; close-out requires the photo too —
/// a submission without one is a 400 and writes nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitPodRequest {
    pub signature_b64: String,
    pub photo_ref: Option<String>,
    pub photo_sha256: Option<String>,
    pub received_by: String,
    pub signed_at: Option<DateTime<Utc>>,
    /// Dispatcher closing the delivery out (audit trail). When
    /// absent, the nil UUID is recorded.
    pub actor_user_id: Option<Uuid>,
}

/// A delivery with its POD (when submitted) and its audit trail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryView {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub transaction_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub driver_id: Option<Uuid>,
    pub status: String,
    pub address_json: Value,
    pub scheduled_date: Option<NaiveDate>,
    pub priority: i32,
    pub vehicle: Option<String>,
    pub pod: Option<DeliveryPod>,
    pub events: Vec<DeliveryEvent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One line on a packing slip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackingSlipLine {
    pub product_id: Uuid,
    pub description: String,
    pub qty: Decimal,
    pub uom: String,
}

/// A packing slip for a delivery: the linked sale's lines (empty
/// when the delivery names no transaction), plus the persisted
/// renderings from `df-delivery::PackingSlip` (42-column text +
/// ZPL label). The rendered slip is stored on first fetch so the
/// exact printed slip can be re-printed later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackingSlipView {
    pub delivery_id: Uuid,
    pub transaction_id: Option<Uuid>,
    pub doc_number: Option<String>,
    pub customer_id: Option<Uuid>,
    pub address_json: Value,
    pub scheduled_date: Option<NaiveDate>,
    pub status: String,
    pub lines: Vec<PackingSlipLine>,
    pub text: String,
    pub zpl: String,
}

// --- helpers ----------------------------------------------------------------

fn actor_or_nil(actor_user_id: Option<Uuid>) -> Uuid {
    actor_user_id.unwrap_or_else(Uuid::nil)
}

fn delivery_status_from_str(value: &str) -> Result<DeliveryStatus, EdgeError> {
    DeliveryStatus::from_str(value).ok_or_else(|| {
        EdgeError::InvalidInput(format!(
            "unknown delivery status {value:?} (draft, assigned, out_for_delivery, delivered, cancelled)"
        ))
    })
}

fn is_hex64(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

impl Edge {
    async fn delivery_view(&self, delivery: Delivery) -> Result<DeliveryView, EdgeError> {
        let pod = df_db::get_delivery_pod_for_delivery(self.pool(), delivery.id).await?;
        let events = df_db::list_delivery_events_for_delivery(self.pool(), delivery.id).await?;
        Ok(DeliveryView {
            id: delivery.id,
            org_id: delivery.org_id,
            store_id: delivery.store_id,
            transaction_id: delivery.transaction_id,
            customer_id: delivery.customer_id,
            driver_id: delivery.driver_id,
            status: delivery.status,
            address_json: delivery.address_json,
            scheduled_date: delivery.scheduled_date,
            priority: delivery.priority,
            vehicle: delivery.vehicle,
            pod,
            events,
            created_at: delivery.created_at,
            updated_at: delivery.updated_at,
        })
    }

    async fn get_delivery_row(&self, id: Uuid) -> Result<Delivery, EdgeError> {
        df_db::get_delivery(self.pool(), id)
            .await
            .map_err(|err| match err {
                df_db::DbError::NotFound(_) => EdgeError::NotFound(format!("delivery {id}")),
                other => EdgeError::from(other),
            })
    }

    // --- dispatch board -----------------------------------------------------

    /// Create a delivery job. When `transaction_id` is given it
    /// must exist; when `customer_id` is given it must exist. The
    /// address must be a JSON object.
    pub async fn create_delivery(
        &self,
        body: CreateDeliveryRequest,
    ) -> Result<DeliveryView, EdgeError> {
        if !body.address_json.is_object() {
            return Err(EdgeError::InvalidInput(
                "address_json must be a JSON object".to_string(),
            ));
        }
        if let Some(transaction_id) = body.transaction_id {
            df_db::get_transaction(self.pool(), transaction_id).await?;
        }
        if let Some(customer_id) = body.customer_id {
            df_db::get_customer(self.pool(), customer_id).await?;
        }
        let mut domain = DomainDelivery::new(
            self.config().org_id,
            self.config().store_id,
            body.transaction_id,
        );
        domain.customer_id = body.customer_id;
        domain.address_json = body.address_json;
        domain.scheduled_date = body.scheduled_date;
        domain.priority = body.priority.unwrap_or(0);
        domain.vehicle = body.vehicle;
        let delivery = Delivery {
            id: domain.id,
            org_id: domain.org_id,
            store_id: domain.store_id,
            transaction_id: domain.transaction_id,
            customer_id: domain.customer_id,
            driver_id: None,
            status: domain.status.as_str().to_string(),
            address_json: domain.address_json,
            scheduled_date: domain.scheduled_date,
            priority: domain.priority,
            vehicle: domain.vehicle,
            created_at: domain.created_at,
            updated_at: domain.updated_at,
        };
        df_db::insert_delivery(self.pool(), &delivery).await?;
        self.delivery_view(delivery).await
    }

    /// Fetch one delivery, with its POD and audit trail.
    pub async fn get_delivery_view(&self, id: Uuid) -> Result<DeliveryView, EdgeError> {
        let delivery = self.get_delivery_row(id).await?;
        self.delivery_view(delivery).await
    }

    /// The dispatch board: this store's open deliveries, highest
    /// priority first, then oldest first
    /// (`df-db::list_dispatch_board`), optionally filtered by
    /// status, driver and scheduled date. Delivered / cancelled
    /// rows are excluded from the board itself — fetch one by id
    /// to see them.
    pub async fn list_deliveries(
        &self,
        status: Option<&str>,
        driver_id: Option<Uuid>,
        scheduled_date: Option<NaiveDate>,
    ) -> Result<Vec<DeliveryView>, EdgeError> {
        if let Some(status) = status {
            delivery_status_from_str(status)?;
        }
        let mut deliveries =
            df_db::list_dispatch_board(self.pool(), self.config().store_id).await?;
        if let Some(status) = status {
            deliveries.retain(|d| d.status == status);
        }
        if let Some(driver_id) = driver_id {
            deliveries.retain(|d| d.driver_id == Some(driver_id));
        }
        if let Some(scheduled_date) = scheduled_date {
            deliveries.retain(|d| d.scheduled_date == Some(scheduled_date));
        }
        let mut views = Vec::with_capacity(deliveries.len());
        for delivery in deliveries {
            views.push(self.delivery_view(delivery).await?);
        }
        Ok(views)
    }

    /// Assign a driver (and optionally a vehicle) to a draft
    /// delivery, via the `df-delivery` state machine
    /// (`df-db::assign_delivery`). The driver, vehicle, status
    /// and audit event commit together.
    pub async fn assign_delivery(
        &self,
        id: Uuid,
        body: AssignDeliveryRequest,
    ) -> Result<DeliveryView, EdgeError> {
        self.get_delivery_row(id).await?;
        df_db::assign_delivery(
            self.pool(),
            id,
            body.driver_id,
            body.vehicle,
            actor_or_nil(body.actor_user_id),
            body.note,
            Utc::now(),
        )
        .await?;
        self.get_delivery_view(id).await
    }

    /// Move a delivery along the status machine
    /// (`df-db::transition_delivery`). A direct jump to
    /// `delivered` is refused — close the delivery out with a
    /// POD.
    pub async fn set_delivery_status(
        &self,
        id: Uuid,
        body: SetDeliveryStatusRequest,
    ) -> Result<DeliveryView, EdgeError> {
        let target = delivery_status_from_str(&body.status)?;
        if target == DeliveryStatus::Delivered {
            return Err(EdgeError::InvalidInput(
                "a delivery cannot be set to delivered directly — submit a proof of delivery (POST /deliveries/{id}/pod)".to_string(),
            ));
        }
        self.get_delivery_row(id).await?;
        df_db::transition_delivery(
            self.pool(),
            id,
            target,
            actor_or_nil(body.actor_user_id),
            body.note,
            Utc::now(),
        )
        .await?;
        self.get_delivery_view(id).await
    }

    // --- packing slip -------------------------------------------------------

    /// Build the packing slip for a delivery: the linked sale's
    /// lines (empty when the delivery names no transaction), plus
    /// the persisted `df-delivery` renderings (42-column text +
    /// ZPL). The slip is rendered and stored on first fetch and
    /// re-used after that, so the exact printed slip can be
    /// re-printed.
    pub async fn packing_slip(&self, id: Uuid) -> Result<PackingSlipView, EdgeError> {
        let delivery = self.get_delivery_row(id).await?;
        let stored: PackingSlipRow =
            match df_db::get_packing_slip_for_delivery(self.pool(), id).await? {
                Some(row) => row,
                None => {
                    df_db::render_and_store_packing_slip(self.pool(), id, &self.config().store_name)
                        .await?
                }
            };
        let mut lines = Vec::new();
        let mut doc_number = None;
        if let Some(transaction_id) = delivery.transaction_id {
            let transaction = df_db::get_transaction(self.pool(), transaction_id).await?;
            doc_number = Some(transaction.doc_number);
            for line in df_db::list_transaction_lines(self.pool(), transaction_id).await? {
                lines.push(PackingSlipLine {
                    product_id: line.product_id,
                    description: line.description,
                    qty: line.qty,
                    uom: line.uom_code,
                });
            }
        }
        Ok(PackingSlipView {
            delivery_id: delivery.id,
            transaction_id: delivery.transaction_id,
            doc_number,
            customer_id: delivery.customer_id,
            address_json: delivery.address_json,
            scheduled_date: delivery.scheduled_date,
            status: delivery.status,
            lines,
            text: stored.rendered_text,
            zpl: stored.rendered_zpl,
        })
    }

    // --- proof of delivery --------------------------------------------------

    /// Submit proof of delivery and close the delivery out. The
    /// signature bytes (base64) and the photo reference are
    /// assembled into a `df-delivery::PodRecord`; close-out
    /// requires both parts, and on an incomplete POD nothing is
    /// written — no pod row, no status change, no audit event.
    /// On success the delivery becomes `delivered`, the audit
    /// event is written, and the linked order auto-closes.
    pub async fn submit_pod(
        &self,
        id: Uuid,
        body: SubmitPodRequest,
    ) -> Result<DeliveryView, EdgeError> {
        let delivery = self.get_delivery_row(id).await?;
        if delivery.status != DeliveryStatus::OutForDelivery.as_str() {
            return Err(EdgeError::InvalidInput(format!(
                "delivery {id} is {} — a POD can only be submitted while out for delivery",
                delivery.status
            )));
        }
        if body.received_by.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "received_by must not be empty".to_string(),
            ));
        }
        if let Some(photo_sha256) = &body.photo_sha256
            && !is_hex64(photo_sha256)
        {
            return Err(EdgeError::InvalidInput(
                "photo_sha256 must be 64 hex characters".to_string(),
            ));
        }
        let signature_bytes = BASE64_STANDARD
            .decode(body.signature_b64.trim())
            .map_err(|_| {
                EdgeError::InvalidInput("signature_b64 is not valid base64".to_string())
            })?;
        let signed_at = body.signed_at.unwrap_or_else(Utc::now);
        let signature =
            PodSignature::capture(signature_bytes, body.received_by.clone(), signed_at)?;
        let mut pod = PodRecord::new(id);
        pod.record_signature(signature, signed_at);
        if let Some(photo_ref) = &body.photo_ref {
            let photo = PodPhoto::record(
                photo_ref.clone(),
                body.photo_sha256.clone().unwrap_or_default(),
                signed_at,
            )?;
            pod.record_photo(photo, signed_at);
        }
        if !pod.is_complete() {
            return Err(EdgeError::InvalidInput(format!(
                "cannot close out delivery {id}: proof of delivery incomplete, missing {}",
                pod.missing_parts().join(", ")
            )));
        }
        // The POD is complete: persist the evidence row, then
        // close out (which re-verifies the stored row through
        // the domain and writes status + audit event + order
        // close-out in one transaction).
        let now = Utc::now();
        let signature = pod.signature.as_ref().expect("pod is complete");
        let photo = pod.photo.as_ref().expect("pod is complete");
        let pod_row = DeliveryPod {
            id: pod.id,
            delivery_id: id,
            signature_sha256: signature.sha256_hex.clone(),
            signature_b64: signature.to_b64(),
            photo_ref: Some(photo.path.clone()),
            photo_sha256: Some(photo.sha256_hex.clone()),
            photo_at: Some(photo.captured_at),
            received_by: signature.signer_name.clone(),
            signed_at: signature.captured_at,
            created_at: now,
        };
        df_db::insert_delivery_pod(self.pool(), &pod_row).await?;
        df_db::close_out_delivery(self.pool(), id, actor_or_nil(body.actor_user_id), now).await?;
        self.get_delivery_view(id).await
    }

    /// The POD for a delivery, when one was submitted.
    pub async fn get_pod(&self, id: Uuid) -> Result<Option<DeliveryPod>, EdgeError> {
        self.get_delivery_row(id).await?;
        Ok(df_db::get_delivery_pod_for_delivery(self.pool(), id).await?)
    }
}
