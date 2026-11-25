//! Units of measure and flooring quantity math.
//!
//! All quantities are `rust_decimal::Decimal` — never floats. Conversions
//! between volume/area units use exact decimal factors; conversions that
//! need a product-specific factor (box -> sq ft, roll -> sq ft) take that
//! factor explicitly, mirroring `product_uoms.to_base_factor` (spec §5.2).

use crate::error::DomainError;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Sellable / purchasable units (spec §5.2 `base_uom` plus flooring units).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Uom {
    Ea,
    Gal,
    Qt,
    SqFt,
    SqYd,
    Box,
    Roll,
    LinearFt,
}

impl Uom {
    pub fn code(self) -> &'static str {
        match self {
            Uom::Ea => "ea",
            Uom::Gal => "gal",
            Uom::Qt => "qt",
            Uom::SqFt => "sqft",
            Uom::SqYd => "sqyd",
            Uom::Box => "box",
            Uom::Roll => "roll",
            Uom::LinearFt => "linft",
        }
    }
}

impl fmt::Display for Uom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Uom {
    type Err = DomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ea" | "each" => Ok(Uom::Ea),
            "gal" | "gallon" => Ok(Uom::Gal),
            "qt" | "quart" => Ok(Uom::Qt),
            "sqft" | "sq ft" | "sq_ft" => Ok(Uom::SqFt),
            "sqyd" | "sq yd" | "sq_yd" => Ok(Uom::SqYd),
            "box" => Ok(Uom::Box),
            "roll" => Ok(Uom::Roll),
            "linft" | "linear ft" | "linear_ft" | "lf" => Ok(Uom::LinearFt),
            other => Err(DomainError::InvalidUomConversion(format!(
                "unknown UOM: {other}"
            ))),
        }
    }
}

fn dec(n: i64) -> Decimal {
    Decimal::from(n)
}

/// Convert a quantity between two units whose relationship is a fixed
/// exact factor. Product-specific conversions (box/roll -> sq ft) need
/// [`convert_with_factor`] with the product's `to_base_factor`.
pub fn convert_qty(qty: Decimal, from: Uom, to: Uom) -> Result<Decimal, DomainError> {
    if qty < Decimal::ZERO {
        return Err(DomainError::InvalidQuantity(format!(
            "quantity must not be negative: {qty}"
        )));
    }
    if from == to {
        return Ok(qty);
    }
    let result = match (from, to) {
        (Uom::Gal, Uom::Qt) => qty * dec(4),
        (Uom::Qt, Uom::Gal) => qty / dec(4),
        (Uom::SqFt, Uom::SqYd) => qty / dec(9),
        (Uom::SqYd, Uom::SqFt) => qty * dec(9),
        _ => {
            return Err(DomainError::InvalidUomConversion(format!(
                "{from} -> {to} needs a product-specific to_base_factor; use convert_with_factor"
            )));
        }
    };
    Ok(result)
}

/// Convert using a product-specific factor: `qty * factor` converts from
/// the selling UOM to the base UOM (e.g. 1 box = 22.5 sq ft).
pub fn convert_with_factor(qty: Decimal, factor: Decimal) -> Result<Decimal, DomainError> {
    if qty < Decimal::ZERO {
        return Err(DomainError::InvalidQuantity(format!(
            "quantity must not be negative: {qty}"
        )));
    }
    if factor <= Decimal::ZERO {
        return Err(DomainError::InvalidUomConversion(format!(
            "to_base_factor must be positive: {factor}"
        )));
    }
    Ok(qty * factor)
}

/// Apply a waste percentage (e.g. flooring `default_waste_pct = 10.00`):
/// `qty * (1 + waste_pct / 100)`.
pub fn apply_waste(qty: Decimal, waste_pct: Decimal) -> Result<Decimal, DomainError> {
    if qty < Decimal::ZERO {
        return Err(DomainError::InvalidQuantity(format!(
            "quantity must not be negative: {qty}"
        )));
    }
    if waste_pct < Decimal::ZERO || waste_pct > dec(100) {
        return Err(DomainError::InvalidPercentage(format!(
            "waste pct must be in [0, 100]: {waste_pct}"
        )));
    }
    Ok(qty * (Decimal::ONE + waste_pct / dec(100)))
}

/// Result of a flooring quantity calculation (spec §4 `vertical_flooring`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlooringCalc {
    /// Area including waste, in sq ft.
    pub waste_adjusted_sqft: Decimal,
    /// Whole boxes to sell (rounded up).
    pub boxes: u32,
    /// Sq ft actually purchased (boxes * coverage).
    pub total_sqft_purchased: Decimal,
}

/// How many boxes cover `area_sqft` at `box_coverage_sqft` per box with
/// `waste_pct` waste, rounding up to whole boxes.
pub fn flooring_boxes_needed(
    area_sqft: Decimal,
    box_coverage_sqft: Decimal,
    waste_pct: Decimal,
) -> Result<FlooringCalc, DomainError> {
    if area_sqft < Decimal::ZERO {
        return Err(DomainError::InvalidQuantity(format!(
            "area must not be negative: {area_sqft}"
        )));
    }
    if box_coverage_sqft <= Decimal::ZERO {
        return Err(DomainError::InvalidUomConversion(format!(
            "box coverage must be positive: {box_coverage_sqft}"
        )));
    }
    let waste_adjusted_sqft = apply_waste(area_sqft, waste_pct)?;
    let boxes_decimal = (waste_adjusted_sqft / box_coverage_sqft).ceil();
    let boxes: u32 = boxes_decimal
        .to_string()
        .parse()
        .map_err(|_| DomainError::InvalidQuantity("box count overflow".to_string()))?;
    Ok(FlooringCalc {
        waste_adjusted_sqft,
        boxes,
        total_sqft_purchased: Decimal::from(boxes) * box_coverage_sqft,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn volume_and_area_conversions_are_exact() {
        assert_eq!(convert_qty(d("2"), Uom::Gal, Uom::Qt).unwrap(), d("8"));
        assert_eq!(convert_qty(d("8"), Uom::Qt, Uom::Gal).unwrap(), d("2"));
        assert_eq!(convert_qty(d("90"), Uom::SqFt, Uom::SqYd).unwrap(), d("10"));
        assert_eq!(convert_qty(d("10"), Uom::SqYd, Uom::SqFt).unwrap(), d("90"));
        assert_eq!(convert_qty(d("3"), Uom::Ea, Uom::Ea).unwrap(), d("3"));
    }

    #[test]
    fn box_conversion_needs_a_factor() {
        assert!(convert_qty(d("1"), Uom::Box, Uom::SqFt).is_err());
        assert_eq!(convert_with_factor(d("2"), d("22.5")).unwrap(), d("45"));
    }

    #[test]
    fn flooring_boxes_round_up_with_waste() {
        // 100 sq ft + 10% waste = 110 sq ft; 22.5 sq ft/box -> 5 boxes (112.5).
        let calc = flooring_boxes_needed(d("100"), d("22.5"), d("10")).unwrap();
        assert_eq!(calc.waste_adjusted_sqft, d("110"));
        assert_eq!(calc.boxes, 5);
        assert_eq!(calc.total_sqft_purchased, d("112.5"));
    }

    #[test]
    fn negative_quantities_rejected() {
        assert!(convert_qty(d("-1"), Uom::Gal, Uom::Qt).is_err());
        assert!(apply_waste(d("10"), d("101")).is_err());
    }
}
