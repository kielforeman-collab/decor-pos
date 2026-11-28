//! Counter checkout, tender resolution, surcharge & credit-limit enforcement,
//! and receipt printing.

use chrono::Utc;
use df_db::DbError;
use df_db::models::{
    ArLedgerEntry, InventoryLedgerEntry, PaymentRow, Transaction, TransactionLine,
};
use df_domain::Money;
use df_payments::SurchargeRule;
use df_sync::{Outbox, OutboxEntry};
use rust_decimal::Decimal;
use uuid::Uuid;

use super::Edge;
use crate::error::EdgeError;
use crate::hw::{ReceiptData, ReceiptLine};
use crate::payments::{
    PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus, TenderKind, compute_surcharge,
    surcharge_rule_for,
};
use crate::pricing::{PricingContext, price_lines};
use crate::types::{CompletedSale, SaleInput, TenderInput};

/// One resolved tender: what the customer owes on it, the surcharge
/// (card only) and the total actually collected.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResolvedTender {
    pub(super) kind: TenderKind,
    /// Portion of the pre-surcharge total this tender covers.
    pub(super) tendered: Money,
    pub(super) surcharge: Money,
    /// Total collected (`tendered + surcharge`).
    pub(super) charged: Money,
    /// The reference the counter supplied with the tender (only
    /// E-Transfer uses it — the customer's confirmation number).
    pub(super) reference: Option<String>,
}

/// Resolve tender amounts against a pre-surcharge total.
///
/// `None` amounts mean "the remaining balance". Cash may exceed the
/// remainder (the excess is change due); card / charge / gift-card
/// tenders may not exceed it. Under-tendering is an error. A tender's
/// surcharge comes from `rules` via
/// [`df_payments::compute_surcharge`] (default rules: 3% on card
/// only).
pub(super) fn resolve_tenders(
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
            reference: tender.reference.clone(),
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

/// Best-effort rollback of already-authorized card tenders when a
/// later split-tender fails or the local SQLite commit fails.
pub(super) async fn void_authorized_cards(provider: &dyn PaymentProvider, refs: &[String]) {
    for reference in refs.iter().rev() {
        let _ = provider.void(reference).await;
    }
}

impl Edge {
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
        self.checkout_as(input, provider, None).await
    }

    /// Same as [`Edge::checkout`], stamping `cashier_id` when supplied
    /// (falling back to the open till's cashier when `None`). Any card
    /// authorization taken before a split-tender decline or failed DB
    /// commit is automatically voided via [`PaymentProvider::void`].
    pub async fn checkout_as(
        &self,
        input: SaleInput,
        provider: &dyn PaymentProvider,
        cashier_id: Option<Uuid>,
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
        let price_level_id = self.effective_price_level(&input).await;
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
        let (resolved, change_due) =
            resolve_tenders(&input.tenders, pre_surcharge, &self.config.surcharge_rules)?;
        let pos_fee_total = resolved
            .iter()
            .map(|t| t.surcharge)
            .fold(Money::zero(self.currency()), |acc, s| {
                acc.checked_add(s).unwrap_or(acc)
            });
        let grand_total = pre_surcharge.checked_add(pos_fee_total)?;

        // Charge-account credit check (Phase 3): runs before any card
        // authorization or write, so a blocked charge leaves nothing
        // behind.
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

        // Authorize card tenders before writing anything. Track every
        // approved card reference so a later split-tender decline/error
        // or a failed DB commit automatically voids prior authorizations.
        let mut payments: Vec<PaymentResult> = Vec::with_capacity(resolved.len());
        let mut authorized_card_refs: Vec<String> = Vec::new();
        for tender in &resolved {
            match tender.kind {
                kind if kind.is_card() => {
                    let result = match provider
                        .authorize(&PaymentRequest {
                            amount: tender.charged,
                            idempotency_key: Uuid::now_v7(),
                            tender: kind,
                            surcharge: tender.surcharge,
                        })
                        .await
                    {
                        Ok(res) => res,
                        Err(err) => {
                            void_authorized_cards(provider, &authorized_card_refs).await;
                            return Err(EdgeError::Payment(err));
                        }
                    };
                    if result.status != PaymentStatus::Approved {
                        void_authorized_cards(provider, &authorized_card_refs).await;
                        return Err(EdgeError::PaymentDeclined(format!(
                            "card authorization declined (ref {})",
                            result.reference
                        )));
                    }
                    authorized_card_refs.push(result.reference.clone());
                    payments.push(result);
                }
                TenderKind::ETransfer => payments.push(PaymentResult {
                    status: PaymentStatus::Approved,
                    authorization: None,
                    reference: tender
                        .reference
                        .clone()
                        .filter(|reference| !reference.trim().is_empty())
                        .unwrap_or_else(|| "ETRANSFER".to_string()),
                    amount: tender.charged,
                    surcharge: tender.surcharge,
                    tender: TenderKind::ETransfer,
                }),
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
                // `resolve_tenders` only produces the kinds above;
                // the guard arm covers every card brand.
                _ => unreachable!("unresolved tender kind reached authorization"),
            }
        }

        // Persist the sale in one local SQLite transaction. If ANY DB
        // step or the commit fails, auto-void every card authorization
        // taken above so the customer is never left with an orphaned charge.
        let persist_result: Result<(Uuid, String), EdgeError> = async {
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
                job_name: input.job_name.clone(),
                salesperson_id: None,
                cashier_id: cashier_id.or_else(|| till.as_ref().map(|t| t.cashier_id)),
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
                        "delta_committed": "0",
                        "delta_on_order": "0",
                        "delta_in_transit": "0",
                        "unit_cost": line.unit_cost.amount().to_string(),
                        "lot_number": null,
                        "reason_type": "sale",
                        "reference_id": tx_id.to_string(),
                        "created_at": now.to_rfc3339(),
                    }),
                ));
            }

            // Charge-account AR entry (the charge portion only — card
            // surcharges never land on AR).
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
                        "reference_type": "transaction",
                        "reference_id": tx_id.to_string(),
                        "due_date": charge_due_date.map(|d| d.to_string()),
                        "job_id": input.job_id.map(|id| id.to_string()),
                        "created_at": now.to_rfc3339(),
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
                    "job_id": input.job_id.map(|id| id.to_string()),
                    "po_number": input.po_number.clone(),
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
            Ok((tx_id, doc_number))
        }
        .await;

        let (tx_id, doc_number) = match persist_result {
            Ok(ok) => ok,
            Err(err) => {
                void_authorized_cards(provider, &authorized_card_refs).await;
                return Err(err);
            }
        };

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

    /// HTTP checkout: uses the configured default provider (or the
    /// mock provider installed by `df-server edge`). A card tender
    /// with no provider configured is a 503-style error here.
    pub async fn checkout_with_default_provider(
        &self,
        input: SaleInput,
    ) -> Result<CompletedSale, EdgeError> {
        self.checkout_with_default_provider_as(input, None).await
    }

    /// HTTP checkout attributed to the signed-in cashier's `user_id`.
    pub async fn checkout_with_default_provider_as(
        &self,
        input: SaleInput,
        cashier_id: Option<Uuid>,
    ) -> Result<CompletedSale, EdgeError> {
        let has_card = input.tenders.iter().any(|t| t.kind.is_card());
        let provider = self.http_provider(has_card)?;
        self.checkout_as(input, provider.as_ref(), cashier_id).await
    }
}
