//! Daily usage report (Phase 2, v0.3; spec §7 — formula tracking only).
//!
//! For one store and one (UTC) date:
//!
//! * tracked formulas — count of `tint_formulas` recorded that day
//!   and a per-color breakdown (by the formula's color snapshot);
//! * colorant usage — per colorant, exact fl oz summed from each
//!   formula's `colorants_json` (`oz + shots / shots_per_fl_oz`),
//!   plus cost at the colorant's `cost_per_fl_oz` via `df-domain`
//!   decimal math;
//! * sales totals — completed `sale` / `return` transactions created
//!   that day (subtotal, tax, eco fee, POS fee, grand total).
//!
//! Dates are compared in Rust, not SQL, so the same query works on
//! both SQLite and PostgreSQL.

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::NaiveDate;
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ReportError;

/// Tracked formulas recorded for one color on the day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorUsage {
    pub color_code: String,
    pub color_name: String,
    pub tints: u32,
}

/// One colorant used across tracked formulas on the day: exact fl oz and cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorantUsage {
    /// `None` when the colorant row was deleted after the formula was
    /// recorded — the code and fl oz are still reported, cost is zero.
    pub colorant_id: Option<Uuid>,
    pub code: String,
    pub name: String,
    pub fl_oz: Decimal,
    pub cost: Money,
}

/// Sales totals for the day. `grand_total` sums completed sales and
/// returns as stored (returns carry negative totals), so it is the
/// net day total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SalesTotals {
    pub sale_count: u32,
    pub return_count: u32,
    pub subtotal: Money,
    pub tax_total: Money,
    pub eco_fee_total: Money,
    pub pos_fee_total: Money,
    pub grand_total: Money,
}

/// The daily usage report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyUsageReport {
    pub store_id: Uuid,
    pub org_id: Uuid,
    pub date: NaiveDate,
    pub currency: Currency,
    pub formulas_recorded: u32,
    /// Sorted by tint count descending, then color code.
    pub per_color: Vec<ColorUsage>,
    /// Sorted by colorant code.
    pub colorant_used: Vec<ColorantUsage>,
    pub total_tint_cost: Money,
    pub sales: SalesTotals,
}

impl DailyUsageReport {
    /// Plain-text rendering (one section per part of the report).
    pub fn render_text(&self) -> String {
        let mut out = format!(
            "Daily usage report — store {} — {}\n",
            self.store_id, self.date
        );
        out.push_str(&format!("Formulas recorded: {}\n", self.formulas_recorded));
        out.push_str("Per color:\n");
        if self.per_color.is_empty() {
            out.push_str("  (none)\n");
        }
        for color in &self.per_color {
            out.push_str(&format!(
                "  {} ({}): {} formula(s)\n",
                color.color_code, color.color_name, color.tints
            ));
        }
        out.push_str("Colorant used:\n");
        if self.colorant_used.is_empty() {
            out.push_str("  (none)\n");
        }
        for usage in &self.colorant_used {
            out.push_str(&format!(
                "  {} ({}): {} fl oz, cost {}\n",
                usage.code,
                usage.name,
                usage.fl_oz,
                usage.cost.amount()
            ));
        }
        out.push_str(&format!(
            "Total tint cost: {}\n",
            self.total_tint_cost.amount()
        ));
        let sales = &self.sales;
        out.push_str(&format!(
            "Sales: {} sale(s), {} return(s); subtotal {}, tax {}, eco {}, POS fees {}, grand total {}\n",
            sales.sale_count,
            sales.return_count,
            sales.subtotal.amount(),
            sales.tax_total.amount(),
            sales.eco_fee_total.amount(),
            sales.pos_fee_total.amount(),
            sales.grand_total.amount()
        ));
        out
    }
}

fn json_decimal(value: &serde_json::Value) -> Option<Decimal> {
    match value {
        serde_json::Value::String(text) => Decimal::from_str(text).ok(),
        serde_json::Value::Number(number) => Decimal::from_str(&number.to_string()).ok(),
        _ => None,
    }
}

fn parse_formula_colorants(
    value: &serde_json::Value,
    colorants: &[df_db::models::Colorant],
) -> Result<Vec<(String, Decimal)>, ReportError> {
    let items = value.as_array().ok_or_else(|| {
        ReportError::InvalidInput(format!("colorants_json is not an array: {value}"))
    })?;
    let mut lines = Vec::with_capacity(items.len());
    for item in items {
        let code = item
            .get("code")
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                ReportError::InvalidInput(format!("formula colorant has no code: {item}"))
            })?
            .to_string();
        let oz = item
            .get("oz")
            .and_then(json_decimal)
            .unwrap_or(Decimal::ZERO);
        let shots = item.get("shots").and_then(|s| s.as_u64()).unwrap_or(0);
        let shots_per_fl_oz = colorants
            .iter()
            .find(|c| c.code.eq_ignore_ascii_case(&code))
            .map(|c| c.shots_per_fl_oz)
            .filter(|&s| s > 0)
            .unwrap_or(48);
        let fl_oz = oz + Decimal::from(shots) / Decimal::from(shots_per_fl_oz);
        lines.push((code, fl_oz));
    }
    Ok(lines)
}

pub async fn daily_usage_report(
    pool: &df_db::AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    date: NaiveDate,
    currency: Currency,
) -> Result<DailyUsageReport, ReportError> {
    let zero = Money::zero(currency);
    // --- tracked formulas -------------------------------------------
    let colorants = df_db::list_colorants_for_org(pool, org_id).await?;
    let formulas = df_db::list_tint_formulas_for_store(pool, store_id).await?;
    let mut formulas_recorded = 0u32;
    let mut per_color: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut used: BTreeMap<String, Decimal> = BTreeMap::new();
    for formula in formulas {
        if formula.dispensed_at.date_naive() != date {
            continue;
        }
        formulas_recorded += 1;
        *per_color
            .entry((
                formula.color_code_snapshot.clone(),
                formula.color_name_snapshot.clone(),
            ))
            .or_default() += 1;
        for (code, fl_oz) in parse_formula_colorants(&formula.colorants_json, &colorants)? {
            *used.entry(code).or_default() += fl_oz;
        }
    }
    let mut colorant_used: Vec<ColorantUsage> = Vec::new();
    let mut total_tint_cost = zero;
    for (code, fl_oz) in used {
        let colorant = colorants
            .iter()
            .find(|c| c.code.eq_ignore_ascii_case(&code));
        let (colorant_id, name, cost) = match colorant {
            Some(colorant) => (
                Some(colorant.id),
                colorant.name.clone(),
                Money::new(colorant.cost_per_fl_oz * fl_oz, currency),
            ),
            None => (None, code.clone(), zero),
        };
        total_tint_cost = total_tint_cost.checked_add(cost)?;
        colorant_used.push(ColorantUsage {
            colorant_id,
            code,
            name,
            fl_oz,
            cost,
        });
    }
    let mut per_color: Vec<ColorUsage> = per_color
        .into_iter()
        .map(|((color_code, color_name), tints)| ColorUsage {
            color_code,
            color_name,
            tints,
        })
        .collect();
    per_color.sort_by(|a, b| b.tints.cmp(&a.tints).then(a.color_code.cmp(&b.color_code)));

    // --- sales ------------------------------------------------------
    let transactions = df_db::list_transactions_for_store(pool, store_id).await?;
    let mut sale_count = 0u32;
    let mut return_count = 0u32;
    let mut subtotal = zero;
    let mut tax_total = zero;
    let mut eco_fee_total = zero;
    let mut pos_fee_total = zero;
    let mut grand_total = zero;
    for tx in transactions {
        if tx.created_at.date_naive() != date || tx.status != "completed" {
            continue;
        }
        match tx.doc_type.as_str() {
            "sale" => sale_count += 1,
            "return" => return_count += 1,
            _ => continue,
        }
        subtotal = subtotal.checked_add(Money::new(tx.subtotal, currency))?;
        tax_total = tax_total.checked_add(Money::new(tx.tax_total, currency))?;
        eco_fee_total = eco_fee_total.checked_add(Money::new(tx.eco_fee_total, currency))?;
        pos_fee_total = pos_fee_total.checked_add(Money::new(tx.pos_fee_total, currency))?;
        grand_total = grand_total.checked_add(Money::new(tx.grand_total, currency))?;
    }

    Ok(DailyUsageReport {
        store_id,
        org_id,
        date,
        currency,
        formulas_recorded,
        per_color,
        colorant_used,
        total_tint_cost,
        sales: SalesTotals {
            sale_count,
            return_count,
            subtotal,
            tax_total,
            eco_fee_total,
            pos_fee_total,
            grand_total,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use chrono::Utc;
    use df_db::models::{
        ColorCatalogEntry, Colorant, Organization, Product, Store, TintFormula, Transaction,
    };

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn money(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    async fn seeded_pool() -> (df_db::AnyPool, Uuid, Uuid, Uuid) {
        let pool = test_pool().await;
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
        let product_id = Uuid::from_u128(0x3000);
        df_db::insert_organization(
            &pool,
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
            &pool,
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
        df_db::insert_product(
            &pool,
            &Product {
                id: product_id,
                org_id,
                sku: "BM-REGAL-1G".to_string(),
                manufacturer_code: Some("BM".to_string()),
                name: "Regal Select Interior Gallon".to_string(),
                department_id: Uuid::now_v7(),
                class_id: None,
                product_line_id: None,
                primary_vendor_id: None,
                product_type: "paint_base".to_string(),
                base_uom: "gal".to_string(),
                list_price: dec("60"),
                standard_cost: dec("30"),
                avg_cost: dec("30"),
                min_margin_pct: None,
                sheen: Some("Eggshell".to_string()),
                paint_base_code: Some("1X".to_string()),
                container_volume_ml: Some(3785),
                eco_fee_category: None,
                tax_class_id: Uuid::now_v7(),
                prompt_template_id: None,
                is_active: true,
                map: None,
                msrp: None,
            },
        )
        .await
        .unwrap();
        let color_id = Uuid::from_u128(0x6001);
        df_db::insert_color(
            &pool,
            &ColorCatalogEntry {
                id: color_id,
                org_id,
                manufacturer: "Benjamin Moore".to_string(),
                color_code: "OC-17".to_string(),
                color_name: "White Dove".to_string(),
                collection: None,
                rgb_hex: Some("#F3EFE6".to_string()),
                recommended_base: None,
            },
        )
        .await
        .unwrap();
        for (id, code, name, cost) in [
            (0x5001u128, "Y3", "Yellow Oxide", "0.4200"),
            (0x5002u128, "R1", "Red Oxide", "0.5000"),
        ] {
            df_db::insert_colorant(
                &pool,
                &Colorant {
                    id: Uuid::from_u128(id),
                    org_id,
                    system_name: "Gennex".to_string(),
                    code: code.to_string(),
                    name: name.to_string(),
                    cost_per_fl_oz: dec(cost),
                    shots_per_fl_oz: 48,
                },
            )
            .await
            .unwrap();
        }
        (pool, org_id, store_id, product_id)
    }

    #[tokio::test]
    async fn daily_usage_summarises_formulas_colorant_and_sales() {
        let (pool, org_id, store_id, product_id) = seeded_pool().await;
        let today = NaiveDate::from_ymd_opt(2026, 5, 10).unwrap();
        let today_ts = today.and_hms_opt(14, 0, 0).unwrap().and_utc();
        let yesterday_ts = (today - chrono::Duration::days(1))
            .and_hms_opt(14, 0, 0)
            .unwrap()
            .and_utc();

        // One formula today (Y3 0 oz + 12 shots = 0.25 fl oz, R1 1 oz), one yesterday (excluded).
        for ts in [today_ts, yesterday_ts] {
            df_db::insert_tint_formula(
                &pool,
                &TintFormula {
                    id: Uuid::now_v7(),
                    org_id,
                    store_id,
                    customer_id: None,
                    job_id: None,
                    color_id: Some(Uuid::from_u128(0x6001)),
                    color_code_snapshot: "OC-17".to_string(),
                    color_name_snapshot: "White Dove".to_string(),
                    base_product_id: product_id,
                    container_volume_ml: 3785,
                    sheen: Some("Eggshell".to_string()),
                    room_or_usage: None,
                    is_custom_match: false,
                    colorants_json: serde_json::json!([
                        { "code": "Y3", "oz": 0, "shots": 12 },
                        { "code": "R1", "oz": 1, "shots": 0 }
                    ]),
                    total_tint_cost: dec("0.6050"),
                    tint_surcharge: Decimal::ZERO,
                    dispenser_name: None,
                    external_formula_ref: None,
                    dispensed_at: ts,
                },
            )
            .await
            .unwrap();
        }

        // One completed sale today, one voided sale (excluded), one return today.
        for (doc_type, status, sub, tax, grand, ts) in [
            ("sale", "completed", "100.00", "8.00", "108.00", today_ts),
            ("sale", "voided", "50.00", "4.00", "54.00", today_ts),
            ("return", "completed", "-20.00", "-1.60", "-21.60", today_ts),
            (
                "sale",
                "completed",
                "999.00",
                "0.00",
                "999.00",
                yesterday_ts,
            ),
        ] {
            df_db::insert_transaction(
                &pool,
                &Transaction {
                    id: Uuid::now_v7(),
                    org_id,
                    store_id,
                    till_session_id: None,
                    doc_type: doc_type.to_string(),
                    doc_number: format!("TX-{}-{}", doc_type, Uuid::now_v7()),
                    status: status.to_string(),
                    customer_id: None,
                    job_id: None,
                    po_number: None,
                    job_name: None,
                    salesperson_id: None,
                    cashier_id: Some(Uuid::now_v7()),
                    currency: "USD".to_string(),
                    subtotal: dec(sub),
                    tax_total: dec(tax),
                    eco_fee_total: Decimal::ZERO,
                    pos_fee_total: Decimal::ZERO,
                    grand_total: dec(grand),
                    deposit_paid: Decimal::ZERO,
                    fulfillment_mode: "counter".to_string(),
                    scheduled_at: None,
                    completed_at: Some(ts),
                    created_at: ts,
                },
            )
            .await
            .unwrap();
        }

        let report = daily_usage_report(&pool, org_id, store_id, today, Currency::Usd)
            .await
            .unwrap();
        assert_eq!(report.formulas_recorded, 1);
        assert_eq!(report.per_color.len(), 1);
        assert_eq!(report.per_color[0].color_code, "OC-17");
        assert_eq!(report.per_color[0].tints, 1);
        assert_eq!(report.colorant_used.len(), 2);
        let y3 = report
            .colorant_used
            .iter()
            .find(|c| c.code == "Y3")
            .unwrap();
        assert_eq!(y3.fl_oz, dec("0.25"));
        assert_eq!(y3.cost, money("0.1050"));
        let r1 = report
            .colorant_used
            .iter()
            .find(|c| c.code == "R1")
            .unwrap();
        assert_eq!(r1.fl_oz, dec("1"));
        assert_eq!(r1.cost, money("0.5000"));
        assert_eq!(report.total_tint_cost, money("0.6050"));
        assert_eq!(report.sales.sale_count, 1);
        assert_eq!(report.sales.return_count, 1);
        assert_eq!(report.sales.grand_total, money("86.40"));
        let text = report.render_text();
        assert!(text.contains("Formulas recorded: 1"));
        assert!(text.contains("OC-17 (White Dove): 1 formula(s)"));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["formulas_recorded"], 1);
    }

    #[tokio::test]
    async fn daily_usage_is_zero_on_a_quiet_day() {
        let (pool, org_id, store_id, _) = seeded_pool().await;
        let date = NaiveDate::from_ymd_opt(2026, 5, 11).unwrap();
        let report = daily_usage_report(&pool, org_id, store_id, date, Currency::Usd)
            .await
            .unwrap();
        assert_eq!(report.formulas_recorded, 0);
        assert!(report.per_color.is_empty());
        assert!(report.colorant_used.is_empty());
        assert_eq!(report.sales.sale_count, 0);
    }
}
