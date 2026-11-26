//! `EdiVendorPlugin` over the HTTP plugin protocol (spec §6.2,
//! Phase 4, v0.5).
//!
//! This is the vendor-pack interface for EDI: a vendor-specific
//! plugin (PPG, ALLPRO, CANPRO, or a buying-group pack) speaks
//! these six calls, and decor-pos core stays vendor-neutral.
//!
//! | Spec method | HTTP call | Request JSON | Response JSON |
//! |---|---|---|---|
//! | `send_po(x12_850)` | `POST /edi/send_po` | `{"x12": "…"}` | `{"file_name": "…", "status": "…"}` |
//! | `poll_acknowledgments()` | `POST /edi/poll_acknowledgments` | `{}` | `{"documents": ["…855 text…"]}` |
//! | `poll_asns()` | `POST /edi/poll_asns` | `{}` | `{"documents": ["…856 text…"]}` |
//! | `poll_invoices()` | `POST /edi/poll_invoices` | `{}` | `{"documents": ["…810 text…"]}` |
//! | `fetch_price_catalog()` | `POST /edi/fetch_price_catalog` | `{}` | `{"x12": "…832 text…"}` or `{"x12": null}` |
//! | `submit_rebate_claim(claim)` | `POST /edi/submit_rebate_claim` | [`RebateClaim`] | [`RebateClaimAck`] |
//!
//! All JSON field names are `snake_case` and match the Rust field
//! names exactly (serde defaults — no renames). The documents are
//! raw X12 text: parsing them is `df-edi`'s job (`parse_855`,
//! `parse_856`, `parse_810`, `parse_832`), not the plugin's.
//!
//! ## Two implementations
//!
//! * [`HttpEdiVendorPlugin`] — the trait over a [`PluginHost`],
//!   for a real out-of-process vendor plugin. Same lifecycle as
//!   [`HttpTinterPlugin`]: spawn from a manifest, call, `stop()`.
//! * [`FileEdiVendorPlugin`] lives in `df-edi` (it needs the
//!   file-drop transport): the same trait backed by a local
//!   drop folder, for testing a vendor pack without a vendor.
//!   It never claims a live vendor connection.
//!
//! A plugin that cannot reach its vendor must fail the call with
//! an error, not return an empty success — an empty `documents`
//! list means "the vendor had nothing new", and is only honest
//! when the plugin actually polled.
//!
//! No COLORx-specific code lives here (spec §9, deferred post-v1.0).

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::{PluginError, Result};
use crate::host::PluginHost;
use crate::manifest::PluginManifest;
use std::time::Duration;

/// A rebate claim to submit to a vendor / buying group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RebateClaim {
    /// Caller-assigned claim id (UUIDv7 string).
    pub claim_id: String,
    pub vendor_id: String,
    pub claim_number: String,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    /// Claim amount (Decimal — no floats).
    pub amount: Decimal,
    pub currency: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// The vendor's (or the file drop's) acknowledgment of a claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RebateClaimAck {
    pub claim_id: String,
    /// e.g. `accepted`, `filed_locally` (file-drop stand-in — no
    /// vendor has confirmed it yet), `rejected`.
    pub status: String,
    /// Vendor-assigned reference, when the vendor gave one.
    #[serde(default)]
    pub vendor_reference: Option<String>,
}

/// The spec §6.2 `EdiVendorPlugin` interface.
///
/// Implementors speak raw X12 text on the wire and return parsed
/// JSON only for the rebate claim. `poll_*` returns every new
/// document since the last poll (empty list = nothing new).
/// `fetch_price_catalog` returns `None` when the vendor has no new
/// catalog.
///
/// The methods are `async fn` in the trait (Rust 2024 RPITIT):
/// the returned futures are not automatically `Send`. That is
/// accepted here — this trait is workspace-internal, both
/// implementors are local, and callers await inline. See
/// `#[allow(async_fn_in_trait)]` below.
#[allow(async_fn_in_trait)]
pub trait EdiVendorPlugin {
    /// Send an 850 to the vendor. Returns the vendor/file
    /// reference for the sent document.
    async fn send_po(&self, x12_850: &str) -> Result<String>;

    /// Poll for new 855 acknowledgments (raw X12 text each).
    async fn poll_acknowledgments(&self) -> Result<Vec<String>>;

    /// Poll for new 856 ASNs (raw X12 text each).
    async fn poll_asns(&self) -> Result<Vec<String>>;

    /// Poll for new 810 invoices (raw X12 text each).
    async fn poll_invoices(&self) -> Result<Vec<String>>;

    /// Fetch the vendor's latest 832 price catalog (raw X12
    /// text), or `None` when there is no new catalog.
    async fn fetch_price_catalog(&self) -> Result<Option<String>>;

    /// Submit a rebate claim to the vendor / buying group.
    async fn submit_rebate_claim(&self, claim: &RebateClaim) -> Result<RebateClaimAck>;
}

/// `EdiVendorPlugin` (spec §6.2) over HTTP, backed by a
/// [`PluginHost`].
pub struct HttpEdiVendorPlugin {
    host: PluginHost,
}

impl HttpEdiVendorPlugin {
    /// Spawn the vendor plugin from its manifest and wait for health.
    pub async fn spawn(manifest: PluginManifest, health_timeout: Duration) -> Result<Self> {
        Ok(Self {
            host: PluginHost::spawn(manifest, health_timeout).await?,
        })
    }

    /// Wrap an already-spawned host.
    pub fn from_host(host: PluginHost) -> Self {
        Self { host }
    }

    /// The underlying host (for health checks and `stop()`).
    pub fn host(&self) -> &PluginHost {
        &self.host
    }

    /// Stop the plugin process. Consumes the adapter.
    pub async fn stop(self) -> Result<()> {
        self.host.stop().await
    }

    fn documents_from(response: serde_json::Value, what: &str) -> Result<Vec<String>> {
        let documents = response.get("documents").ok_or_else(|| {
            PluginError::InvalidResponse(format!("{what} response has no \"documents\" field"))
        })?;
        Ok(serde_json::from_value(documents.clone())?)
    }
}

impl EdiVendorPlugin for HttpEdiVendorPlugin {
    async fn send_po(&self, x12_850: &str) -> Result<String> {
        let response = self
            .host
            .call("/edi/send_po", serde_json::json!({ "x12": x12_850 }))
            .await?;
        let file_name = response
            .get("file_name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                PluginError::InvalidResponse(
                    "send_po response has no \"file_name\" field".to_string(),
                )
            })?;
        Ok(file_name.to_string())
    }

    async fn poll_acknowledgments(&self) -> Result<Vec<String>> {
        let response = self
            .host
            .call("/edi/poll_acknowledgments", serde_json::json!({}))
            .await?;
        Self::documents_from(response, "poll_acknowledgments")
    }

    async fn poll_asns(&self) -> Result<Vec<String>> {
        let response = self
            .host
            .call("/edi/poll_asns", serde_json::json!({}))
            .await?;
        Self::documents_from(response, "poll_asns")
    }

    async fn poll_invoices(&self) -> Result<Vec<String>> {
        let response = self
            .host
            .call("/edi/poll_invoices", serde_json::json!({}))
            .await?;
        Self::documents_from(response, "poll_invoices")
    }

    async fn fetch_price_catalog(&self) -> Result<Option<String>> {
        let response = self
            .host
            .call("/edi/fetch_price_catalog", serde_json::json!({}))
            .await?;
        match response.get("x12") {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(PluginError::InvalidResponse(
                "fetch_price_catalog response \"x12\" is not a string or null".to_string(),
            )),
        }
    }

    async fn submit_rebate_claim(&self, claim: &RebateClaim) -> Result<RebateClaimAck> {
        let response = self
            .host
            .call("/edi/submit_rebate_claim", serde_json::to_value(claim)?)
            .await?;
        Ok(serde_json::from_value(response)?)
    }
}
