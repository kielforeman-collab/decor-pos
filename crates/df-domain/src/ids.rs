//! Identifiers: UUIDv7 primary keys (spec §5).
//!
//! UUIDv7 is time-ordered and globally unique without a central
//! sequence, so offline edge nodes can mint ids that never collide.

use uuid::Uuid;

/// Generate a new UUIDv7 identifier.
pub fn new_id() -> Uuid {
    Uuid::now_v7()
}
