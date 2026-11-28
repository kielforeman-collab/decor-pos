//! Outbox drain, remote/in-memory HQ sync, and sync state reporting.

use chrono::Utc;
use df_sync::{ApplyReport, DeltaBatch, HqSink, Outbox, SyncState};
use uuid::Uuid;

use super::Edge;
use crate::error::EdgeError;

impl Edge {
    // --- sync ---------------------------------------------------------------

    /// Drain pending outbox entries in UUIDv7 order as one batch,
    /// apply them to `hq`, and mark them synced on success. Only the
    /// ids the sink reports in `ApplyReport::applied_ids` are marked
    /// synced: an entry the sink skipped (a conflicting transaction
    /// re-send) or never saw (a partial apply) stays pending, so it
    /// is retried — never silently lost. On failure every entry is
    /// marked failed (retry count bumped) and left pending.
    pub async fn sync_now(&self, hq: &dyn HqSink) -> Result<ApplyReport, EdgeError> {
        let entries = Outbox::pending(&self.pool, self.config.store_id, 1000).await?;
        if entries.is_empty() {
            return Ok(ApplyReport::empty());
        }
        let batch = DeltaBatch::new(self.config.store_id, entries);
        let ids: Vec<Uuid> = batch.entries.iter().map(|e| e.id).collect();
        match hq.apply_batch(&batch).await {
            Ok(report) => {
                // Mark ONLY the ids the sink resolved. Marking the
                // whole batch would silently drop entries a sink
                // skipped or never applied.
                Outbox::mark_synced(&self.pool, &report.applied_ids).await?;
                let pending = Outbox::pending_count(&self.pool, self.config.store_id).await?;
                df_db::upsert_sync_state(
                    &self.pool,
                    &df_db::models::SyncStateRow {
                        store_id: self.config.store_id,
                        last_batch_id: Some(batch.batch_id),
                        last_synced_at: Some(Utc::now()),
                        pending_count: pending as i64,
                    },
                )
                .await?;
                Ok(report)
            }
            Err(err) => {
                let message = err.to_string();
                for id in &ids {
                    let _ = Outbox::mark_failed(&self.pool, *id, &message).await;
                }
                Err(EdgeError::Sync(err))
            }
        }
    }

    /// Sync against the remote HQ when one is configured
    /// (`DF_HQ_URL` / `DF_HQ_TOKEN`, or [`Edge::set_remote_hq`]),
    /// else against the in-process HQ sink configured with
    /// [`Edge::set_hq`]; fail with [`EdgeError::NoHq`] when neither
    /// was configured (the HTTP `/pos/sync/run` endpoint surfaces
    /// this as 503).
    pub async fn sync_now_with_configured_hq(&self) -> Result<ApplyReport, EdgeError> {
        // Clone out of the lock first: holding the std RwLock guard
        // across the await below would make this future !Send
        // (std::sync::RwLockReadGuard is !Send) and axum handlers
        // must be Send.
        let remote = self.remote_hq.read().expect("remote hq lock").clone();
        if let Some(remote) = remote {
            return self.sync_now(&remote).await;
        }
        let hq = self.hq.read().expect("hq lock").clone();
        match hq {
            Some(hq) => self.sync_now(hq.as_ref()).await,
            None => Err(EdgeError::NoHq(
                "no HQ sink configured for this edge (set DF_HQ_URL + DF_HQ_TOKEN, or call Edge::set_remote_hq / Edge::set_hq first)".to_string(),
            )),
        }
    }

    pub async fn sync_state(&self) -> Result<SyncState, EdgeError> {
        let pending_count = Outbox::pending_count(&self.pool, self.config.store_id).await?;
        if let Some(row) = df_db::get_sync_state(&self.pool, self.config.store_id).await? {
            return Ok(SyncState {
                store_id: self.config.store_id,
                last_batch_id: row.last_batch_id,
                last_synced_at: row.last_synced_at,
                pending_count,
            });
        }
        Ok(SyncState {
            store_id: self.config.store_id,
            last_batch_id: None,
            last_synced_at: Outbox::sync_state(&self.pool, self.config.store_id)
                .await?
                .last_synced_at,
            pending_count,
        })
    }

    pub async fn pending_outbox_count(&self) -> Result<u64, EdgeError> {
        Ok(Outbox::pending_count(&self.pool, self.config.store_id).await?)
    }
}
