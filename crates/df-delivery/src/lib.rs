//! `df-delivery` — delivery logistics domain for decor-pos
//! (Phase 4, v0.5; spec §4 `delivery_logistics`, §6.1, §7 Phase 4
//! row).
//!
//! Pure domain logic — no database, no network, no maps API, no
//! vendor API. Persistence lives in `df-db` (`crm.rs`,
//! `delivery.rs`, migrations `0006_phase4` /
//! `0007_phase4_crm_delivery`); HTTP endpoints (`GET
//! /deliveries/dispatch`, `POST /deliveries/{id}/pod`) and the
//! driver UI are wired by another worker against this public API
//! (the edge wiring layer in `df-edge::delivery` currently
//! carries these rules inline and must not grow a second copy of
//! them).
//!
//! Alignment note: the delivery status storage strings are
//! migration `0006_phase4`'s (`draft`, `assigned`,
//! `out_for_delivery`, `delivered`, `cancelled`); the enum keeps
//! the task's words (`Awaiting`, `Failed`) and translates. The
//! signature is stored as base64 text plus SHA-256
//! (`delivery_pods.signature_b64`), the photo as a file reference
//! plus SHA-256 (`photo_ref` / `photo_sha256` / `photo_at`).
//! Close-out requires signature AND photo — see [`pod`].
//!
//! ## What is implemented
//!
//! * [`dispatch`] — [`Delivery`](dispatch::Delivery) dispatch
//!   board with the enforced state machine `awaiting → assigned
//!   → out-for-delivery → delivered / failed`, an audit trail of
//!   [`DeliveryEvent`](dispatch::DeliveryEvent)s (who / when /
//!   from / to), and
//!   [`sort_dispatch_board`](dispatch::sort_dispatch_board)
//!   (highest priority first, then oldest).
//! * [`pod`] — [`PodRecord`](pod::PodRecord) proof of delivery:
//!   signature capture (bytes + SHA-256 + signer + time) and
//!   photo attachment (file reference + SHA-256 + time).
//!   Close-out happens only through
//!   [`Delivery::close_out`](dispatch::Delivery::close_out),
//!   which requires a complete POD — signature AND photo — and
//!   otherwise returns [`DeliveryError::PodIncomplete`] having
//!   changed nothing and posted nothing.
//! * [`packing_slip`] — [`PackingSlip`](packing_slip::PackingSlip)
//!   with plain-text (42-column), ESC/POS, and ZPL renderings in
//!   the `df-hw` style.
//!
//! ## Example — the exit test in miniature
//!
//! ```rust
//! use chrono::Utc;
//! use df_delivery::dispatch::{Delivery, DeliveryStatus};
//! use df_delivery::pod::{PodPhoto, PodRecord, PodSignature, sha256_hex};
//! use uuid::Uuid;
//!
//! let now = Utc::now();
//! let actor = Uuid::now_v7();
//! let mut delivery = Delivery::new(Uuid::now_v7(), Uuid::now_v7(), Some(Uuid::now_v7()));
//! delivery.assign(Uuid::now_v7(), None, actor, now, None).unwrap();
//! delivery
//!     .transition(DeliveryStatus::OutForDelivery, actor, now, None)
//!     .unwrap();
//! let mut pod = PodRecord::new(delivery.id);
//! // No POD yet: close-out fails and posts nothing.
//! assert!(delivery.close_out(&pod, actor, now).is_err());
//! pod.record_signature(
//!     PodSignature::capture(b"signature-png".to_vec(), "Dana Driver", now).unwrap(),
//!     now,
//! );
//! pod.record_photo(
//!     PodPhoto::record("pod/photo.jpg", sha256_hex(b"photo-bytes"), now).unwrap(),
//!     now,
//! );
//! assert!(pod.is_complete());
//! delivery.close_out(&pod, actor, now).unwrap();
//! assert_eq!(delivery.status, DeliveryStatus::Delivered);
//! ```
//!
//! Money is `rust_decimal::Decimal` throughout — never floats.
//! Benjamin Moore COLORx is deferred post-v1.0 (spec §9) and
//! appears nowhere in this crate.

pub mod dispatch;
pub mod error;
pub mod packing_slip;
pub mod pod;

pub use dispatch::{Delivery, DeliveryEvent, DeliveryStatus, sort_dispatch_board};
pub use error::{DeliveryError, Result};
pub use packing_slip::{PackingSlip, PackingSlipLine, SLIP_WIDTH};
pub use pod::{PodPhoto, PodRecord, PodSignature, sha256_hex};
