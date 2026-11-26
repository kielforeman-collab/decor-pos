//! Load ledger entries from the database for export (Phase 3, v0.4).
//!
//! Inventory entries come from `df-db`'s own repo
//! ([`df_db::list_inventory_entries_for_store`]). AR entries have no
//! store-scoped repo in `df-db` yet (only per-customer), so
//! [`load_ar_entries_for_store`] queries `ar_ledger` directly and
//! parses the rows itself — the same text encoding `df-db` uses
//! (UUIDs / decimals / timestamps as text). If `df-db` gains a
//! store-scoped AR list, switch this loader to it and delete the
//! local parsing.

use std::str::FromStr;

use chrono::{DateTime, NaiveDate, Utc};
use df_db::models::{ArLedgerEntry, InventoryLedgerEntry};
use rust_decimal::Decimal;
use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::error::AccountingError;

/// All AR ledger entries for a store, oldest first.
pub async fn load_ar_entries_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<ArLedgerEntry>, AccountingError> {
    let rows =
        sqlx::query("SELECT * FROM ar_ledger WHERE store_id = $1 ORDER BY created_at ASC, id ASC")
            .bind(store_id.to_string())
            .fetch_all(pool)
            .await?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        entries.push(ar_entry_from_row(&row)?);
    }
    Ok(entries)
}

/// All inventory ledger entries for a store, oldest first.
pub async fn load_inventory_entries_for_store(
    pool: &AnyPool,
    store_id: Uuid,
) -> Result<Vec<InventoryLedgerEntry>, AccountingError> {
    Ok(df_db::list_inventory_entries_for_store(pool, store_id).await?)
}

fn ar_entry_from_row(row: &sqlx::any::AnyRow) -> Result<ArLedgerEntry, AccountingError> {
    let text = |name: &str| -> Result<String, AccountingError> {
        row.try_get::<String, _>(name)
            .map_err(|e| AccountingError::InvalidInput(format!("ar_ledger.{name}: {e}")))
    };
    let opt_text = |name: &str| -> Result<Option<String>, AccountingError> {
        row.try_get::<Option<String>, _>(name)
            .map_err(|e| AccountingError::InvalidInput(format!("ar_ledger.{name}: {e}")))
    };
    let uuid = |name: &str| -> Result<Uuid, AccountingError> {
        Uuid::from_str(&text(name)?)
            .map_err(|_| AccountingError::InvalidInput(format!("ar_ledger.{name} is not a UUID")))
    };
    let opt_uuid = |name: &str| -> Result<Option<Uuid>, AccountingError> {
        match opt_text(name)? {
            Some(value) => Ok(Some(Uuid::from_str(&value).map_err(|_| {
                AccountingError::InvalidInput(format!("ar_ledger.{name} is not a UUID: {value}"))
            })?)),
            None => Ok(None),
        }
    };
    Ok(ArLedgerEntry {
        id: uuid("id")?,
        org_id: uuid("org_id")?,
        store_id: uuid("store_id")?,
        customer_id: uuid("customer_id")?,
        job_id: opt_uuid("job_id")?,
        entry_type: text("entry_type")?,
        reference_type: text("reference_type")?,
        reference_id: uuid("reference_id")?,
        amount: Decimal::from_str(&text("amount")?).map_err(|_| {
            AccountingError::InvalidInput("ar_ledger.amount is not a decimal".to_string())
        })?,
        due_date: match opt_text("due_date")? {
            Some(value) => Some(NaiveDate::from_str(&value).map_err(|_| {
                AccountingError::InvalidInput(format!("ar_ledger.due_date is not a date: {value}"))
            })?),
            None => None,
        },
        statement_id: opt_uuid("statement_id")?,
        created_at: DateTime::parse_from_rfc3339(&text("created_at")?)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| {
                AccountingError::InvalidInput(
                    "ar_ledger.created_at is not an RFC 3339 timestamp".to_string(),
                )
            })?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_db::Dialect;
    use df_db::models::Organization;

    #[tokio::test]
    async fn load_ar_entries_for_store_reads_the_ledger() {
        let pool = df_db::connect("sqlite::memory:").await.unwrap();
        df_db::migrate(&pool, Dialect::Sqlite).await.unwrap();
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
        df_db::insert_organization(
            &pool,
            &Organization {
                id: org_id,
                name: "Test Paint Co".to_string(),
                default_currency: "USD".to_string(),
                enabled_modules: serde_json::json!({}),
                settings_json: serde_json::json!({}),
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        let entry = ArLedgerEntry {
            id: Uuid::from_u128(0x1),
            org_id,
            store_id,
            customer_id: Uuid::from_u128(0x4000),
            job_id: None,
            entry_type: "charge".to_string(),
            reference_type: "transaction".to_string(),
            reference_id: Uuid::from_u128(0x5000),
            amount: Decimal::from_str("1200.0000").unwrap(),
            due_date: Some(NaiveDate::from_str("2026-12-05").unwrap()),
            statement_id: None,
            created_at: DateTime::parse_from_rfc3339("2026-11-05T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        df_db::insert_ar_entry(&pool, &entry).await.unwrap();
        let loaded = load_ar_entries_for_store(&pool, store_id).await.unwrap();
        assert_eq!(loaded, vec![entry]);
        // Another store sees nothing.
        assert!(
            load_ar_entries_for_store(&pool, Uuid::from_u128(0x1001))
                .await
                .unwrap()
                .is_empty()
        );
    }
}
