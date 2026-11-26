//! Edge-level chaos test (Phase 6, v1.0): `Edge::sync_now` against a
//! fault-injecting HQ sink.
//!
//! Proves the end-to-end half of the sync contract: when a sink
//! applies only part of a batch and reports honestly, the edge marks
//! ONLY the reported ids synced — the unapplied entries stay pending
//! and complete on the next sync, with exact ledger totals and no
//! double-counting. Before the Phase 6 fix (`sync_now` marked the
//! whole batch synced on any `Ok`), the entries in the second half
//! of this test would have been silently lost.

use df_domain::Currency;
use df_edge::{Edge, EdgeConfig};
use df_sync::{ApplyReport, DeltaBatch, HqSink, MemoryHq, Outbox, OutboxEntry, SyncError};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn open_edge() -> Edge {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(0xA001),
        org_id: Uuid::from_u128(0x2000),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Chaos Test Store".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    Edge::open(config).await.unwrap()
}

fn inv_entry(store: Uuid, delta: &str) -> OutboxEntry {
    OutboxEntry::new(
        store,
        "inventory_ledger",
        Uuid::now_v7(),
        serde_json::json!({
            "product_id": Uuid::from_u128(0xC001).to_string(),
            "delta_on_hand": delta,
            "reason_type": "sale",
            "reference_id": Uuid::now_v7().to_string(),
        }),
    )
}

/// A sink that applies only the first `limit` entries of each batch
/// and reports exactly those — the shape of a real HQ that got cut
/// off mid-batch (or a slow consumer that acked a prefix). It returns
/// `Ok`, so the edge must rely on `ApplyReport::applied_ids`, not on
/// the absence of an error, to know what was resolved.
struct PartialHq {
    inner: Arc<MemoryHq>,
    limit: usize,
}

impl PartialHq {
    fn new(inner: Arc<MemoryHq>, limit: usize) -> Self {
        Self { inner, limit }
    }
}

impl HqSink for PartialHq {
    fn apply_batch<'a>(
        &'a self,
        batch: &'a DeltaBatch,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ApplyReport, SyncError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let prefix = DeltaBatch::new(
                batch.store_id,
                batch.entries.iter().take(self.limit).cloned().collect(),
            );
            self.inner.apply_batch(&prefix).await
        })
    }
}

#[tokio::test]
async fn partial_apply_leaves_unapplied_entries_pending() {
    let edge = open_edge().await;
    let store = Uuid::from_u128(0xA001);
    let hq = Arc::new(MemoryHq::new());
    let product = Uuid::from_u128(0xC001);

    // Six inventory deltas, net -7 (-2,-3,+1,-1,-4,+2).
    for delta in ["-2", "-3", "1", "-1", "-4", "2"] {
        Outbox::enqueue(edge.pool(), &inv_entry(store, delta))
            .await
            .unwrap();
    }
    assert_eq!(Outbox::pending_count(edge.pool(), store).await.unwrap(), 6);

    // Sync 1: the sink applies only the first 3 entries (-2,-3,+1).
    let partial = PartialHq::new(Arc::clone(&hq), 3);
    let report = edge.sync_now(&partial).await.unwrap();
    assert_eq!(report.applied, 3);
    assert_eq!(report.applied_ids.len(), 3);
    assert_eq!(hq.inventory_on_hand(product), dec("-4"));
    // The other 3 are STILL pending — not silently marked synced.
    assert_eq!(Outbox::pending_count(edge.pool(), store).await.unwrap(), 3);

    // Sync 2: a healthy sink completes the remaining 3 (-1,-4,+2).
    let report = edge.sync_now(hq.as_ref()).await.unwrap();
    assert_eq!(report.applied, 3);
    assert_eq!(Outbox::pending_count(edge.pool(), store).await.unwrap(), 0);
    assert_eq!(hq.inventory_on_hand(product), dec("-7"));
    assert_eq!(hq.applied_count(), 6);

    // Sync 3: nothing pending, no-op.
    let report = edge.sync_now(hq.as_ref()).await.unwrap();
    assert_eq!(report, ApplyReport::empty());
}

#[tokio::test]
async fn sync_now_skips_conflict_and_keeps_it_pending() {
    let edge = open_edge().await;
    let store = Uuid::from_u128(0xA001);
    let hq = Arc::new(MemoryHq::new());
    let tx_id = Uuid::from_u128(0xE001);
    let product = Uuid::from_u128(0xC001);

    // HQ already has TX-1 at 216.1088.
    hq.apply_batch(&DeltaBatch::new(
        store,
        vec![OutboxEntry::new(
            store,
            "transaction",
            tx_id,
            serde_json::json!({"id": tx_id.to_string(), "grand_total": "216.1088"}),
        )],
    ))
    .await
    .unwrap();

    // Edge outbox: a conflicting re-send of TX-1 (999.0000) + a good
    // inventory delta. `sync_now` must succeed, apply the good entry,
    // and leave ONLY the conflicting entry pending.
    Outbox::enqueue(
        edge.pool(),
        &OutboxEntry::new(
            store,
            "transaction",
            tx_id,
            serde_json::json!({"id": tx_id.to_string(), "grand_total": "999.0000"}),
        ),
    )
    .await
    .unwrap();
    Outbox::enqueue(edge.pool(), &inv_entry(store, "-5"))
        .await
        .unwrap();

    let report = edge.sync_now(hq.as_ref()).await.unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.conflicts, 1);
    assert_eq!(Outbox::pending_count(edge.pool(), store).await.unwrap(), 1);
    assert_eq!(hq.inventory_on_hand(product), dec("-5"));
    assert_eq!(hq.transaction(tx_id).unwrap()["grand_total"], "216.1088");

    // A second sync re-skips the conflict and changes nothing.
    let report = edge.sync_now(hq.as_ref()).await.unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.conflicts, 1);
    assert_eq!(Outbox::pending_count(edge.pool(), store).await.unwrap(), 1);
    assert_eq!(hq.inventory_on_hand(product), dec("-5"));
    assert_eq!(hq.conflicts().len(), 1);
}
