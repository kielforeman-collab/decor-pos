//! The DB-backed HQ sink: applies a [`DeltaBatch`](crate::outbox::DeltaBatch)
//! to the real HQ tables through the `df-db` repositories, against the
//! HQ [`AnyPool`](sqlx::AnyPool) (PostgreSQL in production, SQLite in
//! tests). This is the live path behind `POST /api/v1/sync/ingest`;
//! [`MemoryHq`](crate::sink::MemoryHq) stays the in-process sink for
//! offline tests and the edge daemon's local sync endpoint.
//!
//! # Idempotency (DB layer)
//!
//! Every entry is applied inside one transaction per batch:
//!
//! 1. The entry id is inserted into `sync_applied_entries` first
//!    (`ON CONFLICT(entry_id) DO NOTHING`). Zero rows affected means
//!    this exact entry was already applied: a replay, counted as a
//!    duplicate, nothing else changes — no duplicate rows, no
//!    double-counted money.
//! 2. The entry's effect is then applied by aggregate type (below),
//!    keyed by `aggregate_id`, so a *different* entry id carrying an
//!    already-stored aggregate is also a no-op, never a double post.
//!
//! # Per-type application
//!
//! * `transaction` — the payload is the transaction summary the edge
//!   enqueues at checkout / return / layaway (see `df-edge`): id,
//!   doc number/type/status, customer, money totals. The sink
//!   rebuilds the `transactions` row (`org_id` and `currency` come
//!   from the batch's store row, which must already exist on HQ;
//!   till session / cashier are not synced, so they stay `NULL`;
//!   `completed_at` is the entry's `created_at` when the status is
//!   `completed`, else `NULL`). A re-send of the same transaction id
//!   with different sync-carried content is a conflict (rule 2 in
//!   [`crate::resolve`]): it is recorded in `sync_conflicts`, NOT in
//!   `applied_ids`, so the edge leaves it pending for an operator —
//!   the stored row is never overwritten. A correction is a new
//!   reversing transaction, never an edit in place.
//! * `inventory_ledger` / `ar_ledger` — append-only ledgers merge by
//!   union on entry id (rule 1). The sink inserts the ledger row
//!   (full row rebuilt from the payload; the edge enriches the
//!   payload with the row's cost/due-date fields — see
//!   `df-edge::inventory::inventory_outbox_entry` and the AR
//!   enqueue sites) unless a row with that id already exists, in
//!   which case the money is already counted and nothing is inserted.
//! * Unknown aggregate types are recorded applied and ignored, like
//!   [`MemoryHq`](crate::sink::MemoryHq): a future HQ version may
//!   interpret them, and they must not be retried forever.
//!
//! A payload missing a required field cannot build its row; it is
//! recorded applied and skipped (same convention as `MemoryHq`),
//! because the money still lands through the ledger entries, which
//! are separate outbox entries. A batch whose store does not exist
//! on HQ fails whole (`SyncError`), so the edge retries after the
//! store is seeded — HQ must hold the store (and org) before the
//! first sync.
//!
//! No floating-point money anywhere: all amounts are
//! `rust_decimal::Decimal`, parsed from the payload's exact decimal
//! strings. No COLORx code.

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::error::SyncError;
use crate::outbox::{DeltaBatch, OutboxEntry};
use crate::sink::{ApplyReport, HqSink};
use df_db::models::{ArLedgerEntry, InventoryLedgerEntry, Transaction};

/// One row of HQ `sync_conflicts` (migration `0013` + resolution columns
/// from migration `0016`). Surfaces skipped transaction conflicts to HQ
/// operators so they can investigate and mark them resolved after
/// issuing a compensating transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncConflictRow {
    pub entry_id: Uuid,
    pub store_id: Uuid,
    pub aggregate_type: String,
    pub aggregate_id: Uuid,
    pub reason: String,
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolution_note: Option<String>,
}

/// DB-backed [`HqSink`]: applies batches to the HQ pool.
#[derive(Debug, Clone)]
pub struct DbHq {
    pool: AnyPool,
}

impl DbHq {
    pub fn new(pool: AnyPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// List HQ `sync_conflicts` rows (newest first), optionally scoped to
    /// one store. When `include_resolved` is `false`, returns only open
    /// unresolved conflicts (`resolved_at IS NULL`).
    pub async fn list_conflicts(
        &self,
        store_id: Option<Uuid>,
        include_resolved: bool,
    ) -> Result<Vec<SyncConflictRow>, SyncError> {
        let rows = match (store_id, include_resolved) {
            (Some(store_id), true) => {
                sqlx::query(
                    "SELECT * FROM sync_conflicts WHERE store_id = $1 ORDER BY created_at DESC",
                )
                .bind(store_id.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(SyncError::Db)?
            }
            (Some(store_id), false) => {
                sqlx::query(
                    "SELECT * FROM sync_conflicts WHERE store_id = $1 AND resolved_at IS NULL ORDER BY created_at DESC",
                )
                .bind(store_id.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(SyncError::Db)?
            }
            (None, true) => {
                sqlx::query("SELECT * FROM sync_conflicts ORDER BY created_at DESC")
                    .fetch_all(&self.pool)
                    .await
                    .map_err(SyncError::Db)?
            }
            (None, false) => {
                sqlx::query(
                    "SELECT * FROM sync_conflicts WHERE resolved_at IS NULL ORDER BY created_at DESC",
                )
                .fetch_all(&self.pool)
                .await
                .map_err(SyncError::Db)?
            }
        };
        rows.iter().map(conflict_from_row).collect()
    }

    /// Mark an HQ `sync_conflicts` row resolved with an operator note
    /// (e.g. referencing the compensating/reversing transaction id).
    /// Returns `Ok(true)` if an open conflict was resolved, `Ok(false)`
    /// if not found or already resolved.
    pub async fn resolve_conflict(
        &self,
        entry_id: Uuid,
        resolution_note: &str,
    ) -> Result<bool, SyncError> {
        let note = resolution_note.trim();
        if note.is_empty() {
            return Err(SyncError::InvalidPayload(
                "resolution_note must not be blank".to_string(),
            ));
        }
        let done = sqlx::query(
            "UPDATE sync_conflicts SET resolved_at = $1, resolution_note = $2 WHERE entry_id = $3 AND resolved_at IS NULL",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(note)
        .bind(entry_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(SyncError::Db)?;
        Ok(done.rows_affected() > 0)
    }

    async fn apply_batch_inner(&self, batch: &DeltaBatch) -> Result<ApplyReport, SyncError> {
        // The store (and its org/currency) must exist on HQ before
        // the first sync; the whole batch fails otherwise and the
        // edge retries after the store is seeded.
        let store = df_db::get_store(&self.pool, batch.store_id)
            .await
            .map_err(|e| match e {
                df_db::DbError::NotFound(_) => SyncError::InvalidPayload(format!(
                    "store {} is not seeded on HQ; seed the store (and org) before the first sync",
                    batch.store_id
                )),
                other => db_err(other),
            })?;
        let mut tx = self.pool.begin().await.map_err(SyncError::Db)?;
        let mut report = ApplyReport::empty();
        for entry in &batch.entries {
            match entry.aggregate_type.as_str() {
                "transaction" => {
                    self.apply_transaction(&mut tx, &store, batch, entry, &mut report)
                        .await?;
                }
                "inventory_ledger" | "ar_ledger" => {
                    self.apply_ledger(&mut tx, &store, batch, entry, &mut report)
                        .await?;
                }
                _ => {
                    // Unknown type: record applied, ignore.
                    if self.mark_applied(&mut tx, batch, entry).await? {
                        report.applied += 1;
                        report.applied_ids.push(entry.id);
                    } else {
                        report.duplicates += 1;
                        report.applied_ids.push(entry.id);
                    }
                }
            }
        }
        tx.commit().await.map_err(SyncError::Db)?;
        Ok(report)
    }

    /// Insert the applied-set marker. Returns `true` when this call
    /// inserted it (first sight of the entry), `false` when the entry
    /// id was already recorded (a replay).
    async fn mark_applied(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        batch: &DeltaBatch,
        entry: &OutboxEntry,
    ) -> Result<bool, SyncError> {
        let done = sqlx::query(
            "INSERT INTO sync_applied_entries (entry_id, store_id, batch_id, aggregate_type, aggregate_id, payload_json, applied_at) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT(entry_id) DO NOTHING",
        )
        .bind(entry.id.to_string())
        .bind(batch.store_id.to_string())
        .bind(batch.batch_id.to_string())
        .bind(&entry.aggregate_type)
        .bind(entry.aggregate_id.to_string())
        .bind(entry.payload.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(SyncError::Db)?;
        Ok(done.rows_affected() > 0)
    }

    async fn record_conflict(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        batch: &DeltaBatch,
        entry: &OutboxEntry,
        reason: &str,
    ) -> Result<(), SyncError> {
        sqlx::query(
            "INSERT INTO sync_conflicts (entry_id, store_id, aggregate_type, aggregate_id, reason, payload_json, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT(entry_id) DO NOTHING",
        )
        .bind(entry.id.to_string())
        .bind(batch.store_id.to_string())
        .bind(&entry.aggregate_type)
        .bind(entry.aggregate_id.to_string())
        .bind(reason)
        .bind(entry.payload.to_string())
        .bind(Utc::now().to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(SyncError::Db)?;
        Ok(())
    }

    async fn apply_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        store: &df_db::models::Store,
        batch: &DeltaBatch,
        entry: &OutboxEntry,
        report: &mut ApplyReport,
    ) -> Result<(), SyncError> {
        let Some(candidate) = transaction_from_payload(entry, store) else {
            // Unparseable payload: record applied, skip (MemoryHq
            // convention — the money still lands via the ledger
            // entries, which are separate outbox entries).
            if self.mark_applied(tx, batch, entry).await? {
                report.applied += 1;
            } else {
                report.duplicates += 1;
            }
            report.applied_ids.push(entry.id);
            return Ok(());
        };
        // Conflict check first: a stored row with different content
        // is skipped and recorded, never overwritten, and never
        // marked applied — the edge leaves it pending.
        let existing = df_db::get_transaction_exec(&mut **tx, entry.aggregate_id)
            .await
            .map_err(db_err)?;
        if let Some(stored) = &existing
            && !transaction_same_content(stored, &candidate, &entry.payload)
        {
            self.record_conflict(
                tx,
                batch,
                entry,
                "transaction re-sent with different content; stored version kept",
            )
            .await?;
            report.conflicts += 1;
            report.conflict_ids.push(entry.id);
            return Ok(());
        }
        if self.mark_applied(tx, batch, entry).await? {
            // Insert only when the row is not already there (a
            // same-content re-send under a new entry id must not
            // violate the PK).
            if existing.is_none() {
                df_db::insert_transaction(&mut **tx, &candidate)
                    .await
                    .map_err(db_err)?;
            }
            report.applied += 1;
        } else {
            report.duplicates += 1;
        }
        report.applied_ids.push(entry.id);
        Ok(())
    }

    async fn apply_ledger(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        store: &df_db::models::Store,
        batch: &DeltaBatch,
        entry: &OutboxEntry,
        report: &mut ApplyReport,
    ) -> Result<(), SyncError> {
        if !self.mark_applied(tx, batch, entry).await? {
            report.duplicates += 1;
            report.applied_ids.push(entry.id);
            return Ok(());
        }
        match entry.aggregate_type.as_str() {
            "inventory_ledger" => {
                if let Some(row) = inventory_from_payload(entry, store)
                    && df_db::get_inventory_entry_exec(&mut **tx, entry.aggregate_id)
                        .await
                        .map_err(db_err)?
                        .is_none()
                {
                    df_db::insert_inventory_delta(&mut **tx, &row)
                        .await
                        .map_err(db_err)?;
                }
            }
            "ar_ledger" => {
                if let Some(row) = ar_from_payload(entry, store)
                    && df_db::get_ar_entry_exec(&mut **tx, entry.aggregate_id)
                        .await
                        .map_err(db_err)?
                        .is_none()
                {
                    df_db::insert_ar_entry_exec(&mut **tx, &row)
                        .await
                        .map_err(db_err)?;
                }
            }
            _ => unreachable!("apply_ledger is only called for ledger types"),
        }
        report.applied += 1;
        report.applied_ids.push(entry.id);
        Ok(())
    }
}

impl HqSink for DbHq {
    fn apply_batch<'a>(
        &'a self,
        batch: &'a DeltaBatch,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ApplyReport, SyncError>> + Send + 'a>,
    > {
        Box::pin(async move { self.apply_batch_inner(batch).await })
    }
}

fn db_err(e: df_db::DbError) -> SyncError {
    match e {
        df_db::DbError::Sqlx(inner) => SyncError::Db(inner),
        other => SyncError::InvalidPayload(other.to_string()),
    }
}

fn payload_uuid(payload: &serde_json::Value, key: &str) -> Option<Uuid> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

fn payload_opt_uuid(payload: &serde_json::Value, key: &str) -> Option<Uuid> {
    match payload.get(key) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Uuid::parse_str(s).ok(),
        _ => None,
    }
}

fn payload_decimal(payload: &serde_json::Value, key: &str) -> Option<Decimal> {
    match payload.get(key) {
        Some(serde_json::Value::String(s)) => Decimal::from_str(s).ok(),
        Some(serde_json::Value::Number(n)) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    }
}

fn payload_str<'a>(payload: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(|v| v.as_str())
}

fn payload_opt_date(payload: &serde_json::Value, key: &str) -> Option<NaiveDate> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
}

/// Payload timestamps are RFC 3339 text (the edge writes
/// `Utc::now().to_rfc3339()`); both dialects store them as TEXT.
fn payload_opt_dt(payload: &serde_json::Value, key: &str) -> Option<DateTime<Utc>> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

/// Rebuild the `transactions` row from a `transaction` outbox
/// payload. `None` when a required field is missing or malformed —
/// the caller records the entry applied and skips it (MemoryHq
/// convention). Fields the edge does not sync (till session,
/// cashier, salesperson) stay `NULL`; `org_id`/`currency` come from
/// the batch's store row; `completed_at` is the entry's creation
/// time for completed transactions, `NULL` otherwise (an open
/// layaway has no completion time).
fn transaction_from_payload(
    entry: &OutboxEntry,
    store: &df_db::models::Store,
) -> Option<Transaction> {
    let payload = &entry.payload;
    let status = payload_str(payload, "status")?.to_string();
    Some(Transaction {
        id: entry.aggregate_id,
        org_id: store.org_id,
        store_id: store.id,
        till_session_id: None,
        doc_number: payload_str(payload, "doc_number")?.to_string(),
        doc_type: payload_str(payload, "doc_type")?.to_string(),
        status: status.clone(),
        customer_id: payload_opt_uuid(payload, "customer_id"),
        job_id: payload_opt_uuid(payload, "job_id"),
        po_number: payload_str(payload, "po_number").map(str::to_string),
        job_name: payload_str(payload, "job_name").map(str::to_string),
        salesperson_id: None,
        cashier_id: None,
        currency: store.currency.clone(),
        subtotal: payload_decimal(payload, "subtotal")?,
        tax_total: payload_decimal(payload, "tax_total")?,
        eco_fee_total: payload_decimal(payload, "eco_fee_total")?,
        pos_fee_total: payload_decimal(payload, "pos_fee_total")?,
        grand_total: payload_decimal(payload, "grand_total")?,
        deposit_paid: payload_decimal(payload, "deposit_paid").unwrap_or(Decimal::ZERO),
        fulfillment_mode: "counter".to_string(),
        scheduled_at: None,
        completed_at: if status == "completed" {
            Some(entry.created_at)
        } else {
            None
        },
        created_at: entry.created_at,
    })
}

/// Rule 2, DB edition: the stored row and the incoming payload agree
/// on every sync-carried field. Optional fields (`job_id`,
/// `po_number`, `deposit_paid`) are compared only when the incoming
/// payload carries them, so an older edge payload that omits them
/// never false-conflicts against a row a newer edge wrote.
fn transaction_same_content(
    stored: &Transaction,
    candidate: &Transaction,
    payload: &serde_json::Value,
) -> bool {
    if stored.doc_number != candidate.doc_number
        || stored.doc_type != candidate.doc_type
        || stored.status != candidate.status
        || stored.customer_id != candidate.customer_id
        || stored.subtotal != candidate.subtotal
        || stored.tax_total != candidate.tax_total
        || stored.eco_fee_total != candidate.eco_fee_total
        || stored.pos_fee_total != candidate.pos_fee_total
        || stored.grand_total != candidate.grand_total
    {
        return false;
    }
    if payload.get("job_id").is_some_and(|v| !v.is_null()) && stored.job_id != candidate.job_id {
        return false;
    }
    if payload.get("po_number").is_some_and(|v| !v.is_null())
        && stored.po_number != candidate.po_number
    {
        return false;
    }
    if payload.get("deposit_paid").is_some() && stored.deposit_paid != candidate.deposit_paid {
        return false;
    }
    true
}

/// Rebuild the `inventory_ledger` row from an `inventory_ledger`
/// payload. `None` when a required field is missing or malformed.
/// The edge enriches the payload with the row's cost/delta/lot/
/// created-at fields; payloads from an older edge that omit them
/// apply with zero deltas (other than `delta_on_hand`), zero cost
/// and no lot — the on-hand balance, which is what HQ derives, is
/// still exact.
fn inventory_from_payload(
    entry: &OutboxEntry,
    store: &df_db::models::Store,
) -> Option<InventoryLedgerEntry> {
    let payload = &entry.payload;
    Some(InventoryLedgerEntry {
        id: entry.aggregate_id,
        org_id: store.org_id,
        store_id: store.id,
        product_id: payload_uuid(payload, "product_id")?,
        lot_number: payload_str(payload, "lot_number").map(str::to_string),
        delta_on_hand: payload_decimal(payload, "delta_on_hand")?,
        delta_committed: payload_decimal(payload, "delta_committed").unwrap_or(Decimal::ZERO),
        delta_on_order: payload_decimal(payload, "delta_on_order").unwrap_or(Decimal::ZERO),
        delta_in_transit: payload_decimal(payload, "delta_in_transit").unwrap_or(Decimal::ZERO),
        unit_cost: payload_decimal(payload, "unit_cost").unwrap_or(Decimal::ZERO),
        reason_type: payload_str(payload, "reason_type")?.to_string(),
        reference_id: payload_uuid(payload, "reference_id")?,
        created_at: payload_opt_dt(payload, "created_at").unwrap_or(entry.created_at),
    })
}

/// Rebuild the `ar_ledger` row from an `ar_ledger` payload. `None`
/// when a required field is missing or malformed. The edge enriches
/// the payload with `reference_type`, `due_date`, `job_id` and
/// `created_at`; payloads from an older edge that omit them apply
/// with the pre-enrichment defaults (`reference_type` falls back to
/// `"transaction"`, no due date) — the balance, which is what HQ
/// derives, is still exact.
fn ar_from_payload(entry: &OutboxEntry, store: &df_db::models::Store) -> Option<ArLedgerEntry> {
    let payload = &entry.payload;
    Some(ArLedgerEntry {
        id: entry.aggregate_id,
        org_id: store.org_id,
        store_id: store.id,
        customer_id: payload_uuid(payload, "customer_id")?,
        job_id: payload_opt_uuid(payload, "job_id"),
        entry_type: payload_str(payload, "entry_type")?.to_string(),
        reference_type: payload_str(payload, "reference_type")
            .unwrap_or("transaction")
            .to_string(),
        reference_id: payload_uuid(payload, "reference_id")?,
        amount: payload_decimal(payload, "amount")?,
        due_date: payload_opt_date(payload, "due_date"),
        statement_id: None,
        created_at: payload_opt_dt(payload, "created_at").unwrap_or(entry.created_at),
    })
}

fn conflict_from_row(row: &sqlx::any::AnyRow) -> Result<SyncConflictRow, SyncError> {
    let parse_uuid = |col: &str| -> Result<Uuid, SyncError> {
        let text: String = row
            .try_get(col)
            .map_err(|e| SyncError::InvalidPayload(format!("column {col}: {e}")))?;
        Uuid::parse_str(&text)
            .map_err(|e| SyncError::InvalidPayload(format!("column {col} is not a UUID: {e}")))
    };
    let parse_dt = |col: &str| -> Result<DateTime<Utc>, SyncError> {
        let text: String = row
            .try_get(col)
            .map_err(|e| SyncError::InvalidPayload(format!("column {col}: {e}")))?;
        DateTime::parse_from_rfc3339(&text)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| SyncError::InvalidPayload(format!("column {col}: {e}")))
    };
    let parse_opt_dt = |col: &str| -> Result<Option<DateTime<Utc>>, SyncError> {
        let text: Option<String> = row
            .try_get(col)
            .map_err(|e| SyncError::InvalidPayload(format!("column {col}: {e}")))?;
        match text {
            None => Ok(None),
            Some(text) => DateTime::parse_from_rfc3339(&text)
                .map(|dt| Some(dt.with_timezone(&Utc)))
                .map_err(|e| SyncError::InvalidPayload(format!("column {col}: {e}"))),
        }
    };
    let payload_text: String = row
        .try_get("payload_json")
        .map_err(|e| SyncError::InvalidPayload(format!("column payload_json: {e}")))?;
    let payload: serde_json::Value = serde_json::from_str(&payload_text)?;
    Ok(SyncConflictRow {
        entry_id: parse_uuid("entry_id")?,
        store_id: parse_uuid("store_id")?,
        aggregate_type: row
            .try_get("aggregate_type")
            .map_err(|e| SyncError::InvalidPayload(format!("column aggregate_type: {e}")))?,
        aggregate_id: parse_uuid("aggregate_id")?,
        reason: row
            .try_get("reason")
            .map_err(|e| SyncError::InvalidPayload(format!("column reason: {e}")))?,
        payload,
        created_at: parse_dt("created_at")?,
        resolved_at: parse_opt_dt("resolved_at")?,
        resolution_note: row
            .try_get("resolution_note")
            .map_err(|e| SyncError::InvalidPayload(format!("column resolution_note: {e}")))?,
    })
}
