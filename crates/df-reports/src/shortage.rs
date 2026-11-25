//! Colorant shortage alerts (Phase 2, v0.3; spec §7).
//!
//! A shortage alert is one colorant at or below its low-level
//! threshold at a store, with the on-hand amount, the threshold and
//! the shortfall (`threshold - on_hand`, `>= 0`). The same list is
//! used both standalone (the edge `GET /pos/tinting/colorants`
//! endpoint and the HQ shortage-alerts endpoint) and as a section of
//! the low-stock report.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ReportError;

/// One colorant at or below its low threshold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorantShortageAlert {
    pub colorant_id: Uuid,
    pub code: String,
    pub name: String,
    pub on_hand_fl_oz: Decimal,
    pub low_threshold_fl_oz: Decimal,
    /// `low_threshold_fl_oz - on_hand_fl_oz` (`>= 0` — the alert only
    /// fires at or below threshold).
    pub shortfall_fl_oz: Decimal,
}

/// Colorants at or below their low-level threshold at a store,
/// sorted by colorant code. Uses
/// [`df_db::list_low_colorant_levels`], which filters in Rust
/// (volumes are TEXT in SQLite, so a SQL comparison would be
/// lexicographic and wrong).
pub async fn colorant_shortage_alerts(
    pool: &df_db::AnyPool,
    store_id: Uuid,
) -> Result<Vec<ColorantShortageAlert>, ReportError> {
    let levels = df_db::list_low_colorant_levels(pool, store_id).await?;
    let mut alerts = Vec::with_capacity(levels.len());
    for level in levels {
        let colorant = df_db::get_colorant(pool, level.colorant_id).await?;
        alerts.push(ColorantShortageAlert {
            colorant_id: level.colorant_id,
            code: colorant.code,
            name: colorant.name,
            on_hand_fl_oz: level.on_hand_fl_oz,
            low_threshold_fl_oz: level.low_threshold_fl_oz,
            shortfall_fl_oz: level.low_threshold_fl_oz - level.on_hand_fl_oz,
        });
    }
    alerts.sort_by(|a, b| a.code.cmp(&b.code));
    Ok(alerts)
}

/// Plain-text rendering of a shortage-alert list (one line per
/// alert, plus a header). Empty list renders as an explicit
/// all-clear line — an empty report is not an error.
pub fn render_shortage_alerts_text(alerts: &[ColorantShortageAlert]) -> String {
    let mut out = String::from("Colorant shortage alerts\n");
    if alerts.is_empty() {
        out.push_str("  (none — all colorants above their low thresholds)\n");
        return out;
    }
    for alert in alerts {
        out.push_str(&format!(
            "  {} ({}): on hand {} fl oz, threshold {} fl oz, short {} fl oz\n",
            alert.code,
            alert.name,
            alert.on_hand_fl_oz,
            alert.low_threshold_fl_oz,
            alert.shortfall_fl_oz
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use chrono::Utc;
    use df_db::models::{Colorant, ColorantLevelRow, Organization, Store};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[tokio::test]
    async fn alerts_list_low_colorants_with_shortfall() {
        let pool = test_pool().await;
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
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
        let y3 = Uuid::from_u128(0x5001);
        let r1 = Uuid::from_u128(0x5002);
        for (id, code, name) in [(y3, "Y3", "Yellow Oxide"), (r1, "R1", "Red Oxide")] {
            df_db::insert_colorant(
                &pool,
                &Colorant {
                    id,
                    org_id,
                    system_name: "Gennex".to_string(),
                    code: code.to_string(),
                    name: name.to_string(),
                    cost_per_fl_oz: dec("0.4200"),
                    shots_per_fl_oz: 48,
                },
            )
            .await
            .unwrap();
        }
        // Y3 at exactly its threshold trips the alert; R1 above it
        // does not.
        for (id, on_hand, threshold) in [(y3, "2", "2"), (r1, "5", "1")] {
            df_db::upsert_colorant_level(
                &pool,
                &ColorantLevelRow {
                    colorant_id: id,
                    store_id,
                    on_hand_fl_oz: dec(on_hand),
                    low_threshold_fl_oz: dec(threshold),
                    updated_at: Utc::now(),
                },
            )
            .await
            .unwrap();
        }
        let alerts = colorant_shortage_alerts(&pool, store_id).await.unwrap();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].code, "Y3");
        assert_eq!(alerts[0].shortfall_fl_oz, dec("0"));
        let text = render_shortage_alerts_text(&alerts);
        assert!(text.contains("Y3 (Yellow Oxide)"));
        assert!(text.contains("short 0 fl oz"));
        assert!(
            render_shortage_alerts_text(&[]).contains("all colorants above their low thresholds")
        );
    }
}
