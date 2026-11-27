//! Shared sale-pricing math for checkout, price preview, quotes and
//! returns.
//!
//! One function prices lines: matrix pricing from `df-db` rules
//! (falling back to the line's `unit_price` when no rule matches),
//! tax from the `df-domain` presets for the store's country/region,
//! and eco fees from the product's category + container volume. Tax
//! is computed per line on `line_total + eco_fee` (eco fees are
//! taxable in the shipped presets) and summed, so the tax total is
//! exactly the sum of the line taxes. All money is `Decimal`.
//!
//! Card surcharge is NOT computed here: it comes from the store's
//! [`crate::types::EdgeConfig::surcharge_rules`] via
//! `df_payments::compute_surcharge` in [`crate::edge`] (default: 3%
//! on card tenders only).

use chrono::Utc;
use df_db::AnyPool;
use df_db::models::Product;
use df_domain::pricing::{PricingInput, evaluate_price};
use df_domain::tax::{
    Country, EcoFeeCategory, compute_tax, eco_fee_for, eco_fee_presets, find_tax_rule, tax_presets,
};
use df_domain::{Currency, Money};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::error::EdgeError;
use crate::types::SaleLineInput;

/// Store tax/eco context, loaded once per pricing run.
#[derive(Debug, Clone)]
pub struct PricingContext {
    pub currency: Currency,
    pub country: Option<Country>,
    pub region: Option<String>,
    pub rules: Vec<df_domain::pricing::PricingRule>,
}

impl PricingContext {
    /// Load the store row (country/region) and the org's pricing
    /// rules. A missing store row means "no tax, no eco fees" rather
    /// than a failed sale — the edge must keep selling offline with
    /// whatever data it has.
    pub async fn load(pool: &AnyPool, org_id: Uuid, store_id: Uuid, currency: Currency) -> Self {
        let (country, region) = match df_db::get_store(pool, store_id).await {
            Ok(store) => (
                match store.country_code.as_str() {
                    "US" => Some(Country::Us),
                    "CA" => Some(Country::Ca),
                    _ => None,
                },
                Some(store.region_code),
            ),
            Err(_) => (None, None),
        };
        let rules = df_db::list_pricing_rules(pool, org_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| row.to_domain().ok())
            .collect();
        Self {
            currency,
            country,
            region,
            rules,
        }
    }
}

/// One priced line.
#[derive(Debug, Clone, PartialEq)]
pub struct PricedLine {
    pub product_id: Uuid,
    pub sku: String,
    pub description: String,
    pub qty: Decimal,
    pub uom: String,
    pub unit_price: Money,
    pub unit_cost: Money,
    pub line_total: Money,
    pub tax_amount: Money,
    pub eco_fee_amount: Money,
    pub rule_id: Option<Uuid>,
}

/// Priced lines plus the pre-surcharge totals.
#[derive(Debug, Clone, PartialEq)]
pub struct PricedSale {
    pub lines: Vec<PricedLine>,
    pub subtotal: Money,
    pub tax_total: Money,
    pub eco_fee_total: Money,
}

fn eco_category(text: Option<&str>) -> EcoFeeCategory {
    match text {
        Some("architectural_paint") => EcoFeeCategory::ArchitecturalPaint,
        Some("stain") => EcoFeeCategory::Stain,
        Some("aerosol") => EcoFeeCategory::Aerosol,
        _ => EcoFeeCategory::None,
    }
}

/// Tax + eco fee for one line, given its signed extended total and
/// quantity. Shared by sales (via [`price_lines`]) and returns
/// (which reuse the original unit price, never matrix pricing).
pub async fn tax_and_eco_for(
    pool: &AnyPool,
    ctx: &PricingContext,
    product_id: Uuid,
    line_total: Money,
    qty: Decimal,
) -> Result<(Money, Money), EdgeError> {
    let currency = ctx.currency;
    let today = Utc::now().date_naive();
    let product: Option<Product> = df_db::get_product(pool, product_id).await.ok();
    let eco_fee = match (&ctx.country, &ctx.region, &product) {
        (Some(country), Some(region), Some(product)) => eco_fee_for(
            &eco_fee_presets(),
            *country,
            region,
            eco_category(product.eco_fee_category.as_deref()),
            product.container_volume_ml.unwrap_or(0) as u32,
            qty,
            today,
        )
        .unwrap_or_else(|| Money::zero(currency)),
        _ => Money::zero(currency),
    };
    let taxable = line_total.checked_add(eco_fee)?;
    let tax_amount = match (&ctx.country, &ctx.region) {
        (Some(country), Some(region)) => match find_tax_rule(&tax_presets(), *country, region) {
            Some(rule) => compute_tax(rule, taxable, false)?.total,
            None => Money::zero(currency),
        },
        _ => Money::zero(currency),
    };
    Ok((tax_amount, eco_fee))
}

/// Price lines with matrix pricing, tax and eco fees. (Returns do
/// not use this function: they reuse the original unit price and call
/// [`tax_and_eco_for`] directly.) `price_level_id` is the effective
/// level for this sale (the caller resolves `SaleInput.price_level_id`
/// ?? the customer's stored level); it is passed straight into the
/// matrix engine, so a level rule (e.g. Gold 20% off list) prices
/// the line.
pub async fn price_lines(
    pool: &AnyPool,
    ctx: &PricingContext,
    customer_id: Option<Uuid>,
    job_id: Option<Uuid>,
    price_level_id: Option<Uuid>,
    lines: &[SaleLineInput],
) -> Result<PricedSale, EdgeError> {
    let currency = ctx.currency;
    let today = Utc::now().date_naive();
    let mut priced = Vec::with_capacity(lines.len());
    let mut subtotal = Money::zero(currency);
    let mut tax_total = Money::zero(currency);
    let mut eco_fee_total = Money::zero(currency);

    for line in lines {
        let qty = line.qty;
        // Matrix pricing: list price is the line's entered price, so
        // evaluate_price falls back to it when no rule matches.
        let input = PricingInput {
            list_price: line.unit_price,
            cost: line.unit_cost,
            qty: line.qty,
            customer_id,
            job_id,
            price_level_id,
            product_id: Some(line.product_id),
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            on: today,
        };
        let outcome = evaluate_price(&ctx.rules, &input);
        let unit_price = outcome.unit_price;
        let line_total = unit_price.mul_decimal(qty);
        let (tax_amount, eco_fee) =
            tax_and_eco_for(pool, ctx, line.product_id, line_total, qty).await?;

        subtotal = subtotal.checked_add(line_total)?;
        tax_total = tax_total.checked_add(tax_amount)?;
        eco_fee_total = eco_fee_total.checked_add(eco_fee)?;
        priced.push(PricedLine {
            product_id: line.product_id,
            sku: line.sku.clone(),
            description: line.description.clone(),
            qty,
            uom: line.uom.clone(),
            unit_price,
            unit_cost: line.unit_cost,
            line_total,
            tax_amount,
            eco_fee_amount: eco_fee,
            rule_id: outcome.rule_id,
        });
    }

    Ok(PricedSale {
        lines: priced,
        subtotal,
        tax_total,
        eco_fee_total,
    })
}
