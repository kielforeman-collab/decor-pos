//! Mobile KPIs for the Outside Sales workspace (Phase 5, v0.6;
//! spec §7 Phase 5 — Outside Sales KPIs).
//!
//! All figures are derived from the edge ledger at read time,
//! never stored: today's completed sales, open quotes/orders,
//! the total open AR balance, and deliveries out today. Money is
//! [`Money`] — never floats.

use df_domain::Money;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// One snapshot of the store's day, as the Outside Sales
/// workspace shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MobileKpis {
    /// ISO date the snapshot is for (store-local day).
    pub date: String,
    /// Completed sales today (grand totals summed).
    pub sales_today: Money,
    /// Number of completed sales today.
    pub sales_today_count: i64,
    /// Open quotes waiting on customers.
    pub open_quotes: i64,
    /// Open orders (portal reorders + BOPIS) not yet completed.
    pub open_orders: i64,
    /// Total open AR balance across all charge accounts.
    pub ar_open_total: Money,
    /// Deliveries out for delivery today.
    pub deliveries_out_today: i64,
}

impl MobileKpis {
    pub fn empty(date: String, currency: df_domain::Currency) -> Self {
        Self {
            date,
            sales_today: Money::new(Decimal::ZERO, currency),
            sales_today_count: 0,
            open_quotes: 0,
            open_orders: 0,
            ar_open_total: Money::new(Decimal::ZERO, currency),
            deliveries_out_today: 0,
        }
    }
}
