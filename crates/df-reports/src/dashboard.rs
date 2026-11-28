//! Store dashboard aggregate (Products / Vendors / Dashboards
//! screens; spec §7).
//!
//! One [`DashboardAggregate`] per store and date range, computed
//! from the real edge ledgers — no second set of books, no invented
//! numbers. Empty sections come back as `[]` / zeros, never `null`.
//!
//! * **Sales + tint** — reused from [`kpi_dashboard`]: the same
//!   completed `sale` / `return` transactions in `[start, end]`
//!   (return subtotals are stored negative, so they reduce the net)
//!   and the same stored `tint_formulas` created in the period
//!   (tracking only). Calling `kpi_dashboard` internally also
//!   computes margin / inventory turns / AR aging, which this
//!   aggregate does not surface — that is the cost of reusing the
//!   one sales derivation instead of maintaining a second.
//! * **Daily** — one row per day in `[start, end]`, including zero
//!   days. `net_sales` is the day's completed-sale subtotals minus
//!   the day's return subtotals (stored negative); `sale_count`
//!   counts completed `sale` documents only.
//! * **By category** — completed-sale lines grouped by the
//!   product's `product_type` string (the import stores
//!   `"paint_base"` / `"stock"` there; there is no department name
//!   on the product row, so `product_type` is the grouping). Line
//!   revenue is `line_total - discount_amount`, the same definition
//!   the KPI margin uses; return lines carry negative quantities
//!   and totals, so they reduce their category.
//! * **Tenders** — payments on the completed transactions in the
//!   range, grouped by the stored `payments.tender` value and shown
//!   under its receipt label (`visa` → `Visa`, … — the same mapping
//!   as the invoice's `tender_label`). Refund payments on returns
//!   reduce their tender's total.
//! * **Top products / customers** — top 10 by revenue / net sales.
//!   Customer names are the customer's display name: company name,
//!   else first + last name, else the account number.
//! * **Low stock** — products whose ledger on-hand balance is below
//!   their `reorder_points` minimum at this store (the
//!   `reorder_points` table from migration `0005_phase3`, the same
//!   table `GET /inventory/reorder-points` reads). Products with no
//!   reorder point are not low-stock by definition — there is
//!   nothing to compare them against.
//!
//! All money is `rust_decimal::Decimal` via [`df_domain::Money`] —
//! never floats. `share_pct` is a plain number 0–100 (an `f64`,
//! rounded to 2 decimal places); it is `0.0` when the section total
//! is zero.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ReportError;
use crate::kpi::{FormulaColorUsage, kpi_dashboard};

/// Sales totals for the dashboard period (subset of the KPI sales:
/// no `returns` magnitude — the dashboard shape carries the net).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardSales {
    pub sale_count: u32,
    pub return_count: u32,
    pub gross_sales: Money,
    pub net_sales: Money,
    pub grand_total: Money,
    /// `net_sales / sale_count`, `None` when there were no sales.
    pub avg_ticket: Option<Money>,
}

/// One day in `[start, end]`, including zero days.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailySalesRow {
    pub date: NaiveDate,
    pub net_sales: Money,
    pub sale_count: u32,
}

/// Completed-sale line revenue grouped by `product_type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryRow {
    pub category: String,
    pub net_sales: Money,
    /// Share of the period's total category revenue, 0–100.
    pub share_pct: f64,
}

/// Payments grouped by tender label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenderRow {
    pub tender: String,
    pub amount: Money,
    pub count: u32,
    /// Share of the period's total tendered amount, 0–100.
    pub share_pct: f64,
}

/// One of the top 10 products by line revenue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopProductRow {
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub qty: Decimal,
    pub revenue: Money,
}

/// One of the top 10 customers by net sales.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopCustomerRow {
    pub customer_id: Uuid,
    pub name: String,
    pub net_sales: Money,
    pub sale_count: u32,
}

/// Formula usage in the period — stored formulas only (tracking).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardTint {
    pub formulas_created: u32,
    pub total_tint_cost: Money,
    pub per_color: Vec<FormulaColorUsage>,
}

/// One product below its reorder-point minimum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LowStockRow {
    pub product_id: Uuid,
    pub sku: String,
    pub name: String,
    pub on_hand: Decimal,
    /// Always present: the row exists because a reorder point set
    /// this minimum. `Option` only because the dashboard contract
    /// allows `null` for a minimum that was never set.
    pub min_qty: Option<Decimal>,
}

/// One store's dashboard aggregate for `[start, end]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardAggregate {
    pub store_id: Uuid,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub sales: DashboardSales,
    pub daily: Vec<DailySalesRow>,
    pub by_category: Vec<CategoryRow>,
    pub tenders: Vec<TenderRow>,
    pub top_products: Vec<TopProductRow>,
    pub top_customers: Vec<TopCustomerRow>,
    pub tint: DashboardTint,
    pub low_stock: Vec<LowStockRow>,
}

/// The display label for a stored tender string
/// (`payments.tender`): the same mapping as the invoice's
/// `tender_label` in `df-edge` (duplicated here because `df-reports`
/// does not depend on `df-edge`). Unknown values pass through
/// unchanged rather than inventing a label.
fn tender_label(tender: &str) -> String {
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

/// A customer's display name: company name, else first + last
/// name, else the account number.
fn customer_display_name(customer: &df_db::models::Customer) -> String {
    if let Some(company) = &customer.company_name {
        if !company.trim().is_empty() {
            return company.clone();
        }
    }
    let first = customer.first_name.clone().unwrap_or_default();
    let last = customer.last_name.clone().unwrap_or_default();
    let joined = format!("{first} {last}").trim().to_string();
    if !joined.is_empty() {
        return joined;
    }
    customer.account_number.clone()
}

/// Share of `total`, as a number 0–100 rounded to 2 decimals.
/// `0.0` when the total is zero (or the division fails).
fn share_pct(part: Decimal, total: Decimal) -> f64 {
    if total.is_zero() {
        return 0.0;
    }
    let ratio = match part
        .checked_div(total)
        .and_then(|r| r.checked_mul(Decimal::from(100)))
    {
        Some(ratio) => ratio,
        None => return 0.0,
    };
    let as_f64 = ratio.to_f64().unwrap_or(0.0);
    (as_f64 * 100.0).round() / 100.0
}

/// Compute one store's dashboard aggregate for `[start, end]`.
/// `end` must not precede `start`.
pub async fn dashboard_aggregate(
    pool: &df_db::AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    start: NaiveDate,
    end: NaiveDate,
    currency: Currency,
) -> Result<DashboardAggregate, ReportError> {
    if end < start {
        return Err(ReportError::InvalidInput(format!(
            "dashboard end {end} precedes start {start}"
        )));
    }
    let zero = Money::zero(currency);

    // --- sales + tint, reused from the KPI dashboard ------------------
    let kpi = kpi_dashboard(pool, org_id, store_id, start, end, end, currency).await?;
    let sales = DashboardSales {
        sale_count: kpi.sales.sale_count,
        return_count: kpi.sales.return_count,
        gross_sales: kpi.sales.gross_sales,
        net_sales: kpi.sales.net_sales,
        grand_total: kpi.sales.grand_total,
        avg_ticket: kpi.sales.avg_ticket,
    };
    let tint = DashboardTint {
        formulas_created: kpi.formulas.formulas_created,
        total_tint_cost: kpi.formulas.total_tint_cost,
        per_color: kpi.formulas.per_color,
    };

    // --- one pass over the period's completed transactions ------------
    let products = df_db::list_products_for_org(pool, org_id).await?;
    let product_by_id: BTreeMap<Uuid, &df_db::models::Product> =
        products.iter().map(|p| (p.id, p)).collect();

    // Daily rows, pre-seeded with every day in the range at zero.
    let mut daily_map: BTreeMap<NaiveDate, (Money, u32)> = BTreeMap::new();
    let mut day = start;
    while day <= end {
        daily_map.insert(day, (zero, 0));
        day = match day.succ_opt() {
            Some(next) => next,
            None => break,
        };
    }

    let mut category_map: BTreeMap<String, Money> = BTreeMap::new();
    let mut tender_map: BTreeMap<String, (Money, u32)> = BTreeMap::new();
    let mut product_map: BTreeMap<Uuid, (Decimal, Money)> = BTreeMap::new();
    let mut customer_map: BTreeMap<Uuid, (Money, u32)> = BTreeMap::new();

    let transactions = df_db::list_transactions_for_store(pool, store_id).await?;
    for tx in &transactions {
        let date = tx.created_at.date_naive();
        if date < start || date > end || tx.status != "completed" {
            continue;
        }
        let is_sale = tx.doc_type == "sale";
        let is_return = tx.doc_type == "return";
        if !is_sale && !is_return {
            continue;
        }
        // Daily: return subtotals are stored negative, so adding
        // the subtotal nets correctly for both doc types.
        if let Some((net, count)) = daily_map.get_mut(&date) {
            *net = net.checked_add(Money::new(tx.subtotal, currency))?;
            if is_sale {
                *count += 1;
            }
        }
        if let Some(customer_id) = tx.customer_id {
            let entry = customer_map.entry(customer_id).or_insert((zero, 0));
            entry.0 = entry.0.checked_add(Money::new(tx.subtotal, currency))?;
            if is_sale {
                entry.1 += 1;
            }
        }
        for line in df_db::list_transaction_lines(pool, tx.id).await? {
            let Some(product) = product_by_id.get(&line.product_id) else {
                continue;
            };
            let line_revenue = Money::new(line.line_total - line.discount_amount, currency);
            let category_entry = category_map
                .entry(product.product_type.clone())
                .or_insert(zero);
            *category_entry = category_entry.checked_add(line_revenue)?;
            let product_entry = product_map
                .entry(line.product_id)
                .or_insert((Decimal::ZERO, zero));
            product_entry.0 += line.qty;
            product_entry.1 = product_entry.1.checked_add(line_revenue)?;
        }
        for payment in df_db::list_payments_for_transaction(pool, tx.id).await? {
            let entry = tender_map
                .entry(tender_label(&payment.tender))
                .or_insert((zero, 0));
            entry.0 = entry.0.checked_add(Money::new(payment.amount, currency))?;
            entry.1 += 1;
        }
    }

    let daily: Vec<DailySalesRow> = daily_map
        .into_iter()
        .map(|(date, (net_sales, sale_count))| DailySalesRow {
            date,
            net_sales,
            sale_count,
        })
        .collect();

    let category_total = category_map
        .values()
        .fold(Decimal::ZERO, |acc, money| acc + money.amount());
    let mut by_category: Vec<CategoryRow> = category_map
        .into_iter()
        .map(|(category, net_sales)| CategoryRow {
            share_pct: share_pct(net_sales.amount(), category_total),
            category,
            net_sales,
        })
        .collect();
    by_category.sort_by(|a, b| {
        b.net_sales
            .amount()
            .cmp(&a.net_sales.amount())
            .then(a.category.cmp(&b.category))
    });

    let tender_total = tender_map
        .values()
        .fold(Decimal::ZERO, |acc, (money, _)| acc + money.amount());
    let mut tenders: Vec<TenderRow> = tender_map
        .into_iter()
        .map(|(tender, (amount, count))| TenderRow {
            share_pct: share_pct(amount.amount(), tender_total),
            tender,
            amount,
            count,
        })
        .collect();
    tenders.sort_by(|a, b| {
        b.amount
            .amount()
            .cmp(&a.amount.amount())
            .then(a.tender.cmp(&b.tender))
    });

    let mut top_products: Vec<TopProductRow> = product_map
        .into_iter()
        .filter_map(|(product_id, (qty, revenue))| {
            let product = product_by_id.get(&product_id)?;
            Some(TopProductRow {
                product_id,
                sku: product.sku.clone(),
                name: product.name.clone(),
                qty,
                revenue,
            })
        })
        .collect();
    top_products.sort_by(|a, b| {
        b.revenue
            .amount()
            .cmp(&a.revenue.amount())
            .then(a.sku.cmp(&b.sku))
    });
    top_products.truncate(10);

    let mut top_customers: Vec<TopCustomerRow> = Vec::new();
    for (customer_id, (net_sales, sale_count)) in customer_map {
        let customer = df_db::get_customer(pool, customer_id).await?;
        top_customers.push(TopCustomerRow {
            customer_id,
            name: customer_display_name(&customer),
            net_sales,
            sale_count,
        });
    }
    top_customers.sort_by(|a, b| {
        b.net_sales
            .amount()
            .cmp(&a.net_sales.amount())
            .then(a.name.cmp(&b.name))
    });
    top_customers.truncate(10);

    // --- low stock: ledger balances vs reorder-point minimums --------
    let balances = df_db::inventory_balances(pool, org_id, store_id).await?;
    let mut low_stock: Vec<LowStockRow> = Vec::new();
    for point in df_db::list_reorder_points_for_store(pool, store_id).await? {
        let Some(balance) = balances.iter().find(|b| b.product_id == point.product_id) else {
            continue;
        };
        if balance.on_hand < point.min_qty {
            low_stock.push(LowStockRow {
                product_id: point.product_id,
                sku: balance.sku.clone(),
                name: balance.name.clone(),
                on_hand: balance.on_hand,
                min_qty: Some(point.min_qty),
            });
        }
    }
    low_stock.sort_by(|a, b| a.sku.cmp(&b.sku));

    Ok(DashboardAggregate {
        store_id,
        start,
        end,
        sales,
        daily,
        by_category,
        tenders,
        top_products,
        top_customers,
        tint,
        low_stock,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use chrono::{TimeZone, Utc};
    use df_db::models::{
        Customer, InventoryLedgerEntry, Organization, PaymentRow, Product, Store, Transaction,
        TransactionLine,
    };
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn at(year: i32, month: u32, day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap()
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    async fn seed_org_store(pool: &df_db::AnyPool) -> (Uuid, Uuid) {
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
        df_db::insert_organization(
            pool,
            &Organization {
                id: org_id,
                name: "Test Paint Co".to_string(),
                default_currency: "USD".to_string(),
                enabled_modules: serde_json::json!({}),
                settings_json: serde_json::json!({}),
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        df_db::insert_store(
            pool,
            &Store {
                id: store_id,
                org_id,
                code: "MAIN".to_string(),
                name: "Main Store".to_string(),
                country_code: "US".to_string(),
                region_code: "CA".to_string(),
                postal_code: "90210".to_string(),
                currency: "USD".to_string(),
                tax_zone_id: Uuid::now_v7(),
                timezone: "America/Los_Angeles".to_string(),
                is_warehouse: false,
            },
        )
        .await
        .unwrap();
        (org_id, store_id)
    }

    fn product(org_id: Uuid, id: Uuid, sku: &str, product_type: &str) -> Product {
        Product {
            id,
            org_id,
            sku: sku.to_string(),
            manufacturer_code: None,
            name: sku.to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: product_type.to_string(),
            base_uom: "gal".to_string(),
            list_price: dec("100.0000"),
            standard_cost: dec("50.0000"),
            avg_cost: dec("50.0000"),
            min_margin_pct: None,
            sheen: None,
            paint_base_code: None,
            container_volume_ml: Some(3785),
            eco_fee_category: Some("architectural_paint".to_string()),
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            upc: None,
            category: None,
            line_code: None,
            color_name: None,
            size_code: None,
            source_metadata: None,
            is_active: true,
            map: None,
            msrp: None,
        }
    }

    fn transaction(
        org_id: Uuid,
        store_id: Uuid,
        doc_type: &str,
        subtotal: &str,
        grand: &str,
        customer_id: Option<Uuid>,
        created_at: chrono::DateTime<Utc>,
    ) -> Transaction {
        Transaction {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            till_session_id: None,
            doc_number: format!("TX-{}", Uuid::now_v7()),
            doc_type: doc_type.to_string(),
            status: "completed".to_string(),
            customer_id,
            job_id: None,
            po_number: None,
            salesperson_id: None,
            cashier_id: None,
            currency: "USD".to_string(),
            subtotal: dec(subtotal),
            tax_total: Decimal::ZERO,
            eco_fee_total: Decimal::ZERO,
            pos_fee_total: Decimal::ZERO,
            grand_total: dec(grand),
            deposit_paid: Decimal::ZERO,
            fulfillment_mode: "counter".to_string(),
            scheduled_at: None,
            completed_at: Some(created_at),
            created_at,
            job_name: None,
        }
    }

    fn line(tx_id: Uuid, product_id: Uuid, qty: &str, price: &str) -> TransactionLine {
        let qty = dec(qty);
        TransactionLine {
            id: Uuid::now_v7(),
            transaction_id: tx_id,
            line_no: 1,
            product_id,
            description: "line".to_string(),
            qty,
            uom_code: "gal".to_string(),
            unit_price: dec(price),
            base_unit_cost: dec("50.0000"),
            tint_unit_cost: Decimal::ZERO,
            discount_amount: Decimal::ZERO,
            tax_amount: Decimal::ZERO,
            eco_fee_amount: Decimal::ZERO,
            line_total: dec(price) * qty,
            tint_formula_id: None,
            prompts_json: serde_json::json!({}),
            vertical_spec_json: serde_json::json!({}),
            linked_po_line_id: None,
            fulfilled_qty: qty,
        }
    }

    fn payment(tx_id: Uuid, tender: &str, amount: &str) -> PaymentRow {
        PaymentRow {
            id: Uuid::now_v7(),
            transaction_id: tx_id,
            tender: tender.to_string(),
            amount: dec(amount),
            reference: "test".to_string(),
            authorization: None,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn dashboard_aggregates_a_sale_and_zero_days() {
        let pool = test_pool().await;
        let (org_id, store_id) = seed_org_store(&pool).await;
        let paint = Uuid::from_u128(0x3000);
        let brush = Uuid::from_u128(0x3001);
        df_db::insert_product(&pool, &product(org_id, paint, "A-1G", "paint_base"))
            .await
            .unwrap();
        df_db::insert_product(&pool, &product(org_id, brush, "BRUSH", "stock"))
            .await
            .unwrap();
        let customer_id = Uuid::from_u128(0x4000);
        df_db::insert_customer(
            &pool,
            &Customer {
                id: customer_id,
                org_id,
                account_number: "C-1".to_string(),
                customer_type: "cash".to_string(),
                company_name: Some("Acme Painting".to_string()),
                first_name: None,
                last_name: None,
                email: None,
                phone: None,
                price_level_id: None,
                salesperson_id: None,
                tax_exempt_cert: None,
                tax_exempt_expires: None,
                charge_enabled: false,
                credit_limit: Decimal::ZERO,
                payment_terms: "NET30".to_string(),
                terms_days: None,
                finance_rate_pct_monthly: None,
                credit_hold: false,
                require_po_number: false,
                require_job_name: false,
                portal_enabled: false,
                address_line1: None,
                address_line2: None,
                city: None,
                province: None,
                postal_code: None,
                notes: None,
            },
        )
        .await
        .unwrap();
        // Stock: 10 gal paint on hand, 0 brushes; reorder point on
        // brushes (min 2) makes brushes low-stock, paint is not.
        df_db::insert_inventory_delta(
            &pool,
            &InventoryLedgerEntry {
                id: Uuid::now_v7(),
                org_id,
                store_id,
                product_id: paint,
                lot_number: None,
                delta_on_hand: dec("10"),
                delta_committed: Decimal::ZERO,
                delta_on_order: Decimal::ZERO,
                delta_in_transit: Decimal::ZERO,
                unit_cost: dec("50.0000"),
                reason_type: "receipt".to_string(),
                reference_id: Uuid::now_v7(),
                created_at: at(2026, 1, 4),
            },
        )
        .await
        .unwrap();
        df_db::upsert_reorder_point(
            &pool,
            &df_db::models::ReorderPoint {
                id: Uuid::now_v7(),
                store_id,
                product_id: brush,
                min_qty: dec("2"),
                max_qty: dec("10"),
                reorder_qty: dec("5"),
            },
        )
        .await
        .unwrap();
        // One sale Jan 5: 2 gal paint @ 100 + 1 brush @ 100:
        // subtotal 300, paid Visa 300.
        let tx = transaction(
            org_id,
            store_id,
            "sale",
            "300.0000",
            "300.0000",
            Some(customer_id),
            at(2026, 1, 5),
        );
        let tx_id = tx.id;
        df_db::insert_transaction(&pool, &tx).await.unwrap();
        df_db::insert_transaction_line(&pool, &line(tx_id, paint, "2", "100.0000"))
            .await
            .unwrap();
        df_db::insert_transaction_line(&pool, &line(tx_id, brush, "1", "100.0000"))
            .await
            .unwrap();
        df_db::insert_payment(&pool, &payment(tx_id, "visa", "300.0000"))
            .await
            .unwrap();

        let start = date(2026, 1, 5);
        let end = date(2026, 1, 6);
        let agg = dashboard_aggregate(&pool, org_id, store_id, start, end, Currency::Usd)
            .await
            .unwrap();
        assert_eq!(agg.sales.sale_count, 1);
        assert_eq!(agg.sales.return_count, 0);
        assert_eq!(agg.sales.gross_sales.amount(), dec("300.0000"));
        assert_eq!(agg.sales.net_sales.amount(), dec("300.0000"));
        assert_eq!(agg.sales.grand_total.amount(), dec("300.0000"));
        assert_eq!(agg.sales.avg_ticket.unwrap().amount(), dec("300.0000"));
        // Daily: Jan 5 has the sale, Jan 6 is a zero row.
        assert_eq!(agg.daily.len(), 2);
        assert_eq!(agg.daily[0].date, start);
        assert_eq!(agg.daily[0].net_sales.amount(), dec("300.0000"));
        assert_eq!(agg.daily[0].sale_count, 1);
        assert_eq!(agg.daily[1].net_sales.amount(), Decimal::ZERO);
        assert_eq!(agg.daily[1].sale_count, 0);
        // Categories: paint_base 200 (66.67%), stock 100 (33.33%).
        assert_eq!(agg.by_category.len(), 2);
        assert_eq!(agg.by_category[0].category, "paint_base");
        assert_eq!(agg.by_category[0].net_sales.amount(), dec("200.0000"));
        assert!((agg.by_category[0].share_pct - 66.67).abs() < 0.01);
        assert_eq!(agg.by_category[1].category, "stock");
        // Tenders: one Visa payment of 300, 100%.
        assert_eq!(agg.tenders.len(), 1);
        assert_eq!(agg.tenders[0].tender, "Visa");
        assert_eq!(agg.tenders[0].amount.amount(), dec("300.0000"));
        assert_eq!(agg.tenders[0].count, 1);
        assert_eq!(agg.tenders[0].share_pct, 100.0);
        // Top products: paint first (revenue 200), brush second.
        assert_eq!(agg.top_products.len(), 2);
        assert_eq!(agg.top_products[0].sku, "A-1G");
        assert_eq!(agg.top_products[0].qty, dec("2"));
        assert_eq!(agg.top_products[0].revenue.amount(), dec("200.0000"));
        // Top customers: Acme Painting, net 300, 1 sale.
        assert_eq!(agg.top_customers.len(), 1);
        assert_eq!(agg.top_customers[0].name, "Acme Painting");
        assert_eq!(agg.top_customers[0].net_sales.amount(), dec("300.0000"));
        assert_eq!(agg.top_customers[0].sale_count, 1);
        // Tint: nothing created in the period.
        assert_eq!(agg.tint.formulas_created, 0);
        assert!(agg.tint.per_color.is_empty());
        // Low stock: brushes only (on hand 0 < min 2).
        assert_eq!(agg.low_stock.len(), 1);
        assert_eq!(agg.low_stock[0].sku, "BRUSH");
        assert_eq!(agg.low_stock[0].on_hand, Decimal::ZERO);
        assert_eq!(agg.low_stock[0].min_qty, Some(dec("2")));
    }

    #[tokio::test]
    async fn dashboard_empty_range_is_zeros_not_nulls() {
        let pool = test_pool().await;
        let (org_id, store_id) = seed_org_store(&pool).await;
        let start = date(2026, 2, 1);
        let end = date(2026, 2, 3);
        let agg = dashboard_aggregate(&pool, org_id, store_id, start, end, Currency::Usd)
            .await
            .unwrap();
        assert_eq!(agg.sales.sale_count, 0);
        assert_eq!(agg.sales.gross_sales.amount(), Decimal::ZERO);
        assert!(agg.sales.avg_ticket.is_none());
        assert_eq!(agg.daily.len(), 3);
        assert!(agg.daily.iter().all(|d| d.sale_count == 0));
        assert!(agg.by_category.is_empty());
        assert!(agg.tenders.is_empty());
        assert!(agg.top_products.is_empty());
        assert!(agg.top_customers.is_empty());
        assert!(agg.low_stock.is_empty());
    }

    #[tokio::test]
    async fn dashboard_rejects_an_inverted_range() {
        let pool = test_pool().await;
        let (org_id, store_id) = seed_org_store(&pool).await;
        let err = dashboard_aggregate(
            &pool,
            org_id,
            store_id,
            date(2026, 1, 6),
            date(2026, 1, 5),
            Currency::Usd,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("precedes start"));
    }
}
