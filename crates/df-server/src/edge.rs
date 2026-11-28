//! Store edge daemon mode (`df-server edge`).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use df_domain::Currency;
use df_edge::{Edge, EdgeConfig, MockTerminal};
use df_sync::MemoryHq;
use uuid::Uuid;

use crate::hq::HardeningFlags;

pub(crate) struct EdgeRunArgs {
    pub(crate) db: String,
    pub(crate) store_id: Uuid,
    pub(crate) listen: SocketAddr,
    pub(crate) org_id: Option<Uuid>,
    pub(crate) store_name: String,
    pub(crate) currency: String,
    pub(crate) country: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) email_outbox: Option<PathBuf>,
    pub(crate) hq_url: Option<String>,
    pub(crate) hq_token_env: String,
    pub(crate) sync_interval_secs: Option<u64>,
    pub(crate) hardening: HardeningFlags,
}

/// The `df-server edge` daemon.
pub(crate) async fn run_edge(args: EdgeRunArgs) -> ! {
    let hardening = args.hardening.into_config("edge");
    let currency: Currency = match args.currency.parse() {
        Ok(currency) => currency,
        Err(err) => {
            eprintln!("df-server edge: invalid --currency: {err}");
            std::process::exit(2);
        }
    };
    // Jurisdiction for the first-boot store seed. Only US/CA have
    // tax presets, so anything else is a startup error, not a
    // silent 0% tax. Empty/absent means "not configured": the seed
    // leaves the codes blank (and warns below).
    let country = match args.country.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(code) => match code.to_ascii_uppercase().as_str() {
            "US" | "CA" => Some(code.to_ascii_uppercase()),
            other => {
                eprintln!("df-server edge: invalid --country {other:?}: expected US or CA");
                std::process::exit(2);
            }
        },
    };
    let region = args
        .region
        .as_deref()
        .map(str::trim)
        .filter(|region| !region.is_empty())
        .map(str::to_ascii_uppercase);
    let org_id = args.org_id.unwrap_or_else(Uuid::now_v7);
    let config = EdgeConfig {
        store_id: args.store_id,
        org_id,
        db_url: args.db,
        store_name: args.store_name,
        currency,
        surcharge_rules: EdgeConfig::default_surcharge_rules(currency),
    };
    let edge = match Edge::open(config).await {
        Ok(edge) => edge,
        Err(err) => {
            eprintln!("df-server edge: failed to open edge: {err}");
            std::process::exit(1);
        }
    };
    // First boot seeds the configured org + store rows (a no-op when
    // they already exist). Without this, a fresh database has an empty
    // `stores` table and store-validating flows — e.g. the GUI product
    // import — fail with "row not found".
    match edge
        .ensure_configured_store(country.as_deref(), region.as_deref())
        .await
    {
        Ok(true) => {
            println!(
                "df-server edge: seeded store {} (org {org_id}) — first boot",
                args.store_id
            );
            if country.is_none() || region.is_none() {
                eprintln!(
                    "df-server edge: WARNING: no --country/--region given, so the seeded store has blank jurisdiction and tax prices as 0 — pass --country CA --region BC (or set COUNTRY/REGION in .env) and re-seed, or fix the store row"
                );
            }
        }
        Ok(false) => {}
        Err(err) => {
            eprintln!("df-server edge: failed to seed store: {err}");
            std::process::exit(1);
        }
    };
    // Standalone/mock terminal: card tenders approve locally so the
    // POS API works out of the box with no real terminal attached.
    edge.set_payment_provider(Arc::new(MockTerminal::new()));
    if let Some(dir) = args.email_outbox {
        edge.set_email_outbox_dir(dir);
    }

    // Remote HQ sync: prefer `--hq-url` / `DF_HQ_URL` + token from
    // `--hq-token-env` (default `DF_HQ_TOKEN` — accepts a long-lived
    // store-scoped `dfs_` service token or staff `dfp_` session).
    // Falls back to in-process `MemoryHq` when no HQ URL is given.
    let hq_url = args
        .hq_url
        .or_else(|| std::env::var("DF_HQ_URL").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(url) = hq_url {
        let token = std::env::var(&args.hq_token_env)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(token) = token else {
            eprintln!(
                "df-server edge: HQ URL {url} configured, but env var {} is unset or empty — mint a store service token with `df-server tokens create`",
                args.hq_token_env
            );
            std::process::exit(2);
        };
        edge.set_remote_hq(&url, token);
        let interval_secs = args
            .sync_interval_secs
            .or_else(|| {
                std::env::var("DF_SYNC_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.trim().parse::<u64>().ok())
            })
            .unwrap_or(30);
        println!(
            "df-server edge: remote HQ sync enabled -> {url} (interval: {}s)",
            interval_secs
        );
        if interval_secs > 0 {
            let sync_edge = edge.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
                // Skip the immediate startup tick so the HTTP server binds first.
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    match sync_edge.sync_now_with_configured_hq().await {
                        Ok(report)
                            if report.applied > 0
                                || report.conflicts > 0
                                || report.duplicates > 0 =>
                        {
                            println!(
                                "df-server edge: synced batch (applied={}, duplicates={}, conflicts={})",
                                report.applied, report.duplicates, report.conflicts
                            );
                        }
                        Ok(_) => {}
                        Err(err) => {
                            eprintln!("df-server edge: background HQ sync failed: {err}");
                        }
                    }
                }
            });
        }
    } else {
        // In-process in-memory HQ sink, so `/pos/sync/run` works out of
        // the box in the local dev path.
        edge.set_hq(Arc::new(MemoryHq::new()));
    }

    println!(
        "df-server edge listening on http://{} (store {}, org {org_id})",
        args.listen, args.store_id
    );
    if let Err(err) = edge.serve_with_hardening(args.listen, hardening).await {
        eprintln!("df-server edge: serve failed: {err}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
