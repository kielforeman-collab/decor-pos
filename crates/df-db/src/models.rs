//! Row models for the spec §5 tables.
//!
//! Money is stored as `rust_decimal::Decimal` (NUMERIC(14,4) in
//! PostgreSQL; NUMERIC/text in SQLite) — never floats. Primary keys are
//! UUIDv7, minted with [`df_domain::new_id`]. JSON columns are
//! `serde_json::Value` in the models; the repository layer serialises
//! them to text for SQLite and JSONB for PostgreSQL.

use chrono::{DateTime, NaiveDate, Utc};
use df_domain::pricing::{CalcMethod, PricingRule};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DbError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub default_currency: String,
    pub enabled_modules: serde_json::Value,
    pub settings_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Store {
    pub id: Uuid,
    pub org_id: Uuid,
    pub code: String,
    pub name: String,
    pub country_code: String,
    pub region_code: String,
    pub postal_code: String,
    pub currency: String,
    pub tax_zone_id: Uuid,
    pub timezone: String,
    pub is_warehouse: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub password_hash: Option<String>,
    pub oidc_subject: Option<String>,
    pub counter_pin_hash: Option<String>,
    pub badge_token_hash: Option<String>,
    pub role_id: Uuid,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Product {
    pub id: Uuid,
    pub org_id: Uuid,
    pub sku: String,
    pub manufacturer_code: Option<String>,
    pub name: String,
    pub department_id: Uuid,
    pub class_id: Option<Uuid>,
    pub product_line_id: Option<Uuid>,
    pub primary_vendor_id: Option<Uuid>,
    pub product_type: String,
    pub base_uom: String,
    pub list_price: Decimal,
    pub standard_cost: Decimal,
    pub avg_cost: Decimal,
    pub min_margin_pct: Option<Decimal>,
    pub sheen: Option<String>,
    pub paint_base_code: Option<String>,
    pub container_volume_ml: Option<i32>,
    pub eco_fee_category: Option<String>,
    pub tax_class_id: Uuid,
    pub prompt_template_id: Option<Uuid>,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorCatalogEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub manufacturer: String,
    pub color_code: String,
    pub color_name: String,
    pub collection: Option<String>,
    pub rgb_hex: Option<String>,
    pub recommended_base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Colorant {
    pub id: Uuid,
    pub org_id: Uuid,
    pub system_name: String,
    pub code: String,
    pub name: String,
    pub cost_per_fl_oz: Decimal,
    pub shots_per_fl_oz: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TintFormula {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub color_id: Option<Uuid>,
    pub color_code_snapshot: String,
    pub color_name_snapshot: String,
    pub base_product_id: Uuid,
    pub container_volume_ml: i32,
    pub sheen: Option<String>,
    pub room_or_usage: Option<String>,
    pub is_custom_match: bool,
    pub colorants_json: serde_json::Value,
    pub total_tint_cost: Decimal,
    pub tint_surcharge: Decimal,
    pub dispenser_name: Option<String>,
    pub external_formula_ref: Option<String>,
    pub dispensed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Customer {
    pub id: Uuid,
    pub org_id: Uuid,
    pub account_number: String,
    pub customer_type: String,
    pub company_name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub price_level_id: Option<Uuid>,
    pub salesperson_id: Option<Uuid>,
    pub tax_exempt_cert: Option<String>,
    pub tax_exempt_expires: Option<NaiveDate>,
    pub charge_enabled: bool,
    pub credit_limit: Decimal,
    pub payment_terms: String,
    pub credit_hold: bool,
    pub require_po_number: bool,
    pub require_job_name: bool,
    pub portal_enabled: bool,
}

/// A row of `pricing_rules` (spec §5.4). `calc_method` is stored as text
/// (`fixed`, `pct_off_list`, `markup_cost`, `target_gm`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingRuleRow {
    pub id: Uuid,
    pub org_id: Uuid,
    pub priority: i32,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub price_level_id: Option<Uuid>,
    pub product_id: Option<Uuid>,
    pub product_line_id: Option<Uuid>,
    pub class_id: Option<Uuid>,
    pub department_id: Option<Uuid>,
    pub vendor_id: Option<Uuid>,
    pub min_qty: Decimal,
    pub calc_method: String,
    pub calc_value: Decimal,
    pub min_margin_floor: Option<Decimal>,
    pub effective_from: Option<DateTime<Utc>>,
    pub effective_to: Option<DateTime<Utc>>,
}

impl PricingRuleRow {
    /// Convert to the domain pricing rule (spec §5.4 `effective_from/to`
    /// are timestamps in the table; the domain engine compares dates).
    pub fn to_domain(&self) -> Result<PricingRule, DbError> {
        let calc_method = match self.calc_method.as_str() {
            "fixed" => CalcMethod::Fixed,
            "pct_off_list" => CalcMethod::PctOffList,
            "markup_cost" => CalcMethod::MarkupCost,
            "target_gm" => CalcMethod::TargetGm,
            other => {
                return Err(DbError::InvalidData(format!(
                    "unknown calc_method: {other}"
                )));
            }
        };
        Ok(PricingRule {
            id: self.id,
            priority: self.priority,
            customer_id: self.customer_id,
            job_id: self.job_id,
            price_level_id: self.price_level_id,
            product_id: self.product_id,
            product_line_id: self.product_line_id,
            class_id: self.class_id,
            department_id: self.department_id,
            vendor_id: self.vendor_id,
            min_qty: self.min_qty,
            calc_method,
            calc_value: self.calc_value,
            min_margin_floor: self.min_margin_floor,
            effective_from: self.effective_from.map(|d| d.date_naive()),
            effective_to: self.effective_to.map(|d| d.date_naive()),
        })
    }
}

/// One append-only row of `ar_ledger` (spec §5.4). `amount` is positive
/// when it increases the AR balance, negative when it reduces it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArLedgerEntry {
    pub id: Uuid,
    pub org_id: Uuid,
    pub store_id: Uuid,
    pub customer_id: Uuid,
    pub job_id: Option<Uuid>,
    pub entry_type: String,
    pub reference_type: String,
    pub reference_id: Uuid,
    pub amount: Decimal,
    pub due_date: Option<NaiveDate>,
    pub statement_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}
