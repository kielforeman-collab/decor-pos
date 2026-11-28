//! Bulk matrix pricing editor with min-margin floor guardrail.

use chrono::{NaiveDate, Utc};
use df_db::models::PricingRuleRow;
use df_domain::Money;
use df_domain::pricing::{PricingRule, validate_rule};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::edge::Edge;
use crate::error::EdgeError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PricingRuleUpdate {
    pub id: Uuid,
    pub calc_method: String,
    pub calc_value: Decimal,
    pub min_margin_floor: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BulkPricingRequest {
    pub rules: Vec<PricingRuleUpdate>,
}

/// The min-margin guardrail for a bulk pricing update: the rule's own
/// computed price at `list_price` / `cost` must not sit below its
/// `min_margin_floor`. Returns the rejection reason, if any.
fn margin_guardrail_rejection(
    rule: &PricingRule,
    list_price: Money,
    cost: Money,
    qty: Decimal,
    on: NaiveDate,
) -> Option<String> {
    let floor = rule.min_margin_floor?;
    let input = df_domain::pricing::PricingInput {
        list_price,
        cost,
        qty,
        customer_id: rule.customer_id,
        job_id: rule.job_id,
        price_level_id: rule.price_level_id,
        product_id: rule.product_id,
        product_line_id: rule.product_line_id,
        class_id: rule.class_id,
        department_id: rule.department_id,
        vendor_id: rule.vendor_id,
        on,
    };
    let Some(price) = rule.compute_price(&input) else {
        return Some(format!(
            "rule {} computes no valid price for its product",
            rule.id
        ));
    };
    let margin = price.margin_pct(cost).ok().flatten();
    let below_floor = match margin {
        Some(margin) => margin < floor,
        None => true,
    };
    if below_floor {
        Some(format!(
            "rule {} would price at {} (margin {:?}%) which is below its min-margin floor of {}%",
            rule.id,
            price.amount(),
            margin,
            floor
        ))
    } else {
        None
    }
}

impl Edge {
    // --- bulk matrix pricing --------------------------------------------------

    /// All pricing rules for the org, by priority.
    pub async fn list_pricing_rules(&self) -> Result<Vec<PricingRuleRow>, EdgeError> {
        Ok(df_db::list_pricing_rules(self.pool(), self.config().org_id).await?)
    }

    /// Bulk-update pricing-rule calculations. Every update is fully
    /// validated first (static rule checks + the min-margin
    /// guardrail, evaluated against the rule's product when the rule
    /// targets one product); a single rejection fails the whole
    /// batch with no rule changed.
    pub async fn bulk_update_pricing_rules(
        &self,
        request: BulkPricingRequest,
    ) -> Result<Vec<PricingRuleRow>, EdgeError> {
        if request.rules.is_empty() {
            return Err(EdgeError::InvalidInput(
                "bulk pricing update needs at least one rule".to_string(),
            ));
        }
        let today = Utc::now().date_naive();
        let mut updated: Vec<PricingRuleRow> = Vec::with_capacity(request.rules.len());
        for input in &request.rules {
            let mut row = df_db::get_pricing_rule(self.pool(), input.id).await?;
            match input.calc_method.as_str() {
                "fixed" | "pct_off_list" | "markup_cost" | "target_gm" => {}
                other => {
                    return Err(EdgeError::InvalidInput(format!(
                        "unknown calc_method for rule {}: {other:?}",
                        input.id
                    )));
                }
            }
            let value_ok = match input.calc_method.as_str() {
                "fixed" => input.calc_value >= Decimal::ZERO,
                "pct_off_list" => {
                    input.calc_value >= Decimal::ZERO && input.calc_value <= Decimal::from(100)
                }
                "markup_cost" => input.calc_value >= Decimal::ZERO,
                "target_gm" => {
                    input.calc_value >= Decimal::ZERO && input.calc_value < Decimal::from(100)
                }
                _ => false,
            };
            if !value_ok {
                return Err(EdgeError::InvalidInput(format!(
                    "calc_value {} is out of range for calc_method {} (rule {})",
                    input.calc_value, input.calc_method, input.id
                )));
            }
            row.calc_method = input.calc_method.clone();
            row.calc_value = input.calc_value;
            row.min_margin_floor = input.min_margin_floor;
            let domain: PricingRule = row.to_domain()?;
            validate_rule(&domain)?;
            if let Some(product_id) = domain.product_id {
                let product = df_db::get_product(self.pool(), product_id).await?;
                let list_price = Money::new(product.list_price, self.config().currency);
                let cost = Money::new(product.avg_cost, self.config().currency);
                if let Some(reason) =
                    margin_guardrail_rejection(&domain, list_price, cost, domain.min_qty, today)
                {
                    return Err(EdgeError::InvalidInput(format!(
                        "bulk pricing update rejected: {reason}"
                    )));
                }
            }
            updated.push(row);
        }
        for row in &updated {
            df_db::update_pricing_rule(self.pool(), row).await?;
        }
        Ok(updated)
    }
}
