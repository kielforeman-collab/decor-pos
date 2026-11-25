//! Color lookup and per-customer formula history (Phase 2, spec §6.1
//! `GET /colors/search`, `GET /customers/{id}/formulas`).
//!
//! These are pure in-memory lookups over slices / a small store, so
//! the counter can repeat a tint without a database round-trip. The
//! persisted versions live in `df-db::tint` (`search_colors`,
//! `list_tint_formulas_for_customer_job`); the two share the same
//! matching rules: case-insensitive substring on color name/code,
//! scoped by manufacturer, and customer history newest-first.

use chrono::{DateTime, Utc};
use df_domain::tint::TintColorant;
use uuid::Uuid;

/// One color-catalog entry (mirrors `df-db::ColorCatalogEntry`
/// without the database id / org fields).
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogColor {
    pub manufacturer: String,
    pub code: String,
    pub name: String,
    pub collection: Option<String>,
    pub rgb_hex: Option<String>,
    pub recommended_base: Option<String>,
}

impl CatalogColor {
    pub fn new(
        manufacturer: impl Into<String>,
        code: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        CatalogColor {
            manufacturer: manufacturer.into(),
            code: code.into(),
            name: name.into(),
            collection: None,
            rgb_hex: None,
            recommended_base: None,
        }
    }
}

/// Search colors by name or code, scoped to one manufacturer.
///
/// Both the manufacturer match and the query match are
/// case-insensitive; the query is a substring of the color code or
/// the color name. An empty query returns every color for that
/// manufacturer, in catalog order.
pub fn search_colors<'a>(
    colors: &'a [CatalogColor],
    manufacturer: &str,
    query: &str,
) -> Vec<&'a CatalogColor> {
    let query = query.to_lowercase();
    colors
        .iter()
        .filter(|color| color.manufacturer.eq_ignore_ascii_case(manufacturer))
        .filter(|color| {
            query.is_empty()
                || color.code.to_lowercase().contains(&query)
                || color.name.to_lowercase().contains(&query)
        })
        .collect()
}

/// One past formula, as kept in a customer's colour history.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredFormula {
    pub id: Uuid,
    pub customer_id: Option<Uuid>,
    pub job_id: Option<Uuid>,
    pub color_code: String,
    pub color_name: String,
    pub base_product_id: Uuid,
    pub dispensed_at: DateTime<Utc>,
    pub colorants: Vec<TintColorant>,
}

/// In-memory per-customer formula history: a repeat tint pulls the
/// same formula, newest first.
#[derive(Debug, Default)]
pub struct FormulaHistory {
    formulas: Vec<StoredFormula>,
}

impl FormulaHistory {
    pub fn new() -> Self {
        FormulaHistory::default()
    }

    pub fn record(&mut self, formula: StoredFormula) {
        self.formulas.push(formula);
    }

    /// A customer's past formulas, newest first. With `job_id` given,
    /// only formulas for that job are returned.
    pub fn history_for_customer(
        &self,
        customer_id: Uuid,
        job_id: Option<Uuid>,
    ) -> Vec<&StoredFormula> {
        let mut found: Vec<&StoredFormula> = self
            .formulas
            .iter()
            .filter(|formula| formula.customer_id == Some(customer_id))
            .filter(|formula| job_id.is_none_or(|job| formula.job_id == Some(job)))
            .collect();
        // Stable sort: equal timestamps keep insertion order.
        found.sort_by_key(|a| std::cmp::Reverse(a.dispensed_at));
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use df_domain::{Currency, Money};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn catalog() -> Vec<CatalogColor> {
        vec![
            CatalogColor::new("Benjamin Moore", "OC-17", "White Dove"),
            CatalogColor::new("Benjamin Moore", "HC-172", "Revere Pewter"),
            CatalogColor::new("Benjamin Moore", "HC-154", "Hale Navy"),
            CatalogColor::new("PPG", "PPG1012-1", "White Dove"),
        ]
    }

    #[test]
    fn search_matches_name_and_code_case_insensitively() {
        let colors = catalog();
        let found = search_colors(&colors, "Benjamin Moore", "white dove");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "OC-17");
        let found = search_colors(&colors, "Benjamin Moore", "oc-");
        assert_eq!(found.len(), 1);
        let found = search_colors(&colors, "benjamin moore", "REVERE");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "HC-172");
    }

    #[test]
    fn search_is_scoped_by_manufacturer() {
        let colors = catalog();
        // PPG also has a "White Dove" — it must not leak into BM results.
        let found = search_colors(&colors, "Benjamin Moore", "White Dove");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manufacturer, "Benjamin Moore");
        let found = search_colors(&colors, "PPG", "White Dove");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].manufacturer, "PPG");
        // Unknown manufacturer: nothing.
        assert!(search_colors(&colors, "Sherwin-Williams", "Dove").is_empty());
    }

    #[test]
    fn search_substring_and_empty_query() {
        let colors = catalog();
        let found = search_colors(&colors, "Benjamin Moore", "ew");
        assert_eq!(found.len(), 1); // Revere Pewter
        let found = search_colors(&colors, "Benjamin Moore", "");
        assert_eq!(found.len(), 3);
    }

    fn stored(
        customer_id: Uuid,
        job_id: Option<Uuid>,
        code: &str,
        dispensed_at: DateTime<Utc>,
    ) -> StoredFormula {
        StoredFormula {
            id: Uuid::now_v7(),
            customer_id: Some(customer_id),
            job_id,
            color_code: code.to_string(),
            color_name: code.to_string(),
            base_product_id: Uuid::now_v7(),
            dispensed_at,
            colorants: vec![TintColorant {
                code: "Y3".to_string(),
                ounces: Decimal::from_str("0").unwrap(),
                shots: 14,
                shots_per_fl_oz: 48,
                cost_per_fl_oz: Money::from_str_amount("0.4200", Currency::Usd).unwrap(),
            }],
        }
    }

    #[test]
    fn history_returns_newest_first_and_filters_by_job() {
        let customer = Uuid::now_v7();
        let other_customer = Uuid::now_v7();
        let job = Uuid::now_v7();
        let mut history = FormulaHistory::new();
        let t0 = Utc::now();
        let t1 = t0 + chrono::Duration::seconds(10);
        let t2 = t0 + chrono::Duration::seconds(20);
        history.record(stored(customer, Some(job), "OC-17", t0));
        history.record(stored(other_customer, None, "OC-65", t1));
        history.record(stored(customer, None, "HC-172", t2));
        history.record(stored(customer, Some(job), "HC-154", t1));

        let all = history.history_for_customer(customer, None);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].color_code, "HC-172"); // t2
        assert_eq!(all[1].color_code, "HC-154"); // t1
        assert_eq!(all[2].color_code, "OC-17"); // t0

        let for_job = history.history_for_customer(customer, Some(job));
        assert_eq!(for_job.len(), 2);
        assert_eq!(for_job[0].color_code, "HC-154");
        assert_eq!(for_job[1].color_code, "OC-17");

        assert!(
            history
                .history_for_customer(Uuid::now_v7(), None)
                .is_empty()
        );
    }
}
