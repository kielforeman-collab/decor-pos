//! Conflict resolution (spec §5, §8 guardrail 2).
//!
//! The rules below are the whole policy. They are stated once here
//! (see [`RESOLUTION_RULES`]) and implemented by the functions in this
//! module plus [`crate::sink::MemoryHq`]; the edge daemon never
//! invents a merge rule of its own.
//!
//! [`RESOLUTION_RULES`] is the normative text — keep it in sync with
//! the code.

/// The normative conflict-resolution rules.
///
/// ```text
/// RESOLUTION RULES (decor-pos delta sync)
///
/// 1. APPEND-ONLY LEDGERS MERGE BY UNION.
///    Inventory and AR are append-only ledgers. Every delta is kept,
///    keyed by its UUIDv7 entry id. NEVER last-writer-wins for ledger
///    deltas: two stores selling the same SKU both subtract stock, and
///    dropping either delta would corrupt the balance. `merge_ledger_deltas`
///    is a set union on entry id; replayed ids are duplicates, not updates.
///
/// 2. TRANSACTIONS ARE IMMUTABLE ONCE COMPLETED.
///    A transaction id is the sale. A re-send with the same id and the
///    same payload is a duplicate (no-op). A re-send with the same id
///    but different content is a CONFLICT: reject it as
///    `SyncError::Conflict`, record it, and NEVER silently overwrite
///    the stored transaction. A correction is a new reversing/adjusting
///    transaction, never an edit in place.
///
/// 3. REFERENCE DATA RESOLVES NEWEST-`updated_at`-WINS.
///    Products, pricing rules and other reference rows are mutable, so
///    the newest `updated_at` wins. The loser is NOT discarded: it is
///    kept in an audit list (`ReferenceResolution::loser_audit`) so a
///    human can review what was superseded. Equal timestamps break
///    deterministically toward the incoming record.
/// ```
pub const RESOLUTION_RULES: &str = "1. Append-only ledgers merge by union \
(keyed by UUIDv7 entry id; NEVER last-writer-wins for ledger deltas). \
2. Transactions are immutable once completed: a conflicting re-send with \
the same id but different content is rejected as SyncError::Conflict and \
recorded, never silently overwritten. \
3. Reference data resolves newest-updated_at-wins, with the loser kept \
in an audit list.";

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::error::SyncError;

/// One append-only ledger delta (inventory or AR).
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerDelta {
    /// UUIDv7 entry id — the union key.
    pub entry_id: Uuid,
    /// What the delta applies to (product id for inventory,
    /// customer id for AR).
    pub aggregate_id: Uuid,
    /// Signed amount (negative = stock sold / AR reduced).
    pub delta: Decimal,
}

/// Merge two ledger delta sets by union on `entry_id`.
///
/// Every delta is kept; an id present in both sets appears once.
/// Result is sorted by `entry_id` (UUIDv7 order = creation order)
/// for determinism. This is rule 1: never last-writer-wins.
pub fn merge_ledger_deltas(existing: &[LedgerDelta], incoming: &[LedgerDelta]) -> Vec<LedgerDelta> {
    let mut merged: Vec<LedgerDelta> = existing.to_vec();
    for delta in incoming {
        if !merged.iter().any(|d| d.entry_id == delta.entry_id) {
            merged.push(delta.clone());
        }
    }
    merged.sort_by_key(|d| d.entry_id);
    merged
}

/// A mutable reference-data row (product, pricing rule, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceRecord {
    pub id: Uuid,
    pub updated_at: DateTime<Utc>,
    pub payload: serde_json::Value,
}

/// Outcome of resolving two versions of one reference row.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceResolution {
    /// The winning version.
    pub winner: ReferenceRecord,
    /// The superseded version — kept for audit, never discarded.
    pub loser_audit: ReferenceRecord,
}

/// Rule 3: newest `updated_at` wins; the loser is kept for audit.
/// Equal timestamps break deterministically toward `incoming`.
pub fn resolve_reference(
    existing: ReferenceRecord,
    incoming: ReferenceRecord,
) -> ReferenceResolution {
    debug_assert_eq!(existing.id, incoming.id, "resolving different rows");
    if incoming.updated_at >= existing.updated_at {
        ReferenceResolution {
            winner: incoming,
            loser_audit: existing,
        }
    } else {
        ReferenceResolution {
            winner: existing,
            loser_audit: incoming,
        }
    }
}

/// Rule 2: a transaction re-send is a duplicate when the payload is
/// identical, and a conflict when the same id carries different
/// content. Returns `Ok(())` for identical payloads, `Err(Conflict)`
/// otherwise. A sink must never overwrite on conflict.
pub fn check_transaction_conflict(
    existing: &serde_json::Value,
    incoming: &serde_json::Value,
) -> Result<(), SyncError> {
    if existing == incoming {
        Ok(())
    } else {
        Err(SyncError::Conflict(
            "transaction re-sent with different content; stored version kept".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn delta(id: u128, aggregate: u128, amount: &str) -> LedgerDelta {
        LedgerDelta {
            entry_id: Uuid::from_u128(id),
            aggregate_id: Uuid::from_u128(aggregate),
            delta: Decimal::from_str(amount).unwrap(),
        }
    }

    #[test]
    fn ledger_merge_is_a_union_not_last_writer_wins() {
        let a = delta(1, 10, "-2");
        let b = delta(2, 10, "-3");
        let merged = merge_ledger_deltas(std::slice::from_ref(&a), &[a.clone(), b.clone()]);
        assert_eq!(merged, vec![a, b]);
        // Both deltas survive: total -5, neither overwrote the other.
        let total = merged
            .iter()
            .map(|d| d.delta)
            .fold(Decimal::ZERO, |x, y| x + y);
        assert_eq!(total, Decimal::from(-5));
        // Order is deterministic (UUIDv7 / id order).
        let reversed = merge_ledger_deltas(&[delta(2, 10, "-3")], &[delta(1, 10, "-2")]);
        assert_eq!(reversed[0].entry_id, Uuid::from_u128(1));
    }

    #[test]
    fn transaction_conflict_rules() {
        let stored = serde_json::json!({"id": "t1", "grand_total": "100.0000"});
        let identical = serde_json::json!({"id": "t1", "grand_total": "100.0000"});
        assert!(check_transaction_conflict(&stored, &identical).is_ok());
        let different = serde_json::json!({"id": "t1", "grand_total": "999.0000"});
        let err = check_transaction_conflict(&stored, &different).unwrap_err();
        assert!(matches!(err, SyncError::Conflict(_)));
    }

    #[test]
    fn reference_resolution_newest_wins_loser_audited() {
        let older = ReferenceRecord {
            id: Uuid::from_u128(7),
            updated_at: DateTime::parse_from_rfc3339("2026-11-25T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            payload: serde_json::json!({"list_price": "10.0000"}),
        };
        let newer = ReferenceRecord {
            id: Uuid::from_u128(7),
            updated_at: DateTime::parse_from_rfc3339("2026-11-25T11:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            payload: serde_json::json!({"list_price": "12.0000"}),
        };
        let res = resolve_reference(older.clone(), newer.clone());
        assert_eq!(res.winner, newer);
        assert_eq!(res.loser_audit, older);
        // And the other way round: the newest still wins.
        let res = resolve_reference(newer.clone(), older.clone());
        assert_eq!(res.winner, newer);
        assert_eq!(res.loser_audit, older);
        // Equal timestamps: incoming wins, deterministically.
        let res = resolve_reference(older.clone(), older.clone());
        assert_eq!(res.winner, older);
    }

    #[test]
    fn resolution_rules_text_states_the_policy() {
        assert!(RESOLUTION_RULES.contains("NEVER last-writer-wins"));
        assert!(RESOLUTION_RULES.contains("SyncError::Conflict"));
        assert!(RESOLUTION_RULES.contains("newest-updated_at-wins"));
    }
}
