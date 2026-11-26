//! Chaos tests for offline sync (Phase 6, v1.0).
//!
//! These tests fault-inject the transport between the edge outbox and
//! HQ — dropped messages, reordered batches, duplicated deliveries,
//! and conflicting edits to the same aggregate from two edges — and
//! PROVE the invariants from spec §8 guardrail 2 / `resolve.rs`:
//!
//! * **Replay safety**: applying a batch twice (or three times) is a
//!   no-op; ledger totals after the first apply equal totals after
//!   every replay, to the cent.
//! * **Append-only merge**: every delta survives; two edges selling
//!   the same SKU both subtract stock — no lost rows, no
//!   double-counting.
//! * **No silent loss**: only ids the sink reports in
//!   `ApplyReport::applied_ids` are marked synced; dropped or
//!   skipped entries stay pending and are redelivered.
//!
//! Everything is deterministic: fixed UUIDs/amounts and a seeded
//! xorshift PRNG — no wall clock, no network, no sleeps.

use df_db::{Dialect, connect, migrate};
use df_sync::{ApplyReport, DeltaBatch, HqSink, MemoryHq, Outbox, OutboxEntry};
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::str::FromStr;
use uuid::Uuid;

const STORE_A: u128 = 0xA001;
const STORE_B: u128 = 0xB001;
const PRODUCT: u128 = 0xC001;
const PRODUCT_2: u128 = 0xC002;
const CUSTOMER: u128 = 0xD001;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn uuid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn inv_entry(id: u128, store: Uuid, product: Uuid, delta: &str) -> OutboxEntry {
    OutboxEntry {
        id: uuid(id),
        store_id: store,
        aggregate_type: "inventory_ledger".to_string(),
        aggregate_id: uuid(id + 0x1000),
        payload: serde_json::json!({
            "product_id": product.to_string(),
            "delta_on_hand": delta,
            "reason_type": "sale",
            "reference_id": uuid(id + 0x2000).to_string(),
        }),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-11-25T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        synced_at: None,
        retry_count: 0,
        last_error: None,
    }
}

fn ar_entry(id: u128, store: Uuid, customer: Uuid, amount: &str) -> OutboxEntry {
    OutboxEntry {
        id: uuid(id),
        store_id: store,
        aggregate_type: "ar_ledger".to_string(),
        aggregate_id: uuid(id + 0x1000),
        payload: serde_json::json!({
            "customer_id": customer.to_string(),
            "amount": amount,
            "entry_type": "charge",
            "reference_id": uuid(id + 0x2000).to_string(),
        }),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-11-25T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        synced_at: None,
        retry_count: 0,
        last_error: None,
    }
}

fn tx_entry(id: u128, store: Uuid, tx_id: Uuid, total: &str) -> OutboxEntry {
    OutboxEntry {
        id: uuid(id),
        store_id: store,
        aggregate_type: "transaction".to_string(),
        aggregate_id: tx_id,
        payload: serde_json::json!({
            "id": tx_id.to_string(),
            "doc_number": format!("TX-{id:04x}"),
            "doc_type": "sale",
            "grand_total": total,
        }),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-11-25T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        synced_at: None,
        retry_count: 0,
        last_error: None,
    }
}

/// The 12-entry fixture used by the drop/reorder/replay tests.
/// Inventory on PRODUCT: -2, -3, +1, -1, -4, +2 = **-7**.
/// AR on CUSTOMER: 100, 50, -25, 75 = **200.0000**.
/// Two transactions: TX-1 216.1088, TX-2 86.6044.
fn fixture(store: Uuid) -> Vec<OutboxEntry> {
    vec![
        inv_entry(0x01, store, uuid(PRODUCT), "-2"),
        inv_entry(0x02, store, uuid(PRODUCT), "-3"),
        ar_entry(0x03, store, uuid(CUSTOMER), "100.0000"),
        inv_entry(0x04, store, uuid(PRODUCT), "1"),
        ar_entry(0x05, store, uuid(CUSTOMER), "50.0000"),
        tx_entry(0x06, store, uuid(0xE001), "216.1088"),
        inv_entry(0x07, store, uuid(PRODUCT), "-1"),
        ar_entry(0x08, store, uuid(CUSTOMER), "-25.0000"),
        inv_entry(0x09, store, uuid(PRODUCT), "-4"),
        tx_entry(0x0A, store, uuid(0xE002), "86.6044"),
        inv_entry(0x0B, store, uuid(PRODUCT), "2"),
        ar_entry(0x0C, store, uuid(CUSTOMER), "75.0000"),
    ]
}

fn batch(store: Uuid, entries: Vec<OutboxEntry>) -> DeltaBatch {
    DeltaBatch::new(store, entries)
}

/// Deterministic xorshift64 — no `rand` dependency needed for a
/// seeded chaos sweep.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[tokio::test]
async fn dropped_messages_are_redelivered_with_exact_totals() {
    let hq = MemoryHq::new();
    let store = uuid(STORE_A);
    let entries = fixture(store);

    // Fault: the transport drops entries 1, 4, 7, 10 (0-based) —
    // HQ never sees them. 4 of 12 messages dropped.
    let dropped_idx: HashSet<usize> = [1, 4, 7, 10].into_iter().collect();
    let (delivered, dropped): (Vec<_>, Vec<_>) = entries
        .iter()
        .enumerate()
        .partition(|(i, _)| !dropped_idx.contains(i));
    let delivered: Vec<OutboxEntry> = delivered.into_iter().map(|(_, e)| e.clone()).collect();
    let dropped: Vec<OutboxEntry> = dropped.into_iter().map(|(_, e)| e.clone()).collect();

    let report = hq.apply_batch(&batch(store, delivered)).await.unwrap();
    assert_eq!(report.applied, 8);
    assert_eq!(report.duplicates, 0);
    // Partial totals after the drop (delivered: inv 0,3,6,8; AR 2,11;
    // TX-1 and TX-2): inv -2+1-1-4 = -6; AR 100+75 = 175.
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), dec("-6"));
    assert_eq!(hq.ar_balance(uuid(CUSTOMER)), dec("175.0000"));
    assert!(hq.transaction(uuid(0xE001)).is_some());
    assert!(hq.transaction(uuid(0xE002)).is_some());
    assert_eq!(hq.applied_count(), 8);

    // Redelivery of the dropped 4: totals become exact, nothing
    // double-counts.
    let report = hq.apply_batch(&batch(store, dropped)).await.unwrap();
    assert_eq!(report.applied, 4);
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), dec("-7"));
    assert_eq!(hq.ar_balance(uuid(CUSTOMER)), dec("200.0000"));
    assert_eq!(
        hq.transaction(uuid(0xE002)).unwrap()["grand_total"],
        "86.6044"
    );
    assert_eq!(hq.applied_count(), 12);
}

#[tokio::test]
async fn reordered_batches_merge_to_exact_totals() {
    let hq = MemoryHq::new();
    let store = uuid(STORE_A);
    let entries = fixture(store);
    let (older, newer) = entries.split_at(6);
    let older = older.to_vec();
    let newer = newer.to_vec();

    // Fault: the newer batch arrives FIRST (out-of-order delivery).
    let report = hq.apply_batch(&batch(store, newer.clone())).await.unwrap();
    assert_eq!(report.applied, 6);
    let report = hq.apply_batch(&batch(store, older.clone())).await.unwrap();
    assert_eq!(report.applied, 6);
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), dec("-7"));
    assert_eq!(hq.ar_balance(uuid(CUSTOMER)), dec("200.0000"));
    assert_eq!(hq.applied_count(), 12);

    // Same set, entries reversed inside one batch: identical totals.
    let hq2 = MemoryHq::new();
    let mut reversed = entries.clone();
    reversed.reverse();
    hq2.apply_batch(&batch(store, reversed)).await.unwrap();
    assert_eq!(hq2.inventory_on_hand(uuid(PRODUCT)), dec("-7"));
    assert_eq!(hq2.ar_balance(uuid(CUSTOMER)), dec("200.0000"));
    assert_eq!(hq2.applied_count(), 12);
}

#[tokio::test]
async fn duplicated_delivery_is_a_noop() {
    let hq = MemoryHq::new();
    let store = uuid(STORE_A);
    let entries = fixture(store);

    let first = hq
        .apply_batch(&batch(store, entries.clone()))
        .await
        .unwrap();
    assert_eq!(
        first,
        ApplyReport {
            applied: 12,
            duplicates: 0,
            conflicts: 0,
            applied_ids: entries.iter().map(|e| e.id).collect(),
        }
    );
    let inv_after_first = hq.inventory_on_hand(uuid(PRODUCT));
    let ar_after_first = hq.ar_balance(uuid(CUSTOMER));

    // Two full replays: every entry a duplicate, totals unchanged.
    for _ in 0..2 {
        let replay = hq
            .apply_batch(&batch(store, entries.clone()))
            .await
            .unwrap();
        assert_eq!(replay.applied, 0);
        assert_eq!(replay.duplicates, 12);
        assert_eq!(replay.conflicts, 0);
        assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), inv_after_first);
        assert_eq!(hq.ar_balance(uuid(CUSTOMER)), ar_after_first);
    }
    assert_eq!(inv_after_first, dec("-7"));
    assert_eq!(ar_after_first, dec("200.0000"));
    assert_eq!(hq.applied_count(), 12);
}

#[tokio::test]
async fn two_edges_same_aggregate_merge_by_union() {
    let hq = MemoryHq::new();
    let store_a = uuid(STORE_A);
    let store_b = uuid(STORE_B);

    // Both edges sell the same SKU and charge the same customer on
    // the same day: A -2 / +100.0000, B -3 / +50.0000. Neither delta
    // may be lost or overwrite the other.
    let batch_a = batch(
        store_a,
        vec![
            inv_entry(0x11, store_a, uuid(PRODUCT), "-2"),
            ar_entry(0x12, store_a, uuid(CUSTOMER), "100.0000"),
        ],
    );
    let batch_b = batch(
        store_b,
        vec![
            inv_entry(0x21, store_b, uuid(PRODUCT), "-3"),
            ar_entry(0x22, store_b, uuid(CUSTOMER), "50.0000"),
        ],
    );
    hq.apply_batch(&batch_a).await.unwrap();
    hq.apply_batch(&batch_b).await.unwrap();
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), dec("-5"));
    assert_eq!(hq.ar_balance(uuid(CUSTOMER)), dec("150.0000"));
    assert_eq!(hq.applied_count(), 4);

    // Replaying edge A's batch (at-least-once delivery) changes
    // nothing: the union keeps 4 distinct entries, totals exact.
    let replay = hq.apply_batch(&batch_a).await.unwrap();
    assert_eq!(replay.applied, 0);
    assert_eq!(replay.duplicates, 2);
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), dec("-5"));
    assert_eq!(hq.ar_balance(uuid(CUSTOMER)), dec("150.0000"));
}

#[tokio::test]
async fn conflicting_edits_from_two_edges_do_not_block_sync() {
    let hq = MemoryHq::new();
    let store_a = uuid(STORE_A);
    let store_b = uuid(STORE_B);
    let tx_id = uuid(0xE001);

    // Edge A sold TX-1 for 216.1088; HQ has it.
    hq.apply_batch(&batch(
        store_a,
        vec![tx_entry(0x31, store_a, tx_id, "216.1088")],
    ))
    .await
    .unwrap();

    // Edge B re-sends the same transaction id with a different total
    // (bad edit), batched with two good entries. The conflict is
    // skipped and recorded; the good entries still apply.
    let bad = tx_entry(0x41, store_b, tx_id, "999.0000");
    let good_inv = inv_entry(0x42, store_b, uuid(PRODUCT_2), "-1");
    let good_ar = ar_entry(0x43, store_b, uuid(CUSTOMER), "10.0000");
    let report = hq
        .apply_batch(&batch(store_b, vec![bad.clone(), good_inv, good_ar]))
        .await
        .unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(report.conflicts, 1);
    assert_eq!(report.duplicates, 0);
    assert!(!report.applied_ids.contains(&bad.id));
    assert_eq!(
        hq.transaction(tx_id).unwrap()["grand_total"],
        "216.1088",
        "stored transaction is never overwritten"
    );
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT_2)), dec("-1"));
    assert_eq!(hq.ar_balance(uuid(CUSTOMER)), dec("10.0000"));
    assert_eq!(hq.conflicts().len(), 1);
    assert_eq!(hq.applied_count(), 3, "conflict not counted as applied");
}

#[tokio::test]
async fn outbox_redrain_marks_only_resolved_ids_synced() {
    let pool = connect("sqlite::memory:").await.unwrap();
    migrate(&pool, Dialect::Sqlite).await.unwrap();
    let store = uuid(STORE_A);
    let hq = MemoryHq::new();

    let entries = fixture(store);
    for entry in &entries {
        Outbox::enqueue(&pool, entry).await.unwrap();
    }
    assert_eq!(Outbox::pending_count(&pool, store).await.unwrap(), 12);

    // Drain 1: transport drops the last 3 entries. Only the 9 the
    // sink resolved are marked synced.
    let pending = Outbox::pending(&pool, store, 1000).await.unwrap();
    assert_eq!(pending.len(), 12);
    let delivered = pending[..9].to_vec();
    let report = hq.apply_batch(&batch(store, delivered)).await.unwrap();
    assert_eq!(report.applied, 9);
    Outbox::mark_synced(&pool, &report.applied_ids)
        .await
        .unwrap();
    assert_eq!(Outbox::pending_count(&pool, store).await.unwrap(), 3);

    // Drain 2: the 3 dropped entries arrive; all resolve.
    let pending = Outbox::pending(&pool, store, 1000).await.unwrap();
    assert_eq!(pending.len(), 3);
    let report = hq.apply_batch(&batch(store, pending)).await.unwrap();
    assert_eq!(report.applied, 3);
    Outbox::mark_synced(&pool, &report.applied_ids)
        .await
        .unwrap();
    assert_eq!(Outbox::pending_count(&pool, store).await.unwrap(), 0);
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), dec("-7"));
    assert_eq!(hq.ar_balance(uuid(CUSTOMER)), dec("200.0000"));
    assert_eq!(hq.applied_count(), 12);
}

#[tokio::test]
async fn outbox_conflicting_entry_stays_pending_without_blocking() {
    let pool = connect("sqlite::memory:").await.unwrap();
    migrate(&pool, Dialect::Sqlite).await.unwrap();
    let store_a = uuid(STORE_A);
    let store_b = uuid(STORE_B);
    let hq = MemoryHq::new();
    let tx_id = uuid(0xE001);

    // HQ already has TX-1 at 216.1088 (from edge A).
    hq.apply_batch(&batch(
        store_a,
        vec![tx_entry(0x51, store_a, tx_id, "216.1088")],
    ))
    .await
    .unwrap();

    // Edge B's outbox: a conflicting re-send of TX-1 + one good entry.
    let bad = tx_entry(0x61, store_b, tx_id, "999.0000");
    let good = inv_entry(0x62, store_b, uuid(PRODUCT), "-5");
    Outbox::enqueue(&pool, &bad).await.unwrap();
    Outbox::enqueue(&pool, &good).await.unwrap();

    // Drain: batch succeeds (conflict skipped), only the good entry
    // is marked synced; the conflicting entry stays pending.
    let pending = Outbox::pending(&pool, store_b, 1000).await.unwrap();
    let report = hq.apply_batch(&batch(store_b, pending)).await.unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(report.conflicts, 1);
    Outbox::mark_synced(&pool, &report.applied_ids)
        .await
        .unwrap();
    assert_eq!(Outbox::pending_count(&pool, store_b).await.unwrap(), 1);
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), dec("-5"));

    // Next drain: the same conflicting entry is skipped again —
    // totals do not move, no duplicate conflict is recorded, and
    // the stored transaction is untouched.
    let pending = Outbox::pending(&pool, store_b, 1000).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, bad.id);
    let report = hq.apply_batch(&batch(store_b, pending)).await.unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.conflicts, 1);
    assert_eq!(hq.conflicts().len(), 1);
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), dec("-5"));
    assert_eq!(hq.transaction(tx_id).unwrap()["grand_total"], "216.1088");
}

#[tokio::test]
async fn randomized_chaos_sweep() {
    // 20-entry set, 40 fault-injected rounds, seeded PRNG.
    // Model: the exact totals of the entries HQ has seen at least
    // once. After every round HQ must equal the model — no lost
    // rows, no double-counting, whatever the transport did.
    let hq = MemoryHq::new();
    let store = uuid(STORE_A);
    let mut entries: Vec<OutboxEntry> = Vec::new();
    for i in 0..10u128 {
        entries.push(inv_entry(
            0x100 + i,
            store,
            uuid(PRODUCT),
            if i % 2 == 0 { "-1" } else { "2" },
        ));
    }
    for i in 0..10u128 {
        entries.push(ar_entry(
            0x200 + i,
            store,
            uuid(CUSTOMER),
            if i % 3 == 0 { "10.0000" } else { "-5.0000" },
        ));
    }
    // Full-set totals: inv 5×(-1) + 5×(2) = 5; AR 4×10 + 6×(-5) = 10.
    let full_inv = dec("5");
    let full_ar = dec("10.0000");

    let mut rng = XorShift(0xC0FFEE20261126);
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut delivered_batches: Vec<Vec<OutboxEntry>> = Vec::new();
    let mut dropped = 0usize;
    let mut duplicated = 0usize;
    let mut reordered = 0usize;

    for _ in 0..40 {
        match rng.below(4) {
            // 0: deliver a random subset (drop the rest).
            0 => {
                let mut subset: Vec<OutboxEntry> = entries
                    .iter()
                    .filter(|_| rng.below(2) == 0)
                    .cloned()
                    .collect();
                if subset.is_empty() {
                    subset.push(entries[rng.below(entries.len())].clone());
                }
                dropped += entries.len() - subset.len();
                for e in &subset {
                    seen.insert(e.id);
                }
                hq.apply_batch(&batch(store, subset)).await.unwrap();
            }
            // 1: duplicate a previously delivered batch, whole.
            1 => {
                if let Some(prev) = delivered_batches.get(rng.below(delivered_batches.len())) {
                    duplicated += prev.len();
                    hq.apply_batch(&batch(store, prev.clone())).await.unwrap();
                }
            }
            // 2: deliver a random subset in shuffled order.
            2 => {
                let mut subset: Vec<OutboxEntry> = entries
                    .iter()
                    .filter(|_| rng.below(2) == 0)
                    .cloned()
                    .collect();
                if subset.is_empty() {
                    subset.push(entries[rng.below(entries.len())].clone());
                }
                for i in (1..subset.len()).rev() {
                    let j = rng.below(i + 1);
                    subset.swap(i, j);
                }
                reordered += 1;
                for e in &subset {
                    seen.insert(e.id);
                }
                delivered_batches.push(subset.clone());
                hq.apply_batch(&batch(store, subset)).await.unwrap();
            }
            // 3: deliver everything seen so far plus one unseen entry.
            _ => {
                let unseen: Vec<&OutboxEntry> =
                    entries.iter().filter(|e| !seen.contains(&e.id)).collect();
                let mut subset: Vec<OutboxEntry> = entries
                    .iter()
                    .filter(|e| seen.contains(&e.id))
                    .cloned()
                    .collect();
                if let Some(first) = unseen.first() {
                    subset.push((*first).clone());
                    seen.insert(first.id);
                }
                delivered_batches.push(subset.clone());
                hq.apply_batch(&batch(store, subset)).await.unwrap();
            }
        }

        // Model check after every round: HQ equals the union of
        // everything delivered at least once — nothing lost, nothing
        // doubled, regardless of drops, duplicates or order.
        let mut model_inv = Decimal::ZERO;
        let mut model_ar = Decimal::ZERO;
        for e in &entries {
            if !seen.contains(&e.id) {
                continue;
            }
            match e.aggregate_type.as_str() {
                "inventory_ledger" => {
                    model_inv +=
                        Decimal::from_str(e.payload["delta_on_hand"].as_str().unwrap()).unwrap();
                }
                "ar_ledger" => {
                    model_ar += Decimal::from_str(e.payload["amount"].as_str().unwrap()).unwrap();
                }
                _ => {}
            }
        }
        assert_eq!(
            hq.inventory_on_hand(uuid(PRODUCT)),
            model_inv,
            "inventory diverged from the delivered-union model"
        );
        assert_eq!(
            hq.ar_balance(uuid(CUSTOMER)),
            model_ar,
            "AR diverged from the delivered-union model"
        );
        assert_eq!(
            hq.applied_count(),
            seen.len(),
            "applied count diverged from distinct delivered entries"
        );
    }

    // Final drain: deliver anything never seen; totals must be the
    // exact full-set totals.
    let remaining: Vec<OutboxEntry> = entries
        .iter()
        .filter(|e| !seen.contains(&e.id))
        .cloned()
        .collect();
    let remaining_n = remaining.len();
    hq.apply_batch(&batch(store, remaining)).await.unwrap();
    assert_eq!(hq.inventory_on_hand(uuid(PRODUCT)), full_inv);
    assert_eq!(hq.ar_balance(uuid(CUSTOMER)), full_ar);
    assert_eq!(hq.applied_count(), 20);
    // The sweep actually exercised faults (not a vacuous pass).
    assert!(dropped > 0, "sweep dropped nothing");
    assert!(reordered > 0, "sweep reordered nothing");
    assert_eq!(
        hq.applied_count(),
        20,
        "20 distinct entries applied after {remaining_n} never-seen entries delivered"
    );
    let _ = duplicated;
}

/// Running totals the sweep model relies on, pinned as a unit check:
/// 5×(-1) + 5×(2) = 5; 4×10 + 6×(-5) = 10.
#[test]
fn chaos_fixture_totals_are_as_documented() {
    let mut inv = Decimal::ZERO;
    for i in 0..10u128 {
        inv += if i % 2 == 0 { dec("-1") } else { dec("2") };
    }
    assert_eq!(inv, dec("5"));
    let mut ar = Decimal::ZERO;
    for i in 0..10u128 {
        ar += if i % 3 == 0 {
            dec("10.0000")
        } else {
            dec("-5.0000")
        };
    }
    assert_eq!(ar, dec("10.0000"));
}
