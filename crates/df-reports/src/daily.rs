//! Daily usage report (Phase 2, v0.3; spec §7).
//!
//! For one store and one (UTC) date:
//!
//! * tints dispensed — count of `done` tint jobs created that day,
//!   a per-color breakdown (by the formula's color snapshot), and a
//!   count of `failed` jobs (shortages, unknown colorants);
//! * colorant consumed — per colorant, exact fl oz summed from the
//!   `done` jobs' `dispensed_json`, plus cost at the colorant's
//!   `cost_per_fl_oz` via `df-domain` decimal math;
//! * sales totals — completed `sale` / `return` transactions created
//!   that day (subtotal, tax, eco fee, POS fee, grand total).
//!
//! Dates are compared on `created_at` in Rust, not SQL: timestamps
//! are RFC 3339 text in SQLite and `TIMESTAMPTZ` in PostgreSQL, so a
//! portable SQL date comparison would need dialect-specific casts
//! (same reason as `df_db::list_tint_jobs_for_store`).

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::NaiveDate;
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ReportError;

/// Tints dispensed for one color on the day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorUsage {
    pub color_code: String,
    pub color_name: String,
    pub tints: u32,
}

/// One colorant consumed on the day: exact fl oz and its cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorantUsage {
    /// `None` when the colorant row was deleted after the dispense —
    /// the code and fl oz are still reported, the cost is zero.
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
    pub tints_dispensed: u32,
    pub tints_failed: u32,
    /// Sorted by tint count descending, then color code.
    pub per_color: Vec<ColorUsage>,
    /// Sorted by colorant code.
    pub colorant_consumed: Vec<ColorantUsage>,
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
        out.push_str(&format!(
            "Tints dispensed: {} ({} failed)\n",
            self.tints_dispensed, self.tints_failed
        ));
        out.push_str("Per color:\n");
        if self.per_color.is_empty() {
            out.push_str("  (none)\n");
        }
        for color in &self.per_color {
            out.push_str(&format!(
                "  {} ({}): {} tint(s)\n",
                color.color_code, color.color_name, color.tints
            ));
        }
        out.push_str("Colorant consumed:\n");
        if self.colorant_consumed.is_empty() {
            out.push_str("  (none)\n");
        }
        for usage in &self.colorant_consumed {
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

/// One dispensed shot parsed out of a tint job's `dispensed_json`
/// (`[{ "code": "Y3", "fl_oz": "…" }]` — `fl_oz` is a decimal string
/// as written by `df-tinter`, a bare JSON number is also accepted).
fn parse_dispensed(value: &serde_json::Value) -> Result<Vec<(String, Decimal)>, ReportError> {
    let items = value.as_array().ok_or_else(|| {
        ReportError::InvalidInput(format!("dispensed_json is not an array: {value}"))
    })?;
    let mut shots = Vec::with_capacity(items.len());
    for item in items {
        let code = item
            .get("code")
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                ReportError::InvalidInput(format!("dispensed shot has no code: {item}"))
            })?
            .to_string();
        let fl_oz_value = item.get("fl_oz").ok_or_else(|| {
            ReportError::InvalidInput(format!("dispensed shot has no fl_oz: {item}"))
        })?;
        let fl_oz_text = match fl_oz_value {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let fl_oz = Decimal::from_str(&fl_oz_text).map_err(|_| {
            ReportError::InvalidInput(format!("dispensed fl_oz is not a decimal: {fl_oz_text}"))
        })?;
        shots.push((code, fl_oz));
    }
    Ok(shots)
}

pub async fn daily_usage_report(
    pool: &df_db::AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    date: NaiveDate,
    currency: Currency,
) -> Result<DailyUsageReport, ReportError> {
    let zero = Money::zero(currency);
    // --- tints ------------------------------------------------------
    let jobs = df_db::list_tint_jobs_for_store(pool, store_id).await?;
    let mut tints_dispensed = 0u32;
    let mut tints_failed = 0u32;
    let mut per_color: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut consumed: BTreeMap<String, Decimal> = BTreeMap::new();
    // Cache formula lookups: several jobs usually share one formula.
    let mut formula_cache: BTreeMap<Uuid, df_db::models::TintFormula> = BTreeMap::new();
    for job in jobs {
        if job.created_at.date_naive() != date {
            continue;
        }
        match job.status.as_str() {
            "done" => {
                tints_dispensed += 1;
                let formula = match formula_cache.get(&job.formula_id) {
                    Some(formula) => formula.clone(),
                    None => {
                        let formula = df_db::get_tint_formula(pool, job.formula_id).await?;
                        formula_cache.insert(job.formula_id, formula.clone());
                        formula
                    }
                };
                *per_color
                    .entry((
                        formula.color_code_snapshot.clone(),
                        formula.color_name_snapshot.clone(),
                    ))
                    .or_default() += 1;
                for (code, fl_oz) in parse_dispensed(&job.dispensed_json)? {
                    *consumed.entry(code).or_default() += fl_oz;
                }
            }
            "failed" => {
                tints_failed += 1;
            }
            _ => {}
        }
    }
    let mut colorant_consumed: Vec<ColorantUsage> = Vec::new();
    let mut total_tint_cost = zero;
    // Resolve codes to colorant rows for names + costs. Codes are
    // unique per (org, system); if two systems share a code the
    // first row wins (see `df_db::list_colorants_for_org`).
    let colorants = df_db::list_colorants_for_org(pool, org_id).await?;
    for (code, fl_oz) in consumed {
        let colorant = colorants.iter().find(|c| c.code == code);
        let (colorant_id, name, cost) = match colorant {
            Some(colorant) => (
                Some(colorant.id),
                colorant.name.clone(),
                Money::new(colorant.cost_per_fl_oz * fl_oz, currency),
            ),
            None => (None, code.clone(), zero),
        };
        total_tint_cost = total_tint_cost.checked_add(cost)?;
        colorant_consumed.push(ColorantUsage {
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
        tints_dispensed,
        tints_failed,
        per_color,
        colorant_consumed,
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
        ColorCatalogEntry, Colorant, ColorantLevelRow, Organization, Product, Store, TintFormula,
        TintJobRow, Transaction,
    };

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn money(s: &str) -> Money {
        Money::from_str_amount(s, Currency::Usd).unwrap()
    }

    /// Pool seeded with org + store + one color + two colorants + one
    /// formula (White Dove: Y3 0 oz + 14 shots, R1 1 oz) + levels.
    /// Returns `(pool, org_id, store_id, formula_id, customer_id)`.
    async fn seeded_pool() -> (df_db::AnyPool, Uuid, Uuid, Uuid, Uuid) {
        let pool = test_pool().await;
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
        let customer_id = Uuid::from_u128(0x4000);
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
        // The formula's base product must exist: SQLite enforces the
        // `tint_formulas.base_product_id -> products(id)` FK.
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
                list_price: dec("100.0000"),
                standard_cost: dec("50.0000"),
                avg_cost: dec("50.0000"),
                min_margin_pct: None,
                sheen: Some("Eggshell".to_string()),
                paint_base_code: Some("1X".to_string()),
                container_volume_ml: Some(3785),
                eco_fee_category: Some("architectural_paint".to_string()),
                tax_class_id: Uuid::now_v7(),
                prompt_template_id: None,
                is_active: true,
                map: None,
                msrp: None,
            },
        )
        .await
        .unwrap();
        let color = ColorCatalogEntry {
            id: Uuid::from_u128(0x6000),
            org_id,
            manufacturer: "Benjamin Moore".to_string(),
            color_code: "OC-17".to_string(),
            color_name: "White Dove".to_string(),
            collection: Some("Off-White".to_string()),
            rgb_hex: None,
            recommended_base: Some("1X".to_string()),
        };
        df_db::insert_color(&pool, &color).await.unwrap();
        for (id, code, name, cost) in [
            (Uuid::from_u128(0x5001), "Y3", "Yellow Oxide", "0.4200"),
            (Uuid::from_u128(0x5002), "R1", "Red Oxide", "0.5000"),
        ] {
            df_db::insert_colorant(
                &pool,
                &Colorant {
                    id,
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
            df_db::upsert_colorant_level(
                &pool,
                &ColorantLevelRow {
                    colorant_id: id,
                    store_id,
                    on_hand_fl_oz: dec("10"),
                    low_threshold_fl_oz: dec("2"),
                    updated_at: Utc::now(),
                },
            )
            .await
            .unwrap();
        }
        let formula_id = Uuid::from_u128(0x7000);
        df_db::insert_tint_formula(
            &pool,
            &TintFormula {
                id: formula_id,
                org_id,
                store_id,
                customer_id: Some(customer_id),
                job_id: None,
                color_id: Some(color.id),
                color_code_snapshot: "OC-17".to_string(),
                color_name_snapshot: "White Dove".to_string(),
                base_product_id: product_id,
                container_volume_ml: 3785,
                sheen: Some("Eggshell".to_string()),
                room_or_usage: None,
                is_custom_match: false,
                colorants_json: serde_json::json!([
                    {"code": "Y3", "oz": 0, "shots": 14},
                    {"code": "R1", "oz": 1, "shots": 0},
                ]),
                total_tint_cost: dec("0.6225"),
                tint_surcharge: dec("0.0000"),
                dispenser_name: Some("virtual-simulator".to_string()),
                external_formula_ref: None,
                dispensed_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        (pool, org_id, store_id, formula_id, customer_id)
    }

    #[tokio::test]
    async fn daily_report_counts_tints_consumption_and_sales() {
        let (pool, org_id, store_id, formula_id, _customer_id) = seeded_pool().await;
        let now = Utc::now();
        let today = now.date_naive();
        let y3_fl_oz = dec("14") / dec("48");
        let tx_id = Uuid::now_v7();
        // The sale transaction first: the tint job's FK to
        // `transactions(id)` requires the sale row to exist.
        for (doc_type, status, grand) in [
            ("sale", "completed", "108.0544"),
            ("quote", "open", "50.0000"),
        ] {
            df_db::insert_transaction(
                &pool,
                &Transaction {
                    id: if doc_type == "sale" {
                        tx_id
                    } else {
                        Uuid::now_v7()
                    },
                    org_id,
                    store_id,
                    till_session_id: None,
                    doc_number: format!("TX-{doc_type}"),
                    doc_type: doc_type.to_string(),
                    status: status.to_string(),
                    customer_id: None,
                    job_id: None,
                    po_number: None,
                    salesperson_id: None,
                    cashier_id: None,
                    currency: "USD".to_string(),
                    subtotal: dec("100.0000"),
                    tax_total: dec("7.3044"),
                    eco_fee_total: dec("0.7500"),
                    pos_fee_total: dec("0.0000"),
                    grand_total: dec(grand),
                    deposit_paid: Decimal::ZERO,
                    fulfillment_mode: "counter".to_string(),
                    scheduled_at: None,
                    completed_at: Some(now),
                    created_at: now,
                    job_name: None,
                },
            )
            .await
            .unwrap();
        }
        // One done job (the White Dove dispense) + one failed job.
        df_db::insert_tint_job(
            &pool,
            &TintJobRow {
                id: Uuid::now_v7(),
                formula_id,
                transaction_id: Some(tx_id),
                line_no: Some(1),
                status: "done".to_string(),
                dispensed_json: serde_json::json!([
                    {"code": "Y3", "fl_oz": y3_fl_oz.to_string()},
                    {"code": "R1", "fl_oz": "1"},
                ]),
                error: None,
                created_at: now,
                completed_at: Some(now),
            },
        )
        .await
        .unwrap();
        df_db::insert_tint_job(
            &pool,
            &TintJobRow {
                id: Uuid::now_v7(),
                formula_id,
                transaction_id: None,
                line_no: None,
                status: "failed".to_string(),
                dispensed_json: serde_json::json!([]),
                error: Some("insufficient colorant R1".to_string()),
                created_at: now,
                completed_at: Some(now),
            },
        )
        .await
        .unwrap();

        let report = daily_usage_report(&pool, org_id, store_id, today, Currency::Usd)
            .await
            .unwrap();
        assert_eq!(report.tints_dispensed, 1);
        assert_eq!(report.tints_failed, 1);
        assert_eq!(report.per_color.len(), 1);
        assert_eq!(report.per_color[0].color_code, "OC-17");
        assert_eq!(report.per_color[0].tints, 1);
        assert_eq!(report.colorant_consumed.len(), 2);
        let y3 = report
            .colorant_consumed
            .iter()
            .find(|u| u.code == "Y3")
            .unwrap();
        assert_eq!(y3.fl_oz, y3_fl_oz);
        assert_eq!(y3.cost, money("0.1225"));
        let r1 = report
            .colorant_consumed
            .iter()
            .find(|u| u.code == "R1")
            .unwrap();
        assert_eq!(r1.fl_oz, dec("1"));
        assert_eq!(r1.cost, money("0.5000"));
        assert_eq!(report.total_tint_cost, money("0.6225"));
        assert_eq!(report.sales.sale_count, 1);
        assert_eq!(report.sales.return_count, 0);
        assert_eq!(report.sales.grand_total, money("108.0544"));
        // JSON round-trips (the wire format for the HTTP endpoints).
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["tints_dispensed"], 1);
        assert_eq!(json["total_tint_cost"]["amount"], "0.6225");
        let text = report.render_text();
        assert!(text.contains("Tints dispensed: 1 (1 failed)"));
        assert!(text.contains("OC-17 (White Dove): 1 tint(s)"));
        assert!(text.contains("Total tint cost: 0.6225"));
    }

    #[tokio::test]
    async fn daily_report_is_empty_on_a_quiet_day() {
        let (pool, org_id, store_id, _formula_id, _customer_id) = seeded_pool().await;
        let yesterday = Utc::now().date_naive() - chrono::Duration::days(1);
        let report = daily_usage_report(&pool, org_id, store_id, yesterday, Currency::Usd)
            .await
            .unwrap();
        assert_eq!(report.tints_dispensed, 0);
        assert!(report.per_color.is_empty());
        assert!(report.colorant_consumed.is_empty());
        assert_eq!(report.total_tint_cost, Money::zero(Currency::Usd));
        assert_eq!(report.sales.sale_count, 0);
        assert!(
            report
                .render_text()
                .contains("Tints dispensed: 0 (0 failed)")
        );
    }

    #[test]
    fn parse_dispensed_rejects_malformed_json() {
        assert!(parse_dispensed(&serde_json::json!({"code": "Y3"})).is_err());
        assert!(
            parse_dispensed(&serde_json::json!([{"code": "Y3", "fl_oz": "not-a-number"}])).is_err()
        );
        // A bare JSON number is accepted (no float math — it is
        // re-parsed as an exact decimal string).
        assert_eq!(
            parse_dispensed(&serde_json::json!([{"code": "Y3", "fl_oz": 1}])).unwrap(),
            vec![("Y3".to_string(), dec("1"))]
        );
    }
}
