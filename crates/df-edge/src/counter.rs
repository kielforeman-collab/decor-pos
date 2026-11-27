//! Counter search + customer/transaction views for the POS UI
//! (v1.2.1 follow-up; spec §6.1 counter wiring).
//!
//! Thin read methods over the `df-db` repositories — the HTTP
//! handlers in [`crate::http`] are thinner still. All money is
//! `Decimal` (exact strings on the wire), never floats.

use std::collections::HashMap;

use df_db::models::{Customer, PaymentRow, PriceLevel, Product, Transaction, TransactionLine};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// One row of `GET /pos/products/search`: the product plus its
/// on-hand stock at this store. `msrp` / `map` are `null` when the
/// price list left them blank (most BM rows).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductSearchResult {
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub base_uom: String,
    pub list_price: Decimal,
    pub standard_cost: Decimal,
    pub msrp: Option<Decimal>,
    pub map: Option<Decimal>,
    pub container_volume_ml: Option<i32>,
    pub on_hand: Decimal,
}

/// One transaction line with the product SKU resolved. The stored
/// [`TransactionLine`] carries only `product_id`; the counter UI
/// filters and displays by SKU, so the view resolves it (a missing
/// product yields `null`, never a failed read).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionLineView {
    #[serde(flatten)]
    pub line: TransactionLine,
    pub sku: Option<String>,
}

/// One transaction with its lines and payments, as returned by
/// `GET /pos/customers/{id}/transactions` and
/// `GET /pos/transactions/{id}`: the header fields flattened, then
/// `lines` and `payments`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransactionView {
    #[serde(flatten)]
    pub transaction: Transaction,
    pub lines: Vec<TransactionLineView>,
    pub payments: Vec<PaymentRow>,
}

fn contains_needle(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(needle)
}

impl Edge {
    /// Search products for the counter: exact `sku`, exact
    /// `barcode` (a barcode is a SKU — `Product` has no separate
    /// barcode field), or a case-insensitive substring over
    /// sku/name for `q`. At most `limit` rows (default 50, hard
    /// cap 50), ordered by SKU. `on_hand` comes from the inventory
    /// balances at this store.
    pub async fn search_products(
        &self,
        q: Option<&str>,
        sku: Option<&str>,
        barcode: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<ProductSearchResult>, EdgeError> {
        let limit = limit.unwrap_or(50).min(50);
        let products: Vec<Product> = if let Some(sku) = sku.filter(|s| !s.trim().is_empty()) {
            vec![df_db::get_product_by_sku(self.pool(), self.config().org_id, sku.trim()).await?]
        } else if let Some(barcode) = barcode.filter(|s| !s.trim().is_empty()) {
            vec![
                df_db::get_product_by_sku(self.pool(), self.config().org_id, barcode.trim())
                    .await?,
            ]
        } else {
            let all = df_db::list_products_for_org(self.pool(), self.config().org_id).await?;
            match q.filter(|s| !s.trim().is_empty()) {
                Some(q) => {
                    let needle = q.trim().to_lowercase();
                    all.into_iter()
                        .filter(|p| {
                            contains_needle(&p.sku, &needle) || contains_needle(&p.name, &needle)
                        })
                        .collect()
                }
                None => all,
            }
        };
        let balances =
            df_db::inventory_balances(self.pool(), self.config().org_id, self.config().store_id)
                .await?;
        let on_hand: HashMap<Uuid, Decimal> = balances
            .into_iter()
            .map(|b| (b.product_id, b.on_hand))
            .collect();
        Ok(products
            .into_iter()
            .take(limit)
            .map(|p| ProductSearchResult {
                on_hand: on_hand.get(&p.id).copied().unwrap_or(Decimal::ZERO),
                product_id: p.id,
                sku: p.sku,
                name: p.name,
                base_uom: p.base_uom,
                list_price: p.list_price,
                standard_cost: p.standard_cost,
                msrp: p.msrp,
                map: p.map,
                container_volume_ml: p.container_volume_ml,
            })
            .collect())
    }

    /// Search customers for the counter: a case-insensitive
    /// substring over account number, company name, first/last
    /// name and email. An empty query returns every customer.
    pub async fn search_customers(&self, q: Option<&str>) -> Result<Vec<Customer>, EdgeError> {
        let all = df_db::list_customers_for_org(self.pool(), self.config().org_id).await?;
        match q.filter(|s| !s.trim().is_empty()) {
            Some(q) => {
                let needle = q.trim().to_lowercase();
                Ok(all
                    .into_iter()
                    .filter(|c| {
                        contains_needle(&c.account_number, &needle)
                            || c.company_name
                                .as_deref()
                                .is_some_and(|v| contains_needle(v, &needle))
                            || c.first_name
                                .as_deref()
                                .is_some_and(|v| contains_needle(v, &needle))
                            || c.last_name
                                .as_deref()
                                .is_some_and(|v| contains_needle(v, &needle))
                            || c.email
                                .as_deref()
                                .is_some_and(|v| contains_needle(v, &needle))
                    })
                    .collect())
            }
            None => Ok(all),
        }
    }

    /// Set a customer's price level (`POST
    /// /pos/customers/{id}/price-level`). A non-null level must
    /// exist; `null` clears the level back to list pricing.
    /// Returns the updated customer.
    pub async fn set_customer_price_level(
        &self,
        id: Uuid,
        price_level_id: Option<Uuid>,
    ) -> Result<Customer, EdgeError> {
        df_db::get_customer(self.pool(), id).await?;
        if let Some(level_id) = price_level_id {
            df_db::get_price_level(self.pool(), level_id).await?;
        }
        df_db::update_customer_price_level(self.pool(), id, price_level_id).await?;
        Ok(df_db::get_customer(self.pool(), id).await?)
    }

    /// One transaction with its lines and payments (404 when the
    /// id is unknown).
    pub async fn transaction_view(&self, id: Uuid) -> Result<TransactionView, EdgeError> {
        let transaction = df_db::get_transaction(self.pool(), id).await?;
        let lines = self.lines_with_skus(id).await?;
        let payments = df_db::list_payments_for_transaction(self.pool(), id).await?;
        Ok(TransactionView {
            transaction,
            lines,
            payments,
        })
    }

    /// Transaction lines with each line's product SKU resolved.
    async fn lines_with_skus(
        &self,
        transaction_id: Uuid,
    ) -> Result<Vec<TransactionLineView>, EdgeError> {
        let lines = df_db::list_transaction_lines(self.pool(), transaction_id).await?;
        let mut views = Vec::with_capacity(lines.len());
        for line in lines {
            let sku = df_db::get_product(self.pool(), line.product_id)
                .await
                .ok()
                .map(|product| product.sku);
            views.push(TransactionLineView { line, sku });
        }
        Ok(views)
    }

    /// A customer's transactions, newest first, each with its lines
    /// and payments. `limit` defaults to 50.
    pub async fn customer_transactions(
        &self,
        customer_id: Uuid,
        limit: Option<usize>,
    ) -> Result<Vec<TransactionView>, EdgeError> {
        df_db::get_customer(self.pool(), customer_id).await?;
        let limit = limit.unwrap_or(50);
        let transactions = df_db::list_transactions_for_customer(self.pool(), customer_id).await?;
        let mut views = Vec::with_capacity(transactions.len().min(limit));
        for tx in transactions.into_iter().take(limit) {
            let lines = self.lines_with_skus(tx.id).await?;
            let payments = df_db::list_payments_for_transaction(self.pool(), tx.id).await?;
            views.push(TransactionView {
                transaction: tx,
                lines,
                payments,
            });
        }
        Ok(views)
    }

    /// The four seeded price levels for this org.
    pub async fn list_price_levels(&self) -> Result<Vec<PriceLevel>, EdgeError> {
        Ok(df_db::list_price_levels(self.pool(), self.config().org_id).await?)
    }
}
