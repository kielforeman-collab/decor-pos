//! Printer and cash-drawer abstractions.
//!
//! Byte generation lives in [`crate::receipt`]; this module only moves
//! bytes to a printer (or, for tests, into memory). Nothing here talks to
//! real hardware — [`MockPrinter`] is the stand-in used by tests and by
//! `df-edge` when no printer is attached.

use crate::error::HwError;
use std::sync::Mutex;

/// A receipt printer. Implementations must be shareable across threads so
/// a single printer can be held behind `Arc<dyn Printer>`.
pub trait Printer: Send + Sync {
    /// Send raw ESC/POS bytes to the printer.
    fn print(&self, bytes: &[u8]) -> Result<(), HwError>;
    /// Human-readable printer name, e.g. `"mock"` or a model string.
    fn name(&self) -> &str;
}

/// In-memory printer for tests and offline use. `print` appends a copy of
/// the bytes; nothing is sent anywhere.
pub struct MockPrinter {
    captured: Mutex<Vec<Vec<u8>>>,
}

impl MockPrinter {
    pub fn new() -> Self {
        Self {
            captured: Mutex::new(Vec::new()),
        }
    }

    /// All byte chunks received so far, in order.
    pub fn captured(&self) -> Vec<Vec<u8>> {
        self.captured.lock().expect("mock printer lock").clone()
    }

    /// All captured bytes concatenated and lossily decoded as UTF-8, so
    /// tests can assert on the printable text of a receipt.
    pub fn captured_text(&self) -> String {
        let bytes: Vec<u8> = self.captured.lock().expect("mock printer lock").concat();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Discard everything captured so far.
    pub fn clear(&self) {
        self.captured.lock().expect("mock printer lock").clear();
    }
}

impl Default for MockPrinter {
    fn default() -> Self {
        Self::new()
    }
}

impl Printer for MockPrinter {
    fn print(&self, bytes: &[u8]) -> Result<(), HwError> {
        self.captured
            .lock()
            .expect("mock printer lock")
            .push(bytes.to_vec());
        Ok(())
    }

    fn name(&self) -> &str {
        "mock"
    }
}

/// In-memory cash drawer for tests. Counts kicks; no hardware is involved.
pub struct MockDrawer {
    kicks: Mutex<usize>,
}

impl MockDrawer {
    pub fn new() -> Self {
        Self {
            kicks: Mutex::new(0),
        }
    }

    /// Record one drawer kick.
    pub fn kick(&self) {
        *self.kicks.lock().expect("mock drawer lock") += 1;
    }

    /// How many times [`MockDrawer::kick`] has been called.
    pub fn kick_count(&self) -> usize {
        *self.kicks.lock().expect("mock drawer lock")
    }
}

impl Default for MockDrawer {
    fn default() -> Self {
        Self::new()
    }
}

/// ESC/POS cash-drawer kick sequence: `ESC p m t1 t2` with `m = 0`
/// (drawer 1), 25 ms on / 250 ms off pulse.
pub fn drawer_kick_bytes() -> Vec<u8> {
    vec![0x1B, 0x70, 0x00, 0x19, 0xFA]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn mock_printer_captures_bytes_and_text() {
        let printer = MockPrinter::new();
        assert_eq!(printer.name(), "mock");
        printer.print(b"hello ").unwrap();
        printer.print(b"world").unwrap();
        assert_eq!(
            printer.captured(),
            vec![b"hello ".to_vec(), b"world".to_vec()]
        );
        assert_eq!(printer.captured_text(), "hello world");
        printer.clear();
        assert!(printer.captured().is_empty());
        assert_eq!(printer.captured_text(), "");
    }

    #[test]
    fn mock_printer_is_shareable_as_arc_dyn_printer() {
        let printer: Arc<dyn Printer> = Arc::new(MockPrinter::new());
        printer.print(b"shared").unwrap();
        assert_eq!(printer.name(), "mock");
    }

    #[test]
    fn drawer_kick_bytes_are_esc_p_m0() {
        assert_eq!(drawer_kick_bytes(), vec![0x1B, 0x70, 0x00, 0x19, 0xFA]);
    }

    #[test]
    fn mock_drawer_counts_kicks() {
        let drawer = MockDrawer::new();
        assert_eq!(drawer.kick_count(), 0);
        drawer.kick();
        drawer.kick();
        assert_eq!(drawer.kick_count(), 2);
    }
}
