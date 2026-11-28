//! Counter holds, quotes, layaways, and transaction voids.

use chrono::Utc;
use df_db::DbError;
use df_db::models::{PaymentRow, Transaction, TransactionLine};
use df_domain::Money;
use df_sync::{Outbox, OutboxEntry};
use rust_decimal::Decimal;
use uuid::Uuid;

use super::Edge;
use super::checkout::void_authorized_cards;
use crate::error::EdgeError;
use crate::payments::{
    PaymentProvider, PaymentRequest, PaymentResult, PaymentStatus, TenderKind, compute_surcharge,
    surcharge_rule_for,
};
use crate::pricing::{PricingContext, price_lines};
use crate::types::{SaleInput, TenderInput};

impl Edge {
    // --- holds & quotes -----------------------------------------------------

    /// Park a sale at the counter; persisted in SQLite (`pos_holds`,
    /// migration `0017`) so parked carts survive edge restarts.
    pub async fn park_hold(&self, input: SaleInput) -> Result<Uuid, EdgeError> {
        self.park_hold_as(input, None).await
    }

    /// Same as [`Edge::park_hold`], attributing the hold to `cashier_id`.
    pub async fn park_hold_as(
        &self,
        input: SaleInput,
        cashier_id: Option<Uuid>,
    ) -> Result<Uuid, EdgeError> {
        self.validate_lines(&input.lines)?;
        let hold_id = Uuid::now_v7();
        let payload_json = serde_json::to_string(&input)
            .map_err(|e| EdgeError::InvalidInput(format!("serialize hold: {e}")))?;
        df_db::insert_pos_hold(
            &self.pool,
            &df_db::PosHoldRow {
                id: hold_id,
                store_id: self.config.store_id,
                cashier_id,
                payload_json,
                created_at: Utc::now(),
            },
        )
        .await?;
        Ok(hold_id)
    }

    /// Resume a parked hold (atomically removes it from `pos_holds`).
    pub async fn resume_hold(&self, hold_id: Uuid) -> Result<SaleInput, EdgeError> {
        let row = df_db::take_pos_hold(&self.pool, self.config.store_id, hold_id)
            .await
            .map_err(|e| match e {
                DbError::NotFound(_) => EdgeError::NotFound(format!("hold {hold_id}")),
                other => EdgeError::Db(other),
            })?;
        serde_json::from_str(&row.payload_json)
            .map_err(|e| EdgeError::InvalidInput(format!("corrupt hold payload {hold_id}: {e}")))
    }

    /// Ids of currently parked holds for this store (DB-backed,
    /// oldest first).
    pub async fn list_holds(&self) -> Vec<Uuid> {
        df_db::list_pos_holds(&self.pool, self.config.store_id)
            .await
            .map(|rows| rows.into_iter().map(|r| r.id).collect())
            .unwrap_or_default()
    }

    /// Record a quote: persisted as a `doc_type = 'quote'` transaction
    /// with lines, but no inventory movement, no payments and no
    /// outbox entries.
    pub async fn record_quote(&self, input: SaleInput) -> Result<Uuid, EdgeError> {
        self.record_quote_as(input, None).await
    }

    /// Same as [`Edge::record_quote`], stamping `cashier_id`.
    pub async fn record_quote_as(
        &self,
        input: SaleInput,
        cashier_id: Option<Uuid>,
    ) -> Result<Uuid, EdgeError> {
        self.validate_lines(&input.lines)?;
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
                job_name: input.job_name.clone(),
                salesperson_id: None,
                cashier_id,
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
    /// movement and no AR entry. Returns the layaway transaction id.
    pub async fn record_layaway(
        &self,
        input: SaleInput,
        deposit: TenderInput,
        provider: &dyn PaymentProvider,
    ) -> Result<Uuid, EdgeError> {
        self.record_layaway_as(input, deposit, provider, None).await
    }

    /// Same as [`Edge::record_layaway`], stamping `cashier_id` and
    /// auto-voiding an authorized card deposit if the local DB commit fails.
    pub async fn record_layaway_as(
        &self,
        input: SaleInput,
        deposit: TenderInput,
        provider: &dyn PaymentProvider,
        cashier_id: Option<Uuid>,
    ) -> Result<Uuid, EdgeError> {
        self.validate_lines(&input.lines)?;
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

        let mut authorized_card_refs: Vec<String> = Vec::new();
        let payment = if deposit.kind.is_card() {
            let result = provider
                .authorize(&PaymentRequest {
                    amount: charged,
                    idempotency_key: Uuid::now_v7(),
                    tender: deposit.kind,
                    surcharge,
                })
                .await?;
            if result.status != PaymentStatus::Approved {
                return Err(EdgeError::PaymentDeclined(format!(
                    "card authorization declined (ref {})",
                    result.reference
                )));
            }
            authorized_card_refs.push(result.reference.clone());
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
                    TenderKind::ETransfer => deposit
                        .reference
                        .clone()
                        .filter(|reference| !reference.trim().is_empty())
                        .unwrap_or_else(|| "ETRANSFER".to_string()),
                    _ => unreachable!("card tenders authorize above"),
                },
                amount: charged,
                surcharge,
                tender: deposit.kind,
            }
        };

        let persist_result: Result<Uuid, EdgeError> = async {
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
                    job_name: input.job_name.clone(),
                    salesperson_id: None,
                    cashier_id,
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
                        "job_id": input.job_id.map(|id| id.to_string()),
                        "po_number": input.po_number.clone(),
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
        .await;

        match persist_result {
            Ok(tx_id) => Ok(tx_id),
            Err(err) => {
                void_authorized_cards(provider, &authorized_card_refs).await;
                Err(err)
            }
        }
    }

    /// HTTP layaway: uses the configured default provider (or the
    /// mock provider when the deposit is not a card tender).
    pub async fn record_layaway_with_default_provider(
        &self,
        input: SaleInput,
        deposit: TenderInput,
    ) -> Result<Uuid, EdgeError> {
        self.record_layaway_with_default_provider_as(input, deposit, None)
            .await
    }

    /// HTTP layaway attributed to the signed-in cashier's `user_id`.
    pub async fn record_layaway_with_default_provider_as(
        &self,
        input: SaleInput,
        deposit: TenderInput,
        cashier_id: Option<Uuid>,
    ) -> Result<Uuid, EdgeError> {
        let provider = self.http_provider(deposit.kind.is_card())?;
        self.record_layaway_as(input, deposit, provider.as_ref(), cashier_id)
            .await
    }

    /// Void a transaction, voiding any original card payment references
    /// through the configured payment provider before flipping status to
    /// `"voided"`.
    pub async fn void_transaction(&self, id: Uuid) -> Result<Transaction, EdgeError> {
        let provider = self.default_provider();
        self.void_transaction_with_provider(id, provider.as_deref())
            .await
    }

    /// Void a transaction with an explicit optional payment provider.
    pub async fn void_transaction_with_provider(
        &self,
        id: Uuid,
        provider: Option<&dyn PaymentProvider>,
    ) -> Result<Transaction, EdgeError> {
        let tx = df_db::get_transaction(&self.pool, id).await?;
        if tx.status == "voided" {
            return Ok(tx);
        }
        let payments = df_db::list_payments_for_transaction(&self.pool, id).await?;
        let card_refs: Vec<String> = payments
            .iter()
            .filter(|p| {
                p.amount > Decimal::ZERO
                    && TenderKind::from_str_opt(&p.tender).is_some_and(TenderKind::is_card)
            })
            .map(|p| p.reference.clone())
            .collect();
        if !card_refs.is_empty()
            && let Some(provider) = provider
        {
            for reference in &card_refs {
                provider.void(reference).await?;
            }
        }
        df_db::set_transaction_status(&self.pool, id, "voided").await?;
        Ok(df_db::get_transaction(&self.pool, id).await?)
    }
}
