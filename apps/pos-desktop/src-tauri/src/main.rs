//! `pos-desktop` — Tauri 2 counter POS binary.
//!
//! With the `tauri-shell` feature this opens the in-process edge and
//! runs the native Tauri window. Without it (the default in this
//! repo's CI/VM, where webkit2gtk is not installed) it prints the
//! documented dev path and exits — see
//! `apps/pos-desktop/README.md`.

#[cfg(feature = "tauri-shell")]
#[tokio::main]
async fn main() {
    let config = pos_desktop_lib::default_config();
    let app = pos_desktop_lib::PosApp::open(config)
        .await
        .expect("failed to open the POS edge");
    pos_desktop_lib::run(app);
}

#[cfg(not(feature = "tauri-shell"))]
fn main() {
    eprintln!("pos-desktop: the native Tauri shell is not compiled into this build.");
    eprintln!("Build it with: cargo run -p pos-desktop --features tauri-shell");
    eprintln!("That needs the Tauri 2 system libraries (webkit2gtk, libsoup, GTK).");
    eprintln!("Or use the dev path instead (see apps/pos-desktop/README.md):");
    eprintln!("  1. cargo run -p df-server -- edge --db sqlite:edge.db \\");
    eprintln!("       --store-id <uuid> --listen 127.0.0.1:8080");
    eprintln!("  2. cd apps/pos-desktop && npm install && npm run dev");
    std::process::exit(2);
}
