//! The sync outbox (spec §5.5 `sync_outbox`).
//!
//! Every edge mutation enqueues one row per aggregate written, in the
//! same local transaction as the mutation itself, so a sale can never
//! be committed locally without a corresponding sync record. Rows are
//! drained in UUIDv7 (`id`) order, which is creation order, and
//! [`Outbox::mark_synced`] stamps `synced_at` once HQ acknowledges a
//! batch. [`Outbox::mark_failed`] bumps `retry_count` and records the
//! error, leaving the row pending for the next drain.
//!
//! Payload conventions (shared with `df-edge`, [`crate::sink::MemoryHq`]
//! and [`crate::db::DbHq`]). The DB sink rebuilds HQ rows from these
//! payloads, so the edge enriches them with the row's full fields —
//! extra keys are ignored by sinks that do not need them, and older
//! edge payloads that omit the enrichment still apply (with zeroed
//! optional fields; balances stay exact):
//!
//! * `aggregate_type = "transaction"` — payload is the transaction
//!   JSON object (`id`, `doc_number`, `doc_type`, `status`,
//!   `customer_id`, `job_id`, `po_number`, `subtotal`, `tax_total`,
//!   `eco_fee_total`, `pos_fee_total`, `grand_total`; layaways also
//!   carry `deposit_paid`).
//! * `aggregate_type = "inventory_ledger"` — payload is the ledger
//!   row (`product_id`, all four deltas as decimal strings,
//!   `unit_cost`, `lot_number`, `reason_type`, `reference_id`,
//!   `created_at` RFC 3339).
//! * `aggregate_type = "ar_ledger"` — payload is the ledger row
//!   (`customer_id`, `amount` as a decimal string, `entry_type`,
//!   `reference_type`, `reference_id`, `due_date` `YYYY-MM-DD`,
//!   `job_id`, `created_at` RFC 3339).
//!
//! Money and timestamps cross the driver as text, following the
//! `df-db` convention (sqlx `Any` has no `Uuid`/`Decimal` codecs):
//! UUIDs as hyphenated text, decimals as exact strings, timestamps as
//! RFC 3339 text.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::any::AnyRow;
use sqlx::{AnyPool, Executor, Row};
use uuid::Uuid;

use crate::error::SyncError;

/// One row of `sync_outbox`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub id: Uuid,
    pub store_id: Uuid,
    pub aggregate_type: String,
    pub aggregate_id: Uuid,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub synced_at: Option<DateTime<Utc>>,
    pub retry_count: i32,
    pub last_error: Option<String>,
}

impl OutboxEntry {
    /// Mint a new unsynced entry with a UUIDv7 id and `Utc::now()`.
    pub fn new(
        store_id: Uuid,
        aggregate_type: impl Into<String>,
        aggregate_id: Uuid,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            store_id,
            aggregate_type: aggregate_type.into(),
            aggregate_id,
            payload,
            created_at: Utc::now(),
            synced_at: None,
            retry_count: 0,
            last_error: None,
        }
    }
}

/// One drain unit: all pending entries for a store, in UUIDv7 order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeltaBatch {
    pub batch_id: Uuid,
    pub store_id: Uuid,
    pub entries: Vec<OutboxEntry>,
}

impl DeltaBatch {
    pub fn new(store_id: Uuid, entries: Vec<OutboxEntry>) -> Self {
        Self {
            batch_id: Uuid::now_v7(),
            store_id,
            entries,
        }
    }
}

/// Sync progress for one store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncState {
    pub store_id: Uuid,
    pub last_batch_id: Option<Uuid>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub pending_count: u64,
}

fn entry_from_row(row: &AnyRow) -> Result<OutboxEntry, SyncError> {
    let parse_uuid = |name: &str| -> Result<Uuid, SyncError> {
        let text: String = row
            .try_get(name)
            .map_err(|e| SyncError::InvalidPayload(format!("column {name}: {e}")))?;
        Uuid::parse_str(&text).map_err(|e| SyncError::InvalidPayload(format!("column {name}: {e}")))
    };
    let parse_dt = |name: &str| -> Result<DateTime<Utc>, SyncError> {
        let text: String = row
            .try_get(name)
            .map_err(|e| SyncError::InvalidPayload(format!("column {name}: {e}")))?;
        DateTime::parse_from_rfc3339(&text)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| SyncError::InvalidPayload(format!("column {name}: {e}")))
    };
    let parse_opt_dt = |name: &str| -> Result<Option<DateTime<Utc>>, SyncError> {
        let text: Option<String> = row
            .try_get(name)
            .map_err(|e| SyncError::InvalidPayload(format!("column {name}: {e}")))?;
        match text {
            None => Ok(None),
            Some(text) => DateTime::parse_from_rfc3339(&text)
                .map(|dt| Some(dt.with_timezone(&Utc)))
                .map_err(|e| SyncError::InvalidPayload(format!("column {name}: {e}"))),
        }
    };
    let payload_text: String = row
        .try_get("payload_json")
        .map_err(|e| SyncError::InvalidPayload(format!("column payload_json: {e}")))?;
    let payload: serde_json::Value = serde_json::from_str(&payload_text)?;
    Ok(OutboxEntry {
        id: parse_uuid("id")?,
        store_id: parse_uuid("store_id")?,
        aggregate_type: row
            .try_get("aggregate_type")
            .map_err(|e| SyncError::InvalidPayload(format!("column aggregate_type: {e}")))?,
        aggregate_id: parse_uuid("aggregate_id")?,
        payload,
        created_at: parse_dt("created_at")?,
        synced_at: parse_opt_dt("synced_at")?,
        retry_count: row
            .try_get("retry_count")
            .map_err(|e| SyncError::InvalidPayload(format!("column retry_count: {e}")))?,
        last_error: row
            .try_get("last_error")
            .map_err(|e| SyncError::InvalidPayload(format!("column last_error: {e}")))?,
    })
}

/// The outbox, backed by a [`sqlx::AnyPool`].
///
/// The associated functions take the pool explicitly so they can be
/// used from anywhere; [`Outbox::enqueue_entry`] delegates to them for
/// the pool this `Outbox` was built with.
#[derive(Debug, Clone)]
pub struct Outbox {
    pool: AnyPool,
}

impl Outbox {
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// Insert one entry. See [`Outbox::enqueue_exec`] for the variant
    /// that runs inside a caller's transaction.
    pub async fn enqueue(pool: &AnyPool, entry: &OutboxEntry) -> Result<(), SyncError> {
        Self::enqueue_exec(pool, entry).await
    }

    /// Insert one entry using any sqlx executor — pass `&mut *tx` to
    /// enqueue atomically with the mutation the entry describes.
    pub async fn enqueue_exec<'e, E>(executor: E, entry: &OutboxEntry) -> Result<(), SyncError>
    where
        E: Executor<'e, Database = sqlx::Any>,
    {
        sqlx::query(
            "INSERT INTO sync_outbox (id, store_id, aggregate_type, aggregate_id, payload_json, created_at, synced_at, retry_count, last_error) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(entry.id.to_string())
        .bind(entry.store_id.to_string())
        .bind(&entry.aggregate_type)
        .bind(entry.aggregate_id.to_string())
        .bind(entry.payload.to_string())
        .bind(entry.created_at.to_rfc3339())
        .bind(entry.synced_at.map(|dt| dt.to_rfc3339()))
        .bind(entry.retry_count)
        .bind(&entry.last_error)
        .execute(executor)
        .await?;
        Ok(())
    }

    /// Unsynced entries for a store, oldest (lowest UUIDv7) first.
    pub async fn pending(
        pool: &AnyPool,
        store_id: Uuid,
        limit: i64,
    ) -> Result<Vec<OutboxEntry>, SyncError> {
        let rows = sqlx::query(
            "SELECT * FROM sync_outbox WHERE store_id = $1 AND synced_at IS NULL ORDER BY id ASC LIMIT $2",
        )
        .bind(store_id.to_string())
        .bind(limit)
        .fetch_all(pool)
        .await?;
        rows.iter().map(entry_from_row).collect()
    }

    /// Number of unsynced entries for a store.
    pub async fn pending_count(pool: &AnyPool, store_id: Uuid) -> Result<u64, SyncError> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM sync_outbox WHERE store_id = $1 AND synced_at IS NULL",
        )
        .bind(store_id.to_string())
        .fetch_one(pool)
        .await?;
        let n: i64 = row
            .try_get("n")
            .map_err(|e| SyncError::InvalidPayload(format!("column n: {e}")))?;
        Ok(n.max(0) as u64)
    }

    /// Mark entries synced, stamping `synced_at = now`.
    pub async fn mark_synced(pool: &AnyPool, ids: &[Uuid]) -> Result<(), SyncError> {
        if ids.is_empty() {
            return Ok(());
        }
        // `$N` placeholders: `$1` is synced_at, `$2..` are the ids.
        // (sqlx's `Any` driver does not rewrite `?`; a literal `?`
        // syntax-errors on PostgreSQL. `$N` works on SQLite too.)
        let placeholders = (2..=ids.len() + 1)
            .map(|i| format!("${i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("UPDATE sync_outbox SET synced_at = $1 WHERE id IN ({placeholders})");
        let mut query = sqlx::query(&sql).bind(Utc::now().to_rfc3339());
        for id in ids {
            query = query.bind(id.to_string());
        }
        query.execute(pool).await?;
        Ok(())
    }

    /// Record a failed sync attempt: bump `retry_count`, store the
    /// error, leave the row pending.
    pub async fn mark_failed(pool: &AnyPool, id: Uuid, error: &str) -> Result<(), SyncError> {
        sqlx::query(
            "UPDATE sync_outbox SET retry_count = retry_count + 1, last_error = $1 WHERE id = $2",
        )
        .bind(error)
        .bind(id.to_string())
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Sync progress derived from the outbox alone: `last_batch_id` is
    /// always `None` here (batch ids live in the `sync_state` table,
    /// maintained by `df-db`/`df-edge`); `last_synced_at` is the newest
    /// `synced_at` in the outbox.
    pub async fn sync_state(pool: &AnyPool, store_id: Uuid) -> Result<SyncState, SyncError> {
        let pending_count = Self::pending_count(pool, store_id).await?;
        let row = sqlx::query(
            "SELECT MAX(synced_at) AS last_synced FROM sync_outbox WHERE store_id = $1 AND synced_at IS NOT NULL",
        )
        .bind(store_id.to_string())
        .fetch_one(pool)
        .await?;
        let last_synced: Option<String> = row
            .try_get("last_synced")
            .map_err(|e| SyncError::InvalidPayload(format!("column last_synced: {e}")))?;
        let last_synced_at = match last_synced {
            None => None,
            Some(text) => Some(
                DateTime::parse_from_rfc3339(&text)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| SyncError::InvalidPayload(format!("column last_synced: {e}")))?,
            ),
        };
        Ok(SyncState {
            store_id,
            last_batch_id: None,
            last_synced_at,
            pending_count,
        })
    }

    // --- instance wrappers -------------------------------------------------

    pub async fn enqueue_entry(&self, entry: &OutboxEntry) -> Result<(), SyncError> {
        Self::enqueue(&self.pool, entry).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_db::{Dialect, connect, migrate};

    async fn test_pool() -> AnyPool {
        let pool = connect("sqlite::memory:").await.unwrap();
        migrate(&pool, Dialect::Sqlite).await.unwrap();
        pool
    }

    fn entry(store_id: Uuid) -> OutboxEntry {
        OutboxEntry::new(
            store_id,
            "transaction",
            Uuid::now_v7(),
            serde_json::json!({"id": "x", "grand_total": "10.0000"}),
        )
    }

    #[tokio::test]
    async fn enqueue_pending_mark_synced_round_trip() {
        let pool = test_pool().await;
        let store_id = Uuid::now_v7();
        let first = entry(store_id);
        let second = entry(store_id);
        Outbox::enqueue(&pool, &first).await.unwrap();
        Outbox::enqueue(&pool, &second).await.unwrap();

        let pending = Outbox::pending(&pool, store_id, 10).await.unwrap();
        assert_eq!(pending.len(), 2);
        // UUIDv7 order: the first-enqueued entry sorts first.
        assert_eq!(pending[0].id, first.id);
        assert_eq!(pending[1].id, second.id);
        assert_eq!(pending[0].payload["grand_total"], "10.0000");
        assert_eq!(Outbox::pending_count(&pool, store_id).await.unwrap(), 2);

        // Limit honoured.
        let limited = Outbox::pending(&pool, store_id, 1).await.unwrap();
        assert_eq!(limited.len(), 1);

        Outbox::mark_synced(&pool, &[first.id]).await.unwrap();
        let pending = Outbox::pending(&pool, store_id, 10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, second.id);
        assert_eq!(Outbox::pending_count(&pool, store_id).await.unwrap(), 1);

        // Other stores are isolated.
        assert!(
            Outbox::pending(&pool, Uuid::now_v7(), 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn mark_failed_bumps_retry_and_keeps_pending() {
        let pool = test_pool().await;
        let store_id = Uuid::now_v7();
        let e = entry(store_id);
        Outbox::enqueue(&pool, &e).await.unwrap();
        Outbox::mark_failed(&pool, e.id, "hq unreachable")
            .await
            .unwrap();
        Outbox::mark_failed(&pool, e.id, "hq unreachable")
            .await
            .unwrap();
        let pending = Outbox::pending(&pool, store_id, 10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].retry_count, 2);
        assert_eq!(pending[0].last_error.as_deref(), Some("hq unreachable"));
        assert!(pending[0].synced_at.is_none());
    }

    #[tokio::test]
    async fn sync_state_derives_from_outbox() {
        let pool = test_pool().await;
        let store_id = Uuid::now_v7();
        let state = Outbox::sync_state(&pool, store_id).await.unwrap();
        assert_eq!(state.pending_count, 0);
        assert!(state.last_synced_at.is_none());
        let e = entry(store_id);
        Outbox::enqueue(&pool, &e).await.unwrap();
        assert_eq!(
            Outbox::sync_state(&pool, store_id)
                .await
                .unwrap()
                .pending_count,
            1
        );
        Outbox::mark_synced(&pool, &[e.id]).await.unwrap();
        let state = Outbox::sync_state(&pool, store_id).await.unwrap();
        assert_eq!(state.pending_count, 0);
        assert!(state.last_synced_at.is_some());
    }

    #[tokio::test]
    async fn enqueue_inside_a_transaction_is_atomic() {
        let pool = test_pool().await;
        let store_id = Uuid::now_v7();
        let e = entry(store_id);
        let mut tx = pool.begin().await.unwrap();
        Outbox::enqueue_exec(&mut *tx, &e).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(Outbox::pending_count(&pool, store_id).await.unwrap(), 1);

        // A rolled-back transaction leaves no entry behind.
        let e2 = entry(store_id);
        let mut tx = pool.begin().await.unwrap();
        Outbox::enqueue_exec(&mut *tx, &e2).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(Outbox::pending_count(&pool, store_id).await.unwrap(), 1);
    }
}
