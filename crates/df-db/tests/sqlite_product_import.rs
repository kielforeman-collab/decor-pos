//! SQLite tests for the GUI product import (`df_db::product_import`).
//!
//! Covers: create-then-update on re-import (no duplicates), a
//! duplicate SKU inside one file, bad rows failing without failing
//! the batch, and the malformed shifted row from the real Benjamin
//! Moore price list (sheet row 1012: every cell holds `K7931X-001`).

use chrono::Utc;
use df_db::models::{Organization, Store};
use df_db::{
    Dialect, ImportProductRow, connect, container_fl_oz_for_size, container_volume_ml_for_size,
    get_product_by_sku, import_products, list_colorants_for_org, list_products_for_org, migrate,
};
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

async fn test_org(pool: &df_db::AnyPool) -> Uuid {
    let org = Organization {
        id: Uuid::now_v7(),
        name: "Test Paint Co".to_string(),
        default_currency: "USD".to_string(),
        enabled_modules: serde_json::json!({"core_pos": true}),
        settings_json: serde_json::json!({}),
        created_at: Utc::now(),
    };
    let id = org.id;
    df_db::insert_organization(pool, &org).await.unwrap();
    id
}

fn row(sheet_row: usize, sku: &str, name: &str, net: &str, list: &str) -> ImportProductRow {
    ImportProductRow {
        row: sheet_row,
        sku: sku.to_string(),
        name: name.to_string(),
        category: "REGAL SELECT INTERIOR".to_string(),
        line: "0F403".to_string(),
        color: "01".to_string(),
        size: "001".to_string(),
        upc: "023906119027".to_string(),
        msrp: "$108.99".to_string(),
        map: "$96.99".to_string(),
        net_price: net.to_string(),
        list_price: list.to_string(),
        brand_code: "BMC".to_string(),
        source_metadata: None,
    }
}

#[tokio::test]
async fn import_creates_then_updates_on_reimport() {
    let pool = test_pool().await;
    let org_id = test_org(&pool).await;
    let rows = vec![
        row(
            2,
            "F40301-001",
            "REGAL SELECT HB SG-WHITE",
            "$57.74",
            "$58.18",
        ),
        row(
            3,
            "0229B1-004",
            "GENNEX WB CLRNT -THALO BLUE",
            "$39.34",
            "$39.64",
        ),
    ];
    let first = import_products(&pool, org_id, None, &rows).await.unwrap();
    assert_eq!(first.created, 2);
    assert_eq!(first.updated, 0);
    assert_eq!(first.failed, 0);

    // Re-import with new prices: updates, never duplicates.
    let repriced = vec![
        row(
            2,
            "F40301-001",
            "REGAL SELECT HB SG-WHITE",
            "$59.00",
            "$60.00",
        ),
        row(
            3,
            "0229B1-004",
            "GENNEX WB CLRNT -THALO BLUE",
            "$39.34",
            "$39.64",
        ),
    ];
    let second = import_products(&pool, org_id, None, &repriced)
        .await
        .unwrap();
    assert_eq!(second.created, 0);
    assert_eq!(second.updated, 2);
    assert_eq!(second.failed, 0);

    let products = list_products_for_org(&pool, org_id).await.unwrap();
    assert_eq!(products.len(), 2);
    let regal = products.iter().find(|p| p.sku == "F40301-001").unwrap();
    assert_eq!(regal.list_price, dec("60.00"));
    assert_eq!(regal.standard_cost, dec("59.00"));
    assert_eq!(regal.avg_cost, dec("59.00"));
    assert_eq!(regal.manufacturer_code.as_deref(), Some("BMC"));
    assert_eq!(regal.name, "REGAL SELECT HB SG-WHITE");
}

#[tokio::test]
async fn import_duplicate_sku_inside_one_file_fails_second_row_only() {
    let pool = test_pool().await;
    let org_id = test_org(&pool).await;
    let rows = vec![
        row(
            2,
            "F40301-001",
            "REGAL SELECT HB SG-WHITE",
            "$57.74",
            "$58.18",
        ),
        row(
            3,
            "F40301-001",
            "REGAL SELECT HB SG-WHITE (again)",
            "$57.74",
            "$58.18",
        ),
        row(
            4,
            "0229B1-004",
            "GENNEX WB CLRNT -THALO BLUE",
            "$39.34",
            "$39.64",
        ),
    ];
    let result = import_products(&pool, org_id, None, &rows).await.unwrap();
    assert_eq!(result.created, 2);
    assert_eq!(result.failed, 1);
    assert_eq!(result.failures.len(), 1);
    assert_eq!(result.failures[0].row, 3);
    assert_eq!(result.failures[0].sku, "F40301-001");
    assert!(
        result.failures[0].reason.contains("duplicate"),
        "reason names the problem: {}",
        result.failures[0].reason
    );
    assert_eq!(list_products_for_org(&pool, org_id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn import_bad_rows_do_not_fail_the_batch() {
    let pool = test_pool().await;
    let org_id = test_org(&pool).await;
    let rows = vec![
        row(
            2,
            "F40301-001",
            "REGAL SELECT HB SG-WHITE",
            "$57.74",
            "$58.18",
        ),
        row(3, "", "Missing SKU", "$1.00", "$2.00"),
        row(4, "SKU-NAMELESS", "", "$1.00", "$2.00"),
        row(5, "SKU-BADPRICE", "Bad price", "not-a-price", "$2.00"),
        row(6, "SKU-NEGATIVE", "Negative price", "$1.00", "-$2.00"),
        row(
            7,
            "0229B1-004",
            "GENNEX WB CLRNT -THALO BLUE",
            "$39.34",
            "$39.64",
        ),
    ];
    let result = import_products(&pool, org_id, None, &rows).await.unwrap();
    assert_eq!(result.created, 2);
    assert_eq!(result.updated, 0);
    assert_eq!(result.failed, 4);
    assert_eq!(
        result.failures.iter().map(|f| f.row).collect::<Vec<_>>(),
        vec![3, 4, 5, 6]
    );
    assert_eq!(list_products_for_org(&pool, org_id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn import_malformed_shifted_row_is_rejected() {
    let pool = test_pool().await;
    let org_id = test_org(&pool).await;
    // Sheet row 1012 of the real price list: every cell holds the
    // same SKU-like string — a shifted row, not a product.
    let mut bad = row(1012, "K7931X-001", "K7931X-001", "K7931X-001", "K7931X-001");
    bad.category = "K7931X-001".to_string();
    bad.size = "K7931X-001".to_string();
    bad.upc = "K7931X-001".to_string();
    bad.msrp = "K7931X-001".to_string();
    bad.map = "K7931X-001".to_string();
    bad.brand_code = "K7931X-001".to_string();
    let rows = vec![
        row(
            2,
            "F40301-001",
            "REGAL SELECT HB SG-WHITE",
            "$57.74",
            "$58.18",
        ),
        bad,
    ];
    let result = import_products(&pool, org_id, None, &rows).await.unwrap();
    assert_eq!(result.created, 1);
    assert_eq!(result.failed, 1);
    assert_eq!(result.failures[0].row, 1012);
    assert_eq!(result.failures[0].sku, "K7931X-001");
    assert!(
        result.failures[0].reason.contains("price"),
        "reason names the price problem: {}",
        result.failures[0].reason
    );
    assert_eq!(list_products_for_org(&pool, org_id).await.unwrap().len(), 1);
}

fn gennex_row(
    sheet_row: usize,
    sku: &str,
    name: &str,
    size: &str,
    net: &str,
    list: &str,
) -> ImportProductRow {
    ImportProductRow {
        row: sheet_row,
        sku: sku.to_string(),
        name: name.to_string(),
        category: "COLORANT".to_string(),
        line: "00229".to_string(),
        color: "".to_string(),
        size: size.to_string(),
        upc: "".to_string(),
        msrp: "$45.00".to_string(),
        map: "$41.00".to_string(),
        net_price: net.to_string(),
        list_price: list.to_string(),
        brand_code: "BMC".to_string(),
        source_metadata: None,
    }
}

async fn test_store(pool: &df_db::AnyPool, org_id: Uuid) -> Uuid {
    let store = Store {
        id: Uuid::now_v7(),
        org_id,
        code: "MAIN".to_string(),
        name: "Main Store".to_string(),
        country_code: "CA".to_string(),
        region_code: "BC".to_string(),
        postal_code: "V7N 1A1".to_string(),
        currency: "CAD".to_string(),
        tax_zone_id: Uuid::now_v7(),
        timezone: "America/Vancouver".to_string(),
        is_warehouse: false,
    };
    let id = store.id;
    df_db::insert_store(pool, &store).await.unwrap();
    id
}

#[tokio::test]
async fn import_gennex_rows_store_msrp_map_volume_and_colorant() {
    let pool = test_pool().await;
    let org_id = test_org(&pool).await;
    let store_id = test_store(&pool, org_id).await;
    // Real Benjamin Moore price-book values (10005510, July 17 2026):
    // S1 is black; quart NET $39.34 / List $39.64, gallon $157.41 / $158.60.
    let quart = gennex_row(
        2,
        "0229S1-004",
        "GENNEX WB CLRNT -BLACK",
        "004",
        "$39.34",
        "$39.64",
    );
    let result = import_products(&pool, org_id, Some(store_id), &[quart])
        .await
        .unwrap();
    assert_eq!(result.created, 1);
    assert_eq!(result.failed, 0);

    let product = get_product_by_sku(&pool, org_id, "0229S1-004")
        .await
        .unwrap();
    assert_eq!(product.msrp, Some(dec("45.00")));
    assert_eq!(product.map, Some(dec("41.00")));
    assert_eq!(product.container_volume_ml, Some(946));

    // One colorant row for code S1: NET $39.34 / 32 fl oz per quart.
    let colorants = list_colorants_for_org(&pool, org_id).await.unwrap();
    assert_eq!(colorants.len(), 1);
    assert_eq!(colorants[0].system_name, "Gennex");
    assert_eq!(colorants[0].code, "S1");
    assert_eq!(colorants[0].name, "GENNEX WB CLRNT -BLACK");
    assert_eq!(colorants[0].cost_per_fl_oz, dec("1.229375"));
    assert_eq!(colorants[0].shots_per_fl_oz, 48);

    // Re-importing the gallon of the same colorant updates the one
    // colorant row (cost per fl oz from the gallon: $157.41 / 128)
    // instead of creating a second S1.
    let gallon = gennex_row(
        3,
        "0229S1-001",
        "GENNEX WB CLRNT -BLACK",
        "001",
        "$157.41",
        "$158.60",
    );
    let result = import_products(&pool, org_id, Some(store_id), &[gallon])
        .await
        .unwrap();
    assert_eq!(result.created, 1);
    let product = get_product_by_sku(&pool, org_id, "0229S1-001")
        .await
        .unwrap();
    assert_eq!(product.container_volume_ml, Some(3785));
    let colorants = list_colorants_for_org(&pool, org_id).await.unwrap();
    assert_eq!(colorants.len(), 1);
    assert_eq!(colorants[0].cost_per_fl_oz, dec("1.229765625"));

    // A colorant from another line (00233, same COLORANT category)
    // imports as a product but gets no Gennex colorant row: the
    // category spans three colorant systems in the real book.
    let other = ImportProductRow {
        row: 4,
        sku: "023301-004".to_string(),
        name: "COLORANT -WHITE".to_string(),
        category: "COLORANT BM PREM".to_string(),
        line: "00233".to_string(),
        color: "01".to_string(),
        size: "004".to_string(),
        upc: "".to_string(),
        msrp: "$45.00".to_string(),
        map: "$41.00".to_string(),
        net_price: "$39.34".to_string(),
        list_price: "$39.64".to_string(),
        brand_code: "BMC".to_string(),
        source_metadata: None,
    };
    let result = import_products(&pool, org_id, Some(store_id), &[other])
        .await
        .unwrap();
    assert_eq!(result.created, 1);
    let colorants = list_colorants_for_org(&pool, org_id).await.unwrap();
    assert_eq!(colorants.len(), 1);
    assert_eq!(colorants[0].code, "S1");
}

#[test]
fn size_codes_map_to_container_volumes() {
    // Numeric BM size codes from the real price book.
    assert_eq!(container_volume_ml_for_size("001"), Some(3785));
    assert_eq!(container_volume_ml_for_size("004"), Some(946));
    assert_eq!(container_volume_ml_for_size("005"), Some(18927));
    assert_eq!(container_volume_ml_for_size("006"), Some(237));
    // Word forms, as a browser import might send them.
    assert_eq!(container_volume_ml_for_size("GAL"), Some(3785));
    assert_eq!(container_volume_ml_for_size("gallon"), Some(3785));
    assert_eq!(container_volume_ml_for_size("QT"), Some(946));
    assert_eq!(container_volume_ml_for_size("quart"), Some(946));
    assert_eq!(container_volume_ml_for_size("PAIL"), Some(18927));
    assert_eq!(container_volume_ml_for_size("5-GAL"), Some(18927));
    assert_eq!(container_volume_ml_for_size("SAMPLE"), Some(237));
    // Kits, eaches, packaging and malformed values carry no volume.
    assert_eq!(container_volume_ml_for_size("014"), None);
    assert_eq!(container_volume_ml_for_size("000"), None);
    assert_eq!(container_volume_ml_for_size(""), None);
    assert_eq!(container_volume_ml_for_size("K7931X-001"), None);
    // Fluid ounces follow the same size: quart 32, gallon 128.
    assert_eq!(container_fl_oz_for_size("004"), Some(dec("32")));
    assert_eq!(container_fl_oz_for_size("001"), Some(dec("128")));
    assert_eq!(container_fl_oz_for_size("005"), Some(dec("640")));
    assert_eq!(container_fl_oz_for_size("006"), Some(dec("8")));
    assert_eq!(container_fl_oz_for_size("014"), None);
}

#[tokio::test]
async fn import_preserves_upc_catalog_fields_and_full_source_metadata() {
    let pool = test_pool().await;
    let org_id = test_org(&pool).await;
    let store_id = test_store(&pool, org_id).await;

    let mut full_row = row(
        2,
        "F40301-001",
        "REGAL SELECT HB SG-WHITE",
        "$57.74",
        "$58.18",
    );
    full_row.source_metadata = Some(serde_json::json!({
        "SKU": "F40301-001",
        "Product Name": "REGAL SELECT HB SG-WHITE",
        "French Description": "REGAL SELECT INT SEMI-LUSTRE BLANC",
        "Category": "REGAL SELECT INTERIOR",
        "French Category Name": "REGAL SELECT INTERIEUR",
        "Line": "0F403",
        "Color": "01",
        "Size": "001",
        "Product UPC": "023906119027",
        "Brand Discount Y/N": "Y",
        "Brand Discount": "5%",
        "Attention: Net Price Change": "YES",
        "Attention: New Catalog SKU": "NO"
    }));

    let res = import_products(&pool, org_id, Some(store_id), &[full_row])
        .await
        .unwrap();
    assert_eq!(res.created, 1);
    assert_eq!(res.failed, 0);

    let product = get_product_by_sku(&pool, org_id, "F40301-001")
        .await
        .unwrap();
    assert_eq!(product.upc.as_deref(), Some("023906119027"));
    assert_eq!(product.category.as_deref(), Some("REGAL SELECT INTERIOR"));
    assert_eq!(product.line_code.as_deref(), Some("0F403"));
    assert_eq!(product.color_name.as_deref(), Some("01"));
    assert_eq!(product.size_code.as_deref(), Some("001"));
    assert_eq!(
        product
            .source_metadata
            .as_ref()
            .and_then(|m| m.get("French Description"))
            .and_then(|v| v.as_str()),
        Some("REGAL SELECT INT SEMI-LUSTRE BLANC")
    );
    assert_eq!(
        product
            .source_metadata
            .as_ref()
            .and_then(|m| m.get("Brand Discount"))
            .and_then(|v| v.as_str()),
        Some("5%")
    );

    // Barcode lookup resolves by Product UPC (via product_barcodes / products.upc)
    // and falls back to SKU.
    let by_upc = df_db::get_product_by_barcode(&pool, org_id, "023906119027")
        .await
        .unwrap();
    assert_eq!(by_upc.id, product.id);
    let by_sku = df_db::get_product_by_barcode(&pool, org_id, "F40301-001")
        .await
        .unwrap();
    assert_eq!(by_sku.id, product.id);

    // Inventory balances surface UPC, catalog fields, and source_metadata.
    let balances = df_db::inventory_balances(&pool, org_id, store_id)
        .await
        .unwrap();
    assert_eq!(balances.len(), 1);
    assert_eq!(balances[0].upc.as_deref(), Some("023906119027"));
    assert_eq!(
        balances[0].category.as_deref(),
        Some("REGAL SELECT INTERIOR")
    );
    assert_eq!(balances[0].line_code.as_deref(), Some("0F403"));
    assert_eq!(balances[0].color_name.as_deref(), Some("01"));
    assert_eq!(balances[0].size_code.as_deref(), Some("001"));
    assert_eq!(balances[0].container_volume_ml, Some(3785));
    assert_eq!(
        balances[0]
            .source_metadata
            .as_ref()
            .and_then(|m| m.get("French Category Name"))
            .and_then(|v| v.as_str()),
        Some("REGAL SELECT INTERIEUR")
    );

    // Partial re-import without UPC/metadata keeps existing values via COALESCE.
    let partial_reimport = ImportProductRow {
        row: 2,
        sku: "F40301-001".to_string(),
        name: "REGAL SELECT HB SG-WHITE".to_string(),
        category: String::new(),
        line: String::new(),
        color: String::new(),
        size: String::new(),
        upc: String::new(),
        msrp: String::new(),
        map: String::new(),
        net_price: "$59.00".to_string(),
        list_price: "$60.00".to_string(),
        brand_code: "BMC".to_string(),
        source_metadata: None,
    };
    let res2 = import_products(&pool, org_id, Some(store_id), &[partial_reimport])
        .await
        .unwrap();
    assert_eq!(res2.updated, 1);
    let after = get_product_by_sku(&pool, org_id, "F40301-001")
        .await
        .unwrap();
    assert_eq!(after.list_price, dec("60.00"));
    assert_eq!(after.upc.as_deref(), Some("023906119027"));
    assert_eq!(after.category.as_deref(), Some("REGAL SELECT INTERIOR"));
    assert!(after.source_metadata.is_some());
}
