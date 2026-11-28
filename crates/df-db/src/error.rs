//! Database errors.

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("invalid data in row: {0}")]
    InvalidData(String),
    #[error("row not found: {0}")]
    NotFound(String),
    /// A delivery close-out was attempted without a complete POD.
    /// Nothing was posted: the delivery keeps its status and no
    /// close-out event exists. `missing` names the absent parts
    /// (`"signature"`, `"photo"`, or both).
    #[error(
        "cannot close out delivery {delivery_id}: proof of delivery incomplete, missing {missing}"
    )]
    PodIncomplete { delivery_id: Uuid, missing: String },
    /// A delivery status transition the state machine does not
    /// allow.
    #[error("illegal delivery transition from {from} to {to}")]
    IllegalDeliveryTransition { from: String, to: String },
    /// An appointment overlaps an existing scheduled appointment
    /// for the same contractor / staff member.
    #[error("appointment overlaps existing appointment {conflicting_id}")]
    AppointmentOverlap { conflicting_id: Uuid },
}

impl From<df_delivery::DeliveryError> for DbError {
    fn from(e: df_delivery::DeliveryError) -> Self {
        match e {
            df_delivery::DeliveryError::PodIncomplete {
                delivery_id,
                missing,
            } => DbError::PodIncomplete {
                delivery_id,
                missing,
            },
            df_delivery::DeliveryError::IllegalTransition { from, to } => {
                DbError::IllegalDeliveryTransition { from, to }
            }
            other => DbError::InvalidData(other.to_string()),
        }
    }
}

impl From<df_crm::CrmError> for DbError {
    fn from(e: df_crm::CrmError) -> Self {
        match e {
            df_crm::CrmError::Overlap { conflicting_id, .. } => {
                DbError::AppointmentOverlap { conflicting_id }
            }
            other => DbError::InvalidData(other.to_string()),
        }
    }
}
