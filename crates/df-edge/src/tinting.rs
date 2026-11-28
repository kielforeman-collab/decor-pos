//! Paint formula tracking and daily/low-stock reports on the edge
//! (formula tracking only — no tint machine control, no colorant
//! canister levels, no dispense jobs).
//!
//! * [`Edge::create_tint_formula`] — store a formula with authoritative
//!   costing computed from the org's `colorants` catalog.
//! * [`Edge::search_tint_formulas`] / [`Edge::search_colors`] —
//!   formula lookup by color or customer colour history, and
//!   color-catalog search (spec §6.1).
//! * [`Edge::list_colorants`] — the org's colorant catalog for the
//!   counter formula builder.
//! * [`Edge::daily_usage`] / [`Edge::low_stock`] — the `df-reports`
//!   reports over this edge's database.

use std::str::FromStr;

use chrono::NaiveDate;
use df_db::models::{ColorCatalogEntry, TintFormula};
use df_domain::Money;
use df_domain::tint::{TintColorant, tint_cost};
use df_reports::{DailyUsageReport, LowStockReport, ProductThreshold};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// One colorant from the org's catalog (formula tracking only — no
/// canister on-hand levels).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorantView {
    pub colorant_id: Uuid,
    pub system_name: String,
    pub code: String,
    pub name: String,
    pub cost_per_fl_oz: Decimal,
    pub shots_per_fl_oz: i32,
}

/// One colorant in a `POST /pos/tinting/formulas` body: the
/// colorant code, ounces and shots (formula tracking only — no
/// machine code anywhere in this path).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFormulaColorant {
    pub code: String,
    pub oz: Decimal,
    pub shots: u32,
}

/// `POST /pos/tinting/formulas` body (mirrors HQ
/// `POST /api/v1/tinting/formulas`, which takes the full
/// `TintFormula` row; the counter sends the fields it knows and
/// the server fills in the rest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTintFormulaRequest {
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub color_code: String,
    pub color_name: String,
    pub base_product_id: Uuid,
    pub container_volume_ml: i32,
    pub sheen: Option<String>,
    pub colorants: Vec<CreateFormulaColorant>,
}

impl Edge {
    /// Create a tint formula (formula tracking only). Every
    /// colorant code must exist in the `colorants` table for this
    /// org — an unknown code is a 400 naming the code, and nothing
    /// is stored. The base product must exist. The total tint cost
    /// is computed from the colorant rows (authoritative cost), not
    /// from anything the caller sends.
    pub async fn create_tint_formula(
        &self,
        request: CreateTintFormulaRequest,
    ) -> Result<TintFormula, EdgeError> {
        if request.color_code.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "color_code is required".to_string(),
            ));
        }
        if request.color_name.trim().is_empty() {
            return Err(EdgeError::InvalidInput(
                "color_name is required".to_string(),
            ));
        }
        if request.colorants.is_empty() {
            return Err(EdgeError::InvalidInput(
                "formula needs at least one colorant".to_string(),
            ));
        }
        if request.container_volume_ml <= 0 {
            return Err(EdgeError::InvalidInput(
                "container_volume_ml must be positive".to_string(),
            ));
        }
        df_db::get_product(self.pool(), request.base_product_id).await?;
        if let Some(customer_id) = request.customer_id {
            df_db::get_customer(self.pool(), customer_id).await?;
        }
        let colorants = df_db::list_colorants_for_org(self.pool(), self.config().org_id).await?;
        let mut lines = Vec::with_capacity(request.colorants.len());
        let mut stored = Vec::with_capacity(request.colorants.len());
        for input in &request.colorants {
            let Some(colorant) = colorants
                .iter()
                .find(|c| c.code.eq_ignore_ascii_case(input.code.trim()))
            else {
                return Err(EdgeError::InvalidInput(format!(
                    "unknown colorant code: {}",
                    input.code
                )));
            };
            lines.push(TintColorant {
                code: colorant.code.clone(),
                ounces: input.oz,
                shots: input.shots,
                shots_per_fl_oz: colorant.shots_per_fl_oz.try_into().map_err(|_| {
                    EdgeError::InvalidInput(format!("bad shots_per_fl_oz for {}", colorant.code))
                })?,
                cost_per_fl_oz: Money::new(colorant.cost_per_fl_oz, self.config().currency),
            });
            let oz_number = serde_json::Number::from_str(&input.oz.to_string()).map_err(|_| {
                EdgeError::InvalidInput(format!("bad oz for colorant {}", input.code))
            })?;
            stored.push(serde_json::json!({
                "code": colorant.code,
                "oz": oz_number,
                "shots": input.shots,
            }));
        }
        let total_tint_cost = tint_cost(&lines)?.amount();
        let formula = TintFormula {
            id: Uuid::now_v7(),
            org_id: self.config().org_id,
            store_id: self.config().store_id,
            customer_id: request.customer_id,
            job_id: request.job_id,
            color_id: None,
            color_code_snapshot: request.color_code.trim().to_string(),
            color_name_snapshot: request.color_name.trim().to_string(),
            base_product_id: request.base_product_id,
            container_volume_ml: request.container_volume_ml,
            sheen: request.sheen,
            room_or_usage: None,
            is_custom_match: false,
            colorants_json: serde_json::Value::Array(stored),
            total_tint_cost,
            tint_surcharge: Decimal::ZERO,
            dispenser_name: None,
            external_formula_ref: None,
            dispensed_at: chrono::Utc::now(),
        };
        df_db::insert_tint_formula(self.pool(), &formula).await?;
        Ok(formula)
    }

    /// Set (or replace) the operator-configured product thresholds
    /// used by [`Edge::low_stock`].
    pub fn set_product_thresholds(&self, thresholds: Vec<ProductThreshold>) {
        *self.product_thresholds().write().expect("thresholds lock") = thresholds;
    }

    /// Search colors by name or code, scoped to one manufacturer
    /// (spec §6.1 `GET /colors/search`).
    pub async fn search_colors(
        &self,
        manufacturer: &str,
        query: &str,
    ) -> Result<Vec<ColorCatalogEntry>, EdgeError> {
        Ok(df_db::search_colors(self.pool(), self.config().org_id, manufacturer, query).await?)
    }

    /// Search stored formulas: by customer colour history
    /// (`customer_id`, optionally one job and/or a color substring —
    /// newest first), or by color name/code across the org.
    /// At least one of `query` / `customer_id` must be given.
    pub async fn search_tint_formulas(
        &self,
        query: Option<&str>,
        customer_id: Option<Uuid>,
        job_id: Option<Uuid>,
    ) -> Result<Vec<TintFormula>, EdgeError> {
        if let Some(customer_id) = customer_id {
            let mut formulas =
                df_db::list_tint_formulas_for_customer_job(self.pool(), customer_id, job_id)
                    .await?;
            if let Some(query) = query {
                let needle = query.to_lowercase();
                formulas.retain(|formula| {
                    formula.color_code_snapshot.to_lowercase().contains(&needle)
                        || formula.color_name_snapshot.to_lowercase().contains(&needle)
                });
            }
            return Ok(formulas);
        }
        match query {
            Some(query) => {
                Ok(
                    df_db::search_tint_formulas_by_color(self.pool(), self.config().org_id, query)
                        .await?,
                )
            }
            None => Err(EdgeError::InvalidInput(
                "formula search needs a color query or a customer_id".to_string(),
            )),
        }
    }

    /// Colorant catalog for this org, sorted by code (used by the
    /// counter formula builder).
    pub async fn list_colorants(&self) -> Result<Vec<ColorantView>, EdgeError> {
        let mut colorants =
            df_db::list_colorants_for_org(self.pool(), self.config().org_id).await?;
        colorants.sort_by(|a, b| a.code.cmp(&b.code));
        Ok(colorants
            .into_iter()
            .map(|c| ColorantView {
                colorant_id: c.id,
                system_name: c.system_name,
                code: c.code,
                name: c.name,
                cost_per_fl_oz: c.cost_per_fl_oz,
                shots_per_fl_oz: c.shots_per_fl_oz,
            })
            .collect())
    }

    /// The daily usage report for `date` (UTC) over this edge's
    /// database.
    pub async fn daily_usage(&self, date: NaiveDate) -> Result<DailyUsageReport, EdgeError> {
        Ok(df_reports::daily_usage_report(
            self.pool(),
            self.config().org_id,
            self.config().store_id,
            date,
            self.config().currency,
        )
        .await?)
    }

    /// The low-stock report for `date`, using the operator-configured
    /// product thresholds ([`Edge::set_product_thresholds`]).
    pub async fn low_stock(&self, date: NaiveDate) -> Result<LowStockReport, EdgeError> {
        let thresholds = self
            .product_thresholds()
            .read()
            .expect("thresholds lock")
            .clone();
        Ok(
            df_reports::low_stock_report(self.pool(), self.config().store_id, date, &thresholds)
                .await?,
        )
    }
}
