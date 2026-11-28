//! Vendor EDI CLI subcommands (`df-server edi ...`).

use std::path::PathBuf;
use std::str::FromStr;

use df_domain::Currency;
use df_edge::{Edge, EdgeConfig};
use uuid::Uuid;

use crate::db::{open_pool, print_json};

/// The database + tenant every `edi` subcommand needs. Bundled so
/// the `run_edi_*` helpers stay under clippy's argument limit.
pub(crate) struct EdiCtx {
    pub(crate) db: String,
    pub(crate) org_id: Uuid,
    pub(crate) store_id: Uuid,
}

/// Open the edge for an `edi` subcommand. The database file must
/// already exist: SQLite would otherwise create an empty one
/// silently, so a missing file is exit 2. The currency is only
/// needed for the `EdgeConfig` (no EDI result carries money in a
/// currency), so it resolves flag → store row → USD.
pub(crate) async fn open_edge_for_edi(ctx: &EdiCtx, currency_flag: Option<String>) -> Edge {
    if let Some(path) = ctx.db.strip_prefix("sqlite:")
        && path != ":memory:"
        && !PathBuf::from(path).exists()
    {
        eprintln!(
            "df-server edi: database file {path} does not exist — run `df-server edge` first (or pass an existing --db)"
        );
        std::process::exit(2);
    }
    let pool = open_pool(&ctx.db).await;
    let currency = match currency_flag {
        Some(text) => match Currency::from_str(&text) {
            Ok(currency) => currency,
            Err(err) => {
                eprintln!("df-server edi: --currency {text:?} is not USD/CAD: {err}");
                std::process::exit(2);
            }
        },
        None => match df_db::get_store(&pool, ctx.store_id).await {
            Ok(store) => match Currency::from_str(&store.currency) {
                Ok(currency) => currency,
                Err(err) => {
                    eprintln!(
                        "df-server edi: store {} has an unsupported currency: {err}",
                        ctx.store_id
                    );
                    std::process::exit(1);
                }
            },
            Err(_) => Currency::Usd,
        },
    };
    let config = EdgeConfig {
        store_id: ctx.store_id,
        org_id: ctx.org_id,
        db_url: ctx.db.clone(),
        store_name: "Decor POS Store".to_string(),
        currency,
        surcharge_rules: EdgeConfig::default_surcharge_rules(currency),
    };
    match Edge::open(config).await {
        Ok(edge) => edge,
        Err(err) => {
            eprintln!("df-server edi: failed to open edge: {err}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn run_edi_send(
    ctx: EdiCtx,
    po_id: Uuid,
    out_dir: Option<PathBuf>,
    currency: Option<String>,
) -> ! {
    let edge = open_edge_for_edi(&ctx, currency).await;
    let out_dir = out_dir.map(|path| path.to_string_lossy().to_string());
    match edge.send_purchase_order(po_id, out_dir).await {
        Ok(response) => print_json(serde_json::to_value(response).expect("response serialises")),
        Err(err) => {
            eprintln!("df-server edi send: {err}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn run_edi_intake(
    ctx: EdiCtx,
    doc_type: String,
    file: PathBuf,
    vendor_id: String,
    po_id: Option<Uuid>,
    currency: Option<String>,
) -> ! {
    let raw_text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) => {
            eprintln!(
                "df-server edi intake: cannot read {}: {err}",
                file.display()
            );
            std::process::exit(2);
        }
    };
    let edge = open_edge_for_edi(&ctx, currency).await;
    let request = df_edge::IntakeRequest {
        doc_type,
        raw_text,
        vendor_id,
        related_po_id: po_id,
    };
    match edge.intake_edi(request).await {
        Ok(response) => print_json(serde_json::to_value(response).expect("response serialises")),
        Err(err) => {
            eprintln!("df-server edi intake: {err}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn run_edi_match(ctx: EdiCtx, po_id: Uuid, currency: Option<String>) -> ! {
    let edge = open_edge_for_edi(&ctx, currency).await;
    match edge.three_way_match(po_id).await {
        Ok(report) => print_json(serde_json::to_value(report).expect("report serialises")),
        Err(err) => {
            eprintln!("df-server edi match: {err}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn run_edi_catalog(
    ctx: EdiCtx,
    file: PathBuf,
    vendor_id: String,
    currency: Option<String>,
) -> ! {
    let csv_text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) => {
            eprintln!(
                "df-server edi catalog: cannot read {}: {err}",
                file.display()
            );
            std::process::exit(2);
        }
    };
    let edge = open_edge_for_edi(&ctx, currency).await;
    match edge.import_vendor_catalog(&csv_text, &vendor_id).await {
        Ok(response) => print_json(serde_json::to_value(response).expect("response serialises")),
        Err(err) => {
            eprintln!("df-server edi catalog: {err}");
            std::process::exit(1);
        }
    }
}
