//! PaintCare / eco-fee remittance reports (Phase 6, v1.0; spec §7).
//!
//! Eco fees are collected per transaction line
//! (`transaction_lines.eco_fee_amount`, computed at checkout from
//! the `eco_fee_rules` bands) and remitted per jurisdiction. A
//! jurisdiction here is a store's `(country_code, region_code)` —
//! the transaction itself carries no jurisdiction, and the store
//! row is the authority for where the sale happened. The report
//! covers every store of the org, so a multi-store org remits one
//! row per store jurisdiction (and per eco-fee category within it).
//!
//! Output is the structured [`EcoRemittanceReport`] (JSON) plus a
//! CSV rendering ([`render_eco_csv`]) suitable for the remittance
//! filing itself. All money is `rust_decimal::Decimal` — never
//! floats. Only completed `sale` / `return` transactions in
//! `[start, end]` count (returns carry negative eco fees, so they
//! reduce the remittance with the correct sign).

use std::collections::BTreeMap;

use chrono::NaiveDate;
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ReportError;

/// Eco fees owed to one jurisdiction, for one eco-fee category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EcoJurisdictionRow {
    pub country_code: String,
    pub region_code: String,
    /// The product's `eco_fee_category` (`None` when the product
    /// carries none — the fee was still charged, so the row is
    /// reported under `"uncategorized"` in CSV).
    pub eco_fee_category: Option<String>,
    pub store_id: Uuid,
    pub store_code: String,
    /// Distinct completed transactions contributing eco fees.
    pub transaction_count: u32,
    pub line_count: u32,
    /// Total units (gallons/containers) the eco fees were charged
    /// on, in each line's base UOM quantity.
    pub units: Decimal,
    pub eco_fee_total: Money,
}

/// Eco-fee remittance for an org over `[start, end]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EcoRemittanceReport {
    pub org_id: Uuid,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub currency: Currency,
    /// Sorted by country, region, then category.
    pub rows: Vec<EcoJurisdictionRow>,
    pub total: Money,
}

impl EcoRemittanceReport {
    /// Plain-text rendering (one line per jurisdiction row).
    pub fn render_text(&self) -> String {
        let mut out = format!(
            "Eco-fee remittance — {} to {} — total {}\n",
            self.start,
            self.end,
            self.total.amount()
        );
        for row in &self.rows {
            out.push_str(&format!(
                "  {}-{} ({}): {} transaction(s), {} line(s), {} units, eco fees {}\n",
                row.country_code,
                row.region_code,
                row.eco_fee_category.as_deref().unwrap_or("uncategorized"),
                row.transaction_count,
                row.line_count,
                row.units,
                row.eco_fee_total.amount()
            ));
        }
        out
    }
}

/// Build the eco-fee remittance for an org over `[start, end]`.
pub async fn eco_remittance_report(
    pool: &df_db::AnyPool,
    org_id: Uuid,
    start: NaiveDate,
    end: NaiveDate,
    currency: Currency,
) -> Result<EcoRemittanceReport, ReportError> {
    if end < start {
        return Err(ReportError::InvalidInput(format!(
            "remittance end {end} precedes start {start}"
        )));
    }
    let zero = Money::zero(currency);
    let products = df_db::list_products_for_org(pool, org_id).await?;
    let category_of = |product_id: Uuid| {
        products
            .iter()
            .find(|p| p.id == product_id)
            .and_then(|p| p.eco_fee_category.clone())
    };
    let stores = df_db::list_stores_for_org(pool, org_id).await?;
    let mut rows: BTreeMap<(String, String, Option<String>, Uuid), EcoJurisdictionRow> =
        BTreeMap::new();
    let mut total = zero;
    for store in &stores {
        let transactions = df_db::list_transactions_for_store(pool, store.id).await?;
        for tx in &transactions {
            let date = tx.created_at.date_naive();
            if date < start
                || date > end
                || tx.status != "completed"
                || (tx.doc_type != "sale" && tx.doc_type != "return")
            {
                continue;
            }
            for line in df_db::list_transaction_lines(pool, tx.id).await? {
                if line.eco_fee_amount == Decimal::ZERO {
                    continue;
                }
                let category = category_of(line.product_id);
                let key = (
                    store.country_code.clone(),
                    store.region_code.clone(),
                    category.clone(),
                    store.id,
                );
                let row = rows.entry(key).or_insert_with(|| EcoJurisdictionRow {
                    country_code: store.country_code.clone(),
                    region_code: store.region_code.clone(),
                    eco_fee_category: category.clone(),
                    store_id: store.id,
                    store_code: store.code.clone(),
                    transaction_count: 0,
                    line_count: 0,
                    units: Decimal::ZERO,
                    eco_fee_total: zero,
                });
                row.line_count += 1;
                row.units += line.qty;
                row.eco_fee_total = row
                    .eco_fee_total
                    .checked_add(Money::new(line.eco_fee_amount, currency))?;
                total = total.checked_add(Money::new(line.eco_fee_amount, currency))?;
            }
        }
    }
    // Transaction counts need the tx ids, so recompute them in a
    // dedicated pass (kept separate so the accumulation above stays
    // a straight fold).
    for store in &stores {
        let transactions = df_db::list_transactions_for_store(pool, store.id).await?;
        for tx in &transactions {
            let date = tx.created_at.date_naive();
            if date < start
                || date > end
                || tx.status != "completed"
                || (tx.doc_type != "sale" && tx.doc_type != "return")
            {
                continue;
            }
            let mut categories: BTreeMap<Option<String>, bool> = BTreeMap::new();
            for line in df_db::list_transaction_lines(pool, tx.id).await? {
                if line.eco_fee_amount == Decimal::ZERO {
                    continue;
                }
                categories.insert(category_of(line.product_id), true);
            }
            for category in categories.keys() {
                let key = (
                    store.country_code.clone(),
                    store.region_code.clone(),
                    category.clone(),
                    store.id,
                );
                if let Some(row) = rows.get_mut(&key) {
                    row.transaction_count += 1;
                }
            }
        }
    }
    let mut rows: Vec<EcoJurisdictionRow> = rows.into_values().collect();
    rows.sort_by(|a, b| {
        a.country_code
            .cmp(&b.country_code)
            .then(a.region_code.cmp(&b.region_code))
            .then(
                a.eco_fee_category
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.eco_fee_category.as_deref().unwrap_or("")),
            )
    });
    Ok(EcoRemittanceReport {
        org_id,
        start,
        end,
        currency,
        rows,
        total,
    })
}

/// Render a remittance report as CSV (header + one row per
/// jurisdiction/category + a `TOTAL` row). Fields are plain
/// (no quoting needed: codes and categories contain no commas in
/// the seeded data, and the writer quotes any field that does).
pub fn render_eco_csv(report: &EcoRemittanceReport) -> String {
    fn field(value: &str) -> String {
        if value.contains(',') || value.contains('"') || value.contains('\n') {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_string()
        }
    }
    let mut out = String::from(
        "country_code,region_code,store_code,eco_fee_category,transaction_count,line_count,units,eco_fee_total\n",
    );
    for row in &report.rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            field(&row.country_code),
            field(&row.region_code),
            field(&row.store_code),
            field(row.eco_fee_category.as_deref().unwrap_or("uncategorized")),
            row.transaction_count,
            row.line_count,
            row.units,
            row.eco_fee_total.amount()
        ));
    }
    out.push_str(&format!("TOTAL,,,,,,,{}\n", report.total.amount()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use chrono::{TimeZone, Utc};
    use df_db::models::{Organization, Product, Store, Transaction, TransactionLine};
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn at(year: i32, month: u32, day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap()
    }

    async fn seed_two_stores(pool: &df_db::AnyPool) -> (Uuid, Uuid, Uuid, Uuid) {
        let org_id = Uuid::from_u128(0x2000);
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
        let mut stores = Vec::new();
        for (id, code, country, region) in [
            (0x1000u128, "MAIN", "US", "CA"),
            (0x1001u128, "NORTH", "CA", "BC"),
        ] {
            let store_id = Uuid::from_u128(id);
            df_db::insert_store(
                pool,
                &Store {
                    id: store_id,
                    org_id,
                    code: code.to_string(),
                    name: code.to_string(),
                    country_code: country.to_string(),
                    region_code: region.to_string(),
                    postal_code: "00000".to_string(),
                    currency: "USD".to_string(),
                    tax_zone_id: Uuid::now_v7(),
                    timezone: "UTC".to_string(),
                    is_warehouse: false,
                },
            )
            .await
            .unwrap();
            stores.push(store_id);
        }
        let product_a = Uuid::from_u128(0x3000);
        let product_b = Uuid::from_u128(0x3001);
        for (id, sku, category) in [
            (product_a, "A-1G", Some("architectural_paint")),
            (product_b, "B-1G", Some("aerosol")),
        ] {
            df_db::insert_product(
                pool,
                &Product {
                    id,
                    org_id,
                    sku: sku.to_string(),
                    manufacturer_code: None,
                    name: sku.to_string(),
                    department_id: Uuid::now_v7(),
                    class_id: None,
                    product_line_id: None,
                    primary_vendor_id: None,
                    product_type: "paint_base".to_string(),
                    base_uom: "gal".to_string(),
                    list_price: dec("100.0000"),
                    standard_cost: dec("50.0000"),
                    avg_cost: dec("50.0000"),
                    min_margin_pct: None,
                    sheen: None,
                    paint_base_code: None,
                    container_volume_ml: Some(3785),
                    eco_fee_category: category.map(str::to_string),
                    tax_class_id: Uuid::now_v7(),
                    prompt_template_id: None,
                    is_active: true,
                    map: None,
                    msrp: None,
                },
            )
            .await
            .unwrap();
        }
        (org_id, stores[0], product_a, product_b)
    }

    fn tx(
        org_id: Uuid,
        store_id: Uuid,
        doc_type: &str,
        eco: &str,
        grand: &str,
        when: chrono::DateTime<Utc>,
    ) -> Transaction {
        Transaction {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            till_session_id: None,
            doc_number: format!("TX-{}", Uuid::now_v7()),
            doc_type: doc_type.to_string(),
            status: "completed".to_string(),
            customer_id: None,
            job_id: None,
            po_number: None,
            salesperson_id: None,
            cashier_id: None,
            currency: "USD".to_string(),
            subtotal: Decimal::ZERO,
            tax_total: Decimal::ZERO,
            eco_fee_total: dec(eco),
            pos_fee_total: Decimal::ZERO,
            grand_total: dec(grand),
            deposit_paid: Decimal::ZERO,
            fulfillment_mode: "counter".to_string(),
            scheduled_at: None,
            completed_at: Some(when),
            created_at: when,
            job_name: None,
        }
    }

    fn line(tx_id: Uuid, product_id: Uuid, qty: &str, eco: &str) -> TransactionLine {
        TransactionLine {
            id: Uuid::now_v7(),
            transaction_id: tx_id,
            line_no: 1,
            product_id,
            description: "line".to_string(),
            qty: dec(qty),
            uom_code: "gal".to_string(),
            unit_price: dec("100.0000"),
            base_unit_cost: dec("50.0000"),
            tint_unit_cost: Decimal::ZERO,
            discount_amount: Decimal::ZERO,
            tax_amount: Decimal::ZERO,
            eco_fee_amount: dec(eco),
            line_total: dec("100.0000") * dec(qty),
            tint_formula_id: None,
            prompts_json: serde_json::json!({}),
            vertical_spec_json: serde_json::json!({}),
            linked_po_line_id: None,
            fulfilled_qty: dec(qty),
        }
    }

    #[tokio::test]
    async fn eco_remittance_groups_by_jurisdiction_and_category() {
        let pool = test_pool().await;
        let (org_id, store_us, product_a, product_b) = seed_two_stores(&pool).await;
        let store_ca = Uuid::from_u128(0x1001);
        let start = NaiveDate::from_ymd_opt(2026, 2, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 2, 28).unwrap();
        // US-CA MAIN: sale Feb 3 — 2 x A @ 0.75 eco = 1.50, plus
        // 1 x B @ 1.25 eco = 1.25 (two categories, one tx);
        // return Feb 10 — 1 x A @ -0.75.
        let sale = tx(
            org_id,
            store_us,
            "sale",
            "2.7500",
            "202.7500",
            at(2026, 2, 3),
        );
        let sale_id = sale.id;
        df_db::insert_transaction(&pool, &sale).await.unwrap();
        df_db::insert_transaction_line(&pool, &line(sale_id, product_a, "2", "1.5000"))
            .await
            .unwrap();
        df_db::insert_transaction_line(&pool, &line(sale_id, product_b, "1", "1.2500"))
            .await
            .unwrap();
        let ret = tx(
            org_id,
            store_us,
            "return",
            "-0.7500",
            "-100.7500",
            at(2026, 2, 10),
        );
        let ret_id = ret.id;
        df_db::insert_transaction(&pool, &ret).await.unwrap();
        df_db::insert_transaction_line(&pool, &line(ret_id, product_a, "-1", "-0.7500"))
            .await
            .unwrap();
        // CA-BC NORTH: sale Feb 5 — 3 x A @ 0.50 eco = 1.50.
        let bc_sale = tx(
            org_id,
            store_ca,
            "sale",
            "1.5000",
            "301.5000",
            at(2026, 2, 5),
        );
        let bc_id = bc_sale.id;
        df_db::insert_transaction(&pool, &bc_sale).await.unwrap();
        df_db::insert_transaction_line(&pool, &line(bc_id, product_a, "3", "1.5000"))
            .await
            .unwrap();
        // Out of period (March) — must be excluded.
        let march = tx(
            org_id,
            store_us,
            "sale",
            "9.0000",
            "109.0000",
            at(2026, 3, 2),
        );
        let march_id = march.id;
        df_db::insert_transaction(&pool, &march).await.unwrap();
        df_db::insert_transaction_line(&pool, &line(march_id, product_a, "12", "9.0000"))
            .await
            .unwrap();

        let report = eco_remittance_report(&pool, org_id, start, end, Currency::Usd)
            .await
            .unwrap();
        // US-CA architectural_paint: 1.50 - 0.75 = 0.75 over 2
        // txs (sale + return), 2 lines, 2 - 1 = 1 unit.
        // US-CA aerosol: 1.25, 1 tx, 1 line, 1 unit.
        // CA-BC architectural_paint: 1.50, 1 tx, 1 line, 3 units.
        // Total 0.75 + 1.25 + 1.50 = 3.50.
        assert_eq!(report.rows.len(), 3);
        assert_eq!(report.total.amount(), dec("3.5000"));
        let us_paint = report
            .rows
            .iter()
            .find(|r| {
                r.country_code == "US"
                    && r.eco_fee_category.as_deref() == Some("architectural_paint")
            })
            .unwrap();
        assert_eq!(us_paint.region_code, "CA");
        assert_eq!(us_paint.eco_fee_total.amount(), dec("0.7500"));
        assert_eq!(us_paint.transaction_count, 2);
        assert_eq!(us_paint.line_count, 2);
        assert_eq!(us_paint.units, dec("1"));
        let us_aerosol = report
            .rows
            .iter()
            .find(|r| r.eco_fee_category.as_deref() == Some("aerosol"))
            .unwrap();
        assert_eq!(us_aerosol.eco_fee_total.amount(), dec("1.2500"));
        assert_eq!(us_aerosol.transaction_count, 1);
        let bc = report.rows.iter().find(|r| r.country_code == "CA").unwrap();
        assert_eq!(bc.region_code, "BC");
        assert_eq!(bc.eco_fee_total.amount(), dec("1.5000"));
        assert_eq!(bc.units, dec("3"));

        let csv = render_eco_csv(&report);
        assert!(csv.starts_with(
            "country_code,region_code,store_code,eco_fee_category,transaction_count,line_count,units,eco_fee_total\n"
        ));
        assert!(csv.contains("US,CA,MAIN,architectural_paint,2,2,1,0.7500"));
        assert!(csv.contains("CA,BC,NORTH,architectural_paint,1,1,3,1.5000"));
        assert!(csv.ends_with("TOTAL,,,,,,,3.5000\n"));
        assert!(report.render_text().contains("total 3.5000"));
    }

    #[tokio::test]
    async fn eco_remittance_empty_org_reports_zero() {
        let pool = test_pool().await;
        let org_id = Uuid::from_u128(0x2000);
        df_db::insert_organization(
            &pool,
            &Organization {
                id: org_id,
                name: "Empty Co".to_string(),
                default_currency: "USD".to_string(),
                enabled_modules: serde_json::json!({}),
                settings_json: serde_json::json!({}),
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        let report = eco_remittance_report(
            &pool,
            org_id,
            NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 2, 28).unwrap(),
            Currency::Usd,
        )
        .await
        .unwrap();
        assert!(report.rows.is_empty());
        assert_eq!(report.total.amount(), Decimal::ZERO);
        assert_eq!(
            render_eco_csv(&report),
            "country_code,region_code,store_code,eco_fee_category,transaction_count,line_count,units,eco_fee_total\nTOTAL,,,,,,,0.0000\n"
        );
    }
}
