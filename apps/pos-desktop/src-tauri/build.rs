//! Tauri build script: only runs the Tauri codegen when the
//! `tauri-shell` feature is enabled. Without it the crate builds as
//! a plain Rust crate (see `src/lib.rs`).

#[cfg(feature = "tauri-shell")]
fn main() {
    tauri_build::build();
}

#[cfg(not(feature = "tauri-shell"))]
fn main() {}
