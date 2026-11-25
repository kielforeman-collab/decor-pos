//! `df-server` — unified CLI binary (`standalone`, `hq`, `edge`, `worker`, `migrate`).
//!
//! Phase 0 stub: the subcommands exist so the CLI surface from spec §3 is
//! visible, but every mode reports that it is not implemented yet. The
//! real modes land in Phase 1 (`edge`, `standalone`) and later phases.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "df-server",
    version,
    about = "decor-pos unified server binary (Phase 0 stub)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// All-in-one single-binary mode (df-api + df-edge + df-jobs).
    Standalone,
    /// Central HQ server mode.
    Hq,
    /// Store edge daemon mode.
    Edge,
    /// Background worker mode.
    Worker,
    /// Run database migrations.
    Migrate,
}

fn main() {
    let cli = Cli::parse();
    let mode = match cli.command {
        Command::Standalone => "standalone",
        Command::Hq => "hq",
        Command::Edge => "edge",
        Command::Worker => "worker",
        Command::Migrate => "migrate",
    };
    eprintln!("df-server {mode}: not implemented in Phase 0 (v0.1) — see spec §7");
    std::process::exit(2);
}
