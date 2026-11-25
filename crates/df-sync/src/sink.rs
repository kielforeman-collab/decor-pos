//! The HQ side of delta sync: the [`HqSink`] trait and [`MemoryHq`].
//!
//! A sink applies a [`DeltaBatch`](crate::outbox::DeltaBatch) and
//! reports how many entries were new vs. replays. Replay safety is a
//! contract on the trait, not an implementation detail: the sink must
//! record applied entry ids and return them in
//! [`ApplyReport::duplicates`]; a replayed batch must change nothing.
//! [`crate::resolve::check_transaction_conflict`] is the rule a sink
//! applies when a transaction id arrives twice with different content.
//!
//! [`MemoryHq`] is the in-memory HQ used by tests and by the edge
//! daemon's local sync endpoint. It is deliberately public (not
//! `#[cfg(test)]`): the Phase 1 exit test and `pos-desktop` sync
//! against it. A PostgreSQL HQ sink replaces it in a later phase.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Mutex;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::SyncError;
use crate::outbox::DeltaBatch;
use crate::resolve;

/// Outcome of applying one batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyReport {
    /// Entries applied for the first time.
    pub applied: usize,
    /// Entries already applied (replays — no state changed).
    pub duplicates: usize,
}

impl ApplyReport {
    pub fn empty() -> Self {
        Self {
            applied: 0,
            duplicates: 0,
        }
    }
}

/// HQ endpoint for delta batches.
///
/// The task asks for native `async fn` in the trait *and* `&dyn
/// HqSink` — but a literal `async fn` in a trait is not
/// dyn-compatible in Rust 1.99 (E0038), and neither is the
/// `-> impl Future` desugaring. So the trait uses the equivalent
/// native boxed-future form below: no `async_trait` macro, callers
/// still write `sink.apply_batch(&batch).await`, and implementors
/// wrap their async block in `Box::pin` (see [`MemoryHq`]).
pub trait HqSink: Send + Sync {
    fn apply_batch<'a>(
        &'a self,
        batch: &'a DeltaBatch,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ApplyReport, SyncError>> + Send + 'a>,
    >;
}

/// In-memory HQ: records applied entry ids, inventory ledger deltas
/// (`product_id -> on-hand delta`), AR ledger entries
/// (`customer_id -> balance`) and transactions by id.
///
/// Payload conventions are those documented in [`crate::outbox`]:
/// `inventory_ledger` entries carry `product_id` + `delta_on_hand`,
/// `ar_ledger` entries carry `customer_id` + `amount`, `transaction`
/// entries carry the transaction JSON (keyed by `aggregate_id`).
#[derive(Debug, Default)]
pub struct MemoryHq {
    inner: Mutex<MemoryHqInner>,
}

#[derive(Debug, Default)]
struct MemoryHqInner {
    applied_entry_ids: HashSet<Uuid>,
    inventory: HashMap<Uuid, Decimal>,
    ar: HashMap<Uuid, Decimal>,
    transactions: HashMap<Uuid, serde_json::Value>,
    conflicts: Vec<String>,
}

impl MemoryHq {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct entry ids applied so far.
    pub fn applied_count(&self) -> usize {
        self.inner
            .lock()
            .expect("memory hq lock")
            .applied_entry_ids
            .len()
    }

    /// On-hand delta for a product (sum of all applied inventory
    /// deltas for it; zero when nothing was applied).
    pub fn inventory_on_hand(&self, product_id: Uuid) -> Decimal {
        self.inner
            .lock()
            .expect("memory hq lock")
            .inventory
            .get(&product_id)
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    /// AR balance for a customer (sum of all applied AR entries).
    pub fn ar_balance(&self, customer_id: Uuid) -> Decimal {
        self.inner
            .lock()
            .expect("memory hq lock")
            .ar
            .get(&customer_id)
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    /// The stored transaction payload for an id, if applied.
    pub fn transaction(&self, id: Uuid) -> Option<serde_json::Value> {
        self.inner
            .lock()
            .expect("memory hq lock")
            .transactions
            .get(&id)
            .cloned()
    }

    /// Conflict descriptions recorded when a conflicting re-send was
    /// rejected (see [`resolve::RESOLUTION_RULES`]).
    pub fn conflicts(&self) -> Vec<String> {
        self.inner.lock().expect("memory hq lock").conflicts.clone()
    }
}

fn payload_uuid(payload: &serde_json::Value, key: &str) -> Option<Uuid> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

fn payload_decimal(payload: &serde_json::Value, key: &str) -> Option<Decimal> {
    match payload.get(key) {
        Some(serde_json::Value::String(s)) => Decimal::from_str(s).ok(),
        Some(serde_json::Value::Number(n)) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    }
}

impl HqSink for MemoryHq {
    fn apply_batch<'a>(
        &'a self,
        batch: &'a DeltaBatch,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ApplyReport, SyncError>> + Send + 'a>,
    > {
        Box::pin(async move { self.apply_batch_inner(batch).await })
    }
}

impl MemoryHq {
    async fn apply_batch_inner(&self, batch: &DeltaBatch) -> Result<ApplyReport, SyncError> {
        let mut inner = self.inner.lock().expect("memory hq lock");

        // Pre-validate the whole batch for transaction conflicts
        // before applying anything, so a rejected batch is atomic.
        for entry in &batch.entries {
            if entry.aggregate_type == "transaction"
                && let Some(existing) = inner.transactions.get(&entry.aggregate_id)
                && resolve::check_transaction_conflict(existing, &entry.payload).is_err()
            {
                let msg = format!(
                    "transaction {} re-sent with different content",
                    entry.aggregate_id
                );
                inner.conflicts.push(msg.clone());
                return Err(SyncError::Conflict(msg));
            }
        }

        let mut report = ApplyReport::empty();
        for entry in &batch.entries {
            if inner.applied_entry_ids.contains(&entry.id) {
                report.duplicates += 1;
                continue;
            }
            match entry.aggregate_type.as_str() {
                "inventory_ledger" => {
                    if let (Some(product_id), Some(delta)) = (
                        payload_uuid(&entry.payload, "product_id"),
                        payload_decimal(&entry.payload, "delta_on_hand"),
                    ) {
                        *inner.inventory.entry(product_id).or_default() += delta;
                    }
                }
                "ar_ledger" => {
                    if let (Some(customer_id), Some(amount)) = (
                        payload_uuid(&entry.payload, "customer_id"),
                        payload_decimal(&entry.payload, "amount"),
                    ) {
                        *inner.ar.entry(customer_id).or_default() += amount;
                    }
                }
                "transaction" => {
                    inner
                        .transactions
                        .insert(entry.aggregate_id, entry.payload.clone());
                }
                _ => {
                    // Unknown aggregate types are recorded as applied
                    // (so they are not retried forever) but otherwise
                    // ignored; a future HQ version may interpret them.
                }
            }
            inner.applied_entry_ids.insert(entry.id);
            report.applied += 1;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::OutboxEntry;

    fn batch(entries: Vec<OutboxEntry>, store_id: Uuid) -> DeltaBatch {
        DeltaBatch::new(store_id, entries)
    }

    fn tx_entry(store_id: Uuid, tx_id: Uuid, total: &str) -> OutboxEntry {
        OutboxEntry::new(
            store_id,
            "transaction",
            tx_id,
            serde_json::json!({"id": tx_id.to_string(), "grand_total": total}),
        )
    }

    #[tokio::test]
    async fn apply_then_replay_is_a_noop() {
        let hq = MemoryHq::new();
        let store_id = Uuid::now_v7();
        let product_id = Uuid::now_v7();
        let customer_id = Uuid::now_v7();
        let tx_id = Uuid::now_v7();

        let entries = vec![
            tx_entry(store_id, tx_id, "216.1088"),
            OutboxEntry::new(
                store_id,
                "inventory_ledger",
                Uuid::now_v7(),
                serde_json::json!({"product_id": product_id.to_string(), "delta_on_hand": "-2", "reason_type": "sale", "reference_id": tx_id.to_string()}),
            ),
            OutboxEntry::new(
                store_id,
                "ar_ledger",
                Uuid::now_v7(),
                serde_json::json!({"customer_id": customer_id.to_string(), "amount": "50.0000", "entry_type": "charge", "reference_id": tx_id.to_string()}),
            ),
        ];
        let report = hq
            .apply_batch(&batch(entries.clone(), store_id))
            .await
            .unwrap();
        assert_eq!(
            report,
            ApplyReport {
                applied: 3,
                duplicates: 0
            }
        );
        assert_eq!(hq.applied_count(), 3);
        assert_eq!(hq.inventory_on_hand(product_id), Decimal::from(-2));
        assert_eq!(
            hq.ar_balance(customer_id),
            Decimal::from_str("50.0000").unwrap()
        );
        assert_eq!(hq.transaction(tx_id).unwrap()["grand_total"], "216.1088");

        // Replaying the identical batch changes nothing.
        let replay = hq.apply_batch(&batch(entries, store_id)).await.unwrap();
        assert_eq!(
            replay,
            ApplyReport {
                applied: 0,
                duplicates: 3
            }
        );
        assert_eq!(hq.inventory_on_hand(product_id), Decimal::from(-2));
        assert_eq!(
            hq.ar_balance(customer_id),
            Decimal::from_str("50.0000").unwrap()
        );
    }

    #[tokio::test]
    async fn conflicting_transaction_resend_is_rejected() {
        let hq = MemoryHq::new();
        let store_id = Uuid::now_v7();
        let tx_id = Uuid::now_v7();
        hq.apply_batch(&batch(
            vec![tx_entry(store_id, tx_id, "100.0000")],
            store_id,
        ))
        .await
        .unwrap();

        // Same transaction id, different totals: rejected, never
        // overwritten, and recorded in the conflict list.
        let err = hq
            .apply_batch(&batch(
                vec![tx_entry(store_id, tx_id, "999.0000")],
                store_id,
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, SyncError::Conflict(_)));
        assert_eq!(hq.transaction(tx_id).unwrap()["grand_total"], "100.0000");
        assert_eq!(hq.conflicts().len(), 1);
        // The rejected entry was not marked applied.
        assert_eq!(hq.applied_count(), 1);
    }

    #[tokio::test]
    async fn ledger_deltas_accumulate_never_overwrite() {
        let hq = MemoryHq::new();
        let store_id = Uuid::now_v7();
        let product_id = Uuid::now_v7();
        let mk = |delta: &str| {
            OutboxEntry::new(
                store_id,
                "inventory_ledger",
                Uuid::now_v7(),
                serde_json::json!({"product_id": product_id.to_string(), "delta_on_hand": delta}),
            )
        };
        hq.apply_batch(&batch(vec![mk("-2"), mk("-3"), mk("1")], store_id))
            .await
            .unwrap();
        assert_eq!(hq.inventory_on_hand(product_id), Decimal::from(-4));
    }
}
