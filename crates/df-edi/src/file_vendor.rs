//! File-drop `EdiVendorPlugin` for testing (Phase 4, v0.5).
//!
//! [`FileEdiVendorPlugin`] implements the spec §6.2
//! [`EdiVendorPlugin`](df_plugin_sdk::EdiVendorPlugin) trait
//! against a [`LocalSftpStandIn`] drop folder instead of a live
//! vendor. It exists so a vendor pack's EDI flow can be exercised
//! end to end — generate an 850, drop a vendor 855/856/810/832
//! into the inbox, poll it back — with no vendor, no network,
//! and no credentials.
//!
//! ## File-name contract
//!
//! * Outbound 850s are written as `850-<po number>.x12`.
//! * Inbound files are picked up by prefix: `855-` (acks),
//!   `856-` (ASNs), `810-` (invoices), `832-` (catalogs). A polled
//!   file is archived to `processed/` so the next poll does not
//!   return it twice.
//! * Rebate claims are written as `rebate-<claim number>.json`
//!   and acknowledged with status `filed_locally` — that status
//!   means "staged in the outbox", **not** "the vendor accepted
//!   the claim". No vendor confirmation exists until a file comes
//!   back in the inbox.
//!
//! This is a test implementation. It never claims a live vendor
//! connection, and it must not be presented as one.

use df_plugin_sdk::{EdiVendorPlugin, RebateClaim, RebateClaimAck, Result as PluginResult};

use crate::error::EdiError;
use crate::purchase_order::parse_850;
use crate::transport::{LocalSftpStandIn, VendorTransport};

fn map_error(error: EdiError) -> df_plugin_sdk::PluginError {
    match error {
        EdiError::Io(io) => df_plugin_sdk::PluginError::Io(io),
        other => df_plugin_sdk::PluginError::InvalidResponse(other.to_string()),
    }
}

/// `EdiVendorPlugin` backed by a local drop folder.
#[derive(Debug, Clone)]
pub struct FileEdiVendorPlugin {
    transport: LocalSftpStandIn,
    vendor_id: String,
}

impl FileEdiVendorPlugin {
    /// Create the plugin for a vendor id and a drop-folder root.
    pub fn new(vendor_id: &str, drop_root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            transport: LocalSftpStandIn::new(drop_root),
            vendor_id: vendor_id.to_string(),
        }
    }

    /// The vendor this plugin pretends to be.
    pub fn vendor_id(&self) -> &str {
        &self.vendor_id
    }

    /// The underlying drop-folder transport.
    pub fn transport(&self) -> &LocalSftpStandIn {
        &self.transport
    }

    fn poll_prefix(&self, prefix: &str) -> PluginResult<Vec<String>> {
        let mut documents = Vec::new();
        for file_name in self.transport.list_inbox().map_err(map_error)? {
            if !file_name.starts_with(prefix) {
                continue;
            }
            let text = self.transport.receive(&file_name).map_err(map_error)?;
            self.transport
                .archive_inbox(&file_name)
                .map_err(map_error)?;
            documents.push(text);
        }
        Ok(documents)
    }
}

impl EdiVendorPlugin for FileEdiVendorPlugin {
    async fn send_po(&self, x12_850: &str) -> PluginResult<String> {
        let po_number = parse_850(x12_850)
            .map(|po| po.po_number)
            .unwrap_or_else(|_| uuid::Uuid::now_v7().to_string());
        let file_name = format!("850-{po_number}.x12");
        self.transport
            .send(&file_name, x12_850)
            .map_err(map_error)?;
        Ok(file_name)
    }

    async fn poll_acknowledgments(&self) -> PluginResult<Vec<String>> {
        self.poll_prefix("855-")
    }

    async fn poll_asns(&self) -> PluginResult<Vec<String>> {
        self.poll_prefix("856-")
    }

    async fn poll_invoices(&self) -> PluginResult<Vec<String>> {
        self.poll_prefix("810-")
    }

    async fn fetch_price_catalog(&self) -> PluginResult<Option<String>> {
        let documents = self.poll_prefix("832-")?;
        Ok(documents.into_iter().next())
    }

    async fn submit_rebate_claim(&self, claim: &RebateClaim) -> PluginResult<RebateClaimAck> {
        let file_name = format!("rebate-{}.json", claim.claim_number);
        let contents =
            serde_json::to_string_pretty(claim).map_err(df_plugin_sdk::PluginError::Json)?;
        self.transport
            .send(&file_name, &contents)
            .map_err(map_error)?;
        Ok(RebateClaimAck {
            claim_id: claim.claim_id.clone(),
            status: "filed_locally".to_string(),
            vendor_reference: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::purchase_order::{
        LowStockItem, generate_purchase_orders, purchase_order_to_x12_850,
    };
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use std::str::FromStr as _;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("df-edi-file-vendor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn file_vendor_send_poll_round_trip() {
        let root = temp_root("roundtrip");
        let plugin = FileEdiVendorPlugin::new("VENDOR-A", &root);
        let date = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
        let items = vec![LowStockItem {
            product_id: uuid::Uuid::now_v7(),
            vendor_id: "VENDOR-A".to_string(),
            vendor_sku: "SKU-A".to_string(),
            description: "Interior Eggshell Gallon".to_string(),
            on_hand: Decimal::from_str("2").unwrap(),
            min_level: Decimal::from_str("10").unwrap(),
            reorder_qty: Decimal::from_str("12").unwrap(),
            unit_cost: Decimal::from_str("24.5000").unwrap(),
            uom: "GAL".to_string(),
        }];
        let po = generate_purchase_orders(&items, uuid::Uuid::now_v7(), date)
            .into_iter()
            .next()
            .unwrap();
        let x12 = purchase_order_to_x12_850(&po, "DECORPOS", "VENDOR-A", 1);
        let file_name = plugin.send_po(&x12).await.unwrap();
        assert_eq!(file_name, "850-PO-20261125-0001.x12");
        assert!(root.join("outbox").join(&file_name).exists());

        // Vendor drops an ack; poll returns it once, then nothing.
        std::fs::create_dir_all(root.join("inbox")).unwrap();
        std::fs::write(
            root.join("inbox").join("855-PO-1.x12"),
            "ST*855*1~BAK*00*AC*PO-20261125-0001**20261126~ACK*1*12*GAL*IA~",
        )
        .unwrap();
        let acks = plugin.poll_acknowledgments().await.unwrap();
        assert_eq!(acks.len(), 1);
        assert!(acks[0].contains("ST*855*1~"));
        assert!(plugin.poll_acknowledgments().await.unwrap().is_empty());
        assert!(plugin.poll_asns().await.unwrap().is_empty());

        let claim = RebateClaim {
            claim_id: uuid::Uuid::now_v7().to_string(),
            vendor_id: "VENDOR-A".to_string(),
            claim_number: "REB-1".to_string(),
            period_start: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            period_end: NaiveDate::from_ymd_opt(2026, 11, 30).unwrap(),
            amount: Decimal::from_str("120.0000").unwrap(),
            currency: "USD".to_string(),
            description: None,
        };
        let ack = plugin.submit_rebate_claim(&claim).await.unwrap();
        assert_eq!(ack.status, "filed_locally");
        assert_eq!(ack.vendor_reference, None);
        assert!(root.join("outbox").join("rebate-REB-1.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
