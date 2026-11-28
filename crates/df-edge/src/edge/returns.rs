//! Sale returns: inventory, AR, and card-provider refund reversal with
//! outbox recording.

use chrono::Utc;
use df_db::DbError;
use df_db::models::{
    ArLedgerEntry, InventoryLedgerEntry, PaymentRow, Transaction, TransactionLine,
};
use df_domain::Money;
use df_sync::{Outbox, OutboxEntry};
use rust_decimal::Decimal;
use uuid::Uuid;

use super::{Edge, money_add};
use crate::error::EdgeError;
use crate::hw::{ReceiptData, ReceiptLine};
use crate::payments::{PaymentProvider, PaymentResult, PaymentStatus, TenderKind};
use crate::pricing::{PricedLine, PricingContext, tax_and_eco_for};
use crate::types::{CompletedSale, ReturnLineInput};

impl Edge {
    // --- returns ------------------------------------------------------------

    /// Record a return against a completed sale: negative-qty lines
    /// reversing inventory, AR credit memo when originally charged,
    /// and card provider refund via the default payment provider when
    /// the original sale was paid by card.
    pub async fn record_return(
        &self,
        original_tx_id: Uuid,
        lines: Vec<ReturnLineInput>,
    ) -> Result<CompletedSale, EdgeError> {
        self.record_return_as(original_tx_id, lines, None).await
    }

    /// Same as [`Edge::record_return`], stamping the signed-in
    /// `cashier_id` and refunding original card payments via the
    /// configured default payment provider.
    pub async fn record_return_as(
        &self,
        original_tx_id: Uuid,
        lines: Vec<ReturnLineInput>,
        cashier_id: Option<Uuid>,
    ) -> Result<CompletedSale, EdgeError> {
        let provider = self.default_provider();
        self.record_return_inner(original_tx_id, lines, provider.as_deref(), cashier_id)
            .await
    }

    /// Record a return using an explicit [`PaymentProvider`] (useful
    /// when a caller manages its own provider handle).
    pub async fn record_return_with_provider(
        &self,
        original_tx_id: Uuid,
        lines: Vec<ReturnLineInput>,
        provider: &dyn PaymentProvider,
        cashier_id: Option<Uuid>,
    ) -> Result<CompletedSale, EdgeError> {
        self.record_return_inner(original_tx_id, lines, Some(provider), cashier_id)
            .await
    }

    async fn record_return_inner(
        &self,
        original_tx_id: Uuid,
        lines: Vec<ReturnLineInput>,
        provider: Option<&dyn PaymentProvider>,
        cashier_id: Option<Uuid>,
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

        let original_payments =
            df_db::list_payments_for_transaction(&self.pool, original_tx_id).await?;
        let was_charged = original_payments
            .iter()
            .any(|p| p.tender == "charge_account");
        let card_payments: Vec<&PaymentRow> = original_payments
            .iter()
            .filter(|p| {
                p.amount > Decimal::ZERO
                    && TenderKind::from_str_opt(&p.tender).is_some_and(TenderKind::is_card)
            })
            .collect();

        // Issue card refunds through the payment provider before writing
        // the return rows, capping each refund chunk at the original card
        // payment amount (exact Decimal arithmetic — zero floats).
        let mut refund_payments: Vec<PaymentResult> = Vec::new();
        if !card_payments.is_empty() {
            let Some(provider) = provider else {
                return Err(EdgeError::NoPaymentProvider(
                    "no payment provider configured for card refund (call Edge::set_payment_provider)"
                        .to_string(),
                ));
            };
            let mut remaining = -grand_total.amount();
            for orig_payment in &card_payments {
                if remaining <= Decimal::ZERO {
                    break;
                }
                let chunk = remaining.min(orig_payment.amount);
                let result = provider
                    .refund(&orig_payment.reference, Money::new(chunk, currency))
                    .await?;
                if result.status != PaymentStatus::Approved {
                    return Err(EdgeError::PaymentDeclined(format!(
                        "card refund declined (ref {})",
                        result.reference
                    )));
                }
                remaining -= chunk;
                refund_payments.push(result);
            }
        }

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
            job_name: original.job_name.clone(),
            salesperson_id: None,
            cashier_id: cashier_id.or(original.cashier_id),
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
                    "delta_committed": "0",
                    "delta_on_order": "0",
                    "delta_in_transit": "0",
                    "unit_cost": line.unit_cost.amount().to_string(),
                    "lot_number": null,
                    "reason_type": "return",
                    "reference_id": tx_id.to_string(),
                    "created_at": now.to_rfc3339(),
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
                    "reference_type": "transaction",
                    "reference_id": tx_id.to_string(),
                    "due_date": null,
                    "job_id": original.job_id.map(|id| id.to_string()),
                    "created_at": now.to_rfc3339(),
                }),
            ));
        }
        for refund in &refund_payments {
            df_db::insert_payment(
                &mut *db_tx,
                &PaymentRow {
                    id: Uuid::now_v7(),
                    transaction_id: tx_id,
                    tender: refund.tender.as_str().to_string(),
                    amount: -refund.amount.amount(),
                    reference: refund.reference.clone(),
                    authorization: refund.authorization.clone(),
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
                "doc_type": "return",
                "status": "completed",
                "customer_id": original.customer_id.map(|id| id.to_string()),
                "job_id": original.job_id.map(|id| id.to_string()),
                "po_number": original.po_number.clone(),
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
                payments: refund_payments.iter().map(Self::payment_summary).collect(),
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
            payments: refund_payments,
            receipt_printed,
        })
    }
}
