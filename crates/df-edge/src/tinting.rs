//! Paint tinting on the edge (Phase 2, v0.3; spec §6.1/§7).
//!
//! The edge hosts an in-process [`VirtualTinter`] (the deterministic
//! simulator from `df-tinter`), backed by the `df-db` colorant levels
//! and `tint_jobs` tables:
//!
//! * [`Edge::dispense_tint`] — resolve a formula (by id, or by a
//!   customer's past color), dispense it through the virtual tinter,
//!   then persist the tint job and decrement the colorant levels in
//!   **one** local SQLite transaction. The virtual tinter validates
//!   the whole dispense before decrementing anything in memory, and
//!   the database decrement validates again inside the same
//!   transaction as the job insert — so a failed dispense decrements
//!   nothing, in memory or on disk. The in-memory tinter levels are
//!   re-synced from the database on every dispense, so the database
//!   stays the source of truth if the two ever disagree.
//! * [`Edge::search_tint_formulas`] / [`Edge::search_colors`] —
//!   formula lookup by color or by customer colour history, and
//!   color-catalog search (spec §6.1).
//! * [`Edge::colorant_levels`] — levels + shortage alerts.
//! * [`Edge::daily_usage`] / [`Edge::low_stock`] — the `df-reports`
//!   reports over this edge's database.
//!
//! The mapping between the in-memory domain type
//! ([`df_tinter::TintJob`]) and the persisted row
//! ([`df_db::models::TintJobRow`]) lives here, at the edge boundary
//! ([`tint_job_to_row`]).
//!
//! Two honest limits:
//!
//! * A stored formula's `sub_shots` must be absent or zero:
//!   `df-domain::TintColorant` models ounces + shots only, and
//!   guessing what a sub-shot means (a 384th? an eighth of a shot?)
//!   would silently mis-dispense. Such a formula fails with a clear
//!   error instead of dispensing the wrong amount.
//! * The virtual tinter is the only tinter hosted here. Real
//!   hardware bridges (file-drop / TCP / serial) and the
//!   `df-plugin-sdk` `HttpTinterPlugin` speak HTTP/JSON out of
//!   process and are **not** unified with the sync in-process
//!   `TinterPlugin` trait in this phase — that adapter is follow-up
//!   wiring (see the README).

use std::str::FromStr;

use chrono::NaiveDate;
use df_db::models::{ColorCatalogEntry, Colorant, TintFormula, TintJobRow};
use df_domain::Money;
use df_domain::tint::{TintColorant, tint_cost};
use df_reports::{ColorantShortageAlert, DailyUsageReport, LowStockReport, ProductThreshold};
use df_tinter::{DispenseRequest, DispensedShot, TintJob, TinterPlugin, VirtualTinter};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

/// A request to dispense one tint formula at the counter.
///
/// Exactly one of `formula_id` / `customer_id` must be given:
/// `formula_id` dispenses that stored formula; `customer_id`
/// (optionally scoped by `job_id` and/or filtered by `color_query`)
/// re-dispenses the customer's newest past formula — the repeat-tint
/// flow. `transaction_id` + `line_no` link the tint job to the sale
/// line it belongs to (the transaction must already exist).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispenseTintRequest {
    pub formula_id: Option<Uuid>,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub color_query: Option<String>,
    pub transaction_id: Option<Uuid>,
    pub line_no: Option<i32>,
    pub dispenser: Option<String>,
}

/// The outcome of a dispense: the recorded job, the exact amounts
/// dispensed and the formula's tint cost (store currency).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispenseTintResponse {
    pub job_id: Uuid,
    pub formula_id: Uuid,
    pub status: String,
    pub dispenser: String,
    pub dispensed: Vec<DispensedShot>,
    pub tint_cost: Money,
}

/// One colorant level with its catalog identity and low flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorantLevelView {
    pub colorant_id: Uuid,
    pub code: String,
    pub name: String,
    pub on_hand_fl_oz: Decimal,
    pub low_threshold_fl_oz: Decimal,
    pub is_low: bool,
}

/// `GET /pos/tinting/colorants` response: every level plus the
/// shortage alerts (the at/below-threshold subset).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorantsResponse {
    pub levels: Vec<ColorantLevelView>,
    pub alerts: Vec<ColorantShortageAlert>,
}

/// Map a domain [`TintJob`] to its persisted row. The inverse
/// direction is not needed: reads go through the `df-db` tint
/// repositories, which return rows directly.
pub fn tint_job_to_row(job: &TintJob) -> TintJobRow {
    TintJobRow {
        id: job.id,
        formula_id: job.formula_id,
        transaction_id: job.transaction_id,
        line_no: job.line_no,
        status: job.status.as_str().to_string(),
        dispensed_json: serde_json::to_value(&job.dispensed).unwrap_or(serde_json::json!([])),
        error: job.error.clone(),
        created_at: job.created_at,
        completed_at: job.completed_at,
    }
}

/// A JSON number or a decimal string, read as an exact [`Decimal`].
/// (Stored formulas carry `oz` as a bare number and `unit_cost` as a
/// string; both forms must parse without float math.)
fn json_decimal(value: &serde_json::Value) -> Option<Decimal> {
    match value {
        serde_json::Value::String(text) => Decimal::from_str(text).ok(),
        serde_json::Value::Number(number) => Decimal::from_str(&number.to_string()).ok(),
        _ => None,
    }
}

fn json_u32(value: &serde_json::Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// Build the dispense lines for a stored formula. The formula's
/// `colorants_json` is `[{ "code", "oz", "shots", "sub_shots"? }]`
/// (spec §5.3); `cost_per_fl_oz` and `shots_per_fl_oz` come from the
/// colorant row, which is authoritative — the snapshot `unit_cost`
/// in the JSON is not trusted for costing. A code with no colorant
/// row, or a nonzero `sub_shots` (see the module docs), is an
/// error before anything dispenses.
fn formula_colorants(
    formula: &TintFormula,
    colorants: &[Colorant],
    currency: df_domain::Currency,
) -> Result<Vec<TintColorant>, EdgeError> {
    let items = formula.colorants_json.as_array().ok_or_else(|| {
        EdgeError::InvalidInput(format!(
            "formula {} colorants_json is not an array",
            formula.id
        ))
    })?;
    let mut lines = Vec::with_capacity(items.len());
    for item in items {
        let code = item
            .get("code")
            .and_then(|c| c.as_str())
            .ok_or_else(|| {
                EdgeError::InvalidInput(format!("formula colorant has no code: {item}"))
            })?
            .to_string();
        let ounces = item.get("oz").and_then(json_decimal).ok_or_else(|| {
            EdgeError::InvalidInput(format!("formula colorant {code} has no oz: {item}"))
        })?;
        let shots = item.get("shots").and_then(json_u32).ok_or_else(|| {
            EdgeError::InvalidInput(format!("formula colorant {code} has no shots: {item}"))
        })?;
        if let Some(sub_shots) = item.get("sub_shots").and_then(json_u32) {
            if sub_shots != 0 {
                return Err(EdgeError::InvalidInput(format!(
                    "formula colorant {code} has sub_shots {sub_shots}, which v0.3 cannot dispense: re-save the formula with ounces + shots only"
                )));
            }
        }
        let Some(colorant) = colorants
            .iter()
            .find(|c| c.code.eq_ignore_ascii_case(&code))
        else {
            return Err(EdgeError::NotFound(format!(
                "no colorant row for code {code} (formula {})",
                formula.id
            )));
        };
        lines.push(TintColorant {
            code,
            ounces,
            shots,
            shots_per_fl_oz: colorant.shots_per_fl_oz.try_into().map_err(|_| {
                EdgeError::InvalidInput(format!("bad shots_per_fl_oz for {}", colorant.code))
            })?,
            cost_per_fl_oz: Money::new(colorant.cost_per_fl_oz, currency),
        });
    }
    if lines.is_empty() {
        return Err(EdgeError::InvalidInput(format!(
            "formula {} has no colorants",
            formula.id
        )));
    }
    Ok(lines)
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
}

impl Edge {
    /// Set (or replace) the operator-configured product thresholds
    /// used by [`Edge::low_stock`]. There is no min-level table yet
    /// (see `df-reports` docs), so thresholds are runtime config.
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

    /// Colorant levels for this store with catalog names, plus the
    /// shortage alerts.
    pub async fn colorant_levels(&self) -> Result<ColorantsResponse, EdgeError> {
        let rows =
            df_db::list_colorant_levels_for_store(self.pool(), self.config().store_id).await?;
        let mut levels = Vec::with_capacity(rows.len());
        for row in rows {
            let colorant = df_db::get_colorant(self.pool(), row.colorant_id).await?;
            levels.push(ColorantLevelView {
                colorant_id: row.colorant_id,
                code: colorant.code,
                name: colorant.name,
                on_hand_fl_oz: row.on_hand_fl_oz,
                low_threshold_fl_oz: row.low_threshold_fl_oz,
                is_low: row.on_hand_fl_oz <= row.low_threshold_fl_oz,
            });
        }
        levels.sort_by(|a, b| a.code.cmp(&b.code));
        let alerts =
            df_reports::colorant_shortage_alerts(self.pool(), self.config().store_id).await?;
        Ok(ColorantsResponse { levels, alerts })
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

    /// Dispense one tint formula through the hosted virtual tinter
    /// and persist the result (see the module docs for the atomicity
    /// argument). Returns the recorded job, the exact amounts
    /// dispensed and the tint cost.
    pub async fn dispense_tint(
        &self,
        request: DispenseTintRequest,
    ) -> Result<DispenseTintResponse, EdgeError> {
        // --- resolve the formula (before anything dispenses) ------
        let formula = match (request.formula_id, request.customer_id) {
            (Some(id), _) => df_db::get_tint_formula(self.pool(), id).await?,
            (None, Some(customer_id)) => {
                let formulas = self
                    .search_tint_formulas(
                        request.color_query.as_deref(),
                        Some(customer_id),
                        request.job_id,
                    )
                    .await?;
                formulas.into_iter().next().ok_or_else(|| {
                    EdgeError::NotFound(format!("no past formula for customer {customer_id}"))
                })?
            }
            (None, None) => {
                return Err(EdgeError::InvalidInput(
                    "dispense needs formula_id or customer_id".to_string(),
                ));
            }
        };
        if let Some(transaction_id) = request.transaction_id {
            df_db::get_transaction(self.pool(), transaction_id).await?;
        }
        let colorants = df_db::list_colorants_for_org(self.pool(), self.config().org_id).await?;
        let lines = formula_colorants(&formula, &colorants, self.config().currency)?;
        let cost = tint_cost(&lines)?;

        // --- dispense in memory (sync, tinter lock held briefly) ---
        // Levels are re-synced from the database on every dispense,
        // so the database stays the source of truth.
        let levels =
            df_db::list_colorant_levels_for_store(self.pool(), self.config().store_id).await?;
        let (outcome, job) = {
            let mut tinter = self.tinter().lock().expect("tinter lock");
            for level in &levels {
                if let Some(colorant) = colorants.iter().find(|c| c.id == level.colorant_id) {
                    tinter.set_level(
                        colorant.code.clone(),
                        level.on_hand_fl_oz,
                        level.low_threshold_fl_oz,
                    );
                }
            }
            let jobs_before = tinter.jobs().len();
            let outcome = tinter.send_formula(DispenseRequest {
                formula_id: formula.id,
                base_product_id: formula.base_product_id,
                base_product_name: formula.color_name_snapshot.clone(),
                container_volume_ml: formula.container_volume_ml,
                colorants: lines,
                transaction_id: request.transaction_id,
                line_no: request.line_no,
                dispenser: request.dispenser.clone(),
            });
            // `send_formula` records the job before it can fail, so
            // the job just pushed is the last one (the lock was held
            // throughout, so nothing interleaved).
            let jobs = tinter.jobs();
            let job = jobs.get(jobs_before).cloned().ok_or_else(|| {
                EdgeError::InvalidInput("tinter recorded no job for the dispense".to_string())
            })?;
            (outcome, job)
        };

        match outcome {
            Ok(ack) => {
                // --- persist job + decrements in ONE transaction ---
                let row = tint_job_to_row(&job);
                let amounts: Vec<(Uuid, Decimal)> = job
                    .dispensed
                    .iter()
                    .map(|shot| {
                        colorants
                            .iter()
                            .find(|c| c.code.eq_ignore_ascii_case(&shot.code))
                            .map(|c| (c.id, shot.fl_oz))
                            .ok_or_else(|| {
                                EdgeError::NotFound(format!(
                                    "no colorant row for dispensed code {}",
                                    shot.code
                                ))
                            })
                    })
                    .collect::<Result<_, _>>()?;
                let mut db_tx = self.pool().begin().await.map_err(df_db::DbError::Sqlx)?;
                df_db::insert_tint_job(&mut *db_tx, &row).await?;
                df_db::decrement_colorant_levels_in_tx(
                    &mut db_tx,
                    self.config().store_id,
                    &amounts,
                )
                .await?;
                db_tx.commit().await.map_err(df_db::DbError::Sqlx)?;
                Ok(DispenseTintResponse {
                    job_id: ack.job_id,
                    formula_id: formula.id,
                    status: ack.status.as_str().to_string(),
                    dispenser: ack.dispenser,
                    dispensed: job.dispensed.clone(),
                    tint_cost: cost,
                })
            }
            Err(err) => {
                // The virtual tinter decremented nothing; persist the
                // failed job row as the trace (best-effort — if the
                // insert itself fails, the dispense error below is
                // still the result the counter sees).
                let row = tint_job_to_row(&job);
                let _ = df_db::insert_tint_job(self.pool(), &row).await;
                Err(EdgeError::Tinter(err))
            }
        }
    }
}

/// Build a fresh virtual tinter pre-loaded with the edge's database
/// levels. Used by tests and by any caller that wants a standalone
/// simulator seeded from persisted levels instead of the edge's
/// hosted one.
pub async fn virtual_tinter_from_levels(
    pool: &df_db::AnyPool,
    org_id: Uuid,
    store_id: Uuid,
) -> Result<VirtualTinter, EdgeError> {
    let colorants = df_db::list_colorants_for_org(pool, org_id).await?;
    let levels = df_db::list_colorant_levels_for_store(pool, store_id).await?;
    let mut tinter = VirtualTinter::new("virtual-simulator");
    for level in levels {
        if let Some(colorant) = colorants.iter().find(|c| c.id == level.colorant_id) {
            tinter.set_level(
                colorant.code.clone(),
                level.on_hand_fl_oz,
                level.low_threshold_fl_oz,
            );
        }
    }
    Ok(tinter)
}
