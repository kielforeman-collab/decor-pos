//! `plugin.toml` manifest parsing and validation (spec §6.2).
//!
//! Every out-of-process plugin registers with a declarative manifest:
//!
//! ```toml
//! name = "tinter-virtual"
//! version = "0.1.0"
//! protocol_version = "1"
//! transport = "http" # "http" | "grpc" | "wasm"
//! endpoint = "http://127.0.0.1:48001"
//! capabilities = ["tinter.dispense", "tinter.list_dispensers"]
//! command = "tinter-virtual"
//! args = ["--port", "48001"]
//! ```
//!
//! `command` / `args` / `working_dir` tell the host how to spawn the
//! plugin child process. They are required for `http` plugins that
//! the host manages; a plugin that is already running (managed
//! externally) may omit them, in which case the caller must spawn it
//! and use the HTTP client directly.
//!
//! Validation: an unknown transport or a missing/empty required field
//! (`name`, `version`, `protocol_version`, `transport`, `endpoint`)
//! is an error. Unknown extra fields are ignored for forward
//! compatibility.

use serde::{Deserialize, Serialize};

use crate::error::{PluginError, Result};

/// Plugin transport. Only `http` is implemented in Phase 2; `grpc`
/// and `wasm` parse fine but the host refuses them with a clear
/// [`PluginError::NotSupported`] — they are never faked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Http,
    Grpc,
    Wasm,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Grpc => "grpc",
            Self::Wasm => "wasm",
        }
    }

    /// Whether the Phase 2 host can actually run this transport.
    pub fn is_supported(self) -> bool {
        matches!(self, Self::Http)
    }
}

/// A parsed `plugin.toml` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    pub transport: Transport,
    /// Base URL for `http` plugins (`http://127.0.0.1:PORT`), or the
    /// socket/address for future transports.
    pub endpoint: String,
    /// Capability strings from spec §6.2, e.g. `tinter.dispense`.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Command used to spawn the plugin child process, if the host
    /// manages the process lifecycle.
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments for `command`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory for the spawned child process.
    #[serde(default)]
    pub working_dir: Option<String>,
}

impl PluginManifest {
    /// Parse and validate a `plugin.toml` document.
    pub fn parse(toml_text: &str) -> Result<Self> {
        let manifest: Self =
            toml::from_str(toml_text).map_err(|e| PluginError::Manifest(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate required fields. Unknown transports are already
    /// rejected by deserialization; this catches empty strings.
    pub fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("name", &self.name),
            ("version", &self.version),
            ("protocol_version", &self.protocol_version),
            ("endpoint", &self.endpoint),
        ] {
            if value.trim().is_empty() {
                return Err(PluginError::Manifest(format!(
                    "required field {field:?} is missing or empty"
                )));
            }
        }
        if self.transport == Transport::Http
            && !self.endpoint.starts_with("http://")
            && !self.endpoint.starts_with("https://")
        {
            return Err(PluginError::Manifest(format!(
                "http plugin endpoint {:?} must start with http:// or https://",
                self.endpoint
            )));
        }
        Ok(())
    }

    /// Whether this plugin declares a capability.
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const VALID_MANIFEST: &str = r#"
name = "tinter-virtual"
version = "0.1.0"
protocol_version = "1"
transport = "http"
endpoint = "http://127.0.0.1:48001"
capabilities = ["tinter.dispense", "tinter.list_dispensers"]
command = "tinter-virtual"
args = ["--port", "48001"]
"#;

    #[test]
    fn parses_valid_manifest() {
        let manifest = PluginManifest::parse(VALID_MANIFEST).unwrap();
        assert_eq!(manifest.name, "tinter-virtual");
        assert_eq!(manifest.transport, Transport::Http);
        assert!(manifest.has_capability("tinter.dispense"));
        assert!(!manifest.has_capability("tinter.watch_results"));
        assert_eq!(manifest.command.as_deref(), Some("tinter-virtual"));
        assert_eq!(manifest.args, vec!["--port", "48001"]);
    }

    #[test]
    fn unknown_transport_is_an_error() {
        let bad = VALID_MANIFEST.replace("\"http\"", "\"carrier-pigeon\"");
        let err = PluginManifest::parse(&bad).unwrap_err();
        assert!(matches!(err, PluginError::Manifest(_)));
        assert!(err.to_string().contains("carrier-pigeon"));
    }

    #[test]
    fn missing_required_fields_are_errors() {
        // No endpoint.
        let no_endpoint = VALID_MANIFEST.replace("endpoint = \"http://127.0.0.1:48001\"\n", "");
        assert!(matches!(
            PluginManifest::parse(&no_endpoint).unwrap_err(),
            PluginError::Manifest(_)
        ));
        // No name.
        let no_name = VALID_MANIFEST.replace("name = \"tinter-virtual\"\n", "");
        assert!(matches!(
            PluginManifest::parse(&no_name).unwrap_err(),
            PluginError::Manifest(_)
        ));
        // Empty version.
        let empty_version = VALID_MANIFEST.replace("version = \"0.1.0\"", "version = \"\"");
        assert!(matches!(
            PluginManifest::parse(&empty_version).unwrap_err(),
            PluginError::Manifest(_)
        ));
        // Malformed TOML.
        assert!(matches!(
            PluginManifest::parse("name = [unclosed").unwrap_err(),
            PluginError::Manifest(_)
        ));
    }

    #[test]
    fn grpc_and_wasm_parse_but_are_not_supported() {
        let grpc = VALID_MANIFEST.replace("\"http\"", "\"grpc\"");
        let manifest = PluginManifest::parse(&grpc).unwrap();
        assert!(!manifest.transport.is_supported());
        let wasm = VALID_MANIFEST.replace("\"http\"", "\"wasm\"");
        assert!(
            !PluginManifest::parse(&wasm)
                .unwrap()
                .transport
                .is_supported()
        );
    }
}
