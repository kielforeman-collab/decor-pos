//! The remote HQ sink: pushes a [`DeltaBatch`](crate::outbox::DeltaBatch)
//! to a live HQ over HTTPS as `POST {base_url}/api/v1/sync/ingest`
//! with a staff session `Bearer` token (see `df-api`'s ingest
//! endpoint — the token needs the `SyncIngest` permission, held by
//! Manager and Admin). The edge daemon uses this when a remote HQ is
//! configured (`DF_HQ_URL` / `DF_HQ_TOKEN` env, or `Edge::set_remote_hq`
//! — that type lives in `df-edge`; this sink is transport only).
//!
//! The HQ answers with the same [`ApplyReport`](crate::sink::ApplyReport)
//! the in-process sinks return, so the edge marks exactly the ids
//! the HQ resolved as synced — a conflict the HQ skipped stays
//! pending at the edge, like a local one. Any non-2xx status or
//! transport failure is a [`SyncError::Http`]: the edge marks the
//! whole batch failed and retries it later; nothing is marked
//! synced on a failed push.
//!
//! The token is never logged: [`RemoteHq`]'s `Debug` redacts it.

use crate::error::SyncError;
use crate::outbox::DeltaBatch;
use crate::sink::{ApplyReport, HqSink};

/// Pushes batches to a remote HQ ingest endpoint.
#[derive(Clone)]
pub struct RemoteHq {
    base_url: String,
    token: String,
    client: reqwest::Client,
}

impl std::fmt::Debug for RemoteHq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteHq")
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl RemoteHq {
    /// `base_url` is the HQ origin (`https://hq.example.invalid` —
    /// a trailing `/` is tolerated); `token` is a staff session
    /// token with the `SyncIngest` permission.
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self::with_client(
            base_url,
            token,
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client builds"),
        )
    }

    /// Same as [`RemoteHq::new`] with a caller-supplied client
    /// (tests point this at a local server).
    pub fn with_client(
        base_url: impl Into<String>,
        token: impl Into<String>,
        client: reqwest::Client,
    ) -> Self {
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        Self {
            base_url,
            token: token.into(),
            client,
        }
    }

    /// The ingest URL this sink posts to.
    pub fn ingest_url(&self) -> String {
        format!("{}/api/v1/sync/ingest", self.base_url)
    }

    async fn apply_batch_inner(&self, batch: &DeltaBatch) -> Result<ApplyReport, SyncError> {
        let response = self
            .client
            .post(self.ingest_url())
            .bearer_auth(&self.token)
            .json(batch)
            .send()
            .await
            .map_err(|e| SyncError::Http(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(SyncError::Http(format!(
                "HQ ingest {status}: {}",
                body.chars().take(300).collect::<String>()
            )));
        }
        response
            .json::<ApplyReport>()
            .await
            .map_err(|e| SyncError::Http(format!("HQ ingest reply is not an ApplyReport: {e}")))
    }
}

impl HqSink for RemoteHq {
    fn apply_batch<'a>(
        &'a self,
        batch: &'a DeltaBatch,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ApplyReport, SyncError>> + Send + 'a>,
    > {
        Box::pin(async move { self.apply_batch_inner(batch).await })
    }
}
