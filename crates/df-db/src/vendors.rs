//! Vendor repositories (`vendors`, migration `0015_vendors`).
//!
//! Write functions are generic over any sqlx executor so the edge
//! can run them inside one local SQLite transaction (`&mut *tx`).
//! `name` is the vendor-id string used on purchase orders (e.g.
//! `"Benjamin Moore"`); it is unique per org and is never edited
//! after creation — it is the PO key, not a display label.

use sqlx::{AnyPool, Executor};
use uuid::Uuid;

use crate::codec::*;
use crate::error::DbError;
use crate::models::{PurchaseOrderRow, Vendor};

fn vendor_from_row(row: &sqlx::any::AnyRow) -> Result<Vendor, DbError> {
    Ok(Vendor {
        id: uuid_col(row, "id")?,
        org_id: uuid_col(row, "org_id")?,
        name: text_col(row, "name")?,
        display_name: opt_text_col(row, "display_name")?,
        contact_name: opt_text_col(row, "contact_name")?,
        email: opt_text_col(row, "email")?,
        phone: opt_text_col(row, "phone")?,
        edi_enabled: bool_col(row, "edi_enabled")?,
        created_at: dt_col(row, "created_at")?,
    })
}

/// Vendors for an org, by name.
pub async fn list_vendors_for_org(pool: &AnyPool, org_id: Uuid) -> Result<Vec<Vendor>, DbError> {
    let rows = sqlx::query("SELECT * FROM vendors WHERE org_id = $1 ORDER BY name ASC")
        .bind(uuid_str(org_id))
        .fetch_all(pool)
        .await?;
    rows.iter().map(vendor_from_row).collect()
}

/// One vendor, org-scoped: a vendor from another org reads as not
/// found, never as a cross-org row.
pub async fn get_vendor(pool: &AnyPool, org_id: Uuid, id: Uuid) -> Result<Vendor, DbError> {
    let row = sqlx::query("SELECT * FROM vendors WHERE org_id = $1 AND id = $2")
        .bind(uuid_str(org_id))
        .bind(uuid_str(id))
        .fetch_one(pool)
        .await
        .map_err(not_found(&format!("vendor {id}")))?;
    vendor_from_row(&row)
}

/// One vendor by its PO key (`name`), org-scoped. `None` when the
/// org has no vendor under that name yet.
pub async fn get_vendor_by_name(
    pool: &AnyPool,
    org_id: Uuid,
    name: &str,
) -> Result<Option<Vendor>, DbError> {
    let row = sqlx::query("SELECT * FROM vendors WHERE org_id = $1 AND name = $2")
        .bind(uuid_str(org_id))
        .bind(name)
        .fetch_optional(pool)
        .await?;
    match row {
        None => Ok(None),
        Some(row) => Ok(Some(vendor_from_row(&row)?)),
    }
}

pub async fn insert_vendor<'e, E>(executor: E, vendor: &Vendor) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO vendors (id, org_id, name, display_name, contact_name, email, phone, edi_enabled, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(uuid_str(vendor.id))
    .bind(uuid_str(vendor.org_id))
    .bind(&vendor.name)
    .bind(&vendor.display_name)
    .bind(&vendor.contact_name)
    .bind(&vendor.email)
    .bind(&vendor.phone)
    .bind(vendor.edi_enabled)
    .bind(dt_str(vendor.created_at))
    .execute(executor)
    .await?;
    Ok(())
}

/// Replace a vendor row wholesale (the edge merges the requested
/// partial update onto the stored row first). `name` is part of the
/// row but the edge never changes it — it is the PO key.
pub async fn update_vendor<'e, E>(executor: E, vendor: &Vendor) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "UPDATE vendors SET display_name = $1, contact_name = $2, email = $3, phone = $4, edi_enabled = $5 WHERE id = $6",
    )
    .bind(&vendor.display_name)
    .bind(&vendor.contact_name)
    .bind(&vendor.email)
    .bind(&vendor.phone)
    .bind(vendor.edi_enabled)
    .bind(uuid_str(vendor.id))
    .execute(executor)
    .await?;
    Ok(())
}

/// Ensure a vendor row exists for a PO's vendor-id string: the
/// backfill in migration `0015_vendors` covers POs that predate the
/// table, and this covers POs created after it, so every PO vendor
/// always has a row on the Vendors screen. A no-op when the row
/// already exists; the backfilled/entered contact fields are never
/// overwritten. New rows carry no contact details (`NULL`).
pub async fn ensure_vendor_for_po<'e, E>(
    executor: E,
    org_id: Uuid,
    vendor_id: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError>
where
    E: Executor<'e, Database = sqlx::Any>,
{
    sqlx::query(
        "INSERT INTO vendors (id, org_id, name, display_name, edi_enabled, created_at) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (org_id, name) DO NOTHING",
    )
    .bind(uuid_str(Uuid::now_v7()))
    .bind(uuid_str(org_id))
    .bind(vendor_id)
    .bind(vendor_id)
    .bind(false)
    .bind(dt_str(created_at))
    .execute(executor)
    .await?;
    Ok(())
}

/// POs carrying one vendor-id string, newest first. The caller
/// filters to its own org's stores: `purchase_orders` has no
/// `org_id` column, and two orgs can use the same vendor name.
pub async fn list_purchase_orders_for_vendor(
    pool: &AnyPool,
    vendor_id: &str,
) -> Result<Vec<PurchaseOrderRow>, DbError> {
    let rows =
        sqlx::query("SELECT * FROM purchase_orders WHERE vendor_id = $1 ORDER BY created_at DESC")
            .bind(vendor_id)
            .fetch_all(pool)
            .await?;
    rows.iter()
        .map(|row| crate::purchase_orders::po_from_row(row))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    async fn test_pool() -> AnyPool {
        let pool = crate::connect("sqlite::memory:").await.unwrap();
        crate::migrate(&pool, crate::Dialect::Sqlite).await.unwrap();
        pool
    }

    fn vendor(org_id: Uuid, name: &str) -> Vendor {
        Vendor {
            id: Uuid::now_v7(),
            org_id,
            name: name.to_string(),
            display_name: Some(name.to_string()),
            contact_name: None,
            email: None,
            phone: None,
            edi_enabled: false,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn vendors_crud_is_org_scoped() {
        let pool = test_pool().await;
        let org_id = Uuid::from_u128(0x2000);
        let other_org = Uuid::from_u128(0x2001);

        assert!(
            list_vendors_for_org(&pool, org_id)
                .await
                .unwrap()
                .is_empty()
        );

        let mut created = vendor(org_id, "Benjamin Moore");
        insert_vendor(&pool, &created).await.unwrap();
        let listed = list_vendors_for_org(&pool, org_id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Benjamin Moore");
        assert!(
            list_vendors_for_org(&pool, other_org)
                .await
                .unwrap()
                .is_empty()
        );

        // A vendor from another org reads as not found.
        assert!(get_vendor(&pool, other_org, created.id).await.is_err());
        let fetched = get_vendor(&pool, org_id, created.id).await.unwrap();
        assert_eq!(fetched.display_name.as_deref(), Some("Benjamin Moore"));
        assert!(
            get_vendor_by_name(&pool, org_id, "Benjamin Moore")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            get_vendor_by_name(&pool, org_id, "No Such Vendor")
                .await
                .unwrap()
                .is_none()
        );

        // Update merges through the stored row; `name` is untouched.
        created.display_name = Some("BM".to_string());
        created.contact_name = Some("Pat".to_string());
        created.edi_enabled = true;
        update_vendor(&pool, &created).await.unwrap();
        let updated = get_vendor(&pool, org_id, created.id).await.unwrap();
        assert_eq!(updated.name, "Benjamin Moore");
        assert_eq!(updated.display_name.as_deref(), Some("BM"));
        assert_eq!(updated.contact_name.as_deref(), Some("Pat"));
        assert!(updated.edi_enabled);
    }

    #[tokio::test]
    async fn ensure_vendor_for_po_is_idempotent() {
        let pool = test_pool().await;
        let org_id = Uuid::from_u128(0x2000);
        ensure_vendor_for_po(&pool, org_id, "Benjamin Moore", Utc::now())
            .await
            .unwrap();
        ensure_vendor_for_po(&pool, org_id, "Benjamin Moore", Utc::now())
            .await
            .unwrap();
        let listed = list_vendors_for_org(&pool, org_id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].display_name.as_deref(), Some("Benjamin Moore"));
        assert!(listed[0].contact_name.is_none());
    }
}
