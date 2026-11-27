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

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use df_domain::Money;
use df_edge::{
    AgingView, ArCustomerSummary, CollectionsView, ColorantsResponse, CompletedSale,
    CreatePoLineRequest, CreatePurchaseOrderRequest, CreateTintFormulaRequest,
    CreateTransferRequest, Edge, EdgeConfig, InvoiceMatchReport, InvoiceView, PoAcknowledgmentRow,
    PostedArPayment, PricePreview, ProductSearchResult, PurchaseOrderView, ReceivePoLineRequest,
    ReceivePurchaseOrderRequest, ReplenishmentSuggestRequest, ReturnLineInput, SaleInput,
    SendPoResponse, SuggestedPurchaseOrder, TenderInput, TransactionView, Transfer,
    TransferLineView, ZReport,
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

/// `?as_of=YYYY-MM-DD`, defaulting to today (UTC). A malformed
/// date is an error, not a silent default — same rule as the HTTP
/// `aging_date` helper in `df-edge`.
fn aging_date_or_today(as_of: Option<String>) -> Result<chrono::NaiveDate, PosError> {
    match as_of {
        None => Ok(chrono::Utc::now().date_naive()),
        Some(text) => chrono::NaiveDate::from_str(&text).map_err(|_| {
            PosError::Edge(df_edge::EdgeError::InvalidInput(format!(
                "as_of must be YYYY-MM-DD, got {text:?}"
            )))
        }),
    }
}

impl PosApp {
    /// Open the edge on `config`, attach a capturing mock printer
    /// and install the mock terminal as the default provider.
    pub async fn open(config: EdgeConfig) -> Result<Self, PosError> {
        let edge = Edge::open(config).await?;
        // First boot seeds the org + store rows (a no-op when they
        // already exist), so the native app prices tax from the
        // configured jurisdiction instead of 0 — same as `df-server
        // edge`. `DF_COUNTRY` / `DF_REGION` carry the jurisdiction
        // (`CA` / `BC` for this store); absent means a blank seed.
        let country = std::env::var("DF_COUNTRY").ok();
        let region = std::env::var("DF_REGION").ok();
        edge.ensure_configured_store(country.as_deref(), region.as_deref())
            .await?;
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

    // --- Counter-UI port: products, pricing, customers, invoices,
    // tint (same JSON shapes as the edge HTTP API — see
    // `crates/df-edge/src/http.rs`; the frontend's `api.ts`
    // mappers handle both paths). ---

    /// Counter product search: exact `sku` / `barcode`, or a
    /// substring over sku/name for `q` (cap 50).
    pub async fn search_products(
        &self,
        q: Option<String>,
        sku: Option<String>,
        barcode: Option<String>,
        limit: Option<usize>,
    ) -> Result<Vec<ProductSearchResult>, PosError> {
        Ok(self
            .edge
            .search_products(q.as_deref(), sku.as_deref(), barcode.as_deref(), limit)
            .await?)
    }

    /// The seeded price levels for this org.
    pub async fn list_price_levels(&self) -> Result<Vec<df_db::models::PriceLevel>, PosError> {
        Ok(self.edge.list_price_levels().await?)
    }

    /// Counter customer search: substring over account number,
    /// company/first/last name and email.
    pub async fn search_customers(
        &self,
        q: Option<String>,
    ) -> Result<Vec<df_db::models::Customer>, PosError> {
        Ok(self.edge.search_customers(q.as_deref()).await?)
    }

    /// One customer's AR position (ledger balance + credit
    /// controls + entry count).
    pub async fn customer_summary(&self, id: Uuid) -> Result<ArCustomerSummary, PosError> {
        Ok(self.edge.ar_customer_summary(id).await?)
    }

    /// A customer's transactions, newest first, each with lines
    /// (SKUs resolved) and payments.
    pub async fn customer_transactions(
        &self,
        id: Uuid,
        limit: Option<usize>,
    ) -> Result<Vec<TransactionView>, PosError> {
        Ok(self.edge.customer_transactions(id, limit).await?)
    }

    /// One transaction with its lines (SKUs resolved) and payments.
    pub async fn transaction_view(&self, id: Uuid) -> Result<TransactionView, PosError> {
        Ok(self.edge.transaction_view(id).await?)
    }

    /// The letter-size invoice render data for one transaction.
    pub async fn invoice(&self, id: Uuid) -> Result<InvoiceView, PosError> {
        Ok(self.edge.invoice(id).await?)
    }

    /// Tint color catalog search, scoped to one manufacturer.
    pub async fn search_tint_colors(
        &self,
        manufacturer: &str,
        query: &str,
    ) -> Result<Vec<df_db::models::ColorCatalogEntry>, PosError> {
        Ok(self.edge.search_colors(manufacturer, query).await?)
    }

    /// Colorant levels for this store, with shortage alerts.
    pub async fn tint_colorants(&self) -> Result<ColorantsResponse, PosError> {
        Ok(self.edge.colorant_levels().await?)
    }

    /// Stored tint formulas: by customer colour history
    /// (`customer_id`, optionally one job and/or a color
    /// substring) or by color name/code across the org.
    pub async fn search_tint_formulas(
        &self,
        query: Option<String>,
        customer_id: Option<Uuid>,
        job_id: Option<Uuid>,
    ) -> Result<Vec<df_db::models::TintFormula>, PosError> {
        Ok(self
            .edge
            .search_tint_formulas(query.as_deref(), customer_id, job_id)
            .await?)
    }

    /// Create a tint formula (formula tracking only — no machine
    /// code anywhere in this path).
    pub async fn create_tint_formula(
        &self,
        request: CreateTintFormulaRequest,
    ) -> Result<df_db::models::TintFormula, PosError> {
        Ok(self.edge.create_tint_formula(request).await?)
    }

    // --- Orders / Transfers / Accounts screens: the same Edge
    // methods the back office calls over HTTP, wrapped so the
    // Tauri commands below (and the tests) can use them directly. ---

    /// Stores in this org, by code — the destination picker for
    /// inter-store transfers.
    pub async fn list_stores(&self) -> Result<Vec<df_db::models::Store>, PosError> {
        df_db::list_stores_for_org(self.edge.pool(), self.edge.config().org_id)
            .await
            .map_err(|e| PosError::Edge(e.into()))
    }

    /// Stock positions for every product at a store.
    pub async fn inventory_balances(
        &self,
        store_id: Uuid,
    ) -> Result<Vec<df_db::InventoryBalance>, PosError> {
        Ok(self.edge.inventory_balances(store_id).await?)
    }

    /// Replenishment suggestions for a store (nothing is ordered
    /// until a PO is created from one).
    pub async fn suggest_replenishment(
        &self,
        store_id: Option<Uuid>,
        days: Option<i64>,
        lead_time_days: Option<i64>,
    ) -> Result<Vec<SuggestedPurchaseOrder>, PosError> {
        Ok(self
            .edge
            .suggest_replenishment(ReplenishmentSuggestRequest {
                store_id,
                days,
                lead_time_days,
            })
            .await?)
    }

    /// Purchase orders for a store, newest first.
    pub async fn list_purchase_orders(
        &self,
        store_id: Uuid,
    ) -> Result<Vec<PurchaseOrderView>, PosError> {
        Ok(self.edge.list_purchase_orders(store_id).await?)
    }

    /// Create a draft PO from explicit lines.
    pub async fn create_purchase_order(
        &self,
        vendor_id: String,
        store_id: Option<Uuid>,
        lines: Vec<CreatePoLineRequest>,
    ) -> Result<PurchaseOrderView, PosError> {
        Ok(self
            .edge
            .create_purchase_order(CreatePurchaseOrderRequest {
                vendor_id,
                store_id,
                lines,
            })
            .await?)
    }

    /// Receive against a PO (per-line quantities).
    pub async fn receive_purchase_order(
        &self,
        id: Uuid,
        lines: Vec<ReceivePoLineRequest>,
    ) -> Result<PurchaseOrderView, PosError> {
        Ok(self
            .edge
            .receive_purchase_order(id, ReceivePurchaseOrderRequest { lines })
            .await?)
    }

    /// Transfers out of or into a store, newest first.
    pub async fn list_transfers(&self, store_id: Uuid) -> Result<Vec<Transfer>, PosError> {
        Ok(self.edge.list_transfers(store_id).await?)
    }

    /// Draft a transfer. Nothing moves until it is shipped.
    pub async fn create_transfer(
        &self,
        from_store_id: Uuid,
        to_store_id: Uuid,
        lines: Vec<TransferLineView>,
    ) -> Result<Transfer, PosError> {
        Ok(self
            .edge
            .create_transfer(CreateTransferRequest {
                from_store_id,
                to_store_id,
                lines,
            })
            .await?)
    }

    /// Ship a draft transfer (stock leaves the source store).
    pub async fn ship_transfer(&self, id: Uuid) -> Result<Transfer, PosError> {
        Ok(self.edge.ship_transfer(id).await?)
    }

    /// Receive a shipped transfer (stock lands at the destination).
    pub async fn receive_transfer(&self, id: Uuid) -> Result<Transfer, PosError> {
        Ok(self.edge.receive_transfer(id).await?)
    }

    /// AR aging for a store (buckets current / 31–60 / 61–90 /
    /// 91–120 / 120+).
    pub async fn ar_aging(
        &self,
        store_id: Uuid,
        as_of: Option<String>,
    ) -> Result<AgingView, PosError> {
        Ok(self
            .edge
            .ar_aging(store_id, aging_date_or_today(as_of)?)
            .await?)
    }

    /// Collections workspace for a store (accounts with a balance).
    pub async fn ar_collections(
        &self,
        store_id: Uuid,
        as_of: Option<String>,
    ) -> Result<CollectionsView, PosError> {
        Ok(self
            .edge
            .ar_collections(store_id, aging_date_or_today(as_of)?)
            .await?)
    }

    /// Take a payment on a charge account (FIFO allocation).
    /// `method` is a label only — no provider authorization here.
    pub async fn post_ar_payment(
        &self,
        customer_id: Uuid,
        amount: Money,
        method: String,
    ) -> Result<PostedArPayment, PosError> {
        Ok(self
            .edge
            .post_ar_payment(customer_id, amount, &method)
            .await?)
    }

    /// Set (or clear, with `None`) a customer's price level.
    pub async fn set_customer_price_level(
        &self,
        id: Uuid,
        price_level_id: Option<Uuid>,
    ) -> Result<df_db::models::Customer, PosError> {
        Ok(self
            .edge
            .set_customer_price_level(id, price_level_id)
            .await?)
    }

    /// Send a draft PO as X12 850 (stages the file; without an
    /// out-dir nothing is written anywhere).
    pub async fn send_purchase_order(
        &self,
        id: Uuid,
        out_dir: Option<String>,
    ) -> Result<SendPoResponse, PosError> {
        Ok(self.edge.send_purchase_order(id, out_dir).await?)
    }

    /// EDI documents linked to a PO (sent 850s, received 856/810/
    /// 855/832). Returned as plain JSON values — `EdiDocument`
    /// lives in `df-edi`, which this crate does not depend on.
    pub async fn list_edi_documents(
        &self,
        po_id: Uuid,
    ) -> Result<Vec<serde_json::Value>, PosError> {
        let docs = self.edge.list_edi_documents(po_id).await?;
        docs.iter()
            .map(|d| {
                serde_json::to_value(d)
                    .map_err(|e| PosError::Edge(df_edge::EdgeError::InvalidInput(e.to_string())))
            })
            .collect()
    }

    /// 855 acknowledgments for a PO.
    pub async fn list_po_acknowledgments(
        &self,
        po_id: Uuid,
    ) -> Result<Vec<PoAcknowledgmentRow>, PosError> {
        Ok(self.edge.list_po_acknowledgments(po_id).await?)
    }

    /// 3-way match for a PO (ordered vs shipped vs billed).
    pub async fn three_way_match(&self, po_id: Uuid) -> Result<InvoiceMatchReport, PosError> {
        Ok(self.edge.three_way_match(po_id).await?)
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
            "store_id": app.edge().config().store_id,
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

    // --- Counter-UI port (same JSON shapes as the edge HTTP API;
    // the frontend's `api.ts` mappers handle both paths). ---

    #[tauri::command]
    pub async fn search_products(
        app: State<'_, PosApp>,
        q: Option<String>,
        sku: Option<String>,
        barcode: Option<String>,
        limit: Option<usize>,
    ) -> Result<Vec<ProductSearchResult>, String> {
        app.search_products(q, sku, barcode, limit)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_price_levels(
        app: State<'_, PosApp>,
    ) -> Result<Vec<df_db::models::PriceLevel>, String> {
        app.list_price_levels().await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn search_customers(
        app: State<'_, PosApp>,
        q: Option<String>,
    ) -> Result<Vec<df_db::models::Customer>, String> {
        app.search_customers(q).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn customer_summary(
        app: State<'_, PosApp>,
        id: Uuid,
    ) -> Result<ArCustomerSummary, String> {
        app.customer_summary(id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn customer_transactions(
        app: State<'_, PosApp>,
        id: Uuid,
        limit: Option<usize>,
    ) -> Result<Vec<TransactionView>, String> {
        app.customer_transactions(id, limit).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn get_transaction(
        app: State<'_, PosApp>,
        id: Uuid,
    ) -> Result<TransactionView, String> {
        app.transaction_view(id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn get_invoice(app: State<'_, PosApp>, id: Uuid) -> Result<InvoiceView, String> {
        app.invoice(id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_tint_colors(
        app: State<'_, PosApp>,
        manufacturer: Option<String>,
        query: Option<String>,
    ) -> Result<Vec<df_db::models::ColorCatalogEntry>, String> {
        app.search_tint_colors(
            manufacturer.as_deref().unwrap_or(""),
            query.as_deref().unwrap_or(""),
        )
        .await
        .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_tint_colorants(app: State<'_, PosApp>) -> Result<ColorantsResponse, String> {
        app.tint_colorants().await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_tint_formulas(
        app: State<'_, PosApp>,
        query: Option<String>,
        customer_id: Option<Uuid>,
        job_id: Option<Uuid>,
    ) -> Result<Vec<df_db::models::TintFormula>, String> {
        app.search_tint_formulas(query, customer_id, job_id)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn create_tint_formula(
        app: State<'_, PosApp>,
        customer_id: Option<Uuid>,
        job_id: Option<Uuid>,
        color_code: String,
        color_name: String,
        base_product_id: Uuid,
        container_volume_ml: i32,
        sheen: Option<String>,
        colorants: Vec<df_edge::CreateFormulaColorant>,
    ) -> Result<df_db::models::TintFormula, String> {
        app.create_tint_formula(CreateTintFormulaRequest {
            customer_id,
            job_id,
            color_code,
            color_name,
            base_product_id,
            container_volume_ml,
            sheen,
            colorants,
        })
        .await
        .map_err(cmd_err)
    }

    // --- Orders / Transfers / Accounts screens (same JSON shapes
    // as the edge HTTP API; the frontend's `api.ts` mappers handle
    // both paths). ---

    #[tauri::command]
    pub async fn list_stores(app: State<'_, PosApp>) -> Result<Vec<df_db::models::Store>, String> {
        app.list_stores().await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn inventory_balances(
        app: State<'_, PosApp>,
        store_id: Uuid,
    ) -> Result<Vec<df_db::InventoryBalance>, String> {
        app.inventory_balances(store_id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn suggest_replenishment(
        app: State<'_, PosApp>,
        store_id: Option<Uuid>,
        days: Option<i64>,
        lead_time_days: Option<i64>,
    ) -> Result<Vec<SuggestedPurchaseOrder>, String> {
        app.suggest_replenishment(store_id, days, lead_time_days)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_purchase_orders(
        app: State<'_, PosApp>,
        store_id: Uuid,
    ) -> Result<Vec<PurchaseOrderView>, String> {
        app.list_purchase_orders(store_id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn create_purchase_order(
        app: State<'_, PosApp>,
        vendor_id: String,
        store_id: Option<Uuid>,
        lines: Vec<CreatePoLineRequest>,
    ) -> Result<PurchaseOrderView, String> {
        app.create_purchase_order(vendor_id, store_id, lines)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn receive_purchase_order(
        app: State<'_, PosApp>,
        id: Uuid,
        lines: Vec<ReceivePoLineRequest>,
    ) -> Result<PurchaseOrderView, String> {
        app.receive_purchase_order(id, lines).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_transfers(
        app: State<'_, PosApp>,
        store_id: Uuid,
    ) -> Result<Vec<Transfer>, String> {
        app.list_transfers(store_id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn create_transfer(
        app: State<'_, PosApp>,
        from_store_id: Uuid,
        to_store_id: Uuid,
        lines: Vec<TransferLineView>,
    ) -> Result<Transfer, String> {
        app.create_transfer(from_store_id, to_store_id, lines)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn ship_transfer(app: State<'_, PosApp>, id: Uuid) -> Result<Transfer, String> {
        app.ship_transfer(id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn receive_transfer(app: State<'_, PosApp>, id: Uuid) -> Result<Transfer, String> {
        app.receive_transfer(id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn get_ar_aging(
        app: State<'_, PosApp>,
        store_id: Uuid,
        as_of: Option<String>,
    ) -> Result<AgingView, String> {
        app.ar_aging(store_id, as_of).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn get_ar_collections(
        app: State<'_, PosApp>,
        store_id: Uuid,
        as_of: Option<String>,
    ) -> Result<CollectionsView, String> {
        app.ar_collections(store_id, as_of).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn post_ar_payment(
        app: State<'_, PosApp>,
        customer_id: Uuid,
        amount: Money,
        method: String,
    ) -> Result<PostedArPayment, String> {
        app.post_ar_payment(customer_id, amount, method)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn set_customer_price_level(
        app: State<'_, PosApp>,
        id: Uuid,
        price_level_id: Option<Uuid>,
    ) -> Result<df_db::models::Customer, String> {
        app.set_customer_price_level(id, price_level_id)
            .await
            .map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn send_purchase_order(
        app: State<'_, PosApp>,
        id: Uuid,
        out_dir: Option<String>,
    ) -> Result<SendPoResponse, String> {
        app.send_purchase_order(id, out_dir).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_edi_documents(
        app: State<'_, PosApp>,
        po_id: Uuid,
    ) -> Result<Vec<serde_json::Value>, String> {
        app.list_edi_documents(po_id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn list_po_acknowledgments(
        app: State<'_, PosApp>,
        po_id: Uuid,
    ) -> Result<Vec<PoAcknowledgmentRow>, String> {
        app.list_po_acknowledgments(po_id).await.map_err(cmd_err)
    }

    #[tauri::command]
    pub async fn three_way_match(
        app: State<'_, PosApp>,
        po_id: Uuid,
    ) -> Result<InvoiceMatchReport, String> {
        app.three_way_match(po_id).await.map_err(cmd_err)
    }

    // --- auth commands (Phase 7): the same login / setup / switch /
    // logout / override surface as the edge HTTP `/pos/auth/*`
    // endpoints, backed by the same `df_db` functions the HTTP
    // handlers call. The frontend (`src/auth.ts`) uses these instead
    // of HTTP when it runs inside the Tauri shell, where there is no
    // Vite proxy and no HTTP edge to talk to. Login responses use
    // the canonical flat `df_auth_http::LoginResponse` shape, so
    // `normalizeSession` parses them exactly like the HTTP ones.

    /// Map an auth failure to the same message the HTTP layer
    /// returns (see `df_auth_http::login_error`): a generic 401
    /// text for bad credentials, so the boundary cannot probe
    /// which emails exist or which accounts are active.
    fn auth_err(err: df_auth::AuthError) -> String {
        use df_auth::AuthError;
        match err {
            AuthError::InvalidCredentials | AuthError::AccountInactive => {
                "invalid credentials".to_string()
            }
            AuthError::PortalNotLinked => "portal user has no linked customer".to_string(),
            AuthError::PermissionDenied(message) => message,
            AuthError::InvalidSession => "invalid or expired session".to_string(),
            _ => "login failed".to_string(),
        }
    }

    #[tauri::command]
    pub async fn auth_login(
        app: State<'_, PosApp>,
        email: String,
        password: String,
    ) -> Result<df_auth_http::LoginResponse, String> {
        let outcome = df_db::login_with_password(app.edge().pool(), &email, &password, false)
            .await
            .map_err(auth_err)?;
        Ok(df_auth_http::LoginResponse::from_outcome(&outcome))
    }

    #[tauri::command]
    pub async fn auth_setup(
        app: State<'_, PosApp>,
        email: String,
        display_name: Option<String>,
        password: String,
    ) -> Result<df_auth_http::LoginResponse, String> {
        let outcome = df_db::setup_first_admin(
            app.edge().pool(),
            app.edge().config().org_id,
            &email,
            display_name.as_deref(),
            &password,
        )
        .await
        .map_err(|err| match err {
            df_db::SetupError::AlreadySetup => {
                "setup already complete — a user already exists".to_string()
            }
            df_db::SetupError::InvalidInput(message) => message,
            df_db::SetupError::Database(_) => "setup failed".to_string(),
        })?;
        Ok(df_auth_http::LoginResponse::from_outcome(&outcome))
    }

    #[tauri::command]
    pub async fn auth_setup_status(
        app: State<'_, PosApp>,
    ) -> Result<df_auth_http::SetupStatusResponse, String> {
        let count = df_db::count_users(app.edge().pool())
            .await
            .map_err(|_| "setup status check failed".to_string())?;
        Ok(df_auth_http::SetupStatusResponse {
            needs_setup: count == 0,
        })
    }

    #[tauri::command]
    pub async fn auth_counter_switch(
        app: State<'_, PosApp>,
        email: String,
        pin: Option<String>,
        badge_token: Option<String>,
    ) -> Result<df_auth_http::LoginResponse, String> {
        let pool = app.edge().pool();
        if let Some(pin) = pin.as_deref() {
            let outcome = df_db::login_with_pin(pool, &email, pin)
                .await
                .map_err(auth_err)?;
            return Ok(df_auth_http::LoginResponse::from_outcome(&outcome));
        }
        if let Some(badge) = badge_token.as_deref() {
            let outcome = df_db::login_with_badge(pool, &email, badge)
                .await
                .map_err(auth_err)?;
            return Ok(df_auth_http::LoginResponse::from_outcome(&outcome));
        }
        Err("counter-switch needs a pin or a badge_token".to_string())
    }

    #[tauri::command]
    pub async fn auth_logout(
        app: State<'_, PosApp>,
        token: String,
    ) -> Result<df_auth_http::LogoutResponse, String> {
        // Best-effort, like the HTTP logout: the frontend clears its
        // local session regardless, so an unknown/expired token is
        // `revoked: false`, never an error.
        let pool = app.edge().pool();
        let revoked = match df_db::validate_session_token(pool, &token).await {
            Ok(session) => df_db::revoke_session(pool, session.id).await.is_ok(),
            Err(_) => false,
        };
        Ok(df_auth_http::LogoutResponse { revoked })
    }

    #[tauri::command]
    pub async fn auth_override_verify(
        app: State<'_, PosApp>,
        user_id: String,
        pin: String,
        reason_code: String,
    ) -> Result<df_auth_http::OverrideVerifyResponse, String> {
        if reason_code.trim().is_empty() {
            return Err("reason_code is required".to_string());
        }
        let user_id =
            Uuid::parse_str(&user_id).map_err(|_| format!("user_id is not a UUID: {user_id:?}"))?;
        let pool = app.edge().pool();
        let user = df_db::get_user_by_id(pool, user_id)
            .await
            .map_err(|_| auth_err(df_auth::AuthError::InvalidCredentials))?;
        let approval = df_db::verify_manager_override(
            pool,
            &user.email,
            &df_auth::OverrideCredential::Pin(pin),
        )
        .await
        .map_err(auth_err)?;
        Ok(df_auth_http::OverrideVerifyResponse::from_approval(
            &approval,
        ))
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
            commands::search_products,
            commands::list_price_levels,
            commands::search_customers,
            commands::customer_summary,
            commands::customer_transactions,
            commands::get_transaction,
            commands::get_invoice,
            commands::list_tint_colors,
            commands::list_tint_colorants,
            commands::list_tint_formulas,
            commands::create_tint_formula,
            commands::list_stores,
            commands::inventory_balances,
            commands::suggest_replenishment,
            commands::list_purchase_orders,
            commands::create_purchase_order,
            commands::receive_purchase_order,
            commands::list_transfers,
            commands::create_transfer,
            commands::ship_transfer,
            commands::receive_transfer,
            commands::get_ar_aging,
            commands::get_ar_collections,
            commands::post_ar_payment,
            commands::set_customer_price_level,
            commands::send_purchase_order,
            commands::list_edi_documents,
            commands::list_po_acknowledgments,
            commands::three_way_match,
            commands::auth_login,
            commands::auth_setup,
            commands::auth_setup_status,
            commands::auth_counter_switch,
            commands::auth_logout,
            commands::auth_override_verify,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Decor POS application");
}

/// Default edge config for the desktop app. Store/org ids come from
/// `DF_STORE_ID` / `DF_ORG_ID` (UUIDs) when set, else fixed demo ids
/// so the local `pos-desktop.db` keeps working across runs.
///
/// Currency comes from `DF_CURRENCY` (`USD` or `CAD`) when set and
/// valid, else `USD` — the counter UI sends CAD amounts, so this
/// store runs with `DF_CURRENCY=CAD` (and `DF_COUNTRY=CA`
/// `DF_REGION=BC`, read by [`PosApp::open`] for the first-boot
/// store seed). The `USD` fallback preserves the previous
/// behaviour for any other store; it is not this store's value.
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
    let currency = std::env::var("DF_CURRENCY")
        .ok()
        .and_then(|v| df_domain::Currency::from_str(v.trim()).ok())
        .unwrap_or(df_domain::Currency::Usd);
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

    #[tokio::test]
    async fn pos_app_orders_transfers_accounts_methods() {
        let app = PosApp::open(test_config()).await.unwrap();
        let store_id = Uuid::from_u128(0xA000);

        // One store is seeded on first boot; nothing else exists yet.
        let stores = app.list_stores().await.unwrap();
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].id, store_id);

        assert!(app.list_purchase_orders(store_id).await.unwrap().is_empty());
        assert!(app.list_transfers(store_id).await.unwrap().is_empty());
        assert!(app.inventory_balances(store_id).await.unwrap().is_empty());
        assert!(
            app.suggest_replenishment(Some(store_id), None, None)
                .await
                .unwrap()
                .is_empty()
        );
        let aging = app.ar_aging(store_id, None).await.unwrap();
        assert!(aging.customers.is_empty());
        let collections = app.ar_collections(store_id, None).await.unwrap();
        assert!(collections.customers.is_empty());

        // Unknown ids are errors, not empty values.
        let missing = Uuid::now_v7();
        assert!(
            app.post_ar_payment(
                missing,
                Money::new(Decimal::from(10), Currency::Usd),
                "Cash".into()
            )
            .await
            .is_err()
        );
        assert!(app.set_customer_price_level(missing, None).await.is_err());
        assert!(app.ship_transfer(missing).await.is_err());
        assert!(app.receive_transfer(missing).await.is_err());
        assert!(app.three_way_match(missing).await.is_err());
    }

    #[tokio::test]
    async fn pos_app_counter_reads_and_tint_formula_validation() {
        use std::str::FromStr;

        let app = PosApp::open(test_config()).await.unwrap();

        // The four price levels are seeded by `Edge::open`.
        let levels = app.list_price_levels().await.unwrap();
        assert_eq!(levels.len(), 4);
        assert!(levels.iter().any(|l| l.code == "gold"));

        // Empty catalog / customer book on a fresh edge.
        assert!(
            app.search_products(Some("regal".to_string()), None, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            app.search_customers(Some("nobody".to_string()))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(app.search_tint_colors("", "").await.unwrap().is_empty());
        assert!(app.tint_colorants().await.unwrap().levels.is_empty());
        assert!(
            app.search_tint_formulas(Some("oc-1".to_string()), None, None)
                .await
                .unwrap()
                .is_empty()
        );

        // Unknown ids are errors, not empty values.
        let missing = Uuid::now_v7();
        assert!(app.customer_summary(missing).await.is_err());
        assert!(app.transaction_view(missing).await.is_err());
        assert!(app.invoice(missing).await.is_err());
        assert!(app.customer_transactions(missing, None).await.is_err());

        // Formula creation validates before storing: an unknown
        // colorant code is rejected (no colorant rows seeded here).
        let bad_formula = CreateTintFormulaRequest {
            customer_id: None,
            job_id: None,
            color_code: "OC-1".to_string(),
            color_name: "Test White".to_string(),
            base_product_id: Uuid::now_v7(),
            container_volume_ml: 3785,
            sheen: None,
            colorants: vec![df_edge::CreateFormulaColorant {
                code: "S1".to_string(),
                oz: Decimal::from_str("1.5").unwrap(),
                shots: 0,
            }],
        };
        assert!(app.create_tint_formula(bad_formula).await.is_err());
    }
}
