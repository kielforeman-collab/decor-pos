//! The edge daemon: offline checkout, tills, holds/quotes/returns,
//! sync drain, receipt printing and drawer kick.
//!
//! Everything runs against the local SQLite database — **no network
//! use at all** while selling. A checkout writes the transaction,
//! its lines, inventory deltas, AR entry (charge account), payment
//! rows and outbox entries in **one** local SQLite transaction, so a
//! sale is never half-recorded. The receipt prints (best-effort) and
//! the drawer kicks only after the commit.
//!
//! Card surcharge: the store's [`EdgeConfig::surcharge_rules`]
//! (default: 3% of the card tender amount, added to
//! `pos_fee_total` and collected on the card), computed with
//! `df_payments::compute_surcharge`.
//!
//! Phase 1 counter completeness notes:
//!
//! * Holds are persisted in SQLite (`pos_holds`, migration `0017`) so
//!   parked carts survive edge restarts; quotes are persisted as
//!   `doc_type = 'quote'` transactions with no inventory movement and
//!   no outbox entries.
//! * Card authorizations auto-void via [`PaymentProvider::void`] if a
//!   split-tender authorization fails mid-checkout or if the local
//!   SQLite commit fails, preventing orphaned charges.
//! * Returns against card sales refund the original card reference(s)
//!   via [`PaymentProvider::refund`] and persist negative refund
//!   payment rows; operator voids void original card references when
//!   a payment provider is configured.
//! * Authenticated HTTP counter routes stamp the signed-in cashier's
//!   `user_id` onto sales, returns, quotes, layaways, and holds.
//! * A return validates the returned quantity against the original
//!   line quantity, not against quantities already returned by
//!   earlier returns of the same sale.

use std::sync::{Arc, RwLock};

use chrono::Utc;
use df_db::models::{Organization, Store};
use df_db::{AnyPool, DbError};
use df_domain::{Currency, Money};
use df_reports::ProductThreshold;
use df_sync::{MemoryHq, RemoteHq};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::error::EdgeError;
use crate::hw::{PaymentSummary, Printer, ReceiptBuilder, ReceiptData, drawer_kick_bytes};
use crate::payments::{MockTerminal, PaymentProvider, PaymentResult};
use crate::pricing::{PricingContext, price_lines};
use crate::types::{EdgeConfig, PricePreview, PricePreviewLine, SaleInput, SaleLineInput};

/// Re-exported so callers use one path for the till row type.
pub use df_db::models::TillSession;

mod checkout;
mod holds_quotes;
mod returns;
mod sync;
mod tills;

use checkout::resolve_tenders;

fn money_add(a: Money, b: Money) -> Result<Money, EdgeError> {
    Ok(a.checked_add(b)?)
}

/// Normalise a jurisdiction code for the store seed: trimmed upper
/// case, or blank when nothing was configured (`None` / empty).
fn normalise_code(code: Option<&str>) -> String {
    code.map(str::trim)
        .filter(|code| !code.is_empty())
        .map(str::to_uppercase)
        .unwrap_or_default()
}

/// The edge daemon. Cheaply cloneable — all shared state lives
/// behind `Arc`s — so the HTTP router can hold a clone.
///
/// `Debug` is manual: `dyn Printer` / `dyn PaymentProvider` do not
/// implement it.
#[derive(Clone)]
pub struct Edge {
    config: EdgeConfig,
    pool: AnyPool,
    printer: Arc<RwLock<Option<Arc<dyn Printer>>>>,
    provider: Arc<RwLock<Option<Arc<dyn PaymentProvider>>>>,
    hq: Arc<RwLock<Option<Arc<MemoryHq>>>>,
    /// Remote HQ push target (`POST {base_url}/api/v1/sync/ingest`),
    /// set from `DF_HQ_URL` + `DF_HQ_TOKEN` in [`Edge::open`] or
    /// with [`Edge::set_remote_hq`]. Preferred over the in-process
    /// `hq` by [`Edge::sync_now_with_configured_hq`].
    remote_hq: Arc<RwLock<Option<RemoteHq>>>,
    events: broadcast::Sender<String>,
    /// Operator-configured product min levels for the low-stock
    /// report (no min-level table exists yet — see `df-reports`).
    product_thresholds: Arc<RwLock<Vec<ProductThreshold>>>,
    /// Directory the scheduled-report-email job (Phase 6) writes
    /// rendered reports + `manifest.json` into, and the `/bi`
    /// outbox endpoint reads. `None` until
    /// [`Edge::set_email_outbox_dir`] is called.
    email_outbox_dir: Arc<RwLock<Option<std::path::PathBuf>>>,
}

impl std::fmt::Debug for Edge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Edge")
            .field("store_id", &self.config.store_id)
            .field("org_id", &self.config.org_id)
            // Redacted: a PostgreSQL URL carries the password.
            .field("db_url", &df_db::redact_db_url(&self.config.db_url))
            .field("store_name", &self.config.store_name)
            .field("currency", &self.config.currency)
            // The token itself is never logged (RemoteHq redacts it
            // in its own Debug); only whether one is configured.
            .field(
                "remote_hq_configured",
                &self.remote_hq.read().expect("remote hq lock").is_some(),
            )
            .finish_non_exhaustive()
    }
}

impl Edge {
    /// Connect to the SQLite database at `config.db_url` and run the
    /// SQLite migrations. Seeds nothing.
    ///
    /// Remote HQ: when both `DF_HQ_URL` and `DF_HQ_TOKEN` are set
    /// (non-empty), the edge pushes its outbox to that HQ
    /// (`POST {DF_HQ_URL}/api/v1/sync/ingest`, with a store-scoped
    /// `dfs_` service token or staff session token with `SyncIngest`).
    /// Either variable missing means no remote HQ; the in-process sink
    /// ([`Edge::set_hq`]) still works for offline tests. The token is
    /// held in memory only, never logged.
    pub async fn open(config: EdgeConfig) -> Result<Self, EdgeError> {
        let pool = df_db::connect(&config.db_url).await?;
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await?;
        // Seed the four price levels + their matrix rules
        // (idempotent — see `df_db::ensure_price_levels`); a fresh
        // edge DB has no levels until this runs.
        df_db::ensure_price_levels(&pool, config.org_id).await?;
        let (events, _) = broadcast::channel(64);
        let remote_hq = match (std::env::var("DF_HQ_URL"), std::env::var("DF_HQ_TOKEN")) {
            (Ok(url), Ok(token)) if !url.trim().is_empty() && !token.is_empty() => {
                Some(RemoteHq::new(url, token))
            }
            _ => None,
        };
        Ok(Self {
            config,
            pool,
            printer: Arc::new(RwLock::new(None)),
            provider: Arc::new(RwLock::new(None)),
            hq: Arc::new(RwLock::new(None)),
            remote_hq: Arc::new(RwLock::new(remote_hq)),
            events,
            product_thresholds: Arc::new(RwLock::new(Vec::new())),
            email_outbox_dir: Arc::new(RwLock::new(None)),
        })
    }

    /// Ensure the configured org + store rows exist, creating them
    /// with placeholder details when a fresh database has none.
    ///
    /// `Edge::open` seeds nothing, so a first-boot database has an
    /// empty `stores` table and any flow that validates the store
    /// (e.g. the GUI product import's `get_store` check) fails with
    /// "row not found". The server calls this once at startup; it is
    /// a no-op when the rows already exist, and it never overwrites
    /// existing details.
    ///
    /// `country_code` / `region_code` are the store's jurisdiction
    /// (e.g. `"CA"` / `"BC"`), which is what tax and eco fees price
    /// from — see `PricingContext::load`. Pass `None` to leave them
    /// blank (tax then computes as 0 until the row is fixed); a
    /// blank seed is only correct for a store whose jurisdiction is
    /// genuinely not configured yet. The values are normalised to
    /// trimmed upper case (`"ca"` → `"CA"`) because the tax presets
    /// match on exact codes.
    /// Returns true when it created the store row.
    pub async fn ensure_configured_store(
        &self,
        country_code: Option<&str>,
        region_code: Option<&str>,
    ) -> Result<bool, EdgeError> {
        let org_id = self.config.org_id;
        match df_db::get_organization(self.pool(), org_id).await {
            Ok(_) => {}
            Err(DbError::NotFound(_)) => {
                df_db::insert_organization(
                    self.pool(),
                    &Organization {
                        id: org_id,
                        name: self.config.store_name.clone(),
                        default_currency: self.config.currency.code().to_string(),
                        enabled_modules: serde_json::json!({"core_pos": true}),
                        settings_json: serde_json::json!({}),
                        created_at: Utc::now(),
                    },
                )
                .await?;
            }
            Err(err) => return Err(err.into()),
        }
        match df_db::get_store(self.pool(), self.config.store_id).await {
            Ok(_) => Ok(false),
            Err(DbError::NotFound(_)) => {
                df_db::insert_store(
                    self.pool(),
                    &Store {
                        id: self.config.store_id,
                        org_id,
                        code: "MAIN".to_string(),
                        name: self.config.store_name.clone(),
                        country_code: normalise_code(country_code),
                        region_code: normalise_code(region_code),
                        postal_code: String::new(),
                        currency: self.config.currency.code().to_string(),
                        tax_zone_id: Uuid::now_v7(),
                        timezone: "UTC".to_string(),
                        is_warehouse: false,
                    },
                )
                .await?;
                Ok(true)
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Set the directory scheduled report emails queue into (see
    /// `df-jobs` Phase 6). The `/bi/email-outbox` endpoint reads
    /// the same directory's `manifest.json`.
    pub fn set_email_outbox_dir(&self, dir: std::path::PathBuf) {
        *self.email_outbox_dir.write().expect("outbox dir lock") = Some(dir);
    }

    /// The configured email outbox directory, if any.
    pub(crate) fn email_outbox_dir(&self) -> Option<std::path::PathBuf> {
        self.email_outbox_dir
            .read()
            .expect("outbox dir lock")
            .clone()
    }

    pub fn config(&self) -> &EdgeConfig {
        &self.config
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// The operator-configured product thresholds for the low-stock
    /// report (see [`crate::tinting::Edge::set_product_thresholds`]).
    pub(crate) fn product_thresholds(&self) -> &Arc<RwLock<Vec<ProductThreshold>>> {
        &self.product_thresholds
    }

    /// Subscribe to sale/return events (JSON strings) broadcast after
    /// each completed transaction.
    pub fn subscribe_events(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    /// Set the receipt printer. A checkout with no printer still
    /// succeeds and records that printing was skipped.
    pub fn set_printer(&self, printer: Arc<dyn Printer>) {
        *self.printer.write().expect("printer lock") = Some(printer);
    }

    /// Set the default payment provider (used by the HTTP checkout
    /// endpoint and by `df-server edge`, which installs the mock
    /// provider). [`Edge::checkout`] takes its provider as a
    /// parameter instead.
    pub fn set_payment_provider(&self, provider: Arc<dyn PaymentProvider>) {
        *self.provider.write().expect("provider lock") = Some(provider);
    }

    /// Set the in-process HQ sink used by `POST /pos/sync/run`.
    pub fn set_hq(&self, hq: Arc<MemoryHq>) {
        *self.hq.write().expect("hq lock") = Some(hq);
    }

    /// Set the remote HQ push target (`POST {base_url}/api/v1/sync/ingest`
    /// with a staff session `Bearer` token holding `SyncIngest`).
    /// Preferred over the in-process sink by
    /// [`Edge::sync_now_with_configured_hq`]. The token is held in
    /// memory only, never logged.
    pub fn set_remote_hq(&self, base_url: impl Into<String>, token: impl Into<String>) {
        *self.remote_hq.write().expect("remote hq lock") = Some(RemoteHq::new(base_url, token));
    }

    /// Whether a remote HQ push target is configured.
    pub fn remote_hq_configured(&self) -> bool {
        self.remote_hq.read().expect("remote hq lock").is_some()
    }

    fn printer(&self) -> Option<Arc<dyn Printer>> {
        self.printer.read().expect("printer lock").clone()
    }

    pub(crate) fn default_provider(&self) -> Option<Arc<dyn PaymentProvider>> {
        self.provider.read().expect("provider lock").clone()
    }

    fn currency(&self) -> Currency {
        self.config.currency
    }

    // --- pricing ------------------------------------------------------------

    /// The price level a sale prices at: the sale's own level when
    /// given, else the customer's stored level (when the sale has a
    /// customer), else list pricing (`None`).
    pub async fn effective_price_level(&self, input: &SaleInput) -> Option<Uuid> {
        if input.price_level_id.is_some() {
            return input.price_level_id;
        }
        match input.customer_id {
            Some(customer_id) => df_db::get_customer(&self.pool, customer_id)
                .await
                .ok()
                .and_then(|customer| customer.price_level_id),
            None => None,
        }
    }

    /// Totals for a cart without persisting anything.
    pub async fn price_preview(&self, input: &SaleInput) -> Result<PricePreview, EdgeError> {
        self.validate_lines(&input.lines)?;
        let ctx = PricingContext::load(
            &self.pool,
            self.config.org_id,
            self.config.store_id,
            self.currency(),
        )
        .await;
        let price_level_id = self.effective_price_level(input).await;
        let priced = price_lines(
            &self.pool,
            &ctx,
            input.customer_id,
            input.job_id,
            price_level_id,
            &input.lines,
        )
        .await?;
        let pre_surcharge = priced
            .subtotal
            .checked_add(priced.tax_total)?
            .checked_add(priced.eco_fee_total)?;
        let (resolved, _) = if input.tenders.is_empty() {
            (Vec::new(), Money::zero(self.currency()))
        } else {
            resolve_tenders(&input.tenders, pre_surcharge, &self.config.surcharge_rules)?
        };
        let pos_fee_total = resolved
            .iter()
            .map(|t| t.surcharge)
            .fold(Money::zero(self.currency()), |acc, s| {
                acc.checked_add(s).unwrap_or(acc)
            });
        let grand_total = pre_surcharge.checked_add(pos_fee_total)?;
        Ok(PricePreview {
            lines: priced
                .lines
                .iter()
                .map(|l| PricePreviewLine {
                    product_id: l.product_id,
                    description: l.description.clone(),
                    qty: l.qty,
                    unit_price: l.unit_price,
                    line_total: l.line_total,
                    tax_amount: l.tax_amount,
                    eco_fee_amount: l.eco_fee_amount,
                })
                .collect(),
            subtotal: priced.subtotal,
            tax_total: priced.tax_total,
            eco_fee_total: priced.eco_fee_total,
            pos_fee_total,
            grand_total,
        })
    }

    fn validate_lines(&self, lines: &[SaleLineInput]) -> Result<(), EdgeError> {
        if lines.is_empty() {
            return Err(EdgeError::InvalidInput("sale has no lines".to_string()));
        }
        for line in lines {
            if line.qty <= Decimal::ZERO {
                return Err(EdgeError::InvalidInput(format!(
                    "line qty must be positive: {}",
                    line.qty
                )));
            }
            if line.unit_price.currency() != self.currency()
                || line.unit_cost.currency() != self.currency()
            {
                return Err(EdgeError::InvalidInput(
                    "line prices must use the store currency".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// on `df-payments`, so this mapping lives here, at the boundary.
    fn payment_summary(result: &PaymentResult) -> PaymentSummary {
        PaymentSummary {
            tender_label: result.tender.label().to_string(),
            amount: result.amount,
        }
    }

    /// Print a receipt if a printer is configured; returns whether it
    /// printed. Never fails the sale: a missing or failing printer
    /// just records `false`. `kick_drawer` appends the ESC/POS
    /// drawer-kick bytes (cash sales).
    fn print_receipt(&self, receipt: &ReceiptData, kick_drawer: bool) -> bool {
        let Some(printer) = self.printer() else {
            return false;
        };
        let mut bytes = ReceiptBuilder.receipt_bytes(receipt);
        if kick_drawer {
            bytes.extend_from_slice(&drawer_kick_bytes());
        }
        printer.print(&bytes).is_ok()
    }

    /// Provider for an HTTP call: the configured default provider, or
    /// a local mock when no card tender is involved (card
    /// authorization is never reached then). A card tender with no
    /// provider configured is an error.
    fn http_provider(&self, has_card_tender: bool) -> Result<Arc<dyn PaymentProvider>, EdgeError> {
        if let Some(provider) = self.default_provider() {
            return Ok(provider);
        }
        if has_card_tender {
            return Err(EdgeError::NoPaymentProvider(
                "no payment provider configured on this edge".to_string(),
            ));
        }
        Ok(Arc::new(MockTerminal::new()))
    }

    // --- serving ------------------------------------------------------------

    /// The local HTTP API the POS talks to (see [`crate::http`]), with
    /// default HTTP hardening (auth rate limiting, body limit, security
    /// headers, fail-closed CORS).
    pub fn router(&self) -> axum::Router {
        crate::http::router(self.clone())
    }

    /// Build the local HTTP API router with explicit HTTP hardening
    /// settings (CORS origin allowlist, auth rate-limit window, body cap).
    pub fn router_with_hardening(
        &self,
        hardening: df_auth_http::HttpHardeningConfig,
    ) -> axum::Router {
        crate::http::router_with_hardening(self.clone(), hardening)
    }

    /// Serve the router on `addr` until cancelled, reading hardening
    /// settings from the environment (`DF_CORS_ORIGINS`, etc.).
    pub async fn serve(&self, addr: std::net::SocketAddr) -> Result<(), EdgeError> {
        self.serve_with_hardening(addr, df_auth_http::HttpHardeningConfig::from_env())
            .await
    }

    /// Serve the router on `addr` with explicit HTTP hardening settings
    /// and per-connection `ConnectInfo<SocketAddr>` for IP rate limiting.
    pub async fn serve_with_hardening(
        &self,
        addr: std::net::SocketAddr,
        hardening: df_auth_http::HttpHardeningConfig,
    ) -> Result<(), EdgeError> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(
            listener,
            self.router_with_hardening(hardening)
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await?;
        Ok(())
    }
}

/// Test helpers: an edge backed by in-memory SQLite, with an org,
/// store and product row seeded. Used by the `http` tests too.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::payments::TenderKind;
    use crate::types::TenderInput;
    use df_db::models::{Organization, Product, Store};
    use std::sync::Mutex;

    pub(crate) fn test_config() -> EdgeConfig {
        EdgeConfig {
            store_id: Uuid::from_u128(0x1000),
            org_id: Uuid::from_u128(0x2000),
            db_url: "sqlite::memory:".to_string(),
            store_name: "Test Paint Co".to_string(),
            currency: Currency::Usd,
            surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
        }
    }

    pub(crate) async fn test_edge() -> Edge {
        let config = test_config();
        let edge = Edge::open(config).await.unwrap();
        let org = Organization {
            id: Uuid::from_u128(0x2000),
            name: "Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({"core_pos": true}),
            settings_json: serde_json::json!({}),
            created_at: Utc::now(),
        };
        df_db::insert_organization(edge.pool(), &org).await.unwrap();
        let store = Store {
            id: Uuid::from_u128(0x1000),
            org_id: org.id,
            code: "MAIN".to_string(),
            name: "Main Store".to_string(),
            country_code: "US".to_string(),
            region_code: "CA".to_string(),
            postal_code: "90210".to_string(),
            currency: "USD".to_string(),
            tax_zone_id: Uuid::now_v7(),
            timezone: "America/Los_Angeles".to_string(),
            is_warehouse: false,
        };
        df_db::insert_store(edge.pool(), &store).await.unwrap();
        edge
    }

    /// A 1-gallon architectural-paint product: list $100, cost $50,
    /// 3785 ml (PaintCare CA $0.75/container band).
    pub(crate) async fn seed_product(edge: &Edge) -> Uuid {
        let product_id = Uuid::from_u128(0x3000);
        let product = Product {
            id: product_id,
            org_id: Uuid::from_u128(0x2000),
            sku: "BM-REGAL-1G".to_string(),
            manufacturer_code: Some("BM".to_string()),
            name: "Regal Select Interior Gallon".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: Money::from_units(100, Currency::Usd).amount(),
            standard_cost: Money::from_units(50, Currency::Usd).amount(),
            avg_cost: Money::from_units(50, Currency::Usd).amount(),
            min_margin_pct: None,
            sheen: Some("Eggshell".to_string()),
            paint_base_code: Some("1X".to_string()),
            container_volume_ml: Some(3785),
            eco_fee_category: Some("architectural_paint".to_string()),
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            is_active: true,
            map: None,
            msrp: None,
        };
        df_db::insert_product(edge.pool(), &product).await.unwrap();
        product_id
    }

    pub(crate) fn sale_input(product_id: Uuid, qty: &str, tender: TenderKind) -> SaleInput {
        SaleInput {
            customer_id: None,
            job_id: None,
            price_level_id: None,
            job_name: None,
            lines: vec![SaleLineInput {
                product_id,
                sku: "BM-REGAL-1G".to_string(),
                description: "Regal Select Interior Gallon".to_string(),
                qty: Decimal::from_str_exact(qty).unwrap(),
                uom: "gal".to_string(),
                unit_price: Money::from_units(100, Currency::Usd),
                unit_cost: Money::from_units(50, Currency::Usd),
            }],
            tenders: vec![TenderInput {
                kind: tender,
                amount: None,
                reference: None,
            }],
            po_number: None,
        }
    }

    /// A printer that captures every print into memory.
    #[derive(Debug, Default)]
    pub(crate) struct CapturingPrinter {
        pub(crate) prints: Mutex<Vec<Vec<u8>>>,
    }

    impl Printer for CapturingPrinter {
        fn print(&self, bytes: &[u8]) -> Result<(), crate::hw::HwError> {
            self.prints.lock().expect("print lock").push(bytes.to_vec());
            Ok(())
        }

        fn name(&self) -> &str {
            "capturing-test-printer"
        }
    }
}
