//! `pos-desktop` backend — the counter POS brain (Phase 1, v0.2).
//!
//! A [`PosApp`] owns one in-process [`df_edge::Edge`] (local SQLite,
//! offline checkout), a [`df_hw::MockPrinter`] that captures receipt
//! bytes, and two payment providers — [`df_payments::MockTerminal`]
//! and [`df_payments::StandaloneTerminal`] — switchable at the
//! counter. The React frontend talks to it in exactly one of two
//! ways:
//!
//! * **Native Tauri app** (`tauri-shell` feature): the frontend
//!   calls the `#[tauri::command]`s below via `@tauri-apps/api`.
//!   The commands are thin wrappers over the plain methods on
//!   [`PosApp`], with the same JSON shapes as the edge HTTP API.
//! * **Dev path** (no Tauri shell): the frontend talks HTTP to a
//!   `df-server edge` process instead (see
//!   `apps/pos-desktop/README.md`); this crate is then just a plain
//!   library, and the binary prints the dev-path instructions.
//!
//! Money is `rust_decimal::Decimal` throughout — never floats. No
//! card data is handled here: providers return references/tokens
//! only, never PAN/CVV.

use std::sync::{Arc, Mutex};

use df_domain::Money;
use df_edge::{
    CompletedSale, Edge, EdgeConfig, PricePreview, ReturnLineInput, SaleInput, TenderInput, ZReport,
};
use df_hw::{BarcodeEntry, MockPrinter, parse_scanner_input};
use df_payments::{MockTerminal, PaymentProvider, StandaloneTerminal};
use df_sync::{ApplyReport, SyncState};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum PosError {
    #[error("edge error: {0}")]
    Edge(#[from] df_edge::EdgeError),
    #[error("hardware error: {0}")]
    Hw(#[from] df_hw::HwError),
    #[error("unknown payment provider: {0}")]
    UnknownProvider(String),
}

/// Provider names the counter can switch between.
pub const PROVIDER_MOCK: &str = "mock";
pub const PROVIDER_STANDALONE: &str = "standalone";

/// The counter POS application: one edge + printer + providers.
pub struct PosApp {
    edge: Edge,
    printer: Arc<MockPrinter>,
    mock: Arc<MockTerminal>,
    standalone: Arc<StandaloneTerminal>,
    provider_name: Mutex<String>,
    /// In-process HQ sink for `sync_now` (the same `MemoryHq` the
    /// Phase 1 exit test syncs against; a PostgreSQL HQ replaces it
    /// in a later phase).
    hq: Arc<df_sync::MemoryHq>,
}

impl PosApp {
    /// Open the edge on `config`, attach a capturing mock printer
    /// and install the mock terminal as the default provider.
    pub async fn open(config: EdgeConfig) -> Result<Self, PosError> {
        let edge = Edge::open(config).await?;
        let printer = Arc::new(MockPrinter::new());
        edge.set_printer(printer.clone());
        let mock = Arc::new(MockTerminal::new());
        let standalone = Arc::new(StandaloneTerminal::new());
        edge.set_payment_provider(mock.clone());
        Ok(Self {
            edge,
            printer,
            mock,
            standalone,
            provider_name: Mutex::new(PROVIDER_MOCK.to_string()),
            hq: Arc::new(df_sync::MemoryHq::new()),
        })
    }

    pub fn edge(&self) -> &Edge {
        &self.edge
    }

    pub fn store_name(&self) -> &str {
        &self.edge.config().store_name
    }

    pub fn provider_name(&self) -> String {
        self.provider_name.lock().expect("provider lock").clone()
    }

    /// Switch the payment provider (`"mock"` or `"standalone"`).
    /// Stripe Terminal is not selectable in Phase 1: its live calls
    /// are not implemented yet (see `df-payments`).
    pub fn set_provider(&self, name: &str) -> Result<(), PosError> {
        match name {
            PROVIDER_MOCK => self.edge.set_payment_provider(self.mock.clone()),
            PROVIDER_STANDALONE => self.edge.set_payment_provider(self.standalone.clone()),
            other => return Err(PosError::UnknownProvider(other.to_string())),
        }
        *self.provider_name.lock().expect("provider lock") = name.to_string();
        Ok(())
    }

    fn provider(&self) -> Arc<dyn PaymentProvider> {
        match self.provider_name().as_str() {
            PROVIDER_STANDALONE => self.standalone.clone(),
            _ => self.mock.clone(),
        }
    }

    /// Totals for a cart without persisting anything.
    pub async fn price_preview(&self, input: &SaleInput) -> Result<PricePreview, PosError> {
        Ok(self.edge.price_preview(input).await?)
    }

    /// Complete a sale offline against the selected provider.
    pub async fn checkout(&self, input: SaleInput) -> Result<CompletedSale, PosError> {
        let provider = self.provider();
        Ok(self.edge.checkout(input, provider.as_ref()).await?)
    }

    pub async fn open_till(
        &self,
        cashier_id: Uuid,
        opening_float: Money,
    ) -> Result<df_db::models::TillSession, PosError> {
        Ok(self.edge.open_till(cashier_id, opening_float).await?)
    }

    pub async fn close_till(
        &self,
        till_id: Uuid,
        counted_cash: Money,
    ) -> Result<ZReport, PosError> {
        Ok(self.edge.close_till(till_id, counted_cash).await?)
    }

    pub async fn park_hold(&self, input: SaleInput) -> Result<Uuid, PosError> {
        Ok(self.edge.park_hold(input).await?)
    }

    pub async fn list_holds(&self) -> Vec<Uuid> {
        self.edge.list_holds().await
    }

    pub async fn resume_hold(&self, hold_id: Uuid) -> Result<SaleInput, PosError> {
        Ok(self.edge.resume_hold(hold_id).await?)
    }

    pub async fn record_quote(&self, input: SaleInput) -> Result<Uuid, PosError> {
        Ok(self.edge.record_quote(input).await?)
    }

    pub async fn record_layaway(
        &self,
        input: SaleInput,
        deposit: TenderInput,
    ) -> Result<Uuid, PosError> {
        let provider = self.provider();
        Ok(self
            .edge
            .record_layaway(input, deposit, provider.as_ref())
            .await?)
    }

    pub async fn record_return(
        &self,
        original_tx_id: Uuid,
        lines: Vec<ReturnLineInput>,
    ) -> Result<CompletedSale, PosError> {
        Ok(self.edge.record_return(original_tx_id, lines).await?)
    }

    pub async fn void_transaction(&self, id: Uuid) -> Result<df_db::models::Transaction, PosError> {
        Ok(self.edge.void_transaction(id).await?)
    }

    /// Drain the outbox into the in-process HQ sink.
    pub async fn sync_now(&self) -> Result<ApplyReport, PosError> {
        Ok(self.edge.sync_now(self.hq.as_ref()).await?)
    }

    pub async fn sync_state(&self) -> Result<SyncState, PosError> {
        Ok(self.edge.sync_state().await?)
    }

    pub async fn pending_outbox_count(&self) -> Result<u64, PosError> {
        Ok(self.edge.pending_outbox_count().await?)
    }

    /// Parse scanner input against a catalog (real `df-hw` logic:
    /// trims CR/LF framing, applies an optional `qty*barcode`
    /// prefix, applies the catalog quantity multiplier).
    pub fn scan_barcode(
        &self,
        input: &str,
        catalog: &[ScanCatalogEntry],
    ) -> Result<ScanResult, PosError> {
        let entries: Vec<BarcodeEntry> = catalog
            .iter()
            .map(|entry| BarcodeEntry {
                barcode: entry.barcode.clone(),
                uom_code: entry.uom_code.clone(),
                qty_multiplier: entry.qty_multiplier.parse().unwrap_or(Decimal::ONE),
            })
            .collect();
        let scan = parse_scanner_input(input, &entries)?;
        Ok(ScanResult {
            barcode: scan.barcode,
            qty: scan.qty.to_string(),
            uom_code: scan.uom_code,
        })
    }

    /// Text of every receipt captured by the mock printer so far
    /// (the receipt preview in the UI).
    pub fn last_receipt_text(&self) -> String {
        self.printer.captured_text()
    }

    /// How many receipts the mock printer has captured.
    pub fn receipt_count(&self) -> usize {
        self.printer.captured().len()
    }
}

/// One catalog row for [`PosApp::scan_barcode`] (JSON in/out).
/// `qty_multiplier` is a decimal string (e.g. `"12"` for a case).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanCatalogEntry {
    pub barcode: String,
    pub uom_code: String,
    pub qty_multiplier: String,
}

/// Result of [`PosApp::scan_barcode`] (JSON out). `qty` is a decimal
/// string in base units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanResult {
    pub barcode: String,
    pub qty: String,
    pub uom_code: String,
}

#[cfg(feature = "tauri-shell")]
fn cmd_err(err: PosError) -> String {
    err.to_string()
}

// --- Tauri commands (only with the `tauri-shell` feature). -----------

#[cfg(feature = "tauri-shell")]
mod commands {
    use super::*;
    use tauri::State;

    #[tauri::command]
    pub async fn get_status(app: State<'_, PosApp>) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({
            "store_name": app.store_name(),
            "provider": app.provider_name(),
            "pending_outbox": app.pending_outbox_count().await.map_err(cmd_err)?,
            "receipts_printed": app.receipt_count(),
        }))
    }

    #[tauri::command]
    pub async fn set_provider(app: State<'_, PosApp>, name: String) -> Result<String, String> {
        app.set_provider(&name).map_err(cmd_err)?;
        Ok(app.provider_name())
    }

    #[tauri::command]
    pub async fn price_preview(
        app: State<'_, PosApp>,
        input: SaleInput,
    ) -> Result<PricePreview, String> {
        app.price_preview(&input).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn checkout(
        app: State<'_, PosApp>,
        input: SaleInput,
    ) -> Result<CompletedSale, String> {
        app.checkout(input).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn open_till(
        app: State<'_, PosApp>,
        cashier_id: Uuid,
        opening_float: Money,
    ) -> Result<df_db::models::TillSession, String> {
        app.open_till(cashier_id, opening_float)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn close_till(
        app: State<'_, PosApp>,
        till_id: Uuid,
        counted_cash: Money,
    ) -> Result<ZReport, String> {
        app.close_till(till_id, counted_cash).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn park_hold(app: State<'_, PosApp>, input: SaleInput) -> Result<Uuid, String> {
        app.park_hold(input).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_holds(app: State<'_, PosApp>) -> Result<Vec<Uuid>, String> {
        Ok(app.list_holds().await)
    }

    #[tauri::command]
    pub async fn resume_hold(app: State<'_, PosApp>, hold_id: Uuid) -> Result<SaleInput, String> {
        app.resume_hold(hold_id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn record_quote(app: State<'_, PosApp>, input: SaleInput) -> Result<Uuid, String> {
        app.record_quote(input).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn record_layaway(
        app: State<'_, PosApp>,
        input: SaleInput,
        deposit: TenderInput,
    ) -> Result<Uuid, String> {
        app.record_layaway(input, deposit).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn record_return(
        app: State<'_, PosApp>,
        original_tx_id: Uuid,
        lines: Vec<ReturnLineInput>,
    ) -> Result<CompletedSale, String> {
        app.record_return(original_tx_id, lines)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn void_transaction(
        app: State<'_, PosApp>,
        id: Uuid,
    ) -> Result<df_db::models::Transaction, String> {
        app.void_transaction(id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn sync_state(app: State<'_, PosApp>) -> Result<SyncState, String> {
        app.sync_state().await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn sync_now(app: State<'_, PosApp>) -> Result<ApplyReport, String> {
        app.sync_now().await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn scan_barcode(
        app: State<'_, PosApp>,
        input: String,
        catalog: Vec<ScanCatalogEntry>,
    ) -> Result<ScanResult, String> {
        app.scan_barcode(&input, &catalog).map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn last_receipt_text(app: State<'_, PosApp>) -> Result<String, String> {
        Ok(app.last_receipt_text())
    }
}

/// Run the native Tauri shell around an already-opened [`PosApp`].
/// Only available with the `tauri-shell` feature (needs webkit2gtk
/// etc. — see `apps/pos-desktop/README.md`).
#[cfg(feature = "tauri-shell")]
pub fn run(app: PosApp) {
    tauri::Builder::default()
        .manage(app)
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::set_provider,
            commands::price_preview,
            commands::checkout,
            commands::open_till,
            commands::close_till,
            commands::park_hold,
            commands::list_holds,
            commands::resume_hold,
            commands::record_quote,
            commands::record_layaway,
            commands::record_return,
            commands::void_transaction,
            commands::sync_state,
            commands::sync_now,
            commands::scan_barcode,
            commands::last_receipt_text,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Decor POS application");
}

/// Default edge config for the desktop app. Store/org ids come from
/// `DF_STORE_ID` / `DF_ORG_ID` (UUIDs) when set, else fixed demo ids
/// so the local `pos-desktop.db` keeps working across runs.
pub fn default_config() -> EdgeConfig {
    let store_id = std::env::var("DF_STORE_ID")
        .ok()
        .and_then(|v| Uuid::parse_str(&v).ok())
        .unwrap_or_else(|| Uuid::from_u128(0xA000));
    let org_id = std::env::var("DF_ORG_ID")
        .ok()
        .and_then(|v| Uuid::parse_str(&v).ok())
        .unwrap_or_else(|| Uuid::from_u128(0xB000));
    let db_url = std::env::var("DF_DB").unwrap_or_else(|_| "sqlite:pos-desktop.db".to_string());
    let store_name =
        std::env::var("DF_STORE_NAME").unwrap_or_else(|_| "Decor POS Store".to_string());
    let currency = df_domain::Currency::Usd;
    EdgeConfig {
        store_id,
        org_id,
        db_url,
        store_name,
        currency,
        surcharge_rules: EdgeConfig::default_surcharge_rules(currency),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::Currency;

    fn test_config() -> EdgeConfig {
        EdgeConfig {
            store_id: Uuid::from_u128(0xA000),
            org_id: Uuid::from_u128(0xB000),
            db_url: "sqlite::memory:".to_string(),
            store_name: "Test Paint Co".to_string(),
            currency: Currency::Usd,
            surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
        }
    }

    #[tokio::test]
    async fn pos_app_opens_switches_provider_and_scans() {
        let app = PosApp::open(test_config()).await.unwrap();
        assert_eq!(app.store_name(), "Test Paint Co");
        assert_eq!(app.provider_name(), "mock");
        assert!(app.set_provider("standalone").is_ok());
        assert_eq!(app.provider_name(), "standalone");
        assert!(app.set_provider("stripe_terminal").is_err());
        assert_eq!(app.provider_name(), "standalone");

        // Scanner parsing is the real df-hw logic, via the app.
        let catalog = vec![ScanCatalogEntry {
            barcode: "012345678905".to_string(),
            uom_code: "ea".to_string(),
            qty_multiplier: "1".to_string(),
        }];
        let scan = app.scan_barcode("2*012345678905\r\n", &catalog).unwrap();
        assert_eq!(scan.barcode, "012345678905");
        assert_eq!(scan.qty, "2");
        assert_eq!(scan.uom_code, "ea");
        assert!(app.scan_barcode("999999999999", &catalog).is_err());
        assert_eq!(app.receipt_count(), 0);
        assert_eq!(app.last_receipt_text(), "");
    }
}
