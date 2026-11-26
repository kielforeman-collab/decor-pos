//! Delivery logistics domain errors.

use thiserror::Error;
use uuid::Uuid;

/// Errors raised by the pure delivery domain rules in this crate.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeliveryError {
    /// A status transition the state machine does not allow.
    #[error("illegal delivery transition from {from} to {to}")]
    IllegalTransition {
        /// Current status, as a storage string.
        from: String,
        /// Requested status, as a storage string.
        to: String,
    },
    /// Close-out was attempted without a complete POD. `missing`
    /// names what is absent (`"signature"`, `"photo"`, or both).
    /// Nothing is posted when this is returned — the delivery
    /// stays in its current status with no close-out event.
    #[error(
        "cannot close out delivery {delivery_id}: proof of delivery incomplete, missing {missing}"
    )]
    PodIncomplete {
        /// The delivery that could not be closed.
        delivery_id: Uuid,
        /// Comma-separated list of the missing POD parts.
        missing: String,
    },
    /// A signature capture carried no bytes.
    #[error("signature capture is empty")]
    EmptySignature,
    /// A photo reference was recorded with no path.
    #[error("photo reference has no path")]
    EmptyPhotoPath,
    /// Stored signature base64 does not decode.
    #[error("signature_b64 is not valid base64")]
    InvalidSignatureEncoding,
    /// Decoded signature bytes do not match the stored SHA-256.
    #[error("signature bytes do not match the stored SHA-256")]
    SignatureHashMismatch,
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, DeliveryError>;
