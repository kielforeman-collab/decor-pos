//! Driver proof of delivery (POD) (spec §4 `delivery_logistics`,
//! §6.1 `POST /deliveries/{id}/pod`, §7 Phase 4).
//!
//! A delivery closes out **only** when its POD is complete:
//! a captured signature AND a photo. Each part carries a SHA-256
//! hash so the stored evidence can be verified later against the
//! bytes / file it came from:
//!
//! * Signature — the raw capture bytes (e.g. PNG from the
//!   driver's signature pad / PWA canvas) are kept in
//!   [`PodSignature::bytes`] with their SHA-256 hex digest. The
//!   database stores them as base64 text (`delivery_pods.
//!   signature_b64`, migration `0006_phase4`) — see
//!   [`PodSignature::to_b64`] / [`PodSignature::from_b64`].
//! * Photo — the photo itself lives on disk / in object storage
//!   (it is far too large for the ledger); the record keeps the
//!   file reference (`path`), the SHA-256 of the file bytes, and
//!   the capture time (`delivery_pods.photo_at`, migration
//!   `0007_phase4_crm_delivery`).
//!
//! Attempting to close out without both parts is an error and
//! posts nothing (see [`Delivery::close_out`](crate::dispatch::Delivery::close_out)).
//! No PAN/CVV or payment data is involved anywhere here.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{DeliveryError, Result};

/// SHA-256 hex digest of a byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// A captured delivery signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodSignature {
    /// Raw signature capture bytes (e.g. PNG).
    #[serde(skip)]
    pub bytes: Vec<u8>,
    /// SHA-256 hex digest of `bytes`.
    pub sha256_hex: String,
    /// Name of the person who signed.
    pub signer_name: String,
    /// When the signature was captured.
    pub captured_at: DateTime<Utc>,
}

impl PodSignature {
    /// Capture a signature. Empty captures are rejected.
    pub fn capture(
        bytes: Vec<u8>,
        signer_name: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<Self> {
        if bytes.is_empty() {
            return Err(DeliveryError::EmptySignature);
        }
        let sha256_hex = sha256_hex(&bytes);
        Ok(Self {
            bytes,
            sha256_hex,
            signer_name: signer_name.into(),
            captured_at: at,
        })
    }

    /// The capture bytes as standard base64 text — the form the
    /// `delivery_pods.signature_b64` column stores.
    pub fn to_b64(&self) -> String {
        BASE64_STANDARD.encode(&self.bytes)
    }

    /// Rebuild a captured signature from stored base64 text. The
    /// stored SHA-256 must match the decoded bytes, so a corrupt
    /// row fails here instead of silently verifying against the
    /// wrong bytes.
    pub fn from_b64(
        b64: &str,
        expected_sha256_hex: &str,
        signer_name: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<Self> {
        let bytes = BASE64_STANDARD
            .decode(b64.trim())
            .map_err(|_| DeliveryError::InvalidSignatureEncoding)?;
        if bytes.is_empty() {
            return Err(DeliveryError::EmptySignature);
        }
        let sha256_hex = sha256_hex(&bytes);
        if sha256_hex != expected_sha256_hex {
            return Err(DeliveryError::SignatureHashMismatch);
        }
        Ok(Self {
            bytes,
            sha256_hex,
            signer_name: signer_name.into(),
            captured_at: at,
        })
    }
}

/// A delivery photo attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodPhoto {
    /// File reference / path (or object-storage key) of the photo.
    /// The photo bytes themselves are not stored in the database.
    pub path: String,
    /// SHA-256 hex digest of the photo file bytes.
    pub sha256_hex: String,
    /// When the photo was taken / attached.
    pub captured_at: DateTime<Utc>,
}

impl PodPhoto {
    /// Record a photo from its path and the SHA-256 of its bytes
    /// (the caller hashes the file it just saved). An empty path
    /// is rejected.
    pub fn record(
        path: impl Into<String>,
        file_sha256_hex: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<Self> {
        let path = path.into();
        if path.trim().is_empty() {
            return Err(DeliveryError::EmptyPhotoPath);
        }
        Ok(Self {
            path,
            sha256_hex: file_sha256_hex.into(),
            captured_at: at,
        })
    }
}

/// One row of `pod_records`: the POD for one delivery (at most
/// one row per delivery — a re-capture overwrites the part it
/// replaces).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PodRecord {
    /// UUIDv7 primary key.
    pub id: Uuid,
    pub delivery_id: Uuid,
    pub signature: Option<PodSignature>,
    pub photo: Option<PodPhoto>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PodRecord {
    /// Start an empty POD for a delivery.
    pub fn new(delivery_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::now_v7(),
            delivery_id,
            signature: None,
            photo: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// True only when a signature AND a photo are both present.
    pub fn is_complete(&self) -> bool {
        self.signature.is_some() && self.photo.is_some()
    }

    /// Names of the missing parts, in order: `"signature"`,
    /// `"photo"`. Empty when complete.
    pub fn missing_parts(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.signature.is_none() {
            missing.push("signature");
        }
        if self.photo.is_none() {
            missing.push("photo");
        }
        missing
    }

    /// Attach (or replace) the signature.
    pub fn record_signature(&mut self, signature: PodSignature, at: DateTime<Utc>) {
        self.signature = Some(signature);
        self.updated_at = at;
    }

    /// Attach (or replace) the photo.
    pub fn record_photo(&mut self, photo: PodPhoto, at: DateTime<Utc>) {
        self.photo = Some(photo);
        self.updated_at = at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{Delivery, DeliveryStatus};

    #[test]
    fn sha256_hex_matches_known_vector() {
        // SHA-256("abc").
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn pod_completeness_gates_close_out() {
        let mut delivery = Delivery::new(Uuid::now_v7(), Uuid::now_v7(), Some(Uuid::now_v7()));
        let actor = Uuid::now_v7();
        let now = Utc::now();
        delivery
            .assign(Uuid::now_v7(), None, actor, now, None)
            .unwrap();
        delivery
            .transition(DeliveryStatus::OutForDelivery, actor, now, None)
            .unwrap();

        let mut pod = PodRecord::new(delivery.id);
        assert!(!pod.is_complete());
        assert_eq!(pod.missing_parts(), vec!["signature", "photo"]);
        // Empty POD: close-out fails, nothing changes.
        assert_eq!(
            delivery.close_out(&pod, actor, now),
            Err(DeliveryError::PodIncomplete {
                delivery_id: delivery.id,
                missing: "signature, photo".to_string(),
            })
        );
        assert_eq!(delivery.status, DeliveryStatus::OutForDelivery);

        // Signature only: still incomplete.
        pod.record_signature(
            PodSignature::capture(b"fake-png-bytes".to_vec(), "Dana Driver", now).unwrap(),
            now,
        );
        assert!(!pod.is_complete());
        assert_eq!(pod.missing_parts(), vec!["photo"]);
        assert!(delivery.close_out(&pod, actor, now).is_err());
        assert_eq!(delivery.status, DeliveryStatus::OutForDelivery);

        // Photo too: close-out succeeds.
        pod.record_photo(
            PodPhoto::record(
                "pod/2026-11-25/delivery-1.jpg",
                sha256_hex(b"fake-jpg"),
                now,
            )
            .unwrap(),
            now,
        );
        assert!(pod.is_complete());
        assert!(pod.missing_parts().is_empty());
        let event = delivery.close_out(&pod, actor, now).unwrap();
        assert_eq!(delivery.status, DeliveryStatus::Delivered);
        assert_eq!(event.to_status, DeliveryStatus::Delivered);
        // Delivered is terminal.
        assert!(delivery.close_out(&pod, actor, now).is_err());
    }

    #[test]
    fn signature_base64_round_trip_and_hash_check() {
        let now = Utc::now();
        let sig = PodSignature::capture(b"fake-png-bytes".to_vec(), "Dana Driver", now).unwrap();
        let b64 = sig.to_b64();
        let back = PodSignature::from_b64(&b64, &sig.sha256_hex, "Dana Driver", now).unwrap();
        assert_eq!(back, sig);
        // Corrupt base64 is rejected.
        assert_eq!(
            PodSignature::from_b64("!!!not-base64!!!", &sig.sha256_hex, "Dana", now),
            Err(DeliveryError::InvalidSignatureEncoding)
        );
        // A hash that does not match the bytes is rejected.
        assert_eq!(
            PodSignature::from_b64(&b64, &sha256_hex(b"other bytes"), "Dana", now),
            Err(DeliveryError::SignatureHashMismatch)
        );
    }

    #[test]
    fn empty_signature_and_photo_path_rejected() {
        let now = Utc::now();
        assert_eq!(
            PodSignature::capture(vec![], "Nobody", now),
            Err(DeliveryError::EmptySignature)
        );
        assert_eq!(
            PodPhoto::record("   ", "deadbeef", now),
            Err(DeliveryError::EmptyPhotoPath)
        );
    }
}
