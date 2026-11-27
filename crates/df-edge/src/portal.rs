//! Contractor portal, mobile KPIs and BOPIS on the edge (Phase 5,
//! v0.6; spec §4 `contractor_portal_ecom`, §6.1 `/portal/v1`, §7
//! Phase 5).
//!
//! The Rust API the HTTP endpoints call — no HTTP here.
//!
//! * Portal (contractor-scoped, `portal_enabled` required on every
//!   call): invoices (the customer's transactions), statements,
//!   formula history, formula reorders and online bill pay.
//! * Bill pay: the amount is authorized through the configured
//!   `PaymentProvider` (mock in dev — no live payment API, no card
//!   data stored) and, only on approval, posted to AR with FIFO
//!   allocation. The idempotency key makes a retried payment a
//!   no-op: the original payment record is returned and nothing is
//!   posted twice. No surcharge is applied to bill-pay in v0.6.
//! * Formula reorder: look up a stored formula and re-create the
//!   sale line for it — same base product, priced through the
//!   normal matrix-pricing path, with the line linked to the
//!   formula. Formula tracking only (Kiel, Nov 26 rescope): the
//!   POS never drives a tint machine.
//! * BOPIS: a public pickup order. Placing it authorizes the card
//!   (mock in dev), reserves stock (committed) and records the
//!   payment; the counter completes the pickup, which moves
//!   committed stock to sold and closes the order.
//! * Mobile KPIs: today's sales, open quotes/orders, total open
//!   AR, deliveries out — derived at read time, never stored.
//!
//! All money is [`Money`] — never floats.

use chrono::{DateTime, Utc};
use df_db::DbError;
use df_db::models::{
    BopisOrderRow, Customer, PortalPaymentRow, Product, StatementRow, Transaction, TransactionLine,
};
use df_db::models::{InventoryLedgerEntry, PaymentRow, TintFormula};
use df_domain::{Currency, Money};
use df_payments::{MockTerminal, PaymentRequest, PaymentStatus, TenderKind};
use df_portal::{
    BillPayIntent, BillPayResult, BopisOrderRequest, Fulfillment, MobileKpis, PortalError,
    ReorderRequest, require_portal_access, validate_bill_pay, validate_bopis_order,
    validate_reorder,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;
use crate::pricing::{PricingContext, price_lines};
use crate::types::SaleLineInput;

/// A portal customer view: the customer row plus the open AR
/// balance the portal shows next to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalCustomerView {
    pub customer: Customer,
    pub open_balance: Money,
}

/// A portal formula reorder: the order created for the formula.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortalReorderResponse {
    pub order_id: Uuid,
    pub doc_number: String,
    pub formula_id: Uuid,
    pub fulfillment: String,
    pub grand_total: Money,
}

/// A BOPIS pickup order, as placed: the order plus the pickup
/// details the counter needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BopisOrderResponse {
    pub order_id: Uuid,
    pub doc_number: String,
    pub grand_total: Money,
    pub pickup_name: String,
    pub contact: String,
    pub scheduled_at: DateTime<Utc>,
    pub payment_reference: String,
}

/// A BOPIS order with its lines and pickup details, for the
/// storefront status lookup and the counter pickup screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BopisOrderView {
    pub order: Transaction,
    pub lines: Vec<TransactionLine>,
    pub pickup_name: String,
    pub contact: String,
}

fn next_doc_number(count: i64) -> String {
    format!("TX-{:06}", count + 1)
}

impl Edge {
    fn portal_provider(&self) -> Arc<dyn df_payments::PaymentProvider> {
        match self.default_provider() {
            Some(provider) => provider,
            // No provider configured: the dev mock. Card data never
            // reaches a live API through this path.
            None => Arc::new(MockTerminal::new()),
        }
    }

    /// Load a customer and require portal access. Every portal
    /// call goes through here — disabling `portal_enabled` locks
    /// the contractor out immediately.
    pub async fn portal_customer(
        &self,
        customer_id: Uuid,
    ) -> Result<PortalCustomerView, EdgeError> {
        let customer = df_db::get_customer(self.pool(), customer_id).await?;
        require_portal_access(customer.portal_enabled)?;
        let open_balance = self.open_balance_for(customer_id).await?;
        Ok(PortalCustomerView {
            customer,
            open_balance,
        })
    }

    /// The contractor's invoices: their transactions, newest
    /// first (completed sales and open orders alike).
    pub async fn portal_invoices(&self, customer_id: Uuid) -> Result<Vec<Transaction>, EdgeError> {
        self.portal_customer(customer_id).await?;
        Ok(df_db::list_transactions_for_customer(self.pool(), customer_id).await?)
    }

    /// The contractor's frozen monthly statements, newest first.
    pub async fn portal_statements(
        &self,
        customer_id: Uuid,
    ) -> Result<Vec<StatementRow>, EdgeError> {
        self.portal_customer(customer_id).await?;
        Ok(df_db::list_statements_for_customer(self.pool(), customer_id).await?)
    }

    /// The contractor's stored formulas (colour history), newest
    /// first — the list a reorder picks from.
    pub async fn portal_formulas(&self, customer_id: Uuid) -> Result<Vec<TintFormula>, EdgeError> {
        self.portal_customer(customer_id).await?;
        self.search_tint_formulas(None, Some(customer_id), None)
            .await
    }

    /// Past bill payments through the portal, newest first.
    pub async fn portal_payments(
        &self,
        customer_id: Uuid,
    ) -> Result<Vec<PortalPaymentRow>, EdgeError> {
        self.portal_customer(customer_id).await?;
        Ok(df_db::list_portal_payments_for_customer(self.pool(), customer_id).await?)
    }

    /// Reorder a stored formula: re-create the sale line for it
    /// as an open order (pickup or delivery). The formula must
    /// belong to the contractor; the line is priced through the
    /// normal matrix-pricing path and linked to the formula.
    pub async fn portal_reorder(
        &self,
        req: ReorderRequest,
    ) -> Result<PortalReorderResponse, EdgeError> {
        validate_reorder(&req)?;
        self.portal_customer(req.customer_id).await?;
        let formula = df_db::get_tint_formula(self.pool(), req.formula_id)
            .await
            .map_err(|_| {
                EdgeError::from(PortalError::FormulaNotFound(req.formula_id.to_string()))
            })?;
        if formula.customer_id != Some(req.customer_id) {
            return Err(EdgeError::from(PortalError::FormulaNotForCustomer));
        }
        let product = df_db::get_product(self.pool(), formula.base_product_id).await?;
        let customer = df_db::get_customer(self.pool(), req.customer_id).await?;
        let priced = self
            .price_order_lines(
                Some(req.customer_id),
                req.job_id,
                customer.price_level_id,
                &[formula_line(
                    &formula,
                    &product,
                    req.qty,
                    self.config().currency,
                )],
            )
            .await?;
        let tx_id = Uuid::now_v7();
        let now = Utc::now();
        let doc_number = next_doc_number(
            df_db::count_transactions_for_store(self.pool(), self.config().store_id).await?,
        );
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        self.insert_order_tx(
            &mut db_tx,
            OrderTxParams {
                tx_id,
                doc_number: &doc_number,
                customer_id: Some(req.customer_id),
                job_id: req.job_id,
                priced: &priced,
                fulfillment_mode: req.fulfillment.as_str(),
                scheduled_at: None,
                now,
            },
        )
        .await?;
        // Link the single line to the formula being reordered.
        let line = &priced.lines[0];
        df_db::insert_transaction_line(
            &mut *db_tx,
            &TransactionLine {
                id: Uuid::now_v7(),
                transaction_id: tx_id,
                line_no: 1,
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
                tint_formula_id: Some(formula.id),
                prompts_json: serde_json::json!({
                    "color_code": formula.color_code_snapshot,
                    "color_name": formula.color_name_snapshot,
                    "sheen": formula.sheen,
                    "room_or_usage": formula.room_or_usage,
                }),
                vertical_spec_json: serde_json::json!({}),
                linked_po_line_id: None,
                fulfilled_qty: Decimal::ZERO,
            },
        )
        .await?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        Ok(PortalReorderResponse {
            order_id: tx_id,
            doc_number,
            formula_id: formula.id,
            fulfillment: req.fulfillment.as_str().to_string(),
            grand_total: priced
                .subtotal
                .checked_add(priced.tax_total)?
                .checked_add(priced.eco_fee_total)?,
        })
    }

    /// Online bill pay: authorize through the payment provider
    /// (mock in dev), then post to AR with FIFO allocation. A
    /// retried idempotency key returns the original payment and
    /// posts nothing twice.
    pub async fn portal_pay_bill(&self, intent: BillPayIntent) -> Result<BillPayResult, EdgeError> {
        validate_bill_pay(&intent, self.config().currency)?;
        self.portal_customer(intent.customer_id).await?;
        if let Some(existing) =
            df_db::get_portal_payment(self.pool(), &intent.idempotency_key).await?
        {
            let allocations =
                df_db::list_allocations_for_payment(self.pool(), existing.payment_entry_id).await?;
            return Ok(BillPayResult {
                payment_entry_id: existing.payment_entry_id,
                customer_id: existing.customer_id,
                amount: Money::new(existing.amount, self.config().currency),
                method: existing.method,
                provider_reference: existing.provider_reference,
                allocations: allocations
                    .iter()
                    .map(|a| (a.charge_entry_id, a.allocated_amount))
                    .collect(),
                replayed: true,
            });
        }
        let provider = self.portal_provider();
        let authorized = provider
            .authorize(&PaymentRequest {
                amount: intent.amount,
                idempotency_key: Uuid::now_v7(),
                tender: TenderKind::Card,
                surcharge: Money::zero(self.config().currency),
            })
            .await
            .map_err(EdgeError::Payment)?;
        if authorized.status != PaymentStatus::Approved {
            return Err(EdgeError::PaymentDeclined(format!(
                "provider returned {:?} for bill payment {}",
                authorized.status, intent.idempotency_key
            )));
        }
        let posted = self
            .post_ar_payment(intent.customer_id, intent.amount, &intent.method)
            .await?;
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        df_db::insert_portal_payment(
            &mut *db_tx,
            &PortalPaymentRow {
                idempotency_key: intent.idempotency_key.clone(),
                customer_id: intent.customer_id,
                payment_entry_id: posted.payment_entry_id,
                amount: intent.amount.amount(),
                method: intent.method.clone(),
                provider_reference: authorized.reference.clone(),
                created_at: Utc::now(),
            },
        )
        .await?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        Ok(BillPayResult {
            payment_entry_id: posted.payment_entry_id,
            customer_id: posted.customer_id,
            amount: posted.amount,
            method: posted.method,
            provider_reference: authorized.reference,
            allocations: posted.allocations,
            replayed: false,
        })
    }

    /// The public catalog the BOPIS storefront browses: active
    /// products for the org, cheapest first by SKU.
    pub async fn catalog_products(&self) -> Result<Vec<Product>, EdgeError> {
        let mut products = df_db::list_products_for_org(self.pool(), self.config().org_id).await?;
        products.retain(|p| p.is_active);
        products.sort_by(|a, b| a.sku.cmp(&b.sku));
        Ok(products)
    }

    /// Place a BOPIS pickup order: price the lines, authorize the
    /// card (mock in dev), reserve the stock (committed) and
    /// record the payment. The order stays open until the counter
    /// completes the pickup.
    pub async fn record_bopis_order(
        &self,
        req: BopisOrderRequest,
    ) -> Result<BopisOrderResponse, EdgeError> {
        validate_bopis_order(&req)?;
        if req.scheduled_at <= Utc::now() {
            return Err(EdgeError::InvalidInput(
                "pickup slot must be in the future".to_string(),
            ));
        }
        let mut lines = Vec::with_capacity(req.lines.len());
        for line in &req.lines {
            let product = df_db::get_product(self.pool(), line.product_id)
                .await
                .map_err(|_| {
                    EdgeError::from(PortalError::ProductNotFound(line.product_id.to_string()))
                })?;
            lines.push(SaleLineInput {
                product_id: product.id,
                sku: product.sku.clone(),
                description: product.name.clone(),
                qty: line.qty,
                uom: product.base_uom.clone(),
                unit_price: Money::new(product.list_price, self.config().currency),
                unit_cost: Money::new(product.standard_cost, self.config().currency),
            });
        }
        let priced = self.price_order_lines(None, None, None, &lines).await?;
        let grand_total = priced
            .subtotal
            .checked_add(priced.tax_total)?
            .checked_add(priced.eco_fee_total)?;
        let provider = self.portal_provider();
        let authorized = provider
            .authorize(&PaymentRequest {
                amount: grand_total,
                idempotency_key: Uuid::now_v7(),
                tender: TenderKind::Card,
                surcharge: Money::zero(self.config().currency),
            })
            .await
            .map_err(EdgeError::Payment)?;
        if authorized.status != PaymentStatus::Approved {
            return Err(EdgeError::PaymentDeclined(format!(
                "provider returned {:?} for BOPIS order",
                authorized.status
            )));
        }
        let tx_id = Uuid::now_v7();
        let now = Utc::now();
        let doc_number = next_doc_number(
            df_db::count_transactions_for_store(self.pool(), self.config().store_id).await?,
        );
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        self.insert_order_tx(
            &mut db_tx,
            OrderTxParams {
                tx_id,
                doc_number: &doc_number,
                customer_id: None,
                job_id: None,
                priced: &priced,
                fulfillment_mode: Fulfillment::Pickup.as_str(),
                scheduled_at: Some(req.scheduled_at),
                now,
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
        // Reserve the stock: committed up, on-hand untouched until
        // pickup completes.
        for line in &priced.lines {
            let delta_id = Uuid::now_v7();
            df_db::insert_inventory_delta(
                &mut *db_tx,
                &InventoryLedgerEntry {
                    id: delta_id,
                    org_id: self.config().org_id,
                    store_id: self.config().store_id,
                    product_id: line.product_id,
                    lot_number: None,
                    delta_on_hand: Decimal::ZERO,
                    delta_committed: line.qty,
                    delta_on_order: Decimal::ZERO,
                    delta_in_transit: Decimal::ZERO,
                    unit_cost: line.unit_cost.amount(),
                    reason_type: "order".to_string(),
                    reference_id: tx_id,
                    created_at: now,
                },
            )
            .await?;
        }
        df_db::insert_payment(
            &mut *db_tx,
            &PaymentRow {
                id: Uuid::now_v7(),
                transaction_id: tx_id,
                tender: TenderKind::Card.as_str().to_string(),
                amount: grand_total.amount(),
                reference: authorized.reference.clone(),
                authorization: authorized.authorization.clone(),
                created_at: now,
            },
        )
        .await?;
        df_db::insert_bopis_order(
            &mut *db_tx,
            &BopisOrderRow {
                order_id: tx_id,
                pickup_name: req.pickup_name.clone(),
                contact: req.contact.clone(),
                created_at: now,
            },
        )
        .await?;
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        Ok(BopisOrderResponse {
            order_id: tx_id,
            doc_number,
            grand_total,
            pickup_name: req.pickup_name,
            contact: req.contact,
            scheduled_at: req.scheduled_at,
            payment_reference: authorized.reference,
        })
    }

    /// A BOPIS order with its lines and pickup details.
    pub async fn bopis_order(&self, order_id: Uuid) -> Result<BopisOrderView, EdgeError> {
        let order = df_db::get_transaction(self.pool(), order_id)
            .await
            .map_err(|_| EdgeError::from(PortalError::OrderNotFound(order_id.to_string())))?;
        if order.doc_type != "order" || order.fulfillment_mode != "pickup" {
            return Err(EdgeError::from(PortalError::OrderNotPickup));
        }
        let lines = df_db::list_transaction_lines(self.pool(), order_id).await?;
        let pickup = df_db::get_bopis_order(self.pool(), order_id).await?;
        Ok(BopisOrderView {
            order,
            lines,
            pickup_name: pickup.pickup_name,
            contact: pickup.contact,
        })
    }

    /// Complete a BOPIS pickup at the counter: the stock moves
    /// from committed to sold (on-hand down, committed released),
    /// the lines are marked fulfilled and the order closes.
    /// Refuses when on-hand stock is short — the counter must
    /// resolve the shortage first, the order is not half-closed.
    pub async fn complete_bopis_pickup(&self, order_id: Uuid) -> Result<BopisOrderView, EdgeError> {
        let order = df_db::get_transaction(self.pool(), order_id)
            .await
            .map_err(|_| EdgeError::from(PortalError::OrderNotFound(order_id.to_string())))?;
        if order.doc_type != "order" || order.fulfillment_mode != "pickup" {
            return Err(EdgeError::from(PortalError::OrderNotPickup));
        }
        if order.status == "completed" {
            return Err(EdgeError::from(PortalError::OrderAlreadyCompleted));
        }
        let lines = df_db::list_transaction_lines(self.pool(), order_id).await?;
        for line in &lines {
            let on_hand =
                df_db::inventory_on_hand(self.pool(), self.config().store_id, line.product_id)
                    .await?;
            if on_hand < line.qty {
                return Err(EdgeError::from(PortalError::InsufficientStock(
                    line.description.clone(),
                    line.qty.to_string(),
                    on_hand.to_string(),
                )));
            }
        }
        let now = Utc::now();
        let mut db_tx = self.pool().begin().await.map_err(DbError::Sqlx)?;
        df_db::complete_transaction(&mut *db_tx, order_id, now).await?;
        for line in &lines {
            df_db::set_transaction_line_fulfilled(&mut *db_tx, line.id, line.qty).await?;
            let delta_id = Uuid::now_v7();
            df_db::insert_inventory_delta(
                &mut *db_tx,
                &InventoryLedgerEntry {
                    id: delta_id,
                    org_id: self.config().org_id,
                    store_id: self.config().store_id,
                    product_id: line.product_id,
                    lot_number: None,
                    delta_on_hand: -line.qty,
                    delta_committed: -line.qty,
                    delta_on_order: Decimal::ZERO,
                    delta_in_transit: Decimal::ZERO,
                    unit_cost: line.base_unit_cost,
                    reason_type: "sale".to_string(),
                    reference_id: order_id,
                    created_at: now,
                },
            )
            .await?;
        }
        db_tx.commit().await.map_err(DbError::Sqlx)?;
        self.bopis_order(order_id).await
    }

    /// Outside Sales KPIs: today's completed sales, open
    /// quotes/orders, total open AR and deliveries out now.
    /// Derived at read time from the edge ledger, never stored.
    /// "Today" is the UTC calendar day — the store-local day is a
    /// follow-up once the edge carries a timezone.
    pub async fn mobile_kpis(&self, store_id: Uuid) -> Result<MobileKpis, EdgeError> {
        let today = Utc::now().date_naive();
        let date = today.to_string();
        let currency = self.config().currency;
        let transactions = df_db::list_transactions_for_store(self.pool(), store_id).await?;
        let mut sales_today = Decimal::ZERO;
        let mut sales_today_count = 0i64;
        let mut open_quotes = 0i64;
        let mut open_orders = 0i64;
        for tx in &transactions {
            if tx.status == "completed"
                && tx.doc_type == "sale"
                && tx.created_at.date_naive() == today
            {
                sales_today += tx.grand_total;
                sales_today_count += 1;
            }
            if tx.status == "open" && tx.doc_type == "quote" {
                open_quotes += 1;
            }
            if tx.status == "open" && tx.doc_type == "order" {
                open_orders += 1;
            }
        }
        let charge_customers =
            df_db::list_charge_customers(self.pool(), self.config().org_id).await?;
        let mut ar_open_total = Decimal::ZERO;
        for customer in &charge_customers {
            ar_open_total += self.open_balance_for(customer.id).await?.amount();
        }
        let board = df_db::list_dispatch_board(self.pool(), store_id).await?;
        let deliveries_out_today = board
            .iter()
            .filter(|d| d.status == "out_for_delivery")
            .count() as i64;
        Ok(MobileKpis {
            date,
            sales_today: Money::new(sales_today, currency),
            sales_today_count,
            open_quotes,
            open_orders,
            ar_open_total: Money::new(ar_open_total, currency),
            deliveries_out_today,
        })
    }

    /// Price order lines through the normal matrix-pricing path
    /// (same as quotes: no till, no tenders, no inventory move).
    async fn price_order_lines(
        &self,
        customer_id: Option<Uuid>,
        job_id: Option<Uuid>,
        price_level_id: Option<Uuid>,
        lines: &[SaleLineInput],
    ) -> Result<crate::pricing::PricedSale, EdgeError> {
        let ctx = PricingContext::load(
            self.pool(),
            self.config().org_id,
            self.config().store_id,
            self.config().currency,
        )
        .await;
        price_lines(
            self.pool(),
            &ctx,
            customer_id,
            job_id,
            price_level_id,
            lines,
        )
        .await
    }
}

/// The header fields for an open `doc_type = 'order'`
/// transaction, bundled so `insert_order_tx` stays under the
/// argument-count lint.
struct OrderTxParams<'a> {
    tx_id: Uuid,
    doc_number: &'a str,
    customer_id: Option<Uuid>,
    job_id: Option<Uuid>,
    priced: &'a crate::pricing::PricedSale,
    fulfillment_mode: &'a str,
    scheduled_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
}

impl Edge {
    /// Insert an open `doc_type = 'order'` transaction header for
    /// a priced set of lines. The caller inserts the lines (a
    /// reorder links its line to a formula; BOPIS does not).
    async fn insert_order_tx(
        &self,
        db_tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        params: OrderTxParams<'_>,
    ) -> Result<(), EdgeError> {
        let OrderTxParams {
            tx_id,
            doc_number,
            customer_id,
            job_id,
            priced,
            fulfillment_mode,
            scheduled_at,
            now,
        } = params;
        df_db::insert_transaction(
            &mut **db_tx,
            &Transaction {
                id: tx_id,
                org_id: self.config().org_id,
                store_id: self.config().store_id,
                till_session_id: None,
                doc_number: doc_number.to_string(),
                doc_type: "order".to_string(),
                status: "open".to_string(),
                customer_id,
                job_id,
                po_number: None,
                job_name: None,
                salesperson_id: None,
                cashier_id: None,
                currency: self.config().currency.code().to_string(),
                subtotal: priced.subtotal.amount(),
                tax_total: priced.tax_total.amount(),
                eco_fee_total: priced.eco_fee_total.amount(),
                pos_fee_total: Money::zero(self.config().currency).amount(),
                grand_total: priced
                    .subtotal
                    .checked_add(priced.tax_total)?
                    .checked_add(priced.eco_fee_total)?
                    .amount(),
                deposit_paid: Decimal::ZERO,
                fulfillment_mode: fulfillment_mode.to_string(),
                scheduled_at,
                completed_at: None,
                created_at: now,
            },
        )
        .await?;
        Ok(())
    }
}

/// The sale line for a formula reorder: the formula's base
/// product, the requested quantity, priced at list (matrix rules
/// may replace it).
fn formula_line(
    formula: &TintFormula,
    product: &Product,
    qty: Decimal,
    currency: Currency,
) -> SaleLineInput {
    SaleLineInput {
        product_id: product.id,
        sku: product.sku.clone(),
        description: format!(
            "{} {} — {} ({})",
            product.name,
            formula.color_code_snapshot,
            formula.color_name_snapshot,
            formula.sheen.clone().unwrap_or_default(),
        ),
        qty,
        uom: product.base_uom.clone(),
        unit_price: Money::new(product.list_price, currency),
        unit_cost: Money::new(product.standard_cost, currency),
    }
}
