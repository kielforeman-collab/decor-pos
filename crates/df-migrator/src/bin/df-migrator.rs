//! `df-migrator` standalone binary: import legacy Decor Fusion
//! CSVs into a decor-pos database and write the reconciliation
//! manifest.
//!
//! ```text
//! df-migrator import --db sqlite:edge.db --org-id <uuid> --store-id <uuid> --dir ./legacy [--out manifest.json]
//! ```
//!
//! The database is migrated first (SQLite or PostgreSQL by URL),
//! then every CSV in `--dir` is imported in dependency order and
//! the manifest is printed as JSON (or written to `--out`). Exit
//! code 0 means every row imported; 1 means some row or file
//! failed — the manifest still lists what happened to each row.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "df-migrator",
    version,
    about = "Import legacy Decor Fusion CSVs into decor-pos"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Import every CSV in a directory (customers, products,
    /// formulas, AR balances, inventory) and print the
    /// reconciliation manifest as JSON.
    Import {
        /// sqlx database URL, e.g. `sqlite:edge.db` or a
        /// PostgreSQL URL. The database is migrated first.
        #[arg(long)]
        db: String,
        /// Organization UUID the import scopes to.
        #[arg(long)]
        org_id: Uuid,
        /// UUID of the store the import scopes to.
        #[arg(long)]
        store_id: Uuid,
        /// Directory holding the legacy CSV files.
        #[arg(long)]
        dir: PathBuf,
        /// Write the manifest JSON to this file instead of
        /// printing it.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn dialect_for(url: &str) -> df_db::Dialect {
    if url.starts_with("postgres") {
        df_db::Dialect::Postgres
    } else {
        df_db::Dialect::Sqlite
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Import {
            db,
            org_id,
            store_id,
            dir,
            out,
        } => {
            let pool = match df_db::connect(&db).await {
                Ok(pool) => pool,
                Err(err) => {
                    eprintln!("df-migrator: failed to connect to {db}: {err}");
                    std::process::exit(1);
                }
            };
            if let Err(err) = df_db::migrate(&pool, dialect_for(&db)).await {
                eprintln!("df-migrator: migration failed: {err}");
                std::process::exit(1);
            }
            let manifest = match df_migrator::import_directory(&pool, org_id, store_id, &dir).await
            {
                Ok(manifest) => manifest,
                Err(err) => {
                    eprintln!("df-migrator: import failed: {err}");
                    std::process::exit(1);
                }
            };
            let json = serde_json::to_string_pretty(&manifest).expect("manifest serialises");
            match out {
                Some(path) => {
                    if let Err(err) = std::fs::write(&path, &json) {
                        eprintln!("df-migrator: cannot write {}: {err}", path.display());
                        std::process::exit(1);
                    }
                    println!(
                        "manifest written to {}: {} read / {} imported / {} skipped / {} failed",
                        path.display(),
                        manifest.total_rows_read,
                        manifest.total_imported,
                        manifest.total_skipped,
                        manifest.total_failed
                    );
                }
                None => println!("{json}"),
            }
            if !manifest.is_clean() {
                eprintln!(
                    "df-migrator: import finished with {} skipped / {} failed row(s) — see the manifest for reasons",
                    manifest.total_skipped, manifest.total_failed
                );
                std::process::exit(1);
            }
        }
    }
}
