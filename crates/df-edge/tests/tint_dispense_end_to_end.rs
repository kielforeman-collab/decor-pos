//! Phase 2 exit test: formula lookup by past color → virtual tinter
//! dispense → exact colorant decrement → tint job recorded against
//! the sale → daily usage report shows the consumption → a colorant
//! dropped to its threshold raises a shortage alert (spec §7,
//! Phase 2 exit criterion, edge half: dispense → costed invoice →
//! can label → formula reorder; the can label itself is covered by
//! `df-tinter`'s label tests and the reorder by the formula-history
//! lookup asserted here).
//!
//! Uses the REAL crates end to end — `df-tinter`'s `VirtualTinter`
//! hosted in `df-edge`, `df-db` SQLite, the edge HTTP API — with
//! fixed UUIDs/amounts, no sleeps, no network.
//!
//! Expected math (White Dove OC-17, 1X base: Y3 0 oz + 14 shots,
//! R1 1 oz + 0 shots; 48 shots per fl oz; Y3 $0.42/fl oz, R1
//! $0.50/fl oz):
//! * Y3 dispensed: 14/48 fl oz, cost 0.1225; R1: 1 fl oz, cost
//!   0.5000; total tint cost 0.6225.
//! * Levels before: Y3 10 fl oz (threshold 2), R1 5 fl oz
//!   (threshold 4). After: Y3 10 − 14/48, R1 exactly 4 — at its
//!   threshold, so the shortage alert fires (inclusive).
//! * Sale: 1 gal at $100 list, eco $0.75, tax 7.25% on $100.75 =
//!   $7.3044, grand $108.0544 (cash, no surcharge).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use df_db::models::{
    ColorCatalogEntry, Colorant, ColorantLevelRow, Customer, Organization, Product, Store,
    TintFormula,
};
use df_domain::{Currency, Money};
use df_edge::{Edge, EdgeConfig, MockTerminal};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;
const CUSTOMER_ID: u128 = 0x4000;
const Y3_ID: u128 = 0x5001;
const R1_ID: u128 = 0x5002;
const COLOR_ID: u128 = 0x6000;
const FORMULA_ID: u128 = 0x7000;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn store_id() -> Uuid {
    Uuid::from_u128(STORE_ID)
}

fn org_id() -> Uuid {
    Uuid::from_u128(ORG_ID)
}

fn customer_id() -> Uuid {
    Uuid::from_u128(CUSTOMER_ID)
}

/// Edge seeded with org + store + base product + charge-free retail
/// customer + White Dove color + Y3/R1 colorants and levels + the
/// customer's past White Dove formula.
async fn seeded_edge() -> Edge {
    let config = EdgeConfig {
        store_id: store_id(),
        org_id: org_id(),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    let edge = Edge::open(config).await.unwrap();
    let pool = edge.pool();
    df_db::insert_organization(
        pool,
        &Organization {
            id: org_id(),
            name: "Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({"core_pos": true, "paint_tinting": true}),
            settings_json: serde_json::json!({}),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    df_db::insert_store(
        pool,
        &Store {
            id: store_id(),
            org_id: org_id(),
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
    df_db::insert_product(
        pool,
        &Product {
            id: Uuid::from_u128(PRODUCT_ID),
            org_id: org_id(),
            sku: "BM-REGAL-1G".to_string(),
            manufacturer_code: Some("BM".to_string()),
            name: "Regal Select Interior Gallon".to_string(),
            department_id: Uuid::now_v7(),
            class_id: None,
            product_line_id: None,
            primary_vendor_id: None,
            product_type: "paint_base".to_string(),
            base_uom: "gal".to_string(),
            list_price: Money::from_units(100, Currency::Usd).amount(),
            standard_cost: Money::from_units(50, Currency::Usd).amount(),
            avg_cost: Money::from_units(50, Currency::Usd).amount(),
            min_margin_pct: None,
            sheen: Some("Eggshell".to_string()),
            paint_base_code: Some("1X".to_string()),
            container_volume_ml: Some(3785),
            eco_fee_category: Some("architectural_paint".to_string()),
            tax_class_id: Uuid::now_v7(),
            prompt_template_id: None,
            is_active: true,
        },
    )
    .await
    .unwrap();
    df_db::insert_customer(
        pool,
        &Customer {
            id: customer_id(),
            org_id: org_id(),
            account_number: "C-1001".to_string(),
            customer_type: "retail".to_string(),
            company_name: None,
            first_name: Some("Test".to_string()),
            last_name: Some("Customer".to_string()),
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: false,
            credit_limit: dec("0"),
            payment_terms: "COD".to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: false,
        },
    )
    .await
    .unwrap();
    df_db::insert_color(
        pool,
        &ColorCatalogEntry {
            id: Uuid::from_u128(COLOR_ID),
            org_id: org_id(),
            manufacturer: "Benjamin Moore".to_string(),
            color_code: "OC-17".to_string(),
            color_name: "White Dove".to_string(),
            collection: Some("Off-White".to_string()),
            rgb_hex: Some("#EFEDE6".to_string()),
            recommended_base: Some("1X".to_string()),
        },
    )
    .await
    .unwrap();
    for (id, code, name, cost) in [
        (Uuid::from_u128(Y3_ID), "Y3", "Yellow Oxide", "0.4200"),
        (Uuid::from_u128(R1_ID), "R1", "Red Oxide", "0.5000"),
    ] {
        df_db::insert_colorant(
            pool,
            &Colorant {
                id,
                org_id: org_id(),
                system_name: "Gennex".to_string(),
                code: code.to_string(),
                name: name.to_string(),
                cost_per_fl_oz: dec(cost),
                shots_per_fl_oz: 48,
            },
        )
        .await
        .unwrap();
    }
    // R1 threshold 4: one dispense of 1 fl oz lands exactly on it.
    for (id, on_hand, threshold) in [
        (Uuid::from_u128(Y3_ID), "10", "2"),
        (Uuid::from_u128(R1_ID), "5", "4"),
    ] {
        df_db::upsert_colorant_level(
            pool,
            &ColorantLevelRow {
                colorant_id: id,
                store_id: store_id(),
                on_hand_fl_oz: dec(on_hand),
                low_threshold_fl_oz: dec(threshold),
                updated_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    }
    edge
}

/// Insert the customer's past White Dove formula (Y3 0 oz + 14
/// shots, R1 1 oz + 0 shots). `r1_oz` lets the shortage test reuse
/// this seed with a formula that needs more R1 than is on hand.
async fn seed_formula(edge: &Edge, formula_id: Uuid, r1_oz: &str) {
    df_db::insert_tint_formula(
        edge.pool(),
        &TintFormula {
            id: formula_id,
            org_id: org_id(),
            store_id: store_id(),
            customer_id: Some(customer_id()),
            job_id: None,
            color_id: Some(Uuid::from_u128(COLOR_ID)),
            color_code_snapshot: "OC-17".to_string(),
            color_name_snapshot: "White Dove".to_string(),
            base_product_id: Uuid::from_u128(PRODUCT_ID),
            container_volume_ml: 3785,
            sheen: Some("Eggshell".to_string()),
            room_or_usage: Some("Master Bedroom Trim".to_string()),
            is_custom_match: false,
            colorants_json: serde_json::json!([
                {"code": "Y3", "oz": 0, "shots": 14},
                {"code": "R1", "oz": r1_oz, "shots": 0},
            ]),
            total_tint_cost: dec("0.6225"),
            tint_surcharge: dec("0.0000"),
            dispenser_name: Some("virtual-simulator".to_string()),
            external_formula_ref: None,
            dispensed_at: Utc::now(),
        },
    )
    .await
    .unwrap();
}

async fn post_json(
    edge: &Edge,
    uri: &str,
    body: serde_json::Value,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let response = edge
        .router()
        .oneshot(
            Request::builder()
                .uri(uri)
                .method("POST")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn get_json(edge: &Edge, uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let response = edge
        .router()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn money_json(amount: &str) -> serde_json::Value {
    serde_json::json!({"amount": amount, "currency": "Usd"})
}

#[tokio::test]
async fn tint_dispense_end_to_end() {
    let edge = seeded_edge().await;
    let users = df_auth_http::test_support::seed_test_users(edge.pool(), org_id()).await;
    let cashier = users.cashier.token.as_str();
    let manager = users.manager.token.as_str();
    edge.set_payment_provider(Arc::new(MockTerminal::new()));
    seed_formula(&edge, Uuid::from_u128(FORMULA_ID), "1").await;
    let today = Utc::now().date_naive();

    // 1. The sale: 1 gal, cash.
    let (status, sale) = post_json(
        &edge,
        "/pos/transactions",
        serde_json::json!({
            "customer_id": customer_id().to_string(),
            "job_id": null,
            "lines": [{
                "product_id": Uuid::from_u128(PRODUCT_ID).to_string(),
                "sku": "BM-REGAL-1G",
                "description": "Regal Select Interior Gallon",
                "qty": "1",
                "uom": "gal",
                "unit_price": money_json("100.0000"),
                "unit_cost": money_json("50.0000"),
            }],
            "tenders": [{"kind": "Cash", "amount": null}],
            "po_number": null,
        }),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sale}");
    assert_eq!(sale["grand_total"]["amount"], "108.0544");
    let tx_id = sale["transaction_id"].as_str().unwrap().to_string();

    // 2. Formula lookup by past color finds the stored formula
    // (the repeat-tint / reorder path).
    let (status, formulas) = get_json(
        &edge,
        &format!("/pos/tinting/formulas?customer_id={}", customer_id()),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{formulas}");
    assert_eq!(formulas.as_array().unwrap().len(), 1);
    assert_eq!(formulas[0]["color_code_snapshot"], "OC-17");
    assert_eq!(formulas[0]["id"], Uuid::from_u128(FORMULA_ID).to_string());

    // 3. Dispense by past color (no formula id given) against the
    // sale line.
    let (status, dispensed) = post_json(
        &edge,
        "/pos/tinting/dispense",
        serde_json::json!({
            "formula_id": null,
            "customer_id": customer_id().to_string(),
            "job_id": null,
            "color_query": "White Dove",
            "transaction_id": tx_id,
            "line_no": 1,
            "dispenser": null,
        }),
        cashier,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dispensed}");
    assert_eq!(dispensed["status"], "done");
    assert_eq!(
        dispensed["formula_id"],
        Uuid::from_u128(FORMULA_ID).to_string()
    );
    assert_eq!(dispensed["tint_cost"]["amount"], "0.6225");
    let shots = dispensed["dispensed"].as_array().unwrap();
    assert_eq!(shots.len(), 2);
    let y3_shot = shots.iter().find(|s| s["code"] == "Y3").unwrap();
    assert_eq!(y3_shot["fl_oz"], (dec("14") / dec("48")).to_string());
    let r1_shot = shots.iter().find(|s| s["code"] == "R1").unwrap();
    assert_eq!(r1_shot["fl_oz"], "1");
    let job_id = dispensed["job_id"].as_str().unwrap().to_string();

    // 4. Levels decremented by exactly the formula amounts.
    let y3_level = df_db::get_colorant_level(edge.pool(), Uuid::from_u128(Y3_ID), store_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(y3_level.on_hand_fl_oz, dec("10") - dec("14") / dec("48"));
    let r1_level = df_db::get_colorant_level(edge.pool(), Uuid::from_u128(R1_ID), store_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r1_level.on_hand_fl_oz, dec("4"));

    // 5. The tint job is recorded against the sale line.
    let jobs = df_db::list_tint_jobs_for_transaction(edge.pool(), tx_id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id.to_string(), job_id);
    assert_eq!(jobs[0].status, "done");
    assert_eq!(jobs[0].line_no, Some(1));

    // 6. The daily usage report shows the consumption + the sale.
    let (status, report) = get_json(
        &edge,
        &format!("/pos/reports/daily-usage?date={today}"),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["tints_dispensed"], 1);
    assert_eq!(report["tints_failed"], 0);
    assert_eq!(report["per_color"][0]["color_code"], "OC-17");
    assert_eq!(report["per_color"][0]["tints"], 1);
    assert_eq!(report["total_tint_cost"]["amount"], "0.6225");
    let consumed = report["colorant_consumed"].as_array().unwrap();
    let y3_used = consumed.iter().find(|u| u["code"] == "Y3").unwrap();
    assert_eq!(y3_used["fl_oz"], (dec("14") / dec("48")).to_string());
    assert_eq!(y3_used["cost"]["amount"], "0.1225");
    assert_eq!(report["sales"]["sale_count"], 1);
    assert_eq!(report["sales"]["grand_total"]["amount"], "108.0544");

    // 7. R1 dropped to exactly its threshold: shortage alert fires.
    let (status, colorants) = get_json(&edge, "/pos/tinting/colorants", cashier).await;
    assert_eq!(status, StatusCode::OK, "{colorants}");
    let alerts = colorants["alerts"].as_array().unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["code"], "R1");
    assert_eq!(alerts[0]["on_hand_fl_oz"], "4");
    assert_eq!(alerts[0]["low_threshold_fl_oz"], "4");
    assert_eq!(alerts[0]["shortfall_fl_oz"], "0");
    let levels = colorants["levels"].as_array().unwrap();
    assert_eq!(levels.len(), 2);
    assert!(
        !levels.iter().find(|l| l["code"] == "Y3").unwrap()["is_low"]
            .as_bool()
            .unwrap()
    );

    // 8. The low-stock report carries the same alert.
    let (status, low_stock) = get_json(
        &edge,
        &format!("/pos/reports/low-stock?date={today}"),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{low_stock}");
    assert_eq!(low_stock["colorants"].as_array().unwrap().len(), 1);
    assert_eq!(low_stock["colorants"][0]["code"], "R1");
}

#[tokio::test]
async fn failed_dispense_decrements_nothing() {
    let edge = seeded_edge().await;
    let users = df_auth_http::test_support::seed_test_users(edge.pool(), org_id()).await;
    let cashier = users.cashier.token.as_str();
    let manager = users.manager.token.as_str();
    edge.set_payment_provider(Arc::new(MockTerminal::new()));
    // This formula needs 6 fl oz of R1; only 5 are on hand.
    let formula_id = Uuid::from_u128(0x7001);
    seed_formula(&edge, formula_id, "6").await;

    let (status, body) = post_json(
        &edge,
        "/pos/tinting/dispense",
        serde_json::json!({
            "formula_id": formula_id.to_string(),
            "customer_id": null,
            "job_id": null,
            "color_query": null,
            "transaction_id": null,
            "line_no": null,
            "dispenser": null,
        }),
        cashier,
    )
    .await;
    // Shortage: 409, and the error names the short colorant.
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"].as_str().unwrap().contains("R1"));

    // Atomic: Y3 untouched too, even though it had enough.
    let y3_level = df_db::get_colorant_level(edge.pool(), Uuid::from_u128(Y3_ID), store_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(y3_level.on_hand_fl_oz, dec("10"));
    let r1_level = df_db::get_colorant_level(edge.pool(), Uuid::from_u128(R1_ID), store_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r1_level.on_hand_fl_oz, dec("5"));

    // The failure is still recorded as a failed tint job.
    let jobs = df_db::list_tint_jobs_for_formula(edge.pool(), formula_id)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, "failed");
    assert!(jobs[0].error.as_ref().unwrap().contains("R1"));

    // ...and the daily usage report counts the failure, not a tint.
    let today = Utc::now().date_naive();
    let (status, report) = get_json(
        &edge,
        &format!("/pos/reports/daily-usage?date={today}"),
        manager,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["tints_dispensed"], 0);
    assert_eq!(report["tints_failed"], 1);
    assert!(report["colorant_consumed"].as_array().unwrap().is_empty());
}
