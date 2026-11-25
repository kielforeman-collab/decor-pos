//! Errors for `df-plugin-sdk`.

use thiserror::Error;

/// Errors from manifest parsing, plugin hosting and plugin calls.
#[derive(Debug, Error)]
pub enum PluginError {
    /// The `plugin.toml` manifest is invalid (unknown transport,
    /// missing required field, bad TOML).
    #[error("invalid plugin manifest: {0}")]
    Manifest(String),

    /// The requested transport is honestly not implemented in this
    /// phase (gRPC and WASM in Phase 2). The message names the
    /// transport so callers never mistake this for a transient error.
    #[error("transport {transport:?} is not supported: {detail}")]
    NotSupported { transport: String, detail: String },

    /// The plugin child process could not be spawned.
    #[error("could not spawn plugin process: {0}")]
    Spawn(String),

    /// The plugin child exited before becoming healthy, or while a
    /// call was in flight.
    #[error("plugin process exited{0}")]
    ProcessExited(String),

    /// The plugin did not answer `GET /health` with `ok` in time.
    #[error("plugin at {endpoint} did not become healthy within {timeout_secs}s")]
    HealthTimeout { endpoint: String, timeout_secs: u64 },

    /// A low-level HTTP failure talking to the plugin (connect,
    /// timeout, malformed response).
    #[error("HTTP error talking to plugin: {0}")]
    Http(String),

    /// The plugin answered with a non-2xx status.
    #[error("plugin returned HTTP {status} for {path}: {body}")]
    HttpStatus {
        status: u16,
        path: String,
        body: String,
    },

    /// The plugin's response was not valid JSON, or did not match the
    /// expected shape.
    #[error("invalid plugin response: {0}")]
    InvalidResponse(String),

    /// JSON (de)serialization failed.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// An I/O error from process management.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience result type for this crate.
pub type Result<T> = std::result::Result<T, PluginError>;

/// The canonical `NotSupported` error for the gRPC transport.
pub fn grpc_not_supported() -> PluginError {
    PluginError::NotSupported {
        transport: "grpc".to_string(),
        detail: "gRPC plugins are not implemented in Phase 2 (spec §6.2); \
                 use transport = \"http\" or wait for the Phase 4 gRPC host"
            .to_string(),
    }
}

/// The canonical `NotSupported` error for the WASM transport.
pub fn wasm_not_supported() -> PluginError {
    PluginError::NotSupported {
        transport: "wasm".to_string(),
        detail: "WASM plugins are not implemented in Phase 2 (spec §6.2); \
                 use transport = \"http\" or wait for the Phase 4 WASM host"
            .to_string(),
    }
}
