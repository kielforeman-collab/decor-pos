//! `df-plugin-sdk` — out-of-process plugin host for decor-pos
//! (Phase 2, v0.3; `EdiVendorPlugin` added Phase 4, v0.5; spec §6.2).
//!
//! Every integration registers via a declarative [`PluginManifest`]
//! (`plugin.toml`) and communicates with a local child process.
//!
//! ## Transports — what is real and what is a stub
//!
//! * **`http` — real.** [`PluginHost::spawn`] spawns the plugin child
//!   process from the manifest's `command`/`args`, polls
//!   `GET {endpoint}/health` until it answers `{"status": "ok"}`, and
//!   calls JSON endpoints with [`PluginHost::call`]. The HTTP client
//!   is a small hand-rolled HTTP/1.1 client over `tokio::TcpStream`
//!   (loopback only, plain `http://`) — no `reqwest`, which is not in
//!   the offline crate cache. It is tested against a real axum
//!   server, not mocked.
//! * **`grpc` / `wasm` — honest stubs.** The manifest parses, but
//!   [`PluginHost::spawn`] returns
//!   [`PluginError::NotSupported`] naming the transport. They are
//!   never faked. Full support is Phase 4.
//!
//! ## Plugin interfaces (spec §6.2)
//!
//! * [`HttpTinterPlugin`] — the `TinterPlugin` interface
//!   (`list_dispensers`, `send_formula`, dispense result) over HTTP.
//!   See `tinter` for the exact JSON mapping and the note on wiring
//!   it to the `df-tinter` trait later.
//! * [`EdiVendorPlugin`] / [`HttpEdiVendorPlugin`] — the
//!   `EdiVendorPlugin` interface (`send_po`, `poll_acknowledgments`,
//!   `poll_asns`, `poll_invoices`, `fetch_price_catalog`,
//!   `submit_rebate_claim`) over HTTP. See `edi` for the exact JSON
//!   mapping and the vendor-pack guide; the file-drop
//!   implementation lives in `df-edi` (`FileEdiVendorPlugin`).
//! * `PaymentPlugin`, `AccountingPlugin` — not implemented; they
//!   ride the same [`PluginHost::call`] mechanism when a plugin for
//!   them exists.
//!
//! ## Vendor-pack guide (spec §6.2, Phase 4)
//!
//! A vendor pack is an out-of-process plugin whose manifest
//! declares `capabilities = ["edi.send_po", "edi.poll_acks",
//! "edi.poll_asns", "edi.poll_invoices", "edi.fetch_catalog",
//! "edi.submit_rebate_claim"]` (any subset — a pack that only
//! sends POs declares only `edi.send_po`) and serves the `POST
//! /edi/*` endpoints in `edi` over HTTP. The plugin speaks raw
//! X12 text; `df-edi` parses it, matches invoices three-way, and
//! applies 832 catalogs only through a reviewed report — the
//! plugin never writes prices itself. A pack that cannot reach
//! its vendor must fail the call, not return an empty success:
//! an empty `documents` list means "polled, nothing new".
//!
//! No COLORx-specific code lives in this crate (spec §9, deferred
//! post-v1.0). The spec §6.2 `plugin.toml` example naming
//! `vendor-bm-colorx` is a future example only and is not
//! implemented.

pub mod edi;
pub mod error;
pub mod host;
pub mod http_client;
pub mod manifest;
pub mod tinter;

pub use edi::{EdiVendorPlugin, HttpEdiVendorPlugin, RebateClaim, RebateClaimAck};
pub use error::{PluginError, Result, grpc_not_supported, wasm_not_supported};
pub use host::{DEFAULT_HEALTH_TIMEOUT, PluginHost};
pub use http_client::{HttpResponse, http_get, http_post_json};
pub use manifest::{PluginManifest, Transport};
pub use tinter::{
    ColorantShot, DispenseRequest, DispenseResult, Dispenser, HttpTinterPlugin, JobAck,
};
