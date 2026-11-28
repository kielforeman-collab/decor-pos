//! HQ HTTP server mode (`df-server hq`).

use std::net::SocketAddr;
use std::time::Duration;

use df_auth_http::HttpHardeningConfig;
use uuid::Uuid;

use crate::db::{migrate_and_report, require_db_url};

/// CLI hardening overrides shared by `df-server hq` and `df-server edge`.
#[derive(Debug, Clone, Default)]
pub(crate) struct HardeningFlags {
    pub(crate) cors_origins: Vec<String>,
    pub(crate) auth_rate_limit_max: Option<u32>,
    pub(crate) auth_rate_limit_window_secs: Option<u64>,
    pub(crate) trust_proxy_headers: bool,
}

impl HardeningFlags {
    pub(crate) fn into_config(self, mode: &str) -> HttpHardeningConfig {
        let mut cfg = HttpHardeningConfig::from_env();
        if !self.cors_origins.is_empty() {
            cfg.cors_origins = self.cors_origins;
        }
        if let Some(max) = self.auth_rate_limit_max {
            cfg.auth_rate_limit.max_requests = max;
        }
        if let Some(secs) = self.auth_rate_limit_window_secs
            && secs > 0
        {
            cfg.auth_rate_limit.window = Duration::from_secs(secs);
        }
        if self.trust_proxy_headers {
            cfg.auth_rate_limit.trust_proxy_headers = true;
        }
        if let Err(err) = cfg.validate() {
            eprintln!("df-server {mode}: invalid HTTP hardening configuration: {err}");
            std::process::exit(2);
        }
        cfg
    }
}

/// `df-server hq`: connect, migrate, then serve the `df-api`
/// router (Phase 7 auth gate + F10 HTTP hardening included).
pub(crate) async fn run_hq(
    db_flag: Option<String>,
    listen: SocketAddr,
    org_id: Uuid,
    store_id: Option<Uuid>,
    hardening_flags: HardeningFlags,
) -> ! {
    let hardening = hardening_flags.into_config("hq");
    let db = require_db_url(db_flag, "hq");
    let (pool, report) = migrate_and_report(&db, "hq").await;
    println!(
        "df-server hq: database {} — {} migration(s) applied in total ({} newly applied this start)",
        df_db::redact_db_url(&db),
        report.applied.len(),
        report.newly_applied.len()
    );
    if !report.newly_applied.is_empty() {
        println!(
            "df-server hq: newly applied migrations: {:?}",
            report.newly_applied
        );
    }
    let state = df_api::ApiState {
        pool: Some(pool),
        org_id: Some(org_id),
        store_id,
        ..df_api::ApiState::default()
    };
    let router = df_api::create_router_with_hardening(state, hardening);
    let listener = match tokio::net::TcpListener::bind(listen).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("df-server hq: cannot bind {listen}: {err}");
            std::process::exit(1);
        }
    };
    println!("df-server hq listening on http://{listen} (org {org_id})");
    println!("OpenAPI: http://{listen}/api-docs/openapi.json");
    if let Err(err) = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    {
        eprintln!("df-server hq: serve failed: {err}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
