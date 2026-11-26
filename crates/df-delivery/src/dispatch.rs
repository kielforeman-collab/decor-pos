//! Dispatch board (spec §4 `delivery_logistics`, §7 Phase 4).
//!
//! A delivery carries one order (`transactions.id`, usually an
//! order awaiting delivery) through a small enforced state
//! machine:
//!
//! ```text
//! awaiting ──► assigned ──► out-for-delivery ──► delivered
//!     │            │               │
//!     └────────────┴───────────────┴──► failed
//! ```
//!
//! `delivered` and `failed` are terminal. Every transition is
//! recorded as a [`DeliveryEvent`] (who / when / from / to), so
//! the board has a complete audit trail. Priority is an integer
//! where a **higher number means more urgent**; the dispatch
//! board sorts priority descending, then oldest first.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DeliveryError, Result};

/// Delivery lifecycle status.
///
/// The variant names are the task's words (`awaiting`, `failed`);
/// the storage strings are migration `0006_phase4`'s
/// (`draft`, `cancelled`) — the same strings `df-edge`'s
/// `DELIVERY_*` constants use. [`DeliveryStatus::as_str`] /
/// [`DeliveryStatus::from_str`] translate between the two, so
/// callers speak one vocabulary and the database the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryStatus {
    /// On the board, not yet assigned to a driver. Stored as
    /// `"draft"`.
    #[serde(rename = "draft")]
    Awaiting,
    /// Assigned to a driver / vehicle, not yet departed.
    Assigned,
    /// Driver has left with the order.
    #[serde(rename = "out_for_delivery")]
    OutForDelivery,
    /// Delivered (only reachable via a complete POD — see
    /// [`crate::pod`]).
    Delivered,
    /// Delivery failed / cancelled. Stored as `"cancelled"`.
    #[serde(rename = "cancelled")]
    Failed,
}

impl DeliveryStatus {
    /// Stable storage string (used as the `status` column).
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryStatus::Awaiting => "draft",
            DeliveryStatus::Assigned => "assigned",
            DeliveryStatus::OutForDelivery => "out_for_delivery",
            DeliveryStatus::Delivered => "delivered",
            DeliveryStatus::Failed => "cancelled",
        }
    }

    /// Is this a terminal status (no outgoing transitions)?
    pub fn is_terminal(self) -> bool {
        matches!(self, DeliveryStatus::Delivered | DeliveryStatus::Failed)
    }

    /// The statuses `self` may move to. `Delivered` is listed
    /// here only so the transition table is explicit — the domain
    /// still requires a complete POD for it (see
    /// [`Delivery::close_out`]).
    pub fn allowed_next(self) -> &'static [DeliveryStatus] {
        match self {
            DeliveryStatus::Awaiting => &[DeliveryStatus::Assigned, DeliveryStatus::Failed],
            DeliveryStatus::Assigned => &[DeliveryStatus::OutForDelivery, DeliveryStatus::Failed],
            DeliveryStatus::OutForDelivery => &[DeliveryStatus::Delivered, DeliveryStatus::Failed],
            DeliveryStatus::Delivered | DeliveryStatus::Failed => &[],
        }
    }

    /// Parse a storage string back. Unknown values are rejected
    /// so a bad row fails loudly.
    ///
    /// Deliberately an inherent method rather than `FromStr`
    /// (see `df_crm::ActivityType::from_str`).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "draft" | "awaiting" => Some(DeliveryStatus::Awaiting),
            "assigned" => Some(DeliveryStatus::Assigned),
            "out_for_delivery" => Some(DeliveryStatus::OutForDelivery),
            "delivered" => Some(DeliveryStatus::Delivered),
            "cancelled" | "failed" => Some(DeliveryStatus::Failed),
            _ => None,
        }
    }
}

/// One row of `deliveries` (migration `0006_phase4`, plus the
/// `priority` / `vehicle` columns migration
/// `0007_phase4_crm_delivery` adds).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// UUIDv7 primary key.
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    /// The order being delivered (`transactions.id`), if the
    /// delivery names one — NULL for a standalone drop.
    pub transaction_id: Option<Uuid>,
    /// The customer receiving it, if known.
    pub customer_id: Option<Uuid>,
    /// Higher number = more urgent.
    pub priority: i32,
    /// Assigned driver (staff user id), if any.
    pub driver_id: Option<Uuid>,
    /// Assigned vehicle label, e.g. `"Van 2"`, if any.
    pub vehicle: Option<String>,
    /// Free-form delivery address object (`street`, `city`,
    /// `postal_code`, …).
    pub address_json: serde_json::Value,
    /// Planned delivery day, if one is set.
    pub scheduled_date: Option<chrono::NaiveDate>,
    pub status: DeliveryStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Delivery {
    /// Build a new delivery in `Awaiting` status with a fresh
    /// UUIDv7 id, no priority, and an empty address object.
    pub fn new(org_id: Uuid, store_id: Uuid, transaction_id: Option<Uuid>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            transaction_id,
            customer_id: None,
            priority: 0,
            driver_id: None,
            vehicle: None,
            address_json: serde_json::json!({}),
            scheduled_date: None,
            status: DeliveryStatus::Awaiting,
            created_at: now,
            updated_at: now,
        }
    }

    /// Assign a driver (and optionally a vehicle). Only valid
    /// from `Awaiting`; moves the delivery to `Assigned` and
    /// returns the audit event.
    pub fn assign(
        &mut self,
        driver_id: Uuid,
        vehicle: Option<String>,
        actor_user_id: Uuid,
        at: DateTime<Utc>,
        note: Option<String>,
    ) -> Result<DeliveryEvent> {
        let event = self.transition(DeliveryStatus::Assigned, actor_user_id, at, note)?;
        self.driver_id = Some(driver_id);
        self.vehicle = vehicle;
        Ok(event)
    }

    /// Move to `to`, enforcing the state machine. Returns the
    /// audit event on success. `Delivered` is rejected here —
    /// use [`Delivery::close_out`] instead, which additionally
    /// requires a complete POD.
    pub fn transition(
        &mut self,
        to: DeliveryStatus,
        actor_user_id: Uuid,
        at: DateTime<Utc>,
        note: Option<String>,
    ) -> Result<DeliveryEvent> {
        if to == DeliveryStatus::Delivered {
            return Err(DeliveryError::IllegalTransition {
                from: self.status.as_str().to_string(),
                to: to.as_str().to_string(),
            });
        }
        if !self.status.allowed_next().contains(&to) {
            return Err(DeliveryError::IllegalTransition {
                from: self.status.as_str().to_string(),
                to: to.as_str().to_string(),
            });
        }
        let event = DeliveryEvent::new(self.id, self.status, to, actor_user_id, note, at);
        self.status = to;
        self.updated_at = at;
        Ok(event)
    }

    /// Close out the delivery as `Delivered`. Requires a complete
    /// POD (signature AND photo — see [`crate::pod::PodRecord`]);
    /// on an incomplete POD this returns
    /// [`DeliveryError::PodIncomplete`] and changes nothing — no
    /// status change, no event. The caller persists the returned
    /// event and the updated delivery.
    pub fn close_out(
        &mut self,
        pod: &crate::pod::PodRecord,
        actor_user_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<DeliveryEvent> {
        if self.status != DeliveryStatus::OutForDelivery {
            return Err(DeliveryError::IllegalTransition {
                from: self.status.as_str().to_string(),
                to: DeliveryStatus::Delivered.as_str().to_string(),
            });
        }
        if !pod.is_complete() {
            return Err(DeliveryError::PodIncomplete {
                delivery_id: self.id,
                missing: pod.missing_parts().join(", "),
            });
        }
        let event = DeliveryEvent::new(
            self.id,
            self.status,
            DeliveryStatus::Delivered,
            actor_user_id,
            Some(format!(
                "POD complete: signature by {} + photo",
                pod.signature
                    .as_ref()
                    .map(|s| s.signer_name.as_str())
                    .unwrap_or("?")
            )),
            at,
        );
        self.status = DeliveryStatus::Delivered;
        self.updated_at = at;
        Ok(event)
    }
}

/// One row of `delivery_events`: the audit trail for a delivery's
/// status changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryEvent {
    /// UUIDv7 primary key.
    pub id: Uuid,
    pub delivery_id: Uuid,
    pub from_status: DeliveryStatus,
    pub to_status: DeliveryStatus,
    /// Staff user who made the change.
    pub actor_user_id: Uuid,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl DeliveryEvent {
    /// Build a new event with a fresh UUIDv7 id.
    pub fn new(
        delivery_id: Uuid,
        from_status: DeliveryStatus,
        to_status: DeliveryStatus,
        actor_user_id: Uuid,
        note: Option<String>,
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            delivery_id,
            from_status,
            to_status,
            actor_user_id,
            note,
            created_at: at,
        }
    }
}

/// Sort key for the dispatch board: highest priority first, then
/// oldest first. Pure function; the repository equivalent is
/// `df_db::list_dispatch_board`.
pub fn sort_dispatch_board(deliveries: &mut [Delivery]) {
    deliveries.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delivery() -> Delivery {
        Delivery::new(Uuid::now_v7(), Uuid::now_v7(), Some(Uuid::now_v7()))
    }

    #[test]
    fn status_round_trips_through_storage_string() {
        for s in [
            DeliveryStatus::Awaiting,
            DeliveryStatus::Assigned,
            DeliveryStatus::OutForDelivery,
            DeliveryStatus::Delivered,
            DeliveryStatus::Failed,
        ] {
            assert_eq!(DeliveryStatus::from_str(s.as_str()), Some(s));
        }
        // Storage strings are the 0006 words: draft / cancelled.
        assert_eq!(DeliveryStatus::Awaiting.as_str(), "draft");
        assert_eq!(DeliveryStatus::Failed.as_str(), "cancelled");
        // The task's words parse too, for forward compatibility.
        assert_eq!(
            DeliveryStatus::from_str("awaiting"),
            Some(DeliveryStatus::Awaiting)
        );
        assert_eq!(
            DeliveryStatus::from_str("failed"),
            Some(DeliveryStatus::Failed)
        );
        assert_eq!(DeliveryStatus::from_str("teleported"), None);
    }

    #[test]
    fn happy_path_transitions_and_audit_trail() {
        let mut d = delivery();
        let actor = Uuid::now_v7();
        let now = Utc::now();
        let driver = Uuid::now_v7();
        let ev = d
            .assign(driver, Some("Van 2".to_string()), actor, now, None)
            .unwrap();
        assert_eq!(d.status, DeliveryStatus::Assigned);
        assert_eq!(d.driver_id, Some(driver));
        assert_eq!(ev.from_status, DeliveryStatus::Awaiting);
        assert_eq!(ev.to_status, DeliveryStatus::Assigned);
        let ev = d
            .transition(DeliveryStatus::OutForDelivery, actor, now, None)
            .unwrap();
        assert_eq!(ev.to_status, DeliveryStatus::OutForDelivery);
    }

    #[test]
    fn illegal_transitions_are_rejected_and_change_nothing() {
        let mut d = delivery();
        let actor = Uuid::now_v7();
        let now = Utc::now();
        // Awaiting -> OutForDelivery skips assignment.
        assert_eq!(
            d.transition(DeliveryStatus::OutForDelivery, actor, now, None),
            Err(DeliveryError::IllegalTransition {
                from: "draft".to_string(),
                to: "out_for_delivery".to_string(),
            })
        );
        assert_eq!(d.status, DeliveryStatus::Awaiting);
        // Delivered is never reachable via plain transition (needs POD).
        assert_eq!(
            d.transition(DeliveryStatus::Delivered, actor, now, None),
            Err(DeliveryError::IllegalTransition {
                from: "draft".to_string(),
                to: "delivered".to_string(),
            })
        );
        // Failed is terminal.
        d.transition(DeliveryStatus::Failed, actor, now, None)
            .unwrap();
        assert_eq!(
            d.transition(DeliveryStatus::Assigned, actor, now, None),
            Err(DeliveryError::IllegalTransition {
                from: "cancelled".to_string(),
                to: "assigned".to_string(),
            })
        );
    }

    #[test]
    fn dispatch_board_sorts_priority_then_oldest() {
        let mut a = delivery();
        a.priority = 1;
        let mut b = delivery();
        b.priority = 5;
        let mut c = delivery();
        c.priority = 5;
        // b and c share priority; b was created first (older).
        let mut board = vec![a.clone(), c.clone(), b.clone()];
        sort_dispatch_board(&mut board);
        assert_eq!(board[0].id, b.id);
        assert_eq!(board[1].id, c.id);
        assert_eq!(board[2].id, a.id);
    }
}
