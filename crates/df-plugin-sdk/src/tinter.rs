//! `TinterPlugin` over the HTTP plugin protocol (spec §6.2).
//!
//! [`HttpTinterPlugin`] speaks the spec §6.2 `TinterPlugin` interface
//! to a tinter plugin process through [`PluginHost`]:
//!
//! | Spec method | HTTP call | Request JSON | Response JSON |
//! |---|---|---|---|
//! | `list_dispensers()` | `POST /tinter/list_dispensers` | `{}` | `{"dispensers": [{"id", "name", "status"}]}` |
//! | `send_formula(DispenseRequest)` | `POST /tinter/send_formula` | [`DispenseRequest`] | [`JobAck`] |
//! | dispense result (see below) | `POST /tinter/dispense_result` | `{"job_id": "…"}` | [`DispenseResult`] |
//!
//! All JSON field names are `snake_case` and match the Rust field
//! names exactly (serde defaults — no renames).
//!
//! ## Mapping note for the later `df-tinter` wiring
//!
//! The `df-tinter` trait is being written in parallel, so this
//! adapter is defined against the spec §6.2 method/field names, not
//! against that trait. To wire it later:
//!
//! * `DispenseRequest` here ↔ the trait's dispense request: map
//!   `color_code` / `color_name` / `base_sku` /
//!   `container_volume_ml` / `sheen` / `colorants[]` (`code`,
//!   `shots`, `sub_shots`) across; `formula_id` and
//!   `base_product_id` are the internal UUIDs and may be `None` when
//!   the plugin only knows SKUs.
//! * `JobAck` here ↔ the trait's job acknowledgment (`job_id`,
//!   `status`, `dispenser_id`).
//! * Spec's `stream_dispensed_formulas() -> Stream<DispensedFormula>`
//!   has **no HTTP equivalent in Phase 2** — HTTP plugins are
//!   request/response. Poll [`HttpTinterPlugin::get_dispense_result`]
//!   with the `job_id` instead; a streaming transport (SSE/WebSocket)
//!   is a later addition, not a silent fake.
//!
//! No COLORx-specific code lives here (spec §9, deferred post-v1.0).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::Result;
use crate::host::PluginHost;
use crate::manifest::PluginManifest;
use std::time::Duration;

/// One colorant shot in a dispense request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorantShot {
    /// Colorant code, e.g. `Y3`.
    pub code: String,
    /// Whole shots (e.g. 48ths of a fluid ounce).
    pub shots: i32,
    /// Sub-shots (e.g. 384ths), default 0.
    #[serde(default)]
    pub sub_shots: i32,
}

/// A formula to dispense — the `send_formula` request body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispenseRequest {
    /// Internal tint-formula id, when the formula already exists.
    #[serde(default)]
    pub formula_id: Option<Uuid>,
    pub color_code: String,
    pub color_name: String,
    /// Base paint SKU, e.g. `PPG-1001`.
    pub base_sku: String,
    /// Internal base-product id, when known.
    #[serde(default)]
    pub base_product_id: Option<Uuid>,
    pub container_volume_ml: i32,
    #[serde(default)]
    pub sheen: Option<String>,
    #[serde(default)]
    pub colorants: Vec<ColorantShot>,
}

/// One dispenser reported by the plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dispenser {
    pub id: String,
    pub name: String,
    /// e.g. `ready`, `busy`, `offline`.
    #[serde(default)]
    pub status: String,
}

/// Acknowledgment of an accepted dispense job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobAck {
    /// Plugin-assigned job id — pass it to `get_dispense_result`.
    pub job_id: String,
    /// e.g. `accepted`, `queued`.
    #[serde(default)]
    pub status: String,
    /// Dispenser that accepted the job, when assigned already.
    #[serde(default)]
    pub dispenser_id: Option<String>,
}

/// The outcome of a dispense job, polled by `job_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispenseResult {
    pub job_id: String,
    /// e.g. `completed`, `failed`, `pending`.
    pub status: String,
    #[serde(default)]
    pub dispensed_at: Option<DateTime<Utc>>,
    /// External formula reference returned by the tinter (barcode/id
    /// for the can label), when the dispense completed.
    #[serde(default)]
    pub external_ref: Option<String>,
    /// Failure reason, when `status` is `failed`.
    #[serde(default)]
    pub error: Option<String>,
}

/// `TinterPlugin` (spec §6.2) over HTTP, backed by a [`PluginHost`].
pub struct HttpTinterPlugin {
    host: PluginHost,
}

impl HttpTinterPlugin {
    /// Spawn the tinter plugin from its manifest and wait for health.
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

    /// `list_dispensers()` — `POST /tinter/list_dispensers`.
    pub async fn list_dispensers(&self) -> Result<Vec<Dispenser>> {
        let response = self
            .host
            .call("/tinter/list_dispensers", serde_json::json!({}))
            .await?;
        let dispensers = response
            .get("dispensers")
            .ok_or_else(|| {
                crate::error::PluginError::InvalidResponse(
                    "list_dispensers response has no \"dispensers\" field".to_string(),
                )
            })?
            .clone();
        Ok(serde_json::from_value(dispensers)?)
    }

    /// `send_formula(DispenseRequest) -> JobAck` —
    /// `POST /tinter/send_formula`.
    pub async fn send_formula(&self, request: &DispenseRequest) -> Result<JobAck> {
        let response = self
            .host
            .call("/tinter/send_formula", serde_json::to_value(request)?)
            .await?;
        Ok(serde_json::from_value(response)?)
    }

    /// Poll the outcome of a dispense job — `POST
    /// /tinter/dispense_result` with `{"job_id": …}`. This is the
    /// Phase 2 HTTP stand-in for the spec's
    /// `stream_dispensed_formulas()` (see the module docs).
    pub async fn get_dispense_result(&self, job_id: &str) -> Result<DispenseResult> {
        let response = self
            .host
            .call(
                "/tinter/dispense_result",
                serde_json::json!({ "job_id": job_id }),
            )
            .await?;
        Ok(serde_json::from_value(response)?)
    }
}
