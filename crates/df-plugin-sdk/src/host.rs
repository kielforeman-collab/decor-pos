//! The HTTP plugin host: spawns and manages a local plugin child
//! process, waits for its `GET /health` endpoint, and calls its JSON
//! endpoints.
//!
//! Only the `http` transport is implemented. Spawning a `grpc` or
//! `wasm` plugin returns [`PluginError::NotSupported`] naming the
//! transport — the host never pretends those work.

use std::time::{Duration, Instant};

use tokio::process::Child;
use tokio::time::sleep;

use crate::error::{PluginError, Result, grpc_not_supported, wasm_not_supported};
use crate::http_client::{HttpResponse, http_get_timeout, http_post_json_timeout};
use crate::manifest::{PluginManifest, Transport};

/// Default time allowed for a plugin to become healthy after spawn.
pub const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Manages one out-of-process HTTP plugin.
///
/// Lifecycle: [`PluginHost::spawn`] (spawns the child from the
/// manifest's `command`/`args`, then polls `GET {endpoint}/health`
/// until it answers `{"status": "ok"}`), [`PluginHost::call`] for
/// JSON calls, [`PluginHost::stop`] to kill the child cleanly.
/// Callers must call `stop()` — dropping the host does not kill the
/// child.
pub struct PluginHost {
    manifest: PluginManifest,
    child: Option<Child>,
    base_url: String,
}

impl std::fmt::Debug for PluginHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginHost")
            .field("manifest", &self.manifest)
            .field("base_url", &self.base_url)
            .field("running", &self.is_running())
            .finish()
    }
}

impl PluginHost {
    /// Spawn the plugin from its manifest and wait for health.
    ///
    /// * `grpc` / `wasm` manifests fail fast with `NotSupported`.
    /// * An `http` manifest without a `command` fails with a manifest
    ///   error — the host cannot manage a process it was never told
    ///   how to start (use the HTTP client directly for externally
    ///   managed plugins).
    pub async fn spawn(manifest: PluginManifest, health_timeout: Duration) -> Result<Self> {
        match manifest.transport {
            Transport::Grpc => return Err(grpc_not_supported()),
            Transport::Wasm => return Err(wasm_not_supported()),
            Transport::Http => {}
        }
        let command = manifest.command.clone().ok_or_else(|| {
            PluginError::Manifest(
                "http plugin manifest must set `command` so the host can spawn it \
                 (omit the host and use the HTTP client directly for externally managed plugins)"
                    .to_string(),
            )
        })?;
        let base_url = manifest.endpoint.trim_end_matches('/').to_string();
        let mut cmd = tokio::process::Command::new(&command);
        cmd.args(&manifest.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Some(dir) = &manifest.working_dir {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| PluginError::Spawn(format!("{command:?}: {e}")))?;
        let mut host = Self {
            manifest,
            child: None,
            base_url,
        };
        // Wait for health, watching for early child exit.
        let deadline = Instant::now() + health_timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(PluginError::ProcessExited(format!(
                        " (exit status {status} before becoming healthy)"
                    )));
                }
                Ok(None) => {}
                Err(e) => return Err(PluginError::Io(e)),
            }
            match host.health_check_once().await {
                Ok(()) => {
                    host.child = Some(child);
                    return Ok(host);
                }
                Err(_) if Instant::now() < deadline => {
                    sleep(Duration::from_millis(100)).await;
                }
                Err(e) => {
                    // Timed out: kill the child so we do not leak it.
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(match e {
                        PluginError::HealthTimeout { .. } => e,
                        _ => PluginError::HealthTimeout {
                            endpoint: host.base_url.clone(),
                            timeout_secs: health_timeout.as_secs(),
                        },
                    });
                }
            }
        }
    }

    /// The manifest this host was spawned from.
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Base URL of the plugin (`endpoint` without a trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Whether the child process is still tracked as running.
    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    async fn health_check_once(&self) -> Result<()> {
        let url = format!("{}/health", self.base_url);
        let response = http_get_timeout(&url, Duration::from_secs(2)).await?;
        response.require_ok("/health")?;
        let body: serde_json::Value = response.json()?;
        match body.get("status").and_then(|s| s.as_str()) {
            Some("ok") => Ok(()),
            other => Err(PluginError::InvalidResponse(format!(
                "health endpoint did not return {{\"status\": \"ok\"}}, got {other:?}"
            ))),
        }
    }

    /// `GET {endpoint}/health` — must answer 200 with
    /// `{"status": "ok"}`.
    pub async fn health_check(&self) -> Result<()> {
        self.health_check_once().await
    }

    /// `POST {endpoint}{path}` with a JSON body; returns the parsed
    /// JSON response. `path` must start with `/`. Non-2xx statuses
    /// are errors.
    pub async fn call(&self, path: &str, request: serde_json::Value) -> Result<serde_json::Value> {
        if !path.starts_with('/') {
            return Err(PluginError::InvalidResponse(format!(
                "plugin call path {path:?} must start with '/'"
            )));
        }
        let url = format!("{}{}", self.base_url, path);
        let response: HttpResponse =
            http_post_json_timeout(&url, &request, Duration::from_secs(10)).await?;
        response.require_ok(path)?;
        response.json()
    }

    /// Stop the plugin child process cleanly (SIGKILL + wait) and
    /// release the host. Idempotent.
    pub async fn stop(mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        }
        Ok(())
    }
}
