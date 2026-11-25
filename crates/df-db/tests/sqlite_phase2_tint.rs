//! SQLite round-trip tests for the Phase 2 tint tables (spec §5.3/§7).
//!
//! Covers: color search (case-insensitive, manufacturer-scoped),
//! colorant CRUD, colorant levels (upsert/get/low filter), the atomic
//! dispense decrement (exact math, shortage leaves the level
//! untouched), formula lookup by id / color / customer history with
//! job filter, and tint-job persistence across its lifecycle.

use chrono::Utc;
use df_db::models::{
    ColorCatalogEntry, Colorant, ColorantLevelRow, Organization, Product, TintFormula, TintJobRow,
};
use df_db::{DbError, Dialect, connect, migrate};
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

async fn test_pool() -> df_db::AnyPool {
    let pool = connect("sqlite::memory:").await.unwrap();
    migrate(&pool, Dialect::Sqlite).await.unwrap();
    pool
}

fn org() -> Organization {
    Organization {
        id: Uuid::now_v7(),
        name: "Test Paint Co".to_string(),
        default_currency: "USD".to_string(),
        enabled_modules: serde_json::json!({"core_pos": true}),
        settings_json: serde_json::json!({}),
        created_at: Utc::now(),
    }
}

fn product(org_id: Uuid) -> Product {
    Product {
        id: Uuid::now_v7(),
        org_id,
        sku: "BASE-1G".to_string(),
        manufacturer_code: None,
        name: "Base Gallon".to_string(),
        department_id: Uuid::now_v7(),
        class_id: None,
        product_line_id: None,
        primary_vendor_id: None,
        product_type: "paint_base".to_string(),
        base_uom: "gal".to_string(),
        list_price: dec("50.0000"),
        standard_cost: dec("30.0000"),
        avg_cost: dec("30.0000"),
        min_margin_pct: None,
        sheen: None,
        paint_base_code: Some("1X".to_string()),
        container_volume_ml: Some(3785),
        eco_fee_category: Some("architectural_paint".to_string()),
        tax_class_id: Uuid::now_v7(),
        prompt_template_id: None,
        is_active: true,
    }
}

fn color(org_id: Uuid, manufacturer: &str, code: &str, name: &str) -> ColorCatalogEntry {
    ColorCatalogEntry {
        id: Uuid::now_v7(),
        org_id,
        manufacturer: manufacturer.to_string(),
        color_code: code.to_string(),
        color_name: name.to_string(),
        collection: Some("Off-White".to_string()),
        rgb_hex: Some("#EFEDE6".to_string()),
        recommended_base: Some("1X".to_string()),
    }
}

fn colorant(org_id: Uuid, system: &str, code: &str, cost: &str) -> Colorant {
    Colorant {
        id: Uuid::now_v7(),
        org_id,
        system_name: system.to_string(),
        code: code.to_string(),
        name: format!("{code} colorant"),
        cost_per_fl_oz: dec(cost),
        shots_per_fl_oz: 48,
    }
}

#[allow(clippy::too_many_arguments)]
fn formula(
    org_id: Uuid,
    store_id: Uuid,
    product_id: Uuid,
    customer_id: Option<Uuid>,
    job_id: Option<Uuid>,
    code: &str,
    name: &str,
    dispensed_at: chrono::DateTime<Utc>,
) -> TintFormula {
    TintFormula {
        id: Uuid::now_v7(),
        org_id,
        store_id,
        customer_id,
        job_id,
        color_id: None,
        color_code_snapshot: code.to_string(),
        color_name_snapshot: name.to_string(),
        base_product_id: product_id,
        container_volume_ml: 3785,
        sheen: Some("Eggshell".to_string()),
        room_or_usage: Some("Master Bedroom Trim".to_string()),
        is_custom_match: false,
        colorants_json: serde_json::json!([
            {"code": "Y3", "oz": 0, "shots": 14, "sub_shots": 2, "unit_cost": "0.42"}
        ]),
        total_tint_cost: dec("0.1225"),
        tint_surcharge: dec("0.0000"),
        dispenser_name: Some("virtual-simulator".to_string()),
        external_formula_ref: None,
        dispensed_at,
    }
}

#[tokio::test]
async fn color_search_is_case_insensitive_and_manufacturer_scoped() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let bm_dove = color(org.id, "Benjamin Moore", "OC-17", "White Dove");
    df_db::insert_color(&pool, &bm_dove).await.unwrap();
    df_db::insert_color(
        &pool,
        &color(org.id, "Benjamin Moore", "HC-172", "Revere Pewter"),
    )
    .await
    .unwrap();
    // Same color name, different manufacturer — must not leak across.
    df_db::insert_color(&pool, &color(org.id, "PPG", "PPG1012-1", "White Dove"))
        .await
        .unwrap();

    let found = df_db::search_colors(&pool, org.id, "Benjamin Moore", "white dove")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].color_code, "OC-17");

    let found = df_db::search_colors(&pool, org.id, "benjamin moore", "OC-")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);

    let found = df_db::search_colors(&pool, org.id, "Benjamin Moore", "ew")
        .await
        .unwrap();
    assert_eq!(found.len(), 1); // Revere Pewter
    assert_eq!(found[0].color_code, "HC-172");

    let found = df_db::search_colors(&pool, org.id, "PPG", "White Dove")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].color_code, "PPG1012-1");

    // LIKE metacharacters in the query are literal, not wildcards.
    let found = df_db::search_colors(&pool, org.id, "Benjamin Moore", "%")
        .await
        .unwrap();
    assert!(found.is_empty());

    let back = df_db::get_color(&pool, bm_dove.id).await.unwrap();
    assert_eq!(back.color_name, "White Dove");
}

#[tokio::test]
async fn colorant_crud() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let mut y3 = colorant(org.id, "Gennex", "Y3", "0.420000");
    df_db::insert_colorant(&pool, &y3).await.unwrap();

    let back = df_db::get_colorant(&pool, y3.id).await.unwrap();
    assert_eq!(back.code, "Y3");
    assert_eq!(back.cost_per_fl_oz, dec("0.420000"));
    let by_code = df_db::get_colorant_by_code(&pool, org.id, "Gennex", "Y3")
        .await
        .unwrap();
    assert_eq!(by_code.id, y3.id);

    y3.cost_per_fl_oz = dec("0.450000");
    y3.name = "Yellow Oxide".to_string();
    df_db::update_colorant(&pool, &y3).await.unwrap();
    assert_eq!(
        df_db::get_colorant(&pool, y3.id)
            .await
            .unwrap()
            .cost_per_fl_oz,
        dec("0.450000")
    );

    let system = df_db::list_colorants_for_system(&pool, org.id, "Gennex")
        .await
        .unwrap();
    assert_eq!(system.len(), 1);

    df_db::delete_colorant(&pool, y3.id).await.unwrap();
    assert!(matches!(
        df_db::get_colorant(&pool, y3.id).await,
        Err(DbError::NotFound(_))
    ));
}

#[tokio::test]
async fn colorant_levels_upsert_get_and_low_filter() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_id = Uuid::now_v7();
    let y3 = colorant(org.id, "Gennex", "Y3", "0.420000");
    let r1 = colorant(org.id, "Gennex", "R1", "0.500000");
    df_db::insert_colorant(&pool, &y3).await.unwrap();
    df_db::insert_colorant(&pool, &r1).await.unwrap();

    assert!(
        df_db::get_colorant_level(&pool, y3.id, store_id)
            .await
            .unwrap()
            .is_none()
    );
    let level = ColorantLevelRow {
        colorant_id: y3.id,
        store_id,
        on_hand_fl_oz: dec("10"),
        low_threshold_fl_oz: dec("2"),
        updated_at: Utc::now(),
    };
    df_db::upsert_colorant_level(&pool, &level).await.unwrap();
    df_db::upsert_colorant_level(
        &pool,
        &ColorantLevelRow {
            colorant_id: r1.id,
            store_id,
            on_hand_fl_oz: dec("1"),
            low_threshold_fl_oz: dec("2"),
            updated_at: Utc::now(),
        },
    )
    .await
    .unwrap();

    let back = df_db::get_colorant_level(&pool, y3.id, store_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.on_hand_fl_oz, dec("10"));

    // Upsert replaces.
    df_db::upsert_colorant_level(
        &pool,
        &ColorantLevelRow {
            colorant_id: y3.id,
            store_id,
            on_hand_fl_oz: dec("9"),
            low_threshold_fl_oz: dec("2"),
            updated_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        df_db::get_colorant_level(&pool, y3.id, store_id)
            .await
            .unwrap()
            .unwrap()
            .on_hand_fl_oz,
        dec("9")
    );

    // Only R1 (1 <= 2) is low; Y3 (9 > 2) is not. The comparison is
    // exact-decimal, not lexicographic ("10" < "2" as text).
    let low = df_db::list_low_colorant_levels(&pool, store_id)
        .await
        .unwrap();
    assert_eq!(low.len(), 1);
    assert_eq!(low[0].colorant_id, r1.id);
}

#[tokio::test]
async fn decrement_is_exact_and_atomic_on_shortage() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_id = Uuid::now_v7();
    let y3 = colorant(org.id, "Gennex", "Y3", "0.420000");
    df_db::insert_colorant(&pool, &y3).await.unwrap();
    df_db::upsert_colorant_level(
        &pool,
        &ColorantLevelRow {
            colorant_id: y3.id,
            store_id,
            on_hand_fl_oz: dec("10"),
            low_threshold_fl_oz: dec("2"),
            updated_at: Utc::now(),
        },
    )
    .await
    .unwrap();

    // 14 shots @ 48/fl oz = 14/48 fl oz exactly.
    let after = df_db::decrement_colorant_level(&pool, y3.id, store_id, dec("14") / dec("48"))
        .await
        .unwrap();
    assert_eq!(after.on_hand_fl_oz, dec("10") - dec("14") / dec("48"));

    // Shortage: error, level untouched.
    let err = df_db::decrement_colorant_level(&pool, y3.id, store_id, dec("100"))
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::InsufficientColorant { .. }));
    assert_eq!(
        df_db::get_colorant_level(&pool, y3.id, store_id)
            .await
            .unwrap()
            .unwrap()
            .on_hand_fl_oz,
        dec("10") - dec("14") / dec("48")
    );

    // No level row at all: NotFound, not a panic.
    assert!(matches!(
        df_db::decrement_colorant_level(&pool, Uuid::now_v7(), store_id, dec("1"))
            .await
            .unwrap_err(),
        DbError::NotFound(_)
    ));

    // Non-positive amount is rejected before touching the DB.
    assert!(matches!(
        df_db::decrement_colorant_level(&pool, y3.id, store_id, dec("0"))
            .await
            .unwrap_err(),
        DbError::InvalidData(_)
    ));
}

#[tokio::test]
async fn formula_lookup_by_id_color_and_customer_history() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_id = Uuid::now_v7();
    let product = product(org.id);
    df_db::insert_product(&pool, &product).await.unwrap();
    let customer_id = Uuid::now_v7();
    let other_customer = Uuid::now_v7();
    let job_id = Uuid::now_v7();
    let t0 = Utc::now();
    let t1 = t0 + chrono::Duration::seconds(10);
    let t2 = t0 + chrono::Duration::seconds(20);

    let f1 = formula(
        org.id,
        store_id,
        product.id,
        Some(customer_id),
        Some(job_id),
        "OC-17",
        "White Dove",
        t0,
    );
    let f2 = formula(
        org.id,
        store_id,
        product.id,
        Some(customer_id),
        None,
        "HC-172",
        "Revere Pewter",
        t2,
    );
    let f3 = formula(
        org.id,
        store_id,
        product.id,
        Some(other_customer),
        None,
        "OC-65",
        "Chantilly Lace",
        t1,
    );
    for f in [&f1, &f2, &f3] {
        df_db::insert_tint_formula(&pool, f).await.unwrap();
    }

    let back = df_db::get_tint_formula(&pool, f1.id).await.unwrap();
    assert_eq!(back.color_code_snapshot, "OC-17");
    assert_eq!(back.total_tint_cost, dec("0.1225"));

    let found = df_db::search_tint_formulas_by_color(&pool, org.id, "white dove")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, f1.id);

    // Customer history, newest first.
    let history = df_db::list_tint_formulas_for_customer_job(&pool, customer_id, None)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].id, f2.id); // t2
    assert_eq!(history[1].id, f1.id); // t0

    // Scoped to one job: only f1.
    let for_job = df_db::list_tint_formulas_for_customer_job(&pool, customer_id, Some(job_id))
        .await
        .unwrap();
    assert_eq!(for_job.len(), 1);
    assert_eq!(for_job[0].id, f1.id);

    // The pre-existing customer-only listing still works.
    let legacy = df_db::list_tint_formulas_for_customer(&pool, customer_id)
        .await
        .unwrap();
    assert_eq!(legacy.len(), 2);
}

#[tokio::test]
async fn tint_job_lifecycle_round_trip() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_id = Uuid::now_v7();
    let product = product(org.id);
    df_db::insert_product(&pool, &product).await.unwrap();
    // tint_jobs.transaction_id references transactions(id) on the
    // edge, so the sale row must exist first.
    let transaction_id = Uuid::now_v7();
    df_db::insert_transaction(
        &pool,
        &df_db::models::Transaction {
            id: transaction_id,
            org_id: org.id,
            store_id,
            till_session_id: None,
            doc_number: "TX-000001".to_string(),
            doc_type: "sale".to_string(),
            status: "completed".to_string(),
            customer_id: None,
            job_id: None,
            po_number: None,
            salesperson_id: None,
            cashier_id: None,
            currency: "USD".to_string(),
            subtotal: dec("50.0000"),
            tax_total: dec("0.0000"),
            eco_fee_total: dec("0.0000"),
            pos_fee_total: dec("0.0000"),
            grand_total: dec("50.0000"),
            deposit_paid: dec("0.0000"),
            fulfillment_mode: "counter".to_string(),
            scheduled_at: None,
            completed_at: Some(Utc::now()),
            created_at: Utc::now(),
        },
    )
    .await
    .unwrap();
    let f = formula(
        org.id,
        store_id,
        product.id,
        None,
        None,
        "OC-17",
        "White Dove",
        Utc::now(),
    );
    df_db::insert_tint_formula(&pool, &f).await.unwrap();

    let mut job = TintJobRow {
        id: Uuid::now_v7(),
        formula_id: f.id,
        transaction_id: Some(transaction_id),
        line_no: Some(1),
        status: "queued".to_string(),
        dispensed_json: serde_json::json!([]),
        error: None,
        created_at: Utc::now(),
        completed_at: None,
    };
    df_db::insert_tint_job(&pool, &job).await.unwrap();
    let back = df_db::get_tint_job(&pool, job.id).await.unwrap();
    assert_eq!(back.status, "queued");
    assert_eq!(back.line_no, Some(1));

    // Queued -> done, with the exact dispensed amounts.
    job.status = "done".to_string();
    job.dispensed_json = serde_json::json!([
        {"code": "Y3", "fl_oz": "0.2916666666666666666666666667"}
    ]);
    job.completed_at = Some(Utc::now());
    df_db::update_tint_job(&pool, &job).await.unwrap();
    let back = df_db::get_tint_job(&pool, job.id).await.unwrap();
    assert_eq!(back.status, "done");
    assert_eq!(back.dispensed_json[0]["code"], serde_json::json!("Y3"));
    assert!(back.completed_at.is_some());

    // Traceable from the formula and from the sale.
    assert_eq!(
        df_db::list_tint_jobs_for_formula(&pool, f.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        df_db::list_tint_jobs_for_transaction(&pool, transaction_id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        df_db::list_tint_jobs_for_transaction(&pool, Uuid::now_v7())
            .await
            .unwrap()
            .is_empty()
    );

    // A failed job keeps its error.
    let failed = TintJobRow {
        id: Uuid::now_v7(),
        formula_id: f.id,
        transaction_id: Some(transaction_id),
        line_no: Some(2),
        status: "failed".to_string(),
        dispensed_json: serde_json::json!([]),
        error: Some("insufficient colorant R1: need 6 fl oz, have 5 fl oz".to_string()),
        created_at: Utc::now(),
        completed_at: Some(Utc::now()),
    };
    df_db::insert_tint_job(&pool, &failed).await.unwrap();
    let back = df_db::get_tint_job(&pool, failed.id).await.unwrap();
    assert_eq!(back.status, "failed");
    assert!(back.error.unwrap().contains("R1"));
    assert_eq!(
        df_db::list_tint_jobs_for_formula(&pool, f.id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn list_tint_jobs_for_store_joins_through_formula() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_a = Uuid::now_v7();
    let store_b = Uuid::now_v7();
    let product = product(org.id);
    df_db::insert_product(&pool, &product).await.unwrap();
    let fa = formula(
        org.id,
        store_a,
        product.id,
        None,
        None,
        "OC-17",
        "White Dove",
        Utc::now(),
    );
    let fb = formula(
        org.id,
        store_b,
        product.id,
        None,
        None,
        "HC-172",
        "Revere Pewter",
        Utc::now(),
    );
    df_db::insert_tint_formula(&pool, &fa).await.unwrap();
    df_db::insert_tint_formula(&pool, &fb).await.unwrap();
    for f in [&fa, &fb] {
        df_db::insert_tint_job(
            &pool,
            &TintJobRow {
                id: Uuid::now_v7(),
                formula_id: f.id,
                transaction_id: None,
                line_no: None,
                status: "done".to_string(),
                dispensed_json: serde_json::json!([]),
                error: None,
                created_at: Utc::now(),
                completed_at: Some(Utc::now()),
            },
        )
        .await
        .unwrap();
    }
    let jobs = df_db::list_tint_jobs_for_store(&pool, store_a)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].formula_id, fa.id);
    assert!(
        df_db::list_tint_jobs_for_store(&pool, Uuid::now_v7())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn multi_decrement_is_atomic_within_a_transaction() {
    let pool = test_pool().await;
    let org = org();
    df_db::insert_organization(&pool, &org).await.unwrap();
    let store_id = Uuid::now_v7();
    let y3 = colorant(org.id, "Gennex", "Y3", "0.4200");
    let r1 = colorant(org.id, "Gennex", "R1", "0.5000");
    df_db::insert_colorant(&pool, &y3).await.unwrap();
    df_db::insert_colorant(&pool, &r1).await.unwrap();
    for (id, on_hand) in [(y3.id, "10"), (r1.id, "5")] {
        df_db::upsert_colorant_level(
            &pool,
            &ColorantLevelRow {
                colorant_id: id,
                store_id,
                on_hand_fl_oz: dec(on_hand),
                low_threshold_fl_oz: dec("1"),
                updated_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    }
    // Happy path: both decrement in one transaction.
    let mut tx = pool.begin().await.unwrap();
    let updated = df_db::decrement_colorant_levels_in_tx(
        &mut tx,
        store_id,
        &[(y3.id, dec("14") / dec("48")), (r1.id, dec("1"))],
    )
    .await
    .unwrap();
    assert_eq!(updated.len(), 2);
    tx.commit().await.unwrap();
    assert_eq!(
        df_db::get_colorant_level(&pool, y3.id, store_id)
            .await
            .unwrap()
            .unwrap()
            .on_hand_fl_oz,
        dec("10") - dec("14") / dec("48")
    );
    assert_eq!(
        df_db::get_colorant_level(&pool, r1.id, store_id)
            .await
            .unwrap()
            .unwrap()
            .on_hand_fl_oz,
        dec("4")
    );
    // Shortage on the second colorant: nothing is written, even
    // though the first colorant had enough.
    let mut tx = pool.begin().await.unwrap();
    let err = df_db::decrement_colorant_levels_in_tx(
        &mut tx,
        store_id,
        &[(y3.id, dec("1")), (r1.id, dec("6"))],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DbError::InsufficientColorant { .. }));
    tx.rollback().await.unwrap();
    assert_eq!(
        df_db::get_colorant_level(&pool, y3.id, store_id)
            .await
            .unwrap()
            .unwrap()
            .on_hand_fl_oz,
        dec("10") - dec("14") / dec("48")
    );
    assert_eq!(
        df_db::get_colorant_level(&pool, r1.id, store_id)
            .await
            .unwrap()
            .unwrap()
            .on_hand_fl_oz,
        dec("4")
    );
    // Unknown colorant row is NotFound.
    let mut tx = pool.begin().await.unwrap();
    let err =
        df_db::decrement_colorant_levels_in_tx(&mut tx, store_id, &[(Uuid::now_v7(), dec("1"))])
            .await
            .unwrap_err();
    assert!(matches!(err, DbError::NotFound(_)));
    tx.rollback().await.unwrap();
}
