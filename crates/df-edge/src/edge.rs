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
//! Phase 1 limitations, stated plainly:
//!
//! * Returns do not call the payment provider for a refund; the
//!   reversal is local (inventory + AR + outbox) and the refund must
//!   be run on the terminal. A provider refund is a Phase 2 item.
//! * A return validates the returned quantity against the original
//!   line quantity, not against quantities already returned by
//!   earlier returns of the same sale.
//! * Holds live in memory (lost on restart); quotes are persisted as
//!   `doc_type = 'quote'` transactions with no inventory movement and
//!   no outbox entries.
//! * If card authorization succeeds but the local commit then fails,
//!   the authorization is not voided automatically (no compensation
//!   yet); the orphan authorization must be voided on the terminal.
//! * The Z-report's `expected_cash` counts cash payments on sales
//!   only; cash handed back on a return is not tracked as a payment
//!   (returns carry no payment rows in Phase 1).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use chrono::Utc;
use df_db::models::{
    ArLedgerEntry, InventoryLedgerEntry, PaymentRow, Transaction, TransactionLine,
};
use df_db::{AnyPool, DbError};
use df_domain::{Currency, Money};
use df_sync::{ApplyReport, DeltaBatch, HqSink, MemoryHq, Outbox, OutboxEntry, SyncState};
use rust_decimal::Decimal;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::error::EdgeError;
use crate::hw::{
    PaymentSummary, Printer, ReceiptBuilder, ReceiptData, ReceiptLine, drawer_kick_bytes,
};
use crate::payments::{
    MockTerminal, PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus, TenderKind,
    compute_surcharge, surcharge_rule_for,
};
use crate::pricing::{PricedLine, PricingContext, price_lines, tax_and_eco_for};
use crate::types::{
    CompletedSale, EdgeConfig, PricePreview, PricePreviewLine, ReturnLineInput, SaleInput,
    SaleLineInput, TenderInput, ZReport,
};
use df_payments::SurchargeRule;
use df_reports::ProductThreshold;
use df_tinter::VirtualTinter;

/// Re-exported so callers use one path for the till row type.
pub use df_db::models::TillSession;

/// One resolved tender: what the customer owes on it, the surcharge
/// (card only) and the total actually collected.
#[derive(Debug, Clone, PartialEq)]
struct ResolvedTender {
    kind: TenderKind,
    /// Portion of the pre-surcharge total this tender covers.
    tendered: Money,
    surcharge: Money,
    /// Total collected (`tendered + surcharge`).
    charged: Money,
}

/// Resolve tender amounts against a pre-surcharge total.
///
/// `None` amounts mean "the remaining balance". Cash may exceed the
/// remainder (the excess is change due); card / charge / gift-card
/// tenders may not exceed it. Under-tendering is an error. A tender's
/// surcharge comes from `rules` via
/// [`df_payments::compute_surcharge`] (default rules: 3% on card
/// only).
fn resolve_tenders(
    tenders: &[TenderInput],
    pre_surcharge_total: Money,
    rules: &[SurchargeRule],
) -> Result<(Vec<ResolvedTender>, Money), EdgeError> {
    let currency = pre_surcharge_total.currency();
    let zero = Money::zero(currency);
    let mut remaining = pre_surcharge_total;
    let mut resolved = Vec::with_capacity(tenders.len());
    for tender in tenders {
        let amount = match tender.amount {
            Some(amount) => amount,
            None => {
                if remaining.is_negative() {
                    zero
                } else {
                    remaining
                }
            }
        };
        if amount.is_negative() {
            return Err(EdgeError::InvalidInput(format!(
                "tender amount must not be negative: {}",
                amount.amount()
            )));
        }
        if tender.kind != TenderKind::Cash && amount.amount() > remaining.amount() {
            return Err(EdgeError::InvalidInput(format!(
                "{:?} tender {} exceeds the remaining balance {}",
                tender.kind,
                amount.amount(),
                remaining.amount()
            )));
        }
        let surcharge = compute_surcharge(surcharge_rule_for(tender.kind, rules), amount);
        let charged = amount.checked_add(surcharge)?;
        remaining = remaining.checked_sub(amount)?;
        resolved.push(ResolvedTender {
            kind: tender.kind,
            tendered: amount,
            surcharge,
            charged,
        });
    }
    if remaining.amount() > Decimal::ZERO {
        return Err(EdgeError::InvalidInput(format!(
            "tenders cover {} but the total is {}; {} is still owed",
            pre_surcharge_total
                .checked_sub(remaining)
                .map(|m| m.amount())
                .unwrap_or_default(),
            pre_surcharge_total.amount(),
            remaining.amount()
        )));
    }
    let change_due = if remaining.is_negative() {
        Money::new(-remaining.amount(), currency)
    } else {
        zero
    };
    Ok((resolved, change_due))
}

fn money_add(a: Money, b: Money) -> Result<Money, EdgeError> {
    Ok(a.checked_add(b)?)
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
    holds: Arc<Mutex<HashMap<Uuid, SaleInput>>>,
    events: broadcast::Sender<String>,
    /// The hosted in-process virtual tinter (Phase 2). Its levels
    /// are re-synced from the database on every dispense; the
    /// database is the source of truth.
    tinter: Arc<Mutex<VirtualTinter>>,
    /// Operator-configured product min levels for the low-stock
    /// report (no min-level table exists yet — see `df-reports`).
    product_thresholds: Arc<RwLock<Vec<ProductThreshold>>>,
}

impl std::fmt::Debug for Edge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Edge")
            .field("store_id", &self.config.store_id)
            .field("org_id", &self.config.org_id)
            .field("db_url", &self.config.db_url)
            .field("store_name", &self.config.store_name)
            .field("currency", &self.config.currency)
            .finish_non_exhaustive()
    }
}

impl Edge {
    /// Connect to the SQLite database at `config.db_url` and run the
    /// SQLite migrations. Seeds nothing.
    pub async fn open(config: EdgeConfig) -> Result<Self, EdgeError> {
        let pool = df_db::connect(&config.db_url).await?;
        df_db::migrate(&pool, df_db::Dialect::Sqlite).await?;
        let (events, _) = broadcast::channel(64);
        Ok(Self {
            config,
            pool,
            printer: Arc::new(RwLock::new(None)),
            provider: Arc::new(RwLock::new(None)),
            hq: Arc::new(RwLock::new(None)),
            holds: Arc::new(Mutex::new(HashMap::new())),
            events,
            tinter: Arc::new(Mutex::new(VirtualTinter::new("virtual-simulator"))),
            product_thresholds: Arc::new(RwLock::new(Vec::new())),
        })
    }

    pub fn config(&self) -> &EdgeConfig {
        &self.config
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// The hosted virtual tinter (shared, behind a lock).
    pub(crate) fn tinter(&self) -> &Arc<Mutex<VirtualTinter>> {
        &self.tinter
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

    fn printer(&self) -> Option<Arc<dyn Printer>> {
        self.printer.read().expect("printer lock").clone()
    }

    fn default_provider(&self) -> Option<Arc<dyn PaymentProvider>> {
        self.provider.read().expect("provider lock").clone()
    }

    fn currency(&self) -> Currency {
        self.config.currency
    }

    // --- tills --------------------------------------------------------------

    pub async fn open_till(
        &self,
        cashier_id: Uuid,
        opening_float: Money,
    ) -> Result<TillSession, EdgeError> {
        if opening_float.is_negative() {
            return Err(EdgeError::InvalidInput(
                "opening float must not be negative".to_string(),
            ));
        }
        if df_db::open_till_for_store(&self.pool, self.config.store_id)
            .await?
            .is_some()
        {
            return Err(EdgeError::Till(
                "a till is already open for this store".to_string(),
            ));
        }
        let till = TillSession {
            id: Uuid::now_v7(),
            org_id: self.config.org_id,
            store_id: self.config.store_id,
            cashier_id,
            opened_at: Utc::now(),
            closed_at: None,
            opening_float: opening_float.amount(),
            counted_cash: None,
            expected_cash: None,
            status: "open".to_string(),
        };
        df_db::insert_till_session(&self.pool, &till).await?;
        Ok(till)
    }

    pub async fn close_till(
        &self,
        till_id: Uuid,
        counted_cash: Money,
    ) -> Result<ZReport, EdgeError> {
        let till = df_db::get_till_session(&self.pool, till_id).await?;
        if till.status != "open" {
            return Err(EdgeError::Till(format!("till {till_id} is not open")));
        }
        let currency = self.currency();
        let zero = Money::zero(currency);
        let transactions = df_db::list_transactions_for_till(&self.pool, till_id).await?;
        let mut sale_count = 0u32;
        let mut gross_sales = zero;
        let mut tax_total = zero;
        let mut cash_sales = zero;
        let mut card_sales = zero;
        let mut charge_sales = zero;
        let mut return_count = 0u32;
        let mut return_total = zero;
        for tx in &transactions {
            if tx.status != "completed" {
                continue;
            }
            match tx.doc_type.as_str() {
                "sale" => {
                    sale_count += 1;
                    gross_sales = money_add(gross_sales, Money::new(tx.grand_total, currency))?;
                    tax_total = money_add(tax_total, Money::new(tx.tax_total, currency))?;
                    for payment in df_db::list_payments_for_transaction(&self.pool, tx.id).await? {
                        match payment.tender.as_str() {
                            "cash" => {
                                cash_sales =
                                    money_add(cash_sales, Money::new(payment.amount, currency))?
                            }
                            "card" => {
                                card_sales =
                                    money_add(card_sales, Money::new(payment.amount, currency))?
                            }
                            "charge_account" => {
                                charge_sales =
                                    money_add(charge_sales, Money::new(payment.amount, currency))?;
                            }
                            _ => {}
                        }
                    }
                }
                "return" => {
                    return_count += 1;
                    return_total = money_add(return_total, Money::new(tx.grand_total, currency))?;
                }
                _ => {}
            }
        }
        let expected_cash = money_add(Money::new(till.opening_float, currency), cash_sales)?;
        let over_short = counted_cash.checked_sub(expected_cash)?;
        let closed_at = Utc::now();
        df_db::close_till_session(
            &self.pool,
            till_id,
            closed_at,
            counted_cash.amount(),
            expected_cash.amount(),
        )
        .await?;
        Ok(ZReport {
            till_id,
            opened_at: till.opened_at,
            closed_at,
            expected_cash,
            counted_cash,
            over_short,
            sale_count,
            gross_sales,
            tax_total,
            cash_sales,
            card_sales,
            charge_sales,
            return_count,
            return_total,
        })
    }

    // --- pricing ------------------------------------------------------------

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
        let priced = price_lines(
            &self.pool,
            &ctx,
            input.customer_id,
            input.job_id,
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

    // --- checkout -----------------------------------------------------------

    /// Complete a sale offline: price, authorize card tenders, then
    /// write transaction + lines + inventory deltas + AR entry (charge
    /// account) + payment rows + outbox entries in one local SQLite
    /// transaction. Prints the receipt (best-effort) and kicks the
    /// drawer when cash was taken.
    pub async fn checkout(
        &self,
        input: SaleInput,
        provider: &dyn PaymentProvider,
    ) -> Result<CompletedSale, EdgeError> {
        self.validate_lines(&input.lines)?;
        if input.tenders.is_empty() {
            return Err(EdgeError::InvalidInput("sale has no tenders".to_string()));
        }
        if input
            .tenders
            .iter()
            .any(|t| t.kind == TenderKind::ChargeAccount)
            && input.customer_id.is_none()
        {
            return Err(EdgeError::InvalidInput(
                "charge-account tender requires a customer".to_string(),
            ));
        }
        let ctx = PricingContext::load(
            &self.pool,
            self.config.org_id,
            self.config.store_id,
            self.currency(),
        )
        .await;
        let priced = price_lines(
            &self.pool,
            &ctx,
            input.customer_id,
            input.job_id,
            &input.lines,
        )
        .await?;
        let pre_surcharge = priced
            .subtotal
            .checked_add(priced.tax_total)?
            .checked_add(priced.eco_fee_total)?;
        let (resolved, change_due) =
            resolve_tenders(&input.tenders, pre_surcharge, &self.config.surcharge_rules)?;
        let pos_fee_total = resolved
            .iter()
            .map(|t| t.surcharge)
            .fold(Money::zero(self.currency()), |acc, s| {
                acc.checked_add(s).unwrap_or(acc)
            });
        let grand_total = pre_surcharge.checked_add(pos_fee_total)?;

        // Charge-account credit check (Phase 3): the charge
        // portion is the tendered amount on ChargeAccount tenders
        // (card surcharges never land on AR). Runs before any
        // authorization or write, so a blocked charge leaves
        // nothing behind. Returns the due date for the AR entry
        // (sale date + the account's terms), or None when no
        // charge is being made / no credit record has synced.
        let charge_total = resolved
            .iter()
            .filter(|t| t.kind == TenderKind::ChargeAccount)
            .map(|t| t.tendered)
            .fold(Money::zero(self.currency()), |acc, m| {
                acc.checked_add(m).unwrap_or(acc)
            });
        let charge_due_date = self
            .check_charge_credit(input.customer_id, charge_total)
            .await?;

        // Authorize card tenders before writing anything.
        let mut payments: Vec<PaymentResult> = Vec::with_capacity(resolved.len());
        for tender in &resolved {
            match tender.kind {
                TenderKind::Card => {
                    let result = provider
                        .authorize(&PaymentRequest {
                            amount: tender.charged,
                            idempotency_key: Uuid::now_v7(),
                            tender: TenderKind::Card,
                            surcharge: tender.surcharge,
                        })
                        .await?;
                    if result.status != PaymentStatus::Approved {
                        return Err(EdgeError::PaymentDeclined(format!(
                            "card authorization declined (ref {})",
                            result.reference
                        )));
                    }
                    payments.push(result);
                }
                TenderKind::Cash => payments.push(PaymentResult {
                    status: PaymentStatus::Approved,
                    authorization: None,
                    reference: "CASH".to_string(),
                    amount: tender.charged,
                    surcharge: tender.surcharge,
                    tender: TenderKind::Cash,
                }),
                TenderKind::ChargeAccount => payments.push(PaymentResult {
                    status: PaymentStatus::Approved,
                    authorization: None,
                    reference: format!(
                        "CHARGE-{}",
                        input
                            .customer_id
                            .map(|id| id.to_string())
                            .unwrap_or_default()
                    ),
                    amount: tender.charged,
                    surcharge: tender.surcharge,
                    tender: TenderKind::ChargeAccount,
                }),
                TenderKind::GiftCard => payments.push(PaymentResult {
                    status: PaymentStatus::Approved,
                    authorization: None,
                    reference: "GIFT-CARD".to_string(),
                    amount: tender.charged,
                    surcharge: tender.surcharge,
                    tender: TenderKind::GiftCard,
                }),
            }
        }

        let tx_id = Uuid::now_v7();
        let now = Utc::now();
        let till = df_db::open_till_for_store(&self.pool, self.config.store_id).await?;
        let doc_number = format!(
            "TX-{:06}",
            df_db::count_transactions_for_store(&self.pool, self.config.store_id).await? + 1
        );
        let transaction = Transaction {
            id: tx_id,
            org_id: self.config.org_id,
            store_id: self.config.store_id,
            till_session_id: till.as_ref().map(|t| t.id),
            doc_number: doc_number.clone(),
            doc_type: "sale".to_string(),
            status: "completed".to_string(),
            customer_id: input.customer_id,
            job_id: input.job_id,
            po_number: input.po_number.clone(),
            salesperson_id: None,
            cashier_id: till.as_ref().map(|t| t.cashier_id),
            currency: self.currency().code().to_string(),
            subtotal: priced.subtotal.amount(),
            tax_total: priced.tax_total.amount(),
            eco_fee_total: priced.eco_fee_total.amount(),
            pos_fee_total: pos_fee_total.amount(),
            grand_total: grand_total.amount(),
            deposit_paid: Money::zero(self.currency()).amount(),
            fulfillment_mode: "counter".to_string(),
            scheduled_at: None,
            completed_at: Some(now),
            created_at: now,
        };

        let mut db_tx = self.pool.begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_transaction(&mut *db_tx, &transaction).await?;
        let mut outbox_entries: Vec<OutboxEntry> = Vec::new();
        for (index, line) in priced.lines.iter().enumerate() {
            let line_id = Uuid::now_v7();
            df_db::insert_transaction_line(
                &mut *db_tx,
                &TransactionLine {
                    id: line_id,
                    transaction_id: tx_id,
                    line_no: (index + 1) as i32,
                    product_id: line.product_id,
                    description: line.description.clone(),
                    qty: line.qty,
                    uom_code: line.uom.clone(),
                    unit_price: line.unit_price.amount(),
                    base_unit_cost: line.unit_cost.amount(),
                    tint_unit_cost: Money::zero(self.currency()).amount(),
                    discount_amount: Money::zero(self.currency()).amount(),
                    tax_amount: line.tax_amount.amount(),
                    eco_fee_amount: line.eco_fee_amount.amount(),
                    line_total: line.line_total.amount(),
                    tint_formula_id: None,
                    prompts_json: serde_json::json!({}),
                    vertical_spec_json: serde_json::json!({}),
                    linked_po_line_id: None,
                    fulfilled_qty: line.qty,
                },
            )
            .await?;
            let delta_id = Uuid::now_v7();
            df_db::insert_inventory_delta(
                &mut *db_tx,
                &InventoryLedgerEntry {
                    id: delta_id,
                    org_id: self.config.org_id,
                    store_id: self.config.store_id,
                    product_id: line.product_id,
                    lot_number: None,
                    delta_on_hand: -line.qty,
                    delta_committed: Decimal::ZERO,
                    delta_on_order: Decimal::ZERO,
                    delta_in_transit: Decimal::ZERO,
                    unit_cost: line.unit_cost.amount(),
                    reason_type: "sale".to_string(),
                    reference_id: tx_id,
                    created_at: now,
                },
            )
            .await?;
            outbox_entries.push(OutboxEntry::new(
                self.config.store_id,
                "inventory_ledger",
                delta_id,
                serde_json::json!({
                    "product_id": line.product_id.to_string(),
                    "delta_on_hand": (-line.qty).to_string(),
                    "reason_type": "sale",
                    "reference_id": tx_id.to_string(),
                }),
            ));
        }

        // Charge-account AR entry (the charge portion only — card
        // surcharges never land on AR). `charge_total` and
        // `charge_due_date` were resolved (and credit-checked)
        // before authorization, above.
        if !charge_total.is_zero() {
            let customer_id = input.customer_id.expect("charge requires a customer");
            let ar_id = Uuid::now_v7();
            df_db::insert_ar_entry_exec(
                &mut *db_tx,
                &ArLedgerEntry {
                    id: ar_id,
                    org_id: self.config.org_id,
                    store_id: self.config.store_id,
                    customer_id,
                    job_id: input.job_id,
                    entry_type: "charge".to_string(),
                    reference_type: "transaction".to_string(),
                    reference_id: tx_id,
                    amount: charge_total.amount(),
                    due_date: charge_due_date,
                    statement_id: None,
                    created_at: now,
                },
            )
            .await?;
            outbox_entries.push(OutboxEntry::new(
                self.config.store_id,
                "ar_ledger",
                ar_id,
                serde_json::json!({
                    "customer_id": customer_id.to_string(),
                    "amount": charge_total.amount().to_string(),
                    "entry_type": "charge",
                    "reference_id": tx_id.to_string(),
                }),
            ));
        }

        for payment in &payments {
            df_db::insert_payment(
                &mut *db_tx,
                &PaymentRow {
                    id: Uuid::now_v7(),
                    transaction_id: tx_id,
                    tender: payment.tender.as_str().to_string(),
                    amount: payment.amount.amount(),
                    reference: payment.reference.clone(),
                    authorization: payment.authorization.clone(),
                    created_at: now,
                },
            )
            .await?;
        }

        outbox_entries.push(OutboxEntry::new(
            self.config.store_id,
            "transaction",
            tx_id,
            serde_json::json!({
                "id": tx_id.to_string(),
                "doc_number": doc_number,
                "doc_type": "sale",
                "status": "completed",
                "customer_id": input.customer_id.map(|id| id.to_string()),
                "subtotal": priced.subtotal.amount().to_string(),
                "tax_total": priced.tax_total.amount().to_string(),
                "eco_fee_total": priced.eco_fee_total.amount().to_string(),
                "pos_fee_total": pos_fee_total.amount().to_string(),
                "grand_total": grand_total.amount().to_string(),
            }),
        ));
        for entry in &outbox_entries {
            Outbox::enqueue_exec(&mut *db_tx, entry).await?;
        }
        db_tx.commit().await.map_err(DbError::Sqlx)?;

        // Best-effort receipt + drawer kick, after the commit.
        let kick_drawer = payments.iter().any(|p| p.tender == TenderKind::Cash);
        let receipt_printed = self.print_receipt(
            &ReceiptData {
                store_name: self.config.store_name.clone(),
                doc_number: doc_number.clone(),
                lines: priced
                    .lines
                    .iter()
                    .map(|l| ReceiptLine {
                        description: l.description.clone(),
                        qty: l.qty,
                        unit_price: l.unit_price,
                        line_total: l.line_total,
                    })
                    .collect(),
                subtotal: priced.subtotal,
                tax_total: priced.tax_total,
                eco_fee_total: priced.eco_fee_total,
                pos_fee_total,
                grand_total,
                payments: payments.iter().map(Self::payment_summary).collect(),
                change_due,
            },
            kick_drawer,
        );
        let _ = self.events.send(
            serde_json::json!({
                "type": "sale_completed",
                "store_id": self.config.store_id.to_string(),
                "transaction_id": tx_id.to_string(),
                "doc_number": doc_number,
                "doc_type": "sale",
                "grand_total": grand_total.amount().to_string(),
            })
            .to_string(),
        );
        Ok(CompletedSale {
            transaction_id: tx_id,
            doc_number,
            subtotal: priced.subtotal,
            tax_total: priced.tax_total,
            eco_fee_total: priced.eco_fee_total,
            pos_fee_total,
            grand_total,
            change_due,
            payments,
            receipt_printed,
        })
    }

    /// Map a [`PaymentResult`] to the receipt's [`PaymentSummary`]
    /// (tender label + amount). `df-hw` deliberately does not depend
    /// on `df-payments`, so this mapping lives here, at the boundary.
    fn payment_summary(result: &PaymentResult) -> PaymentSummary {
        let tender_label = match result.tender {
            TenderKind::Cash => "Cash",
            TenderKind::Card => "Card",
            TenderKind::ChargeAccount => "Charge Account",
            TenderKind::GiftCard => "Gift Card",
        }
        .to_string();
        PaymentSummary {
            tender_label,
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

    /// HTTP checkout: uses the configured default provider (or the
    /// mock provider installed by `df-server edge`). A card tender
    /// with no provider configured is a 503-style error here.
    pub async fn checkout_with_default_provider(
        &self,
        input: SaleInput,
    ) -> Result<CompletedSale, EdgeError> {
        let has_card = input.tenders.iter().any(|t| t.kind == TenderKind::Card);
        let provider = self.http_provider(has_card)?;
        self.checkout(input, provider.as_ref()).await
    }

    // --- returns ------------------------------------------------------------

    /// Record a return against a completed sale: negative-qty lines
    /// reversing inventory (and AR when the original was charged),
    /// with outbox entries. No provider refund is issued in Phase 1
    /// (see module docs).
    pub async fn record_return(
        &self,
        original_tx_id: Uuid,
        lines: Vec<ReturnLineInput>,
    ) -> Result<CompletedSale, EdgeError> {
        if lines.is_empty() {
            return Err(EdgeError::InvalidInput("return has no lines".to_string()));
        }
        let original = df_db::get_transaction(&self.pool, original_tx_id).await?;
        if original.doc_type != "sale" || original.status != "completed" {
            return Err(EdgeError::InvalidInput(format!(
                "transaction {original_tx_id} is not a completed sale"
            )));
        }
        let original_lines = df_db::list_transaction_lines(&self.pool, original_tx_id).await?;
        let ctx = PricingContext::load(
            &self.pool,
            self.config.org_id,
            self.config.store_id,
            self.currency(),
        )
        .await;
        let currency = self.currency();
        let zero = Money::zero(currency);
        let mut priced: Vec<PricedLine> = Vec::with_capacity(lines.len());
        let mut subtotal = zero;
        let mut tax_total = zero;
        let mut eco_fee_total = zero;
        for ret in &lines {
            if ret.qty <= Decimal::ZERO {
                return Err(EdgeError::InvalidInput(format!(
                    "return qty must be positive: {}",
                    ret.qty
                )));
            }
            let Some(orig_line) = original_lines
                .iter()
                .find(|l| l.product_id == ret.product_id)
            else {
                return Err(EdgeError::NotFound(format!(
                    "product {} was not on transaction {original_tx_id}",
                    ret.product_id
                )));
            };
            if ret.qty > orig_line.qty {
                return Err(EdgeError::InvalidInput(format!(
                    "cannot return {} of {}; only {} were sold",
                    ret.qty, ret.product_id, orig_line.qty
                )));
            }
            let qty = -ret.qty;
            let unit_price = Money::new(orig_line.unit_price, currency);
            let unit_cost = Money::new(orig_line.base_unit_cost, currency);
            let line_total = unit_price.mul_decimal(qty);
            let (tax_amount, eco_fee) =
                tax_and_eco_for(&self.pool, &ctx, ret.product_id, line_total, qty).await?;
            subtotal = money_add(subtotal, line_total)?;
            tax_total = money_add(tax_total, tax_amount)?;
            eco_fee_total = money_add(eco_fee_total, eco_fee)?;
            priced.push(PricedLine {
                product_id: ret.product_id,
                sku: String::new(),
                description: orig_line.description.clone(),
                qty,
                uom: orig_line.uom_code.clone(),
                unit_price,
                unit_cost,
                line_total,
                tax_amount,
                eco_fee_amount: eco_fee,
                rule_id: None,
            });
        }
        let grand_total = subtotal
            .checked_add(tax_total)?
            .checked_add(eco_fee_total)?;
        let pos_fee_total = zero;

        let tx_id = Uuid::now_v7();
        let now = Utc::now();
        let doc_number = format!(
            "TX-{:06}",
            df_db::count_transactions_for_store(&self.pool, self.config.store_id).await? + 1
        );
        let transaction = Transaction {
            id: tx_id,
            org_id: self.config.org_id,
            store_id: self.config.store_id,
            till_session_id: original.till_session_id,
            doc_number: doc_number.clone(),
            doc_type: "return".to_string(),
            status: "completed".to_string(),
            customer_id: original.customer_id,
            job_id: original.job_id,
            po_number: original.po_number.clone(),
            salesperson_id: None,
            cashier_id: original.cashier_id,
            currency: currency.code().to_string(),
            subtotal: subtotal.amount(),
            tax_total: tax_total.amount(),
            eco_fee_total: eco_fee_total.amount(),
            pos_fee_total: pos_fee_total.amount(),
            grand_total: grand_total.amount(),
            deposit_paid: Decimal::ZERO,
            fulfillment_mode: "counter".to_string(),
            scheduled_at: None,
            completed_at: Some(now),
            created_at: now,
        };
        let original_payments =
            df_db::list_payments_for_transaction(&self.pool, original_tx_id).await?;
        let was_charged = original_payments
            .iter()
            .any(|p| p.tender == "charge_account");

        let mut db_tx = self.pool.begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_transaction(&mut *db_tx, &transaction).await?;
        let mut outbox_entries: Vec<OutboxEntry> = Vec::new();
        for (index, line) in priced.iter().enumerate() {
            df_db::insert_transaction_line(
                &mut *db_tx,
                &TransactionLine {
                    id: Uuid::now_v7(),
                    transaction_id: tx_id,
                    line_no: (index + 1) as i32,
                    product_id: line.product_id,
                    description: line.description.clone(),
                    qty: line.qty,
                    uom_code: line.uom.clone(),
                    unit_price: line.unit_price.amount(),
                    base_unit_cost: line.unit_cost.amount(),
                    tint_unit_cost: Decimal::ZERO,
                    discount_amount: Decimal::ZERO,
                    tax_amount: line.tax_amount.amount(),
                    eco_fee_amount: line.eco_fee_amount.amount(),
                    line_total: line.line_total.amount(),
                    tint_formula_id: None,
                    prompts_json: serde_json::json!({}),
                    vertical_spec_json: serde_json::json!({}),
                    linked_po_line_id: None,
                    fulfilled_qty: line.qty,
                },
            )
            .await?;
            let delta_id = Uuid::now_v7();
            df_db::insert_inventory_delta(
                &mut *db_tx,
                &InventoryLedgerEntry {
                    id: delta_id,
                    org_id: self.config.org_id,
                    store_id: self.config.store_id,
                    product_id: line.product_id,
                    lot_number: None,
                    delta_on_hand: -line.qty,
                    delta_committed: Decimal::ZERO,
                    delta_on_order: Decimal::ZERO,
                    delta_in_transit: Decimal::ZERO,
                    unit_cost: line.unit_cost.amount(),
                    reason_type: "return".to_string(),
                    reference_id: tx_id,
                    created_at: now,
                },
            )
            .await?;
            outbox_entries.push(OutboxEntry::new(
                self.config.store_id,
                "inventory_ledger",
                delta_id,
                serde_json::json!({
                    "product_id": line.product_id.to_string(),
                    "delta_on_hand": (-line.qty).to_string(),
                    "reason_type": "return",
                    "reference_id": tx_id.to_string(),
                }),
            ));
        }
        if was_charged && let Some(customer_id) = original.customer_id {
            let ar_id = Uuid::now_v7();
            df_db::insert_ar_entry_exec(
                &mut *db_tx,
                &ArLedgerEntry {
                    id: ar_id,
                    org_id: self.config.org_id,
                    store_id: self.config.store_id,
                    customer_id,
                    job_id: original.job_id,
                    entry_type: "credit_memo".to_string(),
                    reference_type: "transaction".to_string(),
                    reference_id: tx_id,
                    amount: grand_total.amount(),
                    due_date: None,
                    statement_id: None,
                    created_at: now,
                },
            )
            .await?;
            outbox_entries.push(OutboxEntry::new(
                self.config.store_id,
                "ar_ledger",
                ar_id,
                serde_json::json!({
                    "customer_id": customer_id.to_string(),
                    "amount": grand_total.amount().to_string(),
                    "entry_type": "credit_memo",
                    "reference_id": tx_id.to_string(),
                }),
            ));
        }
        outbox_entries.push(OutboxEntry::new(
            self.config.store_id,
            "transaction",
            tx_id,
            serde_json::json!({
                "id": tx_id.to_string(),
                "doc_number": doc_number,
                "doc_type": "return",
                "status": "completed",
                "customer_id": original.customer_id.map(|id| id.to_string()),
                "subtotal": subtotal.amount().to_string(),
                "tax_total": tax_total.amount().to_string(),
                "eco_fee_total": eco_fee_total.amount().to_string(),
                "pos_fee_total": "0.0000",
                "grand_total": grand_total.amount().to_string(),
            }),
        ));
        for entry in &outbox_entries {
            Outbox::enqueue_exec(&mut *db_tx, entry).await?;
        }
        db_tx.commit().await.map_err(DbError::Sqlx)?;

        let receipt_printed = self.print_receipt(
            &ReceiptData {
                store_name: self.config.store_name.clone(),
                doc_number: doc_number.clone(),
                lines: priced
                    .iter()
                    .map(|l| ReceiptLine {
                        description: format!("RETURN {}", l.description),
                        qty: l.qty,
                        unit_price: l.unit_price,
                        line_total: l.line_total,
                    })
                    .collect(),
                subtotal,
                tax_total,
                eco_fee_total,
                pos_fee_total,
                grand_total,
                payments: Vec::new(),
                change_due: zero,
            },
            false,
        );
        let _ = self.events.send(
            serde_json::json!({
                "type": "return_completed",
                "store_id": self.config.store_id.to_string(),
                "transaction_id": tx_id.to_string(),
                "doc_number": doc_number,
                "doc_type": "return",
                "grand_total": grand_total.amount().to_string(),
            })
            .to_string(),
        );
        Ok(CompletedSale {
            transaction_id: tx_id,
            doc_number,
            subtotal,
            tax_total,
            eco_fee_total,
            pos_fee_total,
            grand_total,
            change_due: zero,
            payments: Vec::new(),
            receipt_printed,
        })
    }

    // --- holds & quotes -----------------------------------------------------

    /// Park a sale at the counter; returns the hold id. Holds live in
    /// memory in Phase 1 (lost on restart).
    pub async fn park_hold(&self, input: SaleInput) -> Result<Uuid, EdgeError> {
        self.validate_lines(&input.lines)?;
        let hold_id = Uuid::now_v7();
        self.holds
            .lock()
            .expect("holds lock")
            .insert(hold_id, input);
        Ok(hold_id)
    }

    /// Resume a parked hold (removes it from the hold list).
    pub async fn resume_hold(&self, hold_id: Uuid) -> Result<SaleInput, EdgeError> {
        self.holds
            .lock()
            .expect("holds lock")
            .remove(&hold_id)
            .ok_or_else(|| EdgeError::NotFound(format!("hold {hold_id}")))
    }

    /// Ids of currently parked holds (in-memory; lost on restart).
    pub async fn list_holds(&self) -> Vec<Uuid> {
        self.holds
            .lock()
            .expect("holds lock")
            .keys()
            .copied()
            .collect()
    }

    /// Record a quote: persisted as a `doc_type = 'quote'` transaction
    /// with lines, but no inventory movement, no payments and no
    /// outbox entries.
    pub async fn record_quote(&self, input: SaleInput) -> Result<Uuid, EdgeError> {
        self.validate_lines(&input.lines)?;
        let ctx = PricingContext::load(
            &self.pool,
            self.config.org_id,
            self.config.store_id,
            self.currency(),
        )
        .await;
        let priced = price_lines(
            &self.pool,
            &ctx,
            input.customer_id,
            input.job_id,
            &input.lines,
        )
        .await?;
        let tx_id = Uuid::now_v7();
        let now = Utc::now();
        let doc_number = format!(
            "TX-{:06}",
            df_db::count_transactions_for_store(&self.pool, self.config.store_id).await? + 1
        );
        let mut db_tx = self.pool.begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_transaction(
            &mut *db_tx,
            &Transaction {
                id: tx_id,
                org_id: self.config.org_id,
                store_id: self.config.store_id,
                till_session_id: None,
                doc_number,
                doc_type: "quote".to_string(),
                status: "open".to_string(),
                customer_id: input.customer_id,
                job_id: input.job_id,
                po_number: input.po_number.clone(),
                salesperson_id: None,
                cashier_id: None,
                currency: self.currency().code().to_string(),
                subtotal: priced.subtotal.amount(),
                tax_total: priced.tax_total.amount(),
                eco_fee_total: priced.eco_fee_total.amount(),
                pos_fee_total: Money::zero(self.currency()).amount(),
                grand_total: priced
                    .subtotal
                    .checked_add(priced.tax_total)?
                    .checked_add(priced.eco_fee_total)?
                    .amount(),
                deposit_paid: Decimal::ZERO,
                fulfillment_mode: "counter".to_string(),
                scheduled_at: None,
                completed_at: None,
                created_at: now,
            },
        )
        .await?;
        for (index, line) in priced.lines.iter().enumerate() {
            df_db::insert_transaction_line(
                &mut *db_tx,
                &TransactionLine {
                    id: Uuid::now_v7(),
                    transaction_id: tx_id,
                    line_no: (index + 1) as i32,
                    product_id: line.product_id,
                    description: line.description.clone(),
                    qty: line.qty,
                    uom_code: line.uom.clone(),
                    unit_price: line.unit_price.amount(),
                    base_unit_cost: line.unit_cost.amount(),
                    tint_unit_cost: Decimal::ZERO,
                    discount_amount: Decimal::ZERO,
                    tax_amount: line.tax_amount.amount(),
                    eco_fee_amount: line.eco_fee_amount.amount(),
                    line_total: line.line_total.amount(),
                    tint_formula_id: None,
                    prompts_json: serde_json::json!({}),
                    vertical_spec_json: serde_json::json!({}),
                    linked_po_line_id: None,
                    fulfilled_qty: Decimal::ZERO,
                },
            )
            .await?;
        }
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        Ok(tx_id)
    }

    /// Record a layaway: a persisted open `doc_type = 'layaway'`
    /// transaction with a deposit collected now, no inventory
    /// movement and no AR entry. The deposit payment row is recorded
    /// and the transaction is queued in the outbox so HQ sees the
    /// layaway and its deposit. Balance due is
    /// `grand_total - deposit_paid` (the card surcharge on the
    /// deposit, if any, is part of `grand_total`, not of the
    /// deposit). Returns the layaway transaction id.
    pub async fn record_layaway(
        &self,
        input: SaleInput,
        deposit: TenderInput,
        provider: &dyn PaymentProvider,
    ) -> Result<Uuid, EdgeError> {
        self.validate_lines(&input.lines)?;
        let ctx = PricingContext::load(
            &self.pool,
            self.config.org_id,
            self.config.store_id,
            self.currency(),
        )
        .await;
        let priced = price_lines(
            &self.pool,
            &ctx,
            input.customer_id,
            input.job_id,
            &input.lines,
        )
        .await?;
        let pre_surcharge = priced
            .subtotal
            .checked_add(priced.tax_total)?
            .checked_add(priced.eco_fee_total)?;
        // Deposit: explicit amount, or the whole pre-surcharge total.
        // It must be positive and may not exceed the total (no change
        // due on a layaway deposit).
        let tendered = deposit.amount.unwrap_or(pre_surcharge);
        if tendered.is_negative() || tendered.is_zero() {
            return Err(EdgeError::InvalidInput(
                "layaway deposit must be positive".to_string(),
            ));
        }
        if tendered.amount() > pre_surcharge.amount() {
            return Err(EdgeError::InvalidInput(format!(
                "layaway deposit {} exceeds the total {}",
                tendered.amount(),
                pre_surcharge.amount()
            )));
        }
        let surcharge = compute_surcharge(
            surcharge_rule_for(deposit.kind, &self.config.surcharge_rules),
            tendered,
        );
        let charged = tendered.checked_add(surcharge)?;
        let pos_fee_total = surcharge;
        let grand_total = pre_surcharge.checked_add(pos_fee_total)?;

        // Authorize a card deposit before writing anything.
        let payment = if deposit.kind == TenderKind::Card {
            let result = provider
                .authorize(&PaymentRequest {
                    amount: charged,
                    idempotency_key: Uuid::now_v7(),
                    tender: TenderKind::Card,
                    surcharge,
                })
                .await?;
            if result.status != PaymentStatus::Approved {
                return Err(EdgeError::PaymentDeclined(format!(
                    "card authorization declined (ref {})",
                    result.reference
                )));
            }
            result
        } else {
            PaymentResult {
                status: PaymentStatus::Approved,
                authorization: None,
                reference: match deposit.kind {
                    TenderKind::Cash => "CASH".to_string(),
                    TenderKind::ChargeAccount => format!(
                        "CHARGE-{}",
                        input
                            .customer_id
                            .map(|id| id.to_string())
                            .unwrap_or_default()
                    ),
                    TenderKind::GiftCard => "GIFT-CARD".to_string(),
                    TenderKind::Card => unreachable!(),
                },
                amount: charged,
                surcharge,
                tender: deposit.kind,
            }
        };

        let tx_id = Uuid::now_v7();
        let now = Utc::now();
        let doc_number = format!(
            "TX-{:06}",
            df_db::count_transactions_for_store(&self.pool, self.config.store_id).await? + 1
        );
        let mut db_tx = self.pool.begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_transaction(
            &mut *db_tx,
            &Transaction {
                id: tx_id,
                org_id: self.config.org_id,
                store_id: self.config.store_id,
                till_session_id: None,
                doc_number: doc_number.clone(),
                doc_type: "layaway".to_string(),
                status: "open".to_string(),
                customer_id: input.customer_id,
                job_id: input.job_id,
                po_number: input.po_number.clone(),
                salesperson_id: None,
                cashier_id: None,
                currency: self.currency().code().to_string(),
                subtotal: priced.subtotal.amount(),
                tax_total: priced.tax_total.amount(),
                eco_fee_total: priced.eco_fee_total.amount(),
                pos_fee_total: pos_fee_total.amount(),
                grand_total: grand_total.amount(),
                deposit_paid: tendered.amount(),
                fulfillment_mode: "counter".to_string(),
                scheduled_at: None,
                completed_at: None,
                created_at: now,
            },
        )
        .await?;
        for (index, line) in priced.lines.iter().enumerate() {
            df_db::insert_transaction_line(
                &mut *db_tx,
                &TransactionLine {
                    id: Uuid::now_v7(),
                    transaction_id: tx_id,
                    line_no: (index + 1) as i32,
                    product_id: line.product_id,
                    description: line.description.clone(),
                    qty: line.qty,
                    uom_code: line.uom.clone(),
                    unit_price: line.unit_price.amount(),
                    base_unit_cost: line.unit_cost.amount(),
                    tint_unit_cost: Decimal::ZERO,
                    discount_amount: Decimal::ZERO,
                    tax_amount: line.tax_amount.amount(),
                    eco_fee_amount: line.eco_fee_amount.amount(),
                    line_total: line.line_total.amount(),
                    tint_formula_id: None,
                    prompts_json: serde_json::json!({}),
                    vertical_spec_json: serde_json::json!({}),
                    linked_po_line_id: None,
                    fulfilled_qty: Decimal::ZERO,
                },
            )
            .await?;
        }
        df_db::insert_payment(
            &mut *db_tx,
            &PaymentRow {
                id: Uuid::now_v7(),
                transaction_id: tx_id,
                tender: payment.tender.as_str().to_string(),
                amount: payment.amount.amount(),
                reference: payment.reference.clone(),
                authorization: payment.authorization.clone(),
                created_at: now,
            },
        )
        .await?;
        Outbox::enqueue_exec(
            &mut *db_tx,
            &OutboxEntry::new(
                self.config.store_id,
                "transaction",
                tx_id,
                serde_json::json!({
                    "id": tx_id.to_string(),
                    "doc_number": doc_number,
                    "doc_type": "layaway",
                    "status": "open",
                    "customer_id": input.customer_id.map(|id| id.to_string()),
                    "subtotal": priced.subtotal.amount().to_string(),
                    "tax_total": priced.tax_total.amount().to_string(),
                    "eco_fee_total": priced.eco_fee_total.amount().to_string(),
                    "pos_fee_total": pos_fee_total.amount().to_string(),
                    "grand_total": grand_total.amount().to_string(),
                    "deposit_paid": tendered.amount().to_string(),
                }),
            ),
        )
        .await?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        Ok(tx_id)
    }

    /// HTTP layaway: uses the configured default provider (or the
    /// mock provider when the deposit is not a card tender).
    pub async fn record_layaway_with_default_provider(
        &self,
        input: SaleInput,
        deposit: TenderInput,
    ) -> Result<Uuid, EdgeError> {
        let provider = self.http_provider(deposit.kind == TenderKind::Card)?;
        self.record_layaway(input, deposit, provider.as_ref()).await
    }

    /// Void a transaction (status only — Phase 1 does not reverse the
    /// ledgers of an already-synced sale; use a return for that).
    pub async fn void_transaction(&self, id: Uuid) -> Result<Transaction, EdgeError> {
        let tx = df_db::get_transaction(&self.pool, id).await?;
        if tx.status == "voided" {
            return Ok(tx);
        }
        df_db::set_transaction_status(&self.pool, id, "voided").await?;
        Ok(df_db::get_transaction(&self.pool, id).await?)
    }

    // --- sync ---------------------------------------------------------------

    /// Drain pending outbox entries in UUIDv7 order as one batch,
    /// apply them to `hq`, and mark them synced on success. On
    /// failure every entry is marked failed (retry count bumped) and
    /// left pending.
    pub async fn sync_now(&self, hq: &dyn HqSink) -> Result<ApplyReport, EdgeError> {
        let entries = Outbox::pending(&self.pool, self.config.store_id, 1000).await?;
        if entries.is_empty() {
            return Ok(ApplyReport::empty());
        }
        let batch = DeltaBatch::new(self.config.store_id, entries);
        let ids: Vec<Uuid> = batch.entries.iter().map(|e| e.id).collect();
        match hq.apply_batch(&batch).await {
            Ok(report) => {
                Outbox::mark_synced(&self.pool, &ids).await?;
                let pending = Outbox::pending_count(&self.pool, self.config.store_id).await?;
                df_db::upsert_sync_state(
                    &self.pool,
                    &df_db::models::SyncStateRow {
                        store_id: self.config.store_id,
                        last_batch_id: Some(batch.batch_id),
                        last_synced_at: Some(Utc::now()),
                        pending_count: pending as i64,
                    },
                )
                .await?;
                Ok(report)
            }
            Err(err) => {
                let message = err.to_string();
                for id in &ids {
                    let _ = Outbox::mark_failed(&self.pool, *id, &message).await;
                }
                Err(EdgeError::Sync(err))
            }
        }
    }

    /// Sync against the in-process HQ sink configured with
    /// [`Edge::set_hq`], or fail with [`EdgeError::NoHq`] when none
    /// was configured (the HTTP `/pos/sync/run` endpoint surfaces
    /// this as 503).
    pub async fn sync_now_with_configured_hq(&self) -> Result<ApplyReport, EdgeError> {
        let hq = self.hq.read().expect("hq lock").clone();
        match hq {
            Some(hq) => self.sync_now(hq.as_ref()).await,
            None => Err(EdgeError::NoHq(
                "no HQ sink configured for this edge (call Edge::set_hq first)".to_string(),
            )),
        }
    }

    pub async fn sync_state(&self) -> Result<SyncState, EdgeError> {
        let pending_count = Outbox::pending_count(&self.pool, self.config.store_id).await?;
        if let Some(row) = df_db::get_sync_state(&self.pool, self.config.store_id).await? {
            return Ok(SyncState {
                store_id: self.config.store_id,
                last_batch_id: row.last_batch_id,
                last_synced_at: row.last_synced_at,
                pending_count,
            });
        }
        Ok(SyncState {
            store_id: self.config.store_id,
            last_batch_id: None,
            last_synced_at: Outbox::sync_state(&self.pool, self.config.store_id)
                .await?
                .last_synced_at,
            pending_count,
        })
    }

    pub async fn pending_outbox_count(&self) -> Result<u64, EdgeError> {
        Ok(Outbox::pending_count(&self.pool, self.config.store_id).await?)
    }

    // --- serving ------------------------------------------------------------

    /// The local HTTP API the POS talks to (see [`crate::http`]).
    pub fn router(&self) -> axum::Router {
        crate::http::router(self.clone())
    }

    /// Serve the router on `addr` until cancelled.
    pub async fn serve(&self, addr: std::net::SocketAddr) -> Result<(), EdgeError> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, self.router()).await?;
        Ok(())
    }
}

/// Test helpers: an edge backed by in-memory SQLite, with an org,
/// store and product row seeded. Used by the `http` tests too.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use df_db::models::{Organization, Product, Store};

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
        };
        df_db::insert_product(edge.pool(), &product).await.unwrap();
        product_id
    }

    pub(crate) fn sale_input(product_id: Uuid, qty: &str, tender: TenderKind) -> SaleInput {
        SaleInput {
            customer_id: None,
            job_id: None,
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
