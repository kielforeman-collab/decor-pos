//! `df-edge` — **Phase 0 stub**.
//!
//! Local store daemon: offline mirror, hardware/tinter/payment host. Implemented in Phase 1 (spec §7).
//!
//! This crate intentionally contains no implementation yet. It exists so
//! the workspace layout from spec §3 resolves in Phase 0; do not treat
//! anything here as a working feature.

/// Marker proving the crate links. Removed when the real implementation lands.
pub fn phase0_stub() -> &'static str {
    "df-edge: Phase 0 stub — not implemented"
}
