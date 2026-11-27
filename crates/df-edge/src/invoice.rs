//! The letter-size invoice render data for the counter
//! (`GET /pos/transactions/{id}/invoice`).
//!
//! Everything comes from the stored transaction: totals are the
//! stored totals, never recomputed from scratch. Tax is stored as a
//! single `tax_total` (the pricing engine keeps only the total —
//! see `df-edge::pricing`), so the GST/PST split is derived from
//! it only for a BC store (GST 5% + PST 7% = 12%): GST is 5/12 of
//! the stored tax, PST is the remainder, so the two always sum
//! exactly to the stored total. Any other jurisdiction reports
//! GST/PST as zero and the stored `tax_total` stands on its own
//! (the invoice carries `tax_total` too, so the render always
//! reconciles). All money is `Decimal` — never floats.

use df_db::models::{Customer, PaymentRow, TransactionLine};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// One tint on an invoice line, when the line was tinted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceTint {
    pub color_name: String,
    pub color_code: String,
    /// The base product's name (what was tinted).
    pub base: String,
    pub formula: Vec<InvoiceFormulaLine>,
}

/// One colorant in a tint formula: code + ounces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceFormulaLine {
    pub code: String,
    pub oz: Decimal,
}

/// One invoice line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceLine {
    pub line_no: i32,
    pub sku: String,
    pub description: String,
    pub qty: Decimal,
    pub uom: String,
    pub unit_price: Decimal,
    pub discount_amount: Decimal,
    pub line_total: Decimal,
    /// Short container label from the product's volume, e.g.
    /// `Gallon`, `Quart`, `5-gal pail`, `Sample pot`.
    pub size_label: String,
    pub tint: Option<InvoiceTint>,
}

/// One tender on the invoice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceTender {
    pub label: String,
    pub amount: Decimal,
    pub tendered: Decimal,
    pub change: Decimal,
}

/// The charge-account block, present only when the sale took a
/// charge tender. Balances are the live AR ledger: the previous
/// balance is the current balance less this sale's charge (the
/// charge is already posted), so the block shows the account
/// before → after this invoice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceCharge {
    pub previous_balance: Decimal,
    pub charged: Decimal,
    pub new_balance: Decimal,
    pub credit_limit: Decimal,
    pub available: Decimal,
}

/// Invoice totals. `delivery` is zero: counter sales carry no
/// delivery charge (deliveries are separate jobs). `tax_total` is
/// the stored total the GST/PST split derives from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceTotals {
    pub subtotal: Decimal,
    pub discounts: Decimal,
    pub eco_fee_total: Decimal,
    pub delivery: Decimal,
    pub gst: Decimal,
    pub pst: Decimal,
    pub tax_total: Decimal,
    pub grand_total: Decimal,
    pub change_due: Decimal,
    pub charge: Option<InvoiceCharge>,
}

/// The invoice render data for one transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceView {
    pub store_name: String,
    pub doc_number: String,
    pub invoice_number: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub customer: InvoiceCustomer,
    pub job_name: Option<String>,
    pub po_number: Option<String>,
    pub fulfilment_mode: String,
    pub lines: Vec<InvoiceLine>,
    pub tenders: Vec<InvoiceTender>,
    pub totals: InvoiceTotals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceCustomer {
    pub name: Option<String>,
    pub account_number: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub payment_terms: String,
}

/// Split a stored BC tax total into GST (5%) and PST (7%): GST is
/// 5/12 of the total rounded to 4dp, PST is the remainder, so
/// `gst + pst == tax_total` exactly.
pub fn split_bc_tax(tax_total: Decimal) -> (Decimal, Decimal) {
    let gst = (tax_total * Decimal::from(5) / Decimal::from(12)).round_dp(4);
    (gst, tax_total - gst)
}

/// Short container label for a volume in ml. Exact bands only;
/// anything else reports its millilitres, and a missing volume
/// falls back to the line's UOM.
pub fn size_label_for_volume(volume_ml: Option<i32>, uom: &str) -> String {
    match volume_ml {
        Some(3785) => "Gallon".to_string(),
        Some(946) => "Quart".to_string(),
        Some(18927) => "5-gal pail".to_string(),
        Some(237) => "Sample pot".to_string(),
        Some(ml) => format!("{ml} ml"),
        None => uom.to_string(),
    }
}

/// The display label for a stored tender string (`payments.tender`).
pub fn tender_label(tender: &str) -> String {
    match tender {
        "cash" => "Cash".to_string(),
        "card" => "Card".to_string(),
        "visa" => "Visa".to_string(),
        "mastercard" => "Mastercard".to_string(),
        "amex" => "Amex".to_string(),
        "debit" => "Debit".to_string(),
        "etransfer" => "E-Transfer".to_string(),
        "charge_account" => "Charge Account".to_string(),
        "gift_card" => "Gift Card".to_string(),
        other => other.to_string(),
    }
}

fn json_decimal(value: &serde_json::Value) -> Option<Decimal> {
    match value {
        serde_json::Value::String(text) => text.parse().ok(),
        serde_json::Value::Number(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

impl Edge {
    /// Build the invoice render data for a transaction (404 when
    /// the id is unknown).
    pub async fn invoice(&self, id: Uuid) -> Result<InvoiceView, EdgeError> {
        let tx = df_db::get_transaction(self.pool(), id).await?;
        let lines = df_db::list_transaction_lines(self.pool(), id).await?;
        let payments = df_db::list_payments_for_transaction(self.pool(), id).await?;
        let customer: Option<Customer> = match tx.customer_id {
            Some(customer_id) => Some(df_db::get_customer(self.pool(), customer_id).await?),
            None => None,
        };

        let mut invoice_lines = Vec::with_capacity(lines.len());
        let mut discounts = Decimal::ZERO;
        for line in &lines {
            discounts += line.discount_amount;
            let product = df_db::get_product(self.pool(), line.product_id).await.ok();
            let (sku, volume_ml) = match &product {
                Some(product) => (product.sku.clone(), product.container_volume_ml),
                None => (String::new(), None),
            };
            let tint = self.invoice_tint(line).await?;
            invoice_lines.push(InvoiceLine {
                line_no: line.line_no,
                sku,
                description: line.description.clone(),
                qty: line.qty,
                uom: line.uom_code.clone(),
                unit_price: line.unit_price,
                discount_amount: line.discount_amount,
                line_total: line.line_total,
                size_label: size_label_for_volume(volume_ml, &line.uom_code),
                tint,
            });
        }

        // Change due is not stored on the transaction: it is the
        // excess collected over the grand total (cash over-tender).
        let collected: Decimal = payments.iter().map(|p| p.amount).sum();
        let change_due = if collected > tx.grand_total {
            collected - tx.grand_total
        } else {
            Decimal::ZERO
        };
        let tenders = payments
            .iter()
            .map(|p| InvoiceTender {
                label: tender_label(&p.tender),
                amount: p.amount,
                tendered: p.amount,
                change: if p.tender == "cash" {
                    change_due
                } else {
                    Decimal::ZERO
                },
            })
            .collect();

        // GST/PST: BC only, split from the stored total (see the
        // module docs). Anywhere else both are zero and `tax_total`
        // carries the tax.
        let (gst, pst) = match df_db::get_store(self.pool(), self.config().store_id).await {
            Ok(store) if store.country_code == "CA" && store.region_code == "BC" => {
                split_bc_tax(tx.tax_total)
            }
            _ => (Decimal::ZERO, Decimal::ZERO),
        };

        let charge = self.invoice_charge(&payments, customer.as_ref()).await?;

        Ok(InvoiceView {
            store_name: self.config().store_name.clone(),
            doc_number: tx.doc_number.clone(),
            invoice_number: tx.doc_number.clone(),
            created_at: tx.created_at,
            customer: match &customer {
                Some(customer) => InvoiceCustomer {
                    name: customer.company_name.clone().or_else(|| {
                        let name = format!(
                            "{} {}",
                            customer.first_name.clone().unwrap_or_default(),
                            customer.last_name.clone().unwrap_or_default()
                        );
                        let name = name.trim().to_string();
                        if name.is_empty() { None } else { Some(name) }
                    }),
                    account_number: Some(customer.account_number.clone()),
                    email: customer.email.clone(),
                    phone: customer.phone.clone(),
                    payment_terms: customer.payment_terms.clone(),
                },
                None => InvoiceCustomer {
                    name: None,
                    account_number: None,
                    email: None,
                    phone: None,
                    payment_terms: "NET30".to_string(),
                },
            },
            job_name: tx.job_name.clone(),
            po_number: tx.po_number.clone(),
            fulfilment_mode: tx.fulfillment_mode.clone(),
            lines: invoice_lines,
            tenders,
            totals: InvoiceTotals {
                subtotal: tx.subtotal,
                discounts,
                eco_fee_total: tx.eco_fee_total,
                delivery: Decimal::ZERO,
                gst,
                pst,
                tax_total: tx.tax_total,
                grand_total: tx.grand_total,
                change_due,
                charge,
            },
        })
    }

    /// The tint block for one sale line, when the line links a
    /// stored formula. The formula lines carry `{code, oz}`; the
    /// base is the formula's base product name.
    async fn invoice_tint(&self, line: &TransactionLine) -> Result<Option<InvoiceTint>, EdgeError> {
        let Some(formula_id) = line.tint_formula_id else {
            return Ok(None);
        };
        let formula = df_db::get_tint_formula(self.pool(), formula_id).await?;
        let base = df_db::get_product(self.pool(), formula.base_product_id)
            .await
            .map(|p| p.name)
            .unwrap_or_default();
        let mut formula_lines = Vec::new();
        if let Some(items) = formula.colorants_json.as_array() {
            for item in items {
                let code = item
                    .get("code")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                let oz = item
                    .get("oz")
                    .and_then(json_decimal)
                    .unwrap_or(Decimal::ZERO);
                formula_lines.push(InvoiceFormulaLine { code, oz });
            }
        }
        Ok(Some(InvoiceTint {
            color_name: formula.color_name_snapshot,
            color_code: formula.color_code_snapshot,
            base,
            formula: formula_lines,
        }))
    }

    /// The charge block for a sale that took a charge tender, or
    /// `None` when it did not.
    async fn invoice_charge(
        &self,
        payments: &[PaymentRow],
        customer: Option<&Customer>,
    ) -> Result<Option<InvoiceCharge>, EdgeError> {
        let Some(charge_payment) = payments.iter().find(|p| p.tender == "charge_account") else {
            return Ok(None);
        };
        let Some(customer) = customer else {
            return Ok(None);
        };
        let new_balance = df_db::ar_balance_for_customer(self.pool(), customer.id).await?;
        let charged = charge_payment.amount;
        Ok(Some(InvoiceCharge {
            previous_balance: new_balance - charged,
            charged,
            new_balance,
            credit_limit: customer.credit_limit,
            available: customer.credit_limit - new_balance,
        }))
    }
}
