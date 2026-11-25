//! Low-stock report (Phase 2, v0.3; spec §7).
//!
//! Two halves:
//!
//! * **Products** — on-hand balances derived by summing the
//!   append-only inventory ledger per product (never a stored
//!   mutable balance), compared against caller-supplied minimum
//!   levels. The schema has no min-level table (see the crate docs),
//!   so thresholds arrive as [`ProductThreshold`] values — the edge
//!   daemon holds operator-configured ones; a back-office min/max
//!   table is a Phase 3 item.
//! * **Colorants** — the shortage alerts from
//!   [`crate::shortage::colorant_shortage_alerts`].
//!
//! Products below minimum also feed PO suggestions via
//! [`df_edi::generate_purchase_orders`] (one draft PO per vendor,
//! `max(reorder_qty, shortfall)` per line). The suggestions are
//! drafts only — nothing is ordered, sent, or persisted here.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use df_edi::{LowStockItem, PurchaseOrder, generate_purchase_orders};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ReportError;
use crate::shortage::{ColorantShortageAlert, colorant_shortage_alerts};

/// A caller-supplied minimum level for one product, plus the
/// replenishment facts a PO suggestion needs. Until a min/max table
/// exists (Phase 3), the operator supplies these — e.g. the edge
/// daemon's configured thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductThreshold {
    pub product_id: Uuid,
    pub min_level: Decimal,
    pub reorder_qty: Decimal,
    pub vendor_id: String,
    pub vendor_sku: String,
    pub description: String,
    pub unit_cost: Decimal,
    pub uom: String,
}

/// One product below its minimum level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LowStockProduct {
    pub product_id: Uuid,
    pub description: String,
    pub on_hand: Decimal,
    pub min_level: Decimal,
    pub shortfall: Decimal,
    /// `max(reorder_qty, shortfall)` — the quantity the suggested PO
    /// line orders.
    pub suggested_order_qty: Decimal,
}

/// The low-stock report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LowStockReport {
    pub store_id: Uuid,
    pub date: NaiveDate,
    /// Products below minimum, sorted by product id.
    pub products: Vec<LowStockProduct>,
    /// Colorants at/below threshold (same list as the standalone
    /// shortage alerts).
    pub colorants: Vec<ColorantShortageAlert>,
    /// Draft POs suggested for the low products, one per vendor.
    /// Empty when nothing is low.
    pub suggested_purchase_orders: Vec<PurchaseOrder>,
}

impl LowStockReport {
    /// Plain-text rendering.
    pub fn render_text(&self) -> String {
        let mut out = format!(
            "Low-stock report — store {} — {}\n",
            self.store_id, self.date
        );
        out.push_str("Products below minimum:\n");
        if self.products.is_empty() {
            out.push_str("  (none)\n");
        }
        for product in &self.products {
            out.push_str(&format!(
                "  {} ({}): on hand {}, min {}, short {}, suggest ordering {}\n",
                product.product_id,
                product.description,
                product.on_hand,
                product.min_level,
                product.shortfall,
                product.suggested_order_qty
            ));
        }
        out.push_str(&crate::shortage::render_shortage_alerts_text(
            &self.colorants,
        ));
        out.push_str("Suggested purchase orders (drafts — nothing ordered):\n");
        if self.suggested_purchase_orders.is_empty() {
            out.push_str("  (none)\n");
        }
        for po in &self.suggested_purchase_orders {
            out.push_str(&format!(
                "  {} (vendor {}): {} line(s), total {}\n",
                po.po_number,
                po.vendor_id,
                po.lines.len(),
                po.total()
            ));
        }
        out
    }
}

pub async fn low_stock_report(
    pool: &df_db::AnyPool,
    store_id: Uuid,
    date: NaiveDate,
    thresholds: &[ProductThreshold],
) -> Result<LowStockReport, ReportError> {
    // On-hand per product from the append-only ledger.
    let entries = df_db::list_inventory_entries_for_store(pool, store_id).await?;
    let mut on_hand: BTreeMap<Uuid, Decimal> = BTreeMap::new();
    for entry in entries {
        *on_hand.entry(entry.product_id).or_default() += entry.delta_on_hand;
    }
    let mut products = Vec::new();
    let mut low_items: Vec<LowStockItem> = Vec::new();
    for threshold in thresholds {
        let balance = on_hand
            .get(&threshold.product_id)
            .copied()
            .unwrap_or_default();
        if balance >= threshold.min_level {
            continue;
        }
        let shortfall = threshold.min_level - balance;
        let suggested_order_qty = if threshold.reorder_qty > shortfall {
            threshold.reorder_qty
        } else {
            shortfall
        };
        products.push(LowStockProduct {
            product_id: threshold.product_id,
            description: threshold.description.clone(),
            on_hand: balance,
            min_level: threshold.min_level,
            shortfall,
            suggested_order_qty,
        });
        low_items.push(LowStockItem {
            product_id: threshold.product_id,
            vendor_id: threshold.vendor_id.clone(),
            vendor_sku: threshold.vendor_sku.clone(),
            description: threshold.description.clone(),
            on_hand: balance,
            min_level: threshold.min_level,
            reorder_qty: threshold.reorder_qty,
            unit_cost: threshold.unit_cost,
            uom: threshold.uom.clone(),
        });
    }
    let suggested_purchase_orders = generate_purchase_orders(&low_items, store_id, date);
    let colorants = colorant_shortage_alerts(pool, store_id).await?;
    Ok(LowStockReport {
        store_id,
        date,
        products,
        colorants,
        suggested_purchase_orders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::test_pool;
    use chrono::Utc;
    use df_db::models::{Colorant, ColorantLevelRow, InventoryLedgerEntry, Organization, Store};
    use std::str::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[tokio::test]
    async fn low_stock_uses_ledger_balances_and_suggests_pos() {
        let pool = test_pool().await;
        let org_id = Uuid::from_u128(0x2000);
        let store_id = Uuid::from_u128(0x1000);
        let product_a = Uuid::from_u128(0x3000);
        let product_b = Uuid::from_u128(0x3001);
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
        // Product A: +5 then -8 = -3 on hand (below min 2).
        // Product B: +20 (above min 10). Product C: no ledger rows
        // at all, so on hand 0 (below min 4).
        let product_c = Uuid::from_u128(0x3002);
        for (product_id, delta) in [(product_a, "5"), (product_a, "-8"), (product_b, "20")] {
            df_db::insert_inventory_delta(
                &pool,
                &InventoryLedgerEntry {
                    id: Uuid::now_v7(),
                    org_id,
                    store_id,
                    product_id,
                    lot_number: None,
                    delta_on_hand: dec(delta),
                    delta_committed: Decimal::ZERO,
                    delta_on_order: Decimal::ZERO,
                    delta_in_transit: Decimal::ZERO,
                    unit_cost: dec("50.0000"),
                    reason_type: "sale".to_string(),
                    reference_id: Uuid::now_v7(),
                    created_at: Utc::now(),
                },
            )
            .await
            .unwrap();
        }
        // One low colorant (BK at threshold) so the colorant half is
        // exercised too.
        let bk = Uuid::from_u128(0x5003);
        df_db::insert_colorant(
            &pool,
            &Colorant {
                id: bk,
                org_id,
                system_name: "Gennex".to_string(),
                code: "BK".to_string(),
                name: "Black".to_string(),
                cost_per_fl_oz: dec("0.3800"),
                shots_per_fl_oz: 48,
            },
        )
        .await
        .unwrap();
        df_db::upsert_colorant_level(
            &pool,
            &ColorantLevelRow {
                colorant_id: bk,
                store_id,
                on_hand_fl_oz: dec("1"),
                low_threshold_fl_oz: dec("1"),
                updated_at: Utc::now(),
            },
        )
        .await
        .unwrap();

        let thresholds = vec![
            ProductThreshold {
                product_id: product_a,
                min_level: dec("2"),
                reorder_qty: dec("12"),
                vendor_id: "VENDOR-A".to_string(),
                vendor_sku: "BM-REGAL-1G".to_string(),
                description: "Regal Select Interior Gallon".to_string(),
                unit_cost: dec("50.0000"),
                uom: "EA".to_string(),
            },
            ProductThreshold {
                product_id: product_b,
                min_level: dec("10"),
                reorder_qty: dec("12"),
                vendor_id: "VENDOR-A".to_string(),
                vendor_sku: "BM-OTHER".to_string(),
                description: "Other Paint".to_string(),
                unit_cost: dec("40.0000"),
                uom: "EA".to_string(),
            },
            ProductThreshold {
                product_id: product_c,
                min_level: dec("4"),
                reorder_qty: dec("2"),
                vendor_id: "VENDOR-B".to_string(),
                vendor_sku: "BM-THIRD".to_string(),
                description: "Third Paint".to_string(),
                unit_cost: dec("30.0000"),
                uom: "EA".to_string(),
            },
        ];
        let date = Utc::now().date_naive();
        let report = low_stock_report(&pool, store_id, date, &thresholds)
            .await
            .unwrap();
        assert_eq!(report.products.len(), 2);
        let a = report
            .products
            .iter()
            .find(|p| p.product_id == product_a)
            .unwrap();
        assert_eq!(a.on_hand, dec("-3"));
        assert_eq!(a.shortfall, dec("5"));
        // reorder 12 > shortfall 5 -> order 12.
        assert_eq!(a.suggested_order_qty, dec("12"));
        let c = report
            .products
            .iter()
            .find(|p| p.product_id == product_c)
            .unwrap();
        assert_eq!(c.on_hand, dec("0"));
        assert_eq!(c.shortfall, dec("4"));
        // reorder 2 < shortfall 4 -> order the shortfall.
        assert_eq!(c.suggested_order_qty, dec("4"));
        assert_eq!(report.colorants.len(), 1);
        assert_eq!(report.colorants[0].code, "BK");
        // One draft PO per vendor.
        assert_eq!(report.suggested_purchase_orders.len(), 2);
        assert_eq!(report.suggested_purchase_orders[0].vendor_id, "VENDOR-A");
        assert_eq!(report.suggested_purchase_orders[0].lines.len(), 1);
        assert_eq!(report.suggested_purchase_orders[0].lines[0].qty, dec("12"));
        assert_eq!(report.suggested_purchase_orders[1].vendor_id, "VENDOR-B");
        let text = report.render_text();
        assert!(text.contains("Regal Select Interior Gallon"));
        assert!(text.contains("BK (Black)"));
        assert!(text.contains("Suggested purchase orders"));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["products"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn low_stock_is_empty_when_nothing_is_low() {
        let pool = test_pool().await;
        let store_id = Uuid::from_u128(0x1000);
        let date = Utc::now().date_naive();
        let report = low_stock_report(&pool, store_id, date, &[]).await.unwrap();
        assert!(report.products.is_empty());
        assert!(report.colorants.is_empty());
        assert!(report.suggested_purchase_orders.is_empty());
    }
}
