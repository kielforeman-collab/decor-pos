//! Till open/close and Z-report calculation.

use chrono::Utc;
use df_db::models::TillSession;
use df_domain::Money;
use uuid::Uuid;

use super::{Edge, money_add};
use crate::error::EdgeError;
use crate::types::ZReport;

impl Edge {
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
                            // Generic card + every branded card kind.
                            "card" | "visa" | "mastercard" | "amex" | "debit" => {
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
}
