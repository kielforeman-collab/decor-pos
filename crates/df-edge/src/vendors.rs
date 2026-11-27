//! Vendor master data (Products / Vendors / Dashboards screens).
//!
//! The Rust API the HTTP endpoints and the Tauri commands call —
//! no HTTP here. Every method delegates to `df-db` (the single
//! implementation); this module is wiring only (org scoping +
//! PO statistics over [`df_db`] rows).
//!
//! * [`Edge::list_vendors`] — vendors for this org, each with its
//!   PO counts from the real `purchase_orders` table.
//! * [`Edge::get_vendor`] — one vendor + its POs, newest first.
//! * [`Edge::create_vendor`] / [`Edge::update_vendor`] — create /
//!   partial update. `name` is the PO key and is never editable:
//!   it is set at creation and a duplicate name in the org is
//!   rejected, so a PO can always resolve its vendor row.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use df_db::models::Vendor;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;
use crate::inventory::PurchaseOrderView;

/// One vendor, with its PO statistics from the real
/// `purchase_orders` table. `po_count` counts every PO carrying
/// this vendor's `name`; `open_po_count` counts the ones that are
/// neither `received` nor `cancelled`; `last_po_at` is the newest
/// PO's `created_at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VendorView {
    pub id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub contact_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub edi_enabled: bool,
    pub po_count: u32,
    pub open_po_count: u32,
    pub last_po_at: Option<DateTime<Utc>>,
}

/// One vendor + its POs, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VendorDetail {
    pub vendor: VendorView,
    pub purchase_orders: Vec<PurchaseOrderView>,
}

/// Create a vendor (`POST /vendors`). `name` is required — it is
/// the vendor-id string POs carry. Everything else is optional;
/// omitted contact fields stay `NULL` (nothing is invented).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateVendorRequest {
    pub name: String,
    pub display_name: Option<String>,
    pub contact_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub edi_enabled: Option<bool>,
}

/// Partial update (`POST /vendors/{id}`). `None` means "leave the
/// stored value alone" — there is no way to clear a field to `NULL`
/// through this endpoint. `name` is not present at all: it is the
/// PO key and is never editable.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct UpdateVendorRequest {
    pub display_name: Option<String>,
    pub contact_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub edi_enabled: Option<bool>,
}

/// A PO is open while it is neither received nor cancelled.
fn is_open_po(status: &str) -> bool {
    status != "received" && status != "cancelled"
}

impl Edge {
    /// Store ids belonging to this edge's org, for scoping POs:
    /// `purchase_orders` has no `org_id` column, and two orgs can
    /// use the same vendor name.
    async fn org_store_ids(&self) -> Result<HashSet<Uuid>, EdgeError> {
        let stores = df_db::list_stores_for_org(self.pool(), self.config().org_id).await?;
        Ok(stores.into_iter().map(|store| store.id).collect())
    }

    /// The PO statistics for one vendor, from this org's POs only.
    fn vendor_view(
        &self,
        vendor: &Vendor,
        pos: &[df_db::models::PurchaseOrderRow],
        org_store_ids: &HashSet<Uuid>,
    ) -> VendorView {
        let mut po_count = 0u32;
        let mut open_po_count = 0u32;
        let mut last_po_at: Option<DateTime<Utc>> = None;
        for po in pos {
            if !org_store_ids.contains(&po.store_id) {
                continue;
            }
            po_count += 1;
            if is_open_po(&po.status) {
                open_po_count += 1;
            }
            if last_po_at.is_none_or(|at| po.created_at > at) {
                last_po_at = Some(po.created_at);
            }
        }
        VendorView {
            id: vendor.id,
            name: vendor.name.clone(),
            display_name: vendor.display_name.clone(),
            contact_name: vendor.contact_name.clone(),
            email: vendor.email.clone(),
            phone: vendor.phone.clone(),
            edi_enabled: vendor.edi_enabled,
            po_count,
            open_po_count,
            last_po_at,
        }
    }

    /// Vendors for this org, by name, each with its PO statistics.
    pub async fn list_vendors(&self) -> Result<Vec<VendorView>, EdgeError> {
        let org_store_ids = self.org_store_ids().await?;
        let vendors = df_db::list_vendors_for_org(self.pool(), self.config().org_id).await?;
        let mut views = Vec::with_capacity(vendors.len());
        for vendor in &vendors {
            let pos = df_db::list_purchase_orders_for_vendor(self.pool(), &vendor.name).await?;
            views.push(self.vendor_view(vendor, &pos, &org_store_ids));
        }
        Ok(views)
    }

    /// One vendor + its POs (this org's stores only), newest first.
    pub async fn get_vendor(&self, id: Uuid) -> Result<VendorDetail, EdgeError> {
        let vendor = df_db::get_vendor(self.pool(), self.config().org_id, id).await?;
        let org_store_ids = self.org_store_ids().await?;
        let pos = df_db::list_purchase_orders_for_vendor(self.pool(), &vendor.name).await?;
        let view = self.vendor_view(&vendor, &pos, &org_store_ids);
        let mut purchase_orders = Vec::new();
        for po in pos {
            if !org_store_ids.contains(&po.store_id) {
                continue;
            }
            purchase_orders.push(self.purchase_order_view(po.id).await?);
        }
        Ok(VendorDetail {
            vendor: view,
            purchase_orders,
        })
    }

    /// Create a vendor. The name is trimmed and must not be empty;
    /// a vendor with the same name already in this org is rejected
    /// (the `UNIQUE (org_id, name)` key is the backstop, not the
    /// user-facing error).
    pub async fn create_vendor(
        &self,
        request: CreateVendorRequest,
    ) -> Result<VendorView, EdgeError> {
        let name = request.name.trim().to_string();
        if name.is_empty() {
            return Err(EdgeError::InvalidInput(
                "vendor name is required".to_string(),
            ));
        }
        if df_db::get_vendor_by_name(self.pool(), self.config().org_id, &name)
            .await?
            .is_some()
        {
            return Err(EdgeError::InvalidInput(format!(
                "vendor {name} already exists"
            )));
        }
        let vendor = Vendor {
            id: Uuid::now_v7(),
            org_id: self.config().org_id,
            name,
            display_name: request.display_name,
            contact_name: request.contact_name,
            email: request.email,
            phone: request.phone,
            edi_enabled: request.edi_enabled.unwrap_or(false),
            created_at: Utc::now(),
        };
        df_db::insert_vendor(self.pool(), &vendor).await?;
        let org_store_ids = self.org_store_ids().await?;
        Ok(self.vendor_view(&vendor, &[], &org_store_ids))
    }

    /// Partial update of a vendor. `name` cannot change — it is the
    /// PO key. An unknown id (or one from another org) is a 404.
    pub async fn update_vendor(
        &self,
        id: Uuid,
        request: UpdateVendorRequest,
    ) -> Result<VendorView, EdgeError> {
        let mut vendor = df_db::get_vendor(self.pool(), self.config().org_id, id).await?;
        if let Some(display_name) = request.display_name {
            vendor.display_name = Some(display_name);
        }
        if let Some(contact_name) = request.contact_name {
            vendor.contact_name = Some(contact_name);
        }
        if let Some(email) = request.email {
            vendor.email = Some(email);
        }
        if let Some(phone) = request.phone {
            vendor.phone = Some(phone);
        }
        if let Some(edi_enabled) = request.edi_enabled {
            vendor.edi_enabled = edi_enabled;
        }
        df_db::update_vendor(self.pool(), &vendor).await?;
        let org_store_ids = self.org_store_ids().await?;
        let pos = df_db::list_purchase_orders_for_vendor(self.pool(), &vendor.name).await?;
        Ok(self.vendor_view(&vendor, &pos, &org_store_ids))
    }
}
