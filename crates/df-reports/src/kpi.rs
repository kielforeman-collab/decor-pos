//! KPI dashboards + ad-hoc inquiries (Phase 6, v1.0; spec §7).
//!
//! One [`KpiDashboard`] per store and date range, computed from the
//! real edge/HQ ledgers — no second set of books:
//!
//! * **Sales** — completed `sale` / `return` transactions whose
//!   `created_at` falls in `[start, end]` (UTC dates, same
//!   in-Rust date rule as the daily usage report: timestamps are
//!   RFC 3339 text in SQLite and `TIMESTAMPTZ` in PostgreSQL, so a
//!   portable SQL date comparison would need dialect-specific
//!   casts).
//! * **Margin** — per transaction line, revenue is
//!   `line_total - discount_amount` (tax and eco fee are excluded:
//!   `line_total` is `unit_price * qty` at checkout) and COGS is
//!   `(base_unit_cost + tint_unit_cost) * qty`. Return lines carry
//!   negative quantities and totals, so they reduce revenue and
//!   COGS with the correct sign. `margin_pct` is `None` when
//!   revenue is zero.
//! * **Inventory turns** — COGS for the period divided by the
//!   average inventory value `(value at start + value at end) / 2`,
//!   where the value at a date is the append-only ledger summed in
//!   Rust: `Σ delta_on_hand * unit_cost` over entries dated on or
//!   before that date. The ledger carries no period-start snapshot
//!   table, so the start value is reconstructed from the same
//!   ledger — exact, but it values stock at each entry's recorded
//!   unit cost (receipt cost for receipts, product cost at sale
//!   time for sales), not at a single current cost. `None` when
//!   the average value is zero or negative.
//! * **AR aging** — [`df_ar::AgingSummary`] as of `as_of`, derived
//!   from `ar_ledger` + `ar_allocations` via `df-ar`'s
//!   [`open_charges`](df_ar::open_charges) — the same derivation
//!   the statements use, not a re-implementation.
//! * **Formula usage (tracking only)** — stored
//!   `tint_formulas` created in the period, counted per color
//!   code with their recorded tint cost. This is the formula
//!   record as stored; it never touches dispense or machine
//!   control (tinting is formula tracking only — spec §9).
//!
//! [`run_inquiry`] is the ad-hoc side: the same completed
//! transactions filtered by date range and, optionally, customer,
//! product (any line carrying it) or document type.
//!
//! All money is `rust_decimal::Decimal` — never floats.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ReportError;

/// Sales totals for the dashboard period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KpiSales {
    pub sale_count: u32,
    pub return_count: u32,
    /// Gross sales before returns (positive magnitudes only).
    pub gross_sales: Money,
    /// Returns as a positive magnitude.
    pub returns: Money,
    /// `gross_sales - returns`.
    pub net_sales: Money,
    pub tax_total: Money,
    pub eco_fee_total: Money,
    pub grand_total: Money,
    /// `net_sales / sale_count`, `None` when there were no sales.
    pub avg_ticket: Option<Money>,
}

/// Gross margin for the dashboard period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KpiMargin {
    /// Net sales revenue excluding tax and eco fees.
    pub revenue: Money,
    pub cogs: Money,
    pub gross_profit: Money,
    /// `gross_profit / revenue * 100`, `None` when revenue is zero.
    pub margin_pct: Option<Decimal>,
}

/// Inventory position and turns for the dashboard period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KpiInventory {
    /// On-hand value at the period end (ledger valuation, see
    /// module docs).
    pub on_hand_value: Money,
    /// `(value at period start + value at period end) / 2`.
    pub avg_inventory_value: Money,
    /// `COGS / avg inventory value`, `None` when the average
    /// value is zero or negative.
    pub turns: Option<Decimal>,
}

/// Formula usage in the period — stored formulas only (tracking).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormulaUsage {
    pub formulas_created: u32,
    /// Sorted by count descending, then color code.
    pub per_color: Vec<FormulaColorUsage>,
    /// Sum of the formulas' recorded `total_tint_cost`.
    pub total_tint_cost: Money,
}

/// Formulas created for one color in the period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormulaColorUsage {
    pub color_code: String,
    pub color_name: String,
    pub formulas: u32,
}

/// One store's KPI dashboard for `[start, end]`, with AR aging as
/// of `as_of`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KpiDashboard {
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub as_of: NaiveDate,
    pub currency: Currency,
    pub sales: KpiSales,
    pub margin: KpiMargin,
    pub inventory: KpiInventory,
    pub ar_aging: df_ar::AgingSummary,
    pub total_open_ar: Money,
    pub formulas: FormulaUsage,
}

impl KpiDashboard {
    /// Plain-text rendering (one section per KPI group).
    pub fn render_text(&self) -> String {
        let mut out = format!(
            "KPI dashboard — store {} — {} to {} (AR as of {})\n",
            self.store_id, self.start, self.end, self.as_of
        );
        let sales = &self.sales;
        out.push_str(&format!(
            "Sales: {} sale(s), {} return(s); gross {}, returns {}, net {}, tax {}, eco {}, grand {}\n",
            sales.sale_count,
            sales.return_count,
            sales.gross_sales.amount(),
            sales.returns.amount(),
            sales.net_sales.amount(),
            sales.tax_total.amount(),
            sales.eco_fee_total.amount(),
            sales.grand_total.amount()
        ));
        if let Some(avg) = &sales.avg_ticket {
            out.push_str(&format!("Average ticket: {}\n", avg.amount()));
        }
        let margin = &self.margin;
        out.push_str(&format!(
            "Margin: revenue {}, COGS {}, gross profit {}",
            margin.revenue.amount(),
            margin.cogs.amount(),
            margin.gross_profit.amount()
        ));
        match margin.margin_pct {
            Some(pct) => out.push_str(&format!(" ({pct}% margin)\n")),
            None => out.push_str(" (no revenue — margin undefined)\n"),
        }
        let inventory = &self.inventory;
        out.push_str(&format!(
            "Inventory: on-hand value {}, average value {}",
            inventory.on_hand_value.amount(),
            inventory.avg_inventory_value.amount()
        ));
        match inventory.turns {
            Some(turns) => out.push_str(&format!(" ({turns}x turns)\n")),
            None => out.push_str(" (turns undefined — no average value)\n"),
        }
        let aging = &self.ar_aging;
        out.push_str(&format!(
            "AR aging: current {}, 31-60 {}, 61-90 {}, 91-120 {}, 120+ {}; total open {}\n",
            aging.current.amount(),
            aging.days_31_60.amount(),
            aging.days_61_90.amount(),
            aging.days_91_120.amount(),
            aging.over_120.amount(),
            self.total_open_ar.amount()
        ));
        out.push_str(&format!(
            "Formulas (tracking only): {} created, total tint cost {}\n",
            self.formulas.formulas_created,
            self.formulas.total_tint_cost.amount()
        ));
        for color in &self.formulas.per_color {
            out.push_str(&format!(
                "  {} ({}): {} formula(s)\n",
                color.color_code, color.color_name, color.formulas
            ));
        }
        out
    }
}

/// Compute one store's KPI dashboard for `[start, end]`, with AR
/// aging as of `as_of`. `end` must not precede `start`; `as_of`
/// defaults to `end` when it precedes it is left as given (aging is
/// a point-in-time view, independent of the sales window).
pub async fn kpi_dashboard(
    pool: &df_db::AnyPool,
    org_id: Uuid,
    store_id: Uuid,
    start: NaiveDate,
    end: NaiveDate,
    as_of: NaiveDate,
    currency: Currency,
) -> Result<KpiDashboard, ReportError> {
    if end < start {
        return Err(ReportError::InvalidInput(format!(
            "KPI end {end} precedes start {start}"
        )));
    }
    let zero = Money::zero(currency);

    // --- sales + margin -------------------------------------------------
    let transactions = df_db::list_transactions_for_store(pool, store_id).await?;
    let mut sale_count = 0u32;
    let mut return_count = 0u32;
    let mut gross_sales = zero;
    let mut returns = zero;
    let mut tax_total = zero;
    let mut eco_fee_total = zero;
    let mut grand_total = zero;
    let mut revenue = zero;
    let mut cogs = zero;
    for tx in &transactions {
        let date = tx.created_at.date_naive();
        if date < start || date > end || tx.status != "completed" {
            continue;
        }
        match tx.doc_type.as_str() {
            "sale" => {
                sale_count += 1;
                gross_sales = gross_sales.checked_add(Money::new(tx.subtotal, currency))?;
            }
            "return" => {
                return_count += 1;
                // Return subtotals are negative; report the
                // magnitude so `net = gross - returns` reads plainly.
                returns = returns.checked_add(Money::new(-tx.subtotal, currency))?;
            }
            _ => continue,
        }
        tax_total = tax_total.checked_add(Money::new(tx.tax_total, currency))?;
        eco_fee_total = eco_fee_total.checked_add(Money::new(tx.eco_fee_total, currency))?;
        grand_total = grand_total.checked_add(Money::new(tx.grand_total, currency))?;
        for line in df_db::list_transaction_lines(pool, tx.id).await? {
            let line_revenue = Money::new(line.line_total - line.discount_amount, currency);
            revenue = revenue.checked_add(line_revenue)?;
            let line_cogs = Money::new(
                (line.base_unit_cost + line.tint_unit_cost) * line.qty,
                currency,
            );
            cogs = cogs.checked_add(line_cogs)?;
        }
    }
    let net_sales = gross_sales.checked_sub(returns)?;
    let avg_ticket = if sale_count > 0 {
        Some(net_sales.div_decimal(Decimal::from(sale_count))?)
    } else {
        None
    };
    let gross_profit = revenue.checked_sub(cogs)?;
    let margin_pct = if revenue.is_zero() {
        None
    } else {
        Some(
            gross_profit
                .amount()
                .checked_div(revenue.amount())
                .map(|ratio| (ratio * Decimal::from(100)).normalize())
                .unwrap_or(Decimal::ZERO),
        )
    };

    // --- inventory turns ------------------------------------------------
    let entries = df_db::list_inventory_entries_for_store(pool, store_id).await?;
    let value_at = |date: NaiveDate| {
        entries
            .iter()
            .filter(|e| e.created_at.date_naive() <= date)
            .map(|e| e.delta_on_hand * e.unit_cost)
            .fold(Decimal::ZERO, |acc, value| acc + value)
    };
    let start_value = value_at(start.pred_opt().unwrap_or(start));
    let end_value = value_at(end);
    let avg_value = (start_value + end_value) / Decimal::from(2);
    let turns = if avg_value > Decimal::ZERO {
        Some(
            cogs.amount()
                .checked_div(avg_value)
                .map(|ratio| ratio.normalize())
                .unwrap_or(Decimal::ZERO),
        )
    } else {
        None
    };

    // --- AR aging -------------------------------------------------------
    let ar_entries = df_db::list_ar_entries_for_store(pool, store_id).await?;
    let customer_ids: BTreeSet<Uuid> = ar_entries.iter().map(|e| e.customer_id).collect();
    let mut allocations = Vec::new();
    for customer_id in &customer_ids {
        for allocation in df_db::list_allocations_for_customer(pool, *customer_id).await? {
            allocations.push(df_db::allocation_to_domain(&allocation));
        }
    }
    let domain_entries: Vec<df_ar::LedgerEntry> = ar_entries
        .iter()
        .map(df_db::ledger_entry_to_domain)
        .collect();
    let open = df_ar::open_charges(&domain_entries, &allocations);
    let ar_aging = df_ar::AgingSummary::from_open(&open, as_of, currency);
    let total_open_ar = Money::new(df_ar::ledger::open_balance(&open), currency);

    // --- formula usage (tracking only) ----------------------------------
    let formulas = df_db::list_tint_formulas_for_store(pool, store_id).await?;
    let mut per_color: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut formulas_created = 0u32;
    let mut total_tint_cost = zero;
    for formula in &formulas {
        let date = formula.dispensed_at.date_naive();
        if date < start || date > end {
            continue;
        }
        formulas_created += 1;
        *per_color
            .entry((
                formula.color_code_snapshot.clone(),
                formula.color_name_snapshot.clone(),
            ))
            .or_default() += 1;
        total_tint_cost =
            total_tint_cost.checked_add(Money::new(formula.total_tint_cost, currency))?;
    }
    let mut per_color: Vec<FormulaColorUsage> = per_color
        .into_iter()
        .map(|((color_code, color_name), count)| FormulaColorUsage {
            color_code,
            color_name,
            formulas: count,
        })
        .collect();
    per_color.sort_by(|a, b| {
        b.formulas
            .cmp(&a.formulas)
            .then(a.color_code.cmp(&b.color_code))
    });

    Ok(KpiDashboard {
        org_id,
        store_id,
        start,
        end,
        as_of,
        currency,
        sales: KpiSales {
            sale_count,
            return_count,
            gross_sales,
            returns,
            net_sales,
            tax_total,
            eco_fee_total,
            grand_total,
            avg_ticket,
        },
        margin: KpiMargin {
            revenue,
            cogs,
            gross_profit,
            margin_pct,
        },
        inventory: KpiInventory {
            on_hand_value: Money::new(end_value, currency),
            avg_inventory_value: Money::new(avg_value, currency),
            turns,
        },
        ar_aging,
        total_open_ar,
        formulas: FormulaUsage {
            formulas_created,
            per_color,
            total_tint_cost,
        },
    })
}

/// Ad-hoc inquiry filters over one store's completed transactions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdHocInquiry {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub customer_id: Option<Uuid>,
    pub product_id: Option<Uuid>,
    pub doc_type: Option<String>,
}

/// One transaction matching an ad-hoc inquiry, with its line count
/// and the products on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryRow {
    pub transaction_id: Uuid,
    pub doc_number: String,
    pub doc_type: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub customer_id: Option<Uuid>,
    pub subtotal: Money,
    pub tax_total: Money,
    pub eco_fee_total: Money,
    pub grand_total: Money,
    pub line_count: u32,
    pub product_ids: Vec<Uuid>,
}

/// Run an ad-hoc inquiry: completed transactions in `[start, end]`
/// matching every supplied filter. Rows are oldest first.
pub async fn run_inquiry(
    pool: &df_db::AnyPool,
    store_id: Uuid,
    inquiry: &AdHocInquiry,
    currency: Currency,
) -> Result<Vec<InquiryRow>, ReportError> {
    if inquiry.end < inquiry.start {
        return Err(ReportError::InvalidInput(format!(
            "inquiry end {} precedes start {}",
            inquiry.end, inquiry.start
        )));
    }
    let transactions = df_db::list_transactions_for_store(pool, store_id).await?;
    let mut rows = Vec::new();
    for tx in &transactions {
        let date = tx.created_at.date_naive();
        if date < inquiry.start || date > inquiry.end || tx.status != "completed" {
            continue;
        }
        if let Some(customer_id) = inquiry.customer_id
            && tx.customer_id != Some(customer_id)
        {
            continue;
        }
        if let Some(doc_type) = &inquiry.doc_type
            && &tx.doc_type != doc_type
        {
            continue;
        }
        let lines = df_db::list_transaction_lines(pool, tx.id).await?;
        let product_ids: Vec<Uuid> = lines.iter().map(|l| l.product_id).collect();
        if let Some(product_id) = inquiry.product_id
            && !product_ids.contains(&product_id)
        {
            continue;
        }
        rows.push(InquiryRow {
            transaction_id: tx.id,
            doc_number: tx.doc_number.clone(),
            doc_type: tx.doc_type.clone(),
            status: tx.status.clone(),
            created_at: tx.created_at,
            customer_id: tx.customer_id,
            subtotal: Money::new(tx.subtotal, currency),
            tax_total: Money::new(tx.tax_total, currency),
            eco_fee_total: Money::new(tx.eco_fee_total, currency),
            grand_total: Money::new(tx.grand_total, currency),
            line_count: lines.len() as u32,
            product_ids,
        });
    }
    Ok(rows)
}

// --- Typst PDF rendering -----------------------------------------------------

use std::fmt::Write as _;
use std::path::Path;

use crate::statement::{escape_typst, format_money, resolve_typst_binary};

/// Render the Typst source for a KPI dashboard: header, sales,
/// margin, inventory, AR aging and formula-usage tables.
pub fn render_kpi_dashboard_typst(
    dashboard: &KpiDashboard,
    org_name: &str,
    store_name: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "#set page(paper: \"us-letter\", margin: (x: 2cm, y: 2cm))"
    );
    let _ = writeln!(out, "#set text(size: 10pt)");
    let _ = writeln!(out, "#set table(stroke: 0.5pt + gray)");
    let _ = writeln!(
        out,
        "= KPI Dashboard — {} to {}",
        dashboard.start, dashboard.end
    );
    let _ = writeln!(
        out,
        "{}  ·  {}",
        escape_typst(org_name),
        escape_typst(store_name)
    );
    let _ = writeln!(out, "AR aging as of {}", dashboard.as_of);
    let _ = writeln!(out);
    let _ = writeln!(out, "== Sales");
    let sales = &dashboard.sales;
    let _ = writeln!(
        out,
        "#table(columns: 2, [Sales], [{}], [Returns], [{}], [Net sales], [{}], [Tax], [{}], [Eco fees], [{}], [Grand total], [{}])",
        escape_typst(&format_money(sales.gross_sales)),
        escape_typst(&format_money(sales.returns)),
        escape_typst(&format_money(sales.net_sales)),
        escape_typst(&format_money(sales.tax_total)),
        escape_typst(&format_money(sales.eco_fee_total)),
        escape_typst(&format_money(sales.grand_total)),
    );
    if let Some(avg) = &sales.avg_ticket {
        let _ = writeln!(out, "Average ticket: {}", escape_typst(&format_money(*avg)));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "== Margin");
    let margin = &dashboard.margin;
    let _ = writeln!(
        out,
        "#table(columns: 2, [Revenue], [{}], [COGS], [{}], [Gross profit], [{}])",
        escape_typst(&format_money(margin.revenue)),
        escape_typst(&format_money(margin.cogs)),
        escape_typst(&format_money(margin.gross_profit)),
    );
    match margin.margin_pct {
        Some(pct) => {
            let _ = writeln!(out, "Margin: {pct}%");
        }
        None => {
            let _ = writeln!(out, "Margin: — (no revenue)");
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "== Inventory");
    let inventory = &dashboard.inventory;
    let _ = writeln!(
        out,
        "#table(columns: 2, [On-hand value], [{}], [Average value], [{}])",
        escape_typst(&format_money(inventory.on_hand_value)),
        escape_typst(&format_money(inventory.avg_inventory_value)),
    );
    match inventory.turns {
        Some(turns) => {
            let _ = writeln!(out, "Inventory turns: {turns}x");
        }
        None => {
            let _ = writeln!(out, "Inventory turns: — (no average value)");
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "== AR Aging");
    let aging = &dashboard.ar_aging;
    let _ = writeln!(
        out,
        "#table(columns: 2, [Current], [{}], [31–60 days], [{}], [61–90 days], [{}], [91–120 days], [{}], [120+ days], [{}], [Total open], [{}])",
        escape_typst(&format_money(aging.current)),
        escape_typst(&format_money(aging.days_31_60)),
        escape_typst(&format_money(aging.days_61_90)),
        escape_typst(&format_money(aging.days_91_120)),
        escape_typst(&format_money(aging.over_120)),
        escape_typst(&format_money(dashboard.total_open_ar)),
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "== Formula Usage (tracking only)");
    let _ = writeln!(
        out,
        "Formulas created: {} — total tint cost {}",
        dashboard.formulas.formulas_created,
        escape_typst(&format_money(dashboard.formulas.total_tint_cost))
    );
    for color in &dashboard.formulas.per_color {
        let _ = writeln!(
            out,
            "- {} ({}): {} formula(s)",
            escape_typst(&color.color_code),
            escape_typst(&color.color_name),
            color.formulas
        );
    }
    out
}

/// Render a KPI dashboard to real PDF bytes via Typst. Fails with
/// [`ReportError::Typst`] when no Typst binary is resolvable or
/// `typst compile` fails — never a fake PDF.
pub fn render_kpi_dashboard_pdf(
    dashboard: &KpiDashboard,
    org_name: &str,
    store_name: &str,
    typst_bin: Option<&Path>,
) -> Result<Vec<u8>, ReportError> {
    let typst = resolve_typst_binary(typst_bin).ok_or_else(|| {
        ReportError::Typst(
            "no typst binary found: pass --typst-bin, set DF_TYPST_BIN, or install typst (see df-reports statement docs)".to_string(),
        )
    })?;
    let source = render_kpi_dashboard_typst(dashboard, org_name, store_name);
    let dir = std::env::temp_dir().join(format!("df-kpi-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir)?;
    let input = dir.join("kpi.typ");
    let output = dir.join("kpi.pdf");
    std::fs::write(&input, source)?;
    let status = std::process::Command::new(&typst)
        .arg("compile")
        .arg(&input)
        .arg(&output)
        .output()
        .map_err(|e| ReportError::Typst(format!("failed to run typst ({typst:?}): {e}")))?;
    let result = (|| -> Result<Vec<u8>, ReportError> {
        if !status.status.success() {
            return Err(ReportError::Typst(format!(
                "typst compile failed: {}",
                String::from_utf8_lossy(&status.stderr)
            )));
        }
        let bytes = std::fs::read(&output)?;
        if !bytes.starts_with(b"%PDF") {
            return Err(ReportError::Typst(
                "typst produced output that is not a PDF".to_string(),
            ));
        }
        Ok(bytes)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use chrono::{TimeZone, Utc};
    use df_db::models::{
        ArLedgerEntry, Customer, InventoryLedgerEntry, Organization, Product, Store, TintFormula,
        Transaction, TransactionLine,
    };
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn at(year: i32, month: u32, day: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 12, 0, 0).unwrap()
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

    fn product(org_id: Uuid, id: Uuid, sku: &str, list: &str, cost: &str) -> Product {
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
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: dec(list),
            standard_cost: dec(cost),
            avg_cost: dec(cost),
            min_margin_pct: None,
            sheen: None,
            paint_base_code: None,
            container_volume_ml: Some(3785),
            eco_fee_category: Some("architectural_paint".to_string()),
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            is_active: true,
            map: None,
            msrp: None,
        }
    }

    /// Seed amounts for one test transaction (bundled so the
    /// helper stays under clippy's argument limit).
    struct TxSeed<'a> {
        doc_type: &'a str,
        subtotal: &'a str,
        tax: &'a str,
        eco: &'a str,
        grand: &'a str,
        created_at: chrono::DateTime<Utc>,
    }

    fn transaction(org_id: Uuid, store_id: Uuid, seed: TxSeed<'_>) -> Transaction {
        Transaction {
            id: Uuid::now_v7(),
            org_id,
            store_id,
            till_session_id: None,
            doc_number: format!("TX-{}", Uuid::now_v7()),
            doc_type: seed.doc_type.to_string(),
            status: "completed".to_string(),
            customer_id: None,
            job_id: None,
            po_number: None,
            salesperson_id: None,
            cashier_id: None,
            currency: "USD".to_string(),
            subtotal: dec(seed.subtotal),
            tax_total: dec(seed.tax),
            eco_fee_total: dec(seed.eco),
            pos_fee_total: Decimal::ZERO,
            grand_total: dec(seed.grand),
            deposit_paid: Decimal::ZERO,
            fulfillment_mode: "counter".to_string(),
            scheduled_at: None,
            completed_at: Some(seed.created_at),
            created_at: seed.created_at,
            job_name: None,
        }
    }

    fn line(tx_id: Uuid, product_id: Uuid, qty: &str, price: &str, cost: &str) -> TransactionLine {
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
            base_unit_cost: dec(cost),
            tint_unit_cost: Decimal::ZERO,
            discount_amount: Decimal::ZERO,
            tax_amount: Decimal::ZERO,
            eco_fee_amount: dec("0.7500"),
            line_total: dec(price) * qty,
            tint_formula_id: None,
            prompts_json: serde_json::json!({}),
            vertical_spec_json: serde_json::json!({}),
            linked_po_line_id: None,
            fulfilled_qty: qty,
        }
    }

    /// Seeds the exact scenario the KPI assertions below are
    /// computed from. All figures are hand-computed; see the
    /// assertions for the arithmetic.
    async fn seed_kpi_scenario(pool: &df_db::AnyPool) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
        let (org_id, store_id) = seed_org_store(pool).await;
        let product_a = Uuid::from_u128(0x3000);
        let product_b = Uuid::from_u128(0x3001);
        df_db::insert_product(
            pool,
            &product(org_id, product_a, "A-1G", "100.0000", "50.0000"),
        )
        .await
        .unwrap();
        df_db::insert_product(
            pool,
            &product(org_id, product_b, "B-1G", "40.0000", "20.0000"),
        )
        .await
        .unwrap();
        // Inventory: 10 x A @ 50 received Dec 20 (value 500),
        // then 4 x A sold Jan 5 (ledger -4 @ 50). Value the day
        // before the period = 500, at Jan 10 = 300, average = 400.
        for (delta, when) in [("10", at(2025, 12, 20)), ("-4", at(2026, 1, 5))] {
            df_db::insert_inventory_delta(
                pool,
                &InventoryLedgerEntry {
                    id: Uuid::now_v7(),
                    org_id,
                    store_id,
                    product_id: product_a,
                    lot_number: None,
                    delta_on_hand: dec(delta),
                    delta_committed: Decimal::ZERO,
                    delta_on_order: Decimal::ZERO,
                    delta_in_transit: Decimal::ZERO,
                    unit_cost: dec("50.0000"),
                    reason_type: "receipt".to_string(),
                    reference_id: Uuid::now_v7(),
                    created_at: when,
                },
            )
            .await
            .unwrap();
        }
        // Sale Jan 5: 4 x A @ 100 = 400 subtotal, tax 29.0000,
        // eco 3.0000, grand 432.0000. Return Jan 6: 1 x A @ 100
        // = -100 subtotal, tax -7.2500, eco -0.7500, grand -108.0000.
        let sale = transaction(
            org_id,
            store_id,
            TxSeed {
                doc_type: "sale",
                subtotal: "400.0000",
                tax: "29.0000",
                eco: "3.0000",
                grand: "432.0000",
                created_at: at(2026, 1, 5),
            },
        );
        let sale_id = sale.id;
        df_db::insert_transaction(pool, &sale).await.unwrap();
        df_db::insert_transaction_line(pool, &line(sale_id, product_a, "4", "100.0000", "50.0000"))
            .await
            .unwrap();
        let ret = transaction(
            org_id,
            store_id,
            TxSeed {
                doc_type: "return",
                subtotal: "-100.0000",
                tax: "-7.2500",
                eco: "-0.7500",
                grand: "-108.0000",
                created_at: at(2026, 1, 6),
            },
        );
        let ret_id = ret.id;
        df_db::insert_transaction(pool, &ret).await.unwrap();
        df_db::insert_transaction_line(pool, &line(ret_id, product_a, "-1", "100.0000", "50.0000"))
            .await
            .unwrap();
        // A draft (not completed) sale Jan 5 — must be ignored.
        let mut draft = transaction(
            org_id,
            store_id,
            TxSeed {
                doc_type: "sale",
                subtotal: "999.0000",
                tax: "0",
                eco: "0",
                grand: "999.0000",
                created_at: at(2026, 1, 5),
            },
        );
        draft.status = "draft".to_string();
        df_db::insert_transaction(pool, &draft).await.unwrap();
        // AR: charge 400.0000 due Jan 5 (current as of Jan 10),
        // charge 200.0000 due Dec 1 2025 (31-60 bucket as of Jan
        // 10), payment 150.0000 allocated to the Dec charge.
        let customer_id = Uuid::from_u128(0x4000);
        df_db::insert_customer(
            pool,
            &Customer {
                id: customer_id,
                org_id,
                account_number: "C-100".to_string(),
                customer_type: "contractor".to_string(),
                company_name: Some("Acme Painting".to_string()),
                first_name: None,
                last_name: None,
                email: None,
                phone: None,
                price_level_id: None,
                salesperson_id: None,
                tax_exempt_cert: None,
                tax_exempt_expires: None,
                charge_enabled: true,
                credit_limit: dec("10000.0000"),
                payment_terms: "NET30".to_string(),
                terms_days: Some(30),
                finance_rate_pct_monthly: Some(dec("1.5")),
                credit_hold: false,
                require_po_number: false,
                require_job_name: false,
                portal_enabled: false,
            },
        )
        .await
        .unwrap();
        let charge_old = Uuid::now_v7();
        let charge_new = Uuid::now_v7();
        let payment = Uuid::now_v7();
        for (id, entry_type, amount, due) in [
            (
                charge_old,
                "charge",
                "200.0000",
                NaiveDate::from_ymd_opt(2025, 12, 1),
            ),
            (
                charge_new,
                "charge",
                "400.0000",
                NaiveDate::from_ymd_opt(2026, 1, 5),
            ),
            (payment, "payment", "-150.0000", None),
        ] {
            df_db::insert_ar_entry_exec(
                pool,
                &ArLedgerEntry {
                    id,
                    org_id,
                    store_id,
                    customer_id,
                    job_id: None,
                    entry_type: entry_type.to_string(),
                    reference_type: "transaction".to_string(),
                    reference_id: Uuid::now_v7(),
                    amount: dec(amount),
                    due_date: due,
                    statement_id: None,
                    created_at: at(2026, 1, 5),
                },
            )
            .await
            .unwrap();
        }
        df_db::insert_ar_allocation(
            pool,
            &df_db::models::ArAllocation {
                id: Uuid::now_v7(),
                payment_entry_id: payment,
                charge_entry_id: charge_old,
                allocated_amount: dec("150.0000"),
                created_at: at(2026, 1, 6),
            },
        )
        .await
        .unwrap();
        // Formulas (tracking only): 2 x OC-17 on Jan 5 (tint
        // cost 2.5000 each), 1 x HC-172 on Jan 20 — outside the
        // Jan 1–10 window, must be excluded.
        for (code, name, cost, when) in [
            ("OC-17", "White Dove", "2.5000", at(2026, 1, 5)),
            ("OC-17", "White Dove", "2.5000", at(2026, 1, 5)),
            ("HC-172", "Revere Pewter", "3.0000", at(2026, 1, 20)),
        ] {
            df_db::insert_tint_formula(
                pool,
                &TintFormula {
                    id: Uuid::now_v7(),
                    org_id,
                    store_id,
                    customer_id: None,
                    job_id: None,
                    color_id: None,
                    color_code_snapshot: code.to_string(),
                    color_name_snapshot: name.to_string(),
                    base_product_id: product_a,
                    container_volume_ml: 3785,
                    sheen: None,
                    room_or_usage: None,
                    is_custom_match: false,
                    colorants_json: serde_json::json!([]),
                    total_tint_cost: dec(cost),
                    tint_surcharge: Decimal::ZERO,
                    dispenser_name: None,
                    external_formula_ref: None,
                    dispensed_at: when,
                },
            )
            .await
            .unwrap();
        }
        (org_id, store_id, product_a, product_b, customer_id)
    }

    #[tokio::test]
    async fn kpi_dashboard_math_is_exact() {
        let pool = test_pool().await;
        let (org_id, store_id, _, _, _) = seed_kpi_scenario(&pool).await;
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 1, 10).unwrap();
        let dashboard = kpi_dashboard(&pool, org_id, store_id, start, end, end, Currency::Usd)
            .await
            .unwrap();
        // Sales: gross 400, returns 100, net 300; tax 29 - 7.25
        // = 21.75; eco 3 - 0.75 = 2.25; grand 432 - 108 = 324.
        // One sale, so avg ticket = net 300 / 1.
        assert_eq!(dashboard.sales.sale_count, 1);
        assert_eq!(dashboard.sales.return_count, 1);
        assert_eq!(dashboard.sales.gross_sales.amount(), dec("400.0000"));
        assert_eq!(dashboard.sales.returns.amount(), dec("100.0000"));
        assert_eq!(dashboard.sales.net_sales.amount(), dec("300.0000"));
        assert_eq!(dashboard.sales.tax_total.amount(), dec("21.7500"));
        assert_eq!(dashboard.sales.eco_fee_total.amount(), dec("2.2500"));
        assert_eq!(dashboard.sales.grand_total.amount(), dec("324.0000"));
        assert_eq!(
            dashboard.sales.avg_ticket.unwrap().amount(),
            dec("300.0000")
        );
        // Margin: revenue 400 - 100 = 300; COGS 4*50 - 1*50 =
        // 150; gross profit 150; margin 50%.
        assert_eq!(dashboard.margin.revenue.amount(), dec("300.0000"));
        assert_eq!(dashboard.margin.cogs.amount(), dec("150.0000"));
        assert_eq!(dashboard.margin.gross_profit.amount(), dec("150.0000"));
        assert_eq!(dashboard.margin.margin_pct, Some(dec("50")));
        // Turns: COGS 150 / avg value (500 + 300) / 2 = 400
        // = 0.375. On-hand value at Jan 10 = 300.
        assert_eq!(dashboard.inventory.on_hand_value.amount(), dec("300.0000"));
        assert_eq!(
            dashboard.inventory.avg_inventory_value.amount(),
            dec("400.0000")
        );
        assert_eq!(dashboard.inventory.turns, Some(dec("0.375")));
        // AR as of Jan 10: Dec 1 charge 200 - 150 allocated =
        // 50 open in 31-60 (40 days past due); Jan 5 charge 400
        // open current. Total open 450.
        assert_eq!(dashboard.ar_aging.current.amount(), dec("400.0000"));
        assert_eq!(dashboard.ar_aging.days_31_60.amount(), dec("50.0000"));
        assert_eq!(dashboard.ar_aging.days_61_90.amount(), Decimal::ZERO);
        assert_eq!(dashboard.total_open_ar.amount(), dec("450.0000"));
        // Formulas: 2 x OC-17 in window (HC-172 on Jan 20
        // excluded); tint cost 5.0000.
        assert_eq!(dashboard.formulas.formulas_created, 2);
        assert_eq!(dashboard.formulas.per_color.len(), 1);
        assert_eq!(dashboard.formulas.per_color[0].color_code, "OC-17");
        assert_eq!(dashboard.formulas.per_color[0].formulas, 2);
        assert_eq!(dashboard.formulas.total_tint_cost.amount(), dec("5.0000"));
        // The text rendering names the headline figures.
        let text = dashboard.render_text();
        assert!(text.contains("net 300.0000"));
        assert!(text.contains("50% margin"));
        assert!(text.contains("0.375x turns"));
        assert!(text.contains("total open 450.0000"));
    }

    #[tokio::test]
    async fn kpi_dashboard_rejects_reversed_range() {
        let pool = test_pool().await;
        let (org_id, store_id) = seed_org_store(&pool).await;
        let err = kpi_dashboard(
            &pool,
            org_id,
            store_id,
            NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
            Currency::Usd,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("precedes start"));
    }

    #[tokio::test]
    async fn kpi_dashboard_empty_store_has_no_margin_or_turns() {
        let pool = test_pool().await;
        let (org_id, store_id) = seed_org_store(&pool).await;
        let dashboard = kpi_dashboard(
            &pool,
            org_id,
            store_id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
            Currency::Usd,
        )
        .await
        .unwrap();
        assert_eq!(dashboard.sales.sale_count, 0);
        assert_eq!(dashboard.margin.margin_pct, None);
        assert_eq!(dashboard.inventory.turns, None);
        assert_eq!(dashboard.total_open_ar.amount(), Decimal::ZERO);
        assert_eq!(dashboard.formulas.formulas_created, 0);
    }

    #[tokio::test]
    async fn ad_hoc_inquiry_filters_by_product_customer_and_type() {
        let pool = test_pool().await;
        let (org_id, store_id, product_a, product_b, customer_id) = seed_kpi_scenario(&pool).await;
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 1, 31).unwrap();
        // No filters: sale + return (the draft is excluded).
        let rows = run_inquiry(
            &pool,
            store_id,
            &AdHocInquiry {
                start,
                end,
                customer_id: None,
                product_id: None,
                doc_type: None,
            },
            Currency::Usd,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        // Product B was never sold.
        let rows = run_inquiry(
            &pool,
            store_id,
            &AdHocInquiry {
                start,
                end,
                customer_id: None,
                product_id: Some(product_b),
                doc_type: None,
            },
            Currency::Usd,
        )
        .await
        .unwrap();
        assert!(rows.is_empty());
        // Product A: both rows.
        let rows = run_inquiry(
            &pool,
            store_id,
            &AdHocInquiry {
                start,
                end,
                customer_id: None,
                product_id: Some(product_a),
                doc_type: None,
            },
            Currency::Usd,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        // Doc type return: one row, grand -108.0000.
        let rows = run_inquiry(
            &pool,
            store_id,
            &AdHocInquiry {
                start,
                end,
                customer_id: None,
                product_id: None,
                doc_type: Some("return".to_string()),
            },
            Currency::Usd,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].grand_total.amount(), dec("-108.0000"));
        // Customer filter: the seeded sale/return have no
        // customer, so filtering by the AR customer matches none.
        let rows = run_inquiry(
            &pool,
            store_id,
            &AdHocInquiry {
                start,
                end,
                customer_id: Some(customer_id),
                product_id: None,
                doc_type: None,
            },
            Currency::Usd,
        )
        .await
        .unwrap();
        assert!(rows.is_empty());
        let _ = org_id;
    }

    #[tokio::test]
    async fn kpi_dashboard_pdf_is_a_real_pdf_with_the_key_figures() {
        let pool = test_pool().await;
        let (org_id, store_id, _, _, _) = seed_kpi_scenario(&pool).await;
        let dashboard = kpi_dashboard(
            &pool,
            org_id,
            store_id,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
            Currency::Usd,
        )
        .await
        .unwrap();
        let source = render_kpi_dashboard_typst(&dashboard, "Test Paint Co", "Main Store");
        assert!(source.contains("KPI Dashboard"));
        // Money is Typst-escaped in the source (`\$`), so the
        // literal `$` appears escaped.
        assert!(source.contains("\\$300.00"));
        assert!(source.contains("50%"));
        let pdf = render_kpi_dashboard_pdf(&dashboard, "Test Paint Co", "Main Store", None)
            .expect("typst must render the KPI dashboard (see DF_TYPST_BIN)");
        assert!(pdf.starts_with(b"%PDF"), "bytes do not start with %PDF");
        assert!(
            pdf.len() > 1000,
            "PDF suspiciously small: {} bytes",
            pdf.len()
        );
    }
}
