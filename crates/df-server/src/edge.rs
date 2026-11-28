//! Store edge daemon mode (`df-server edge`).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use df_domain::Currency;
use df_edge::{Edge, EdgeConfig, MockTerminal};
use df_sync::MemoryHq;
use uuid::Uuid;

/// The `df-server edge` daemon. The nine arguments are the `Edge`
/// CLI flags, passed straight through (clippy's
/// `too_many_arguments`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_edge(
    db: String,
    store_id: Uuid,
    listen: SocketAddr,
    org_id: Option<Uuid>,
    store_name: String,
    currency: String,
    country: Option<String>,
    region: Option<String>,
    email_outbox: Option<PathBuf>,
) -> ! {
    let currency: Currency = match currency.parse() {
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
    let country = match country.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(code) => match code.to_ascii_uppercase().as_str() {
            "US" | "CA" => Some(code.to_ascii_uppercase()),
            other => {
                eprintln!("df-server edge: invalid --country {other:?}: expected US or CA");
                std::process::exit(2);
            }
        },
    };
    let region = region
        .as_deref()
        .map(str::trim)
        .filter(|region| !region.is_empty())
        .map(str::to_ascii_uppercase);
    let org_id = org_id.unwrap_or_else(Uuid::now_v7);
    let config = EdgeConfig {
        store_id,
        org_id,
        db_url: db,
        store_name,
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
            println!("df-server edge: seeded store {store_id} (org {org_id}) — first boot");
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
    if let Some(dir) = email_outbox {
        edge.set_email_outbox_dir(dir);
    }
    // In-process in-memory HQ sink, so `/pos/sync/run` works out of
    // the box in the dev path (lost on restart; a real HQ sink lands
    // in a later phase).
    edge.set_hq(Arc::new(MemoryHq::new()));
    println!("df-server edge listening on http://{listen} (store {store_id}, org {org_id})");
    if let Err(err) = edge.serve(listen).await {
        eprintln!("df-server edge: serve failed: {err}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
