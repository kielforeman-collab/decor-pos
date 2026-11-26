//! Migrator fixture test: import the sample legacy CSVs under
//! `fixtures/` into an in-memory SQLite edge and verify the
//! reconciliation manifest exactly — per-file counts, file
//! checksums, row checksums, the imported-rows aggregate — plus
//! the database state the import produced, and that a re-import
//! is a no-op (every row skipped or failed again, nothing
//! imported twice).
//!
//! Expected fixture outcome (24 rows read):
//! * customers: 5 read / 3 imported / 1 skipped (duplicate
//!   C-1001) / 1 failed (missing account_number)
//! * products: 5 read / 3 imported / 1 skipped (duplicate
//!   BM-REGAL-1G) / 1 failed (list_price not a decimal)
//! * formulas: 4 read / 2 imported / 1 skipped (duplicate
//!   OC-17) / 1 failed (unknown base sku XX-UNKNOWN)
//! * ar_balances: 5 read / 3 imported / 1 skipped (duplicate
//!   INV-1001) / 1 failed (unknown account C-9999)
//! * inventory: 5 read / 3 imported / 1 skipped (duplicate
//!   BM-REGAL-1G) / 1 failed (unknown sku XX-UNKNOWN)

use std::path::PathBuf;

use chrono::Utc;
use df_db::models::{Organization, Store};
use df_migrator::manifest::{row_checksum, sha256_hex, uuid_from_seed};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use uuid::Uuid;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// SHA-256 hex computed independently of the migrator helpers
/// (raw `sha2` over the given bytes).
fn independent_sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[tokio::test]
async fn fixtures_import_counts_checksums_and_state() {
    let pool = df_db::connect("sqlite::memory:").await.unwrap();
    df_db::migrate(&pool, df_db::Dialect::Sqlite).await.unwrap();
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

    let manifest = df_migrator::import_directory(&pool, org_id, store_id, &fixtures_dir())
        .await
        .unwrap();

    // --- totals ---------------------------------------------------------------
    assert_eq!(manifest.total_rows_read, 24);
    assert_eq!(manifest.total_imported, 14);
    assert_eq!(manifest.total_skipped, 5);
    assert_eq!(manifest.total_failed, 5);
    assert!(!manifest.is_clean());

    // --- per-file counts -------------------------------------------------------
    let by_kind: std::collections::HashMap<&str, &df_migrator::FileManifest> = manifest
        .files
        .iter()
        .map(|file| (file.kind.as_str(), file))
        .collect();
    assert_eq!(by_kind.len(), 5);
    let expect = |kind: &str, read: usize, imported: usize, skipped: usize, failed: usize| {
        let file = by_kind[kind];
        assert_eq!(file.rows_read, read, "{kind} rows_read");
        assert_eq!(file.imported, imported, "{kind} imported");
        assert_eq!(file.skipped, skipped, "{kind} skipped");
        assert_eq!(file.failed, failed, "{kind} failed");
        assert!(file.file_error.is_none(), "{kind} file_error");
    };
    expect("customers", 5, 3, 1, 1);
    expect("products", 5, 3, 1, 1);
    expect("formulas", 4, 2, 1, 1);
    expect("ar_balances", 5, 3, 1, 1);
    expect("inventory", 5, 3, 1, 1);

    // --- file checksums: SHA-256 of the raw fixture bytes ----------------------
    for file in &manifest.files {
        let raw = std::fs::read(fixtures_dir().join(&file.file_name)).unwrap();
        assert_eq!(
            file.file_sha256.as_deref(),
            Some(independent_sha256_hex(&raw).as_str()),
            "{} file_sha256",
            file.file_name
        );
    }

    // --- row checksums: the first customer row, recomputed by hand -------------
    // Canonical bytes: fields in column order, trimmed, joined
    // with \x1f.
    let canonical = "C-1001\x1fcontractor\x1fHarbourview Painting Ltd.\x1f\x1f\x1f\x1f604-555-0101\x1ftrue\x1f5000.0000\x1fNET30";
    let expected_row_sha = independent_sha256_hex(canonical.as_bytes());
    let customers = by_kind["customers"];
    assert_eq!(customers.rows[0].sha256, expected_row_sha);
    assert_eq!(customers.rows[0].status, df_migrator::RowStatus::Imported);
    // The migrator's own helper agrees with the independent hash.
    assert_eq!(
        row_checksum(
            &[
                "C-1001",
                "contractor",
                "Harbourview Painting Ltd.",
                "",
                "",
                "",
                "604-555-0101",
                "true",
                "5000.0000",
                "NET30"
            ]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>()
        ),
        expected_row_sha
    );
    // Imported-rows aggregate: SHA-256 over the concatenation of
    // the three imported customer rows' checksums, in file order.
    let mut hasher = Sha256::new();
    for row in &customers.rows {
        if row.status == df_migrator::RowStatus::Imported {
            hasher.update(row.sha256.as_bytes());
        }
    }
    assert_eq!(
        customers.imported_rows_sha256.as_deref(),
        Some(format!("{:x}", hasher.finalize()).as_str())
    );

    // --- failure reasons name the problem --------------------------------------
    assert!(
        customers.rows[3]
            .reason
            .as_deref()
            .unwrap()
            .contains("missing account_number"),
        "customers row 4 reason: {:?}",
        customers.rows[3].reason
    );
    assert_eq!(customers.rows[3].status, df_migrator::RowStatus::Failed);
    assert_eq!(customers.rows[4].status, df_migrator::RowStatus::Skipped);
    assert!(
        customers.rows[4]
            .reason
            .as_deref()
            .unwrap()
            .contains("duplicate account_number C-1001")
    );
    assert!(
        by_kind["products"].rows[3]
            .reason
            .as_deref()
            .unwrap()
            .contains("not a decimal")
    );
    assert!(
        by_kind["formulas"].rows[2]
            .reason
            .as_deref()
            .unwrap()
            .contains("unknown base sku XX-UNKNOWN")
    );
    assert!(
        by_kind["ar_balances"].rows[3]
            .reason
            .as_deref()
            .unwrap()
            .contains("unknown customer account C-9999")
    );
    assert!(
        by_kind["inventory"].rows[3]
            .reason
            .as_deref()
            .unwrap()
            .contains("unknown sku XX-UNKNOWN")
    );

    // --- database state ---------------------------------------------------------
    let imported_customers = df_db::list_customers_for_org(&pool, org_id).await.unwrap();
    assert_eq!(imported_customers.len(), 3);
    let harbourview = df_db::get_customer_by_account(&pool, org_id, "C-1001")
        .await
        .unwrap();
    assert_eq!(harbourview.credit_limit, dec("5000.0000"));
    assert!(harbourview.charge_enabled);
    let products = df_db::list_products_for_org(&pool, org_id).await.unwrap();
    assert_eq!(products.len(), 3);
    let regal = df_db::get_product_by_sku(&pool, org_id, "BM-REGAL-1G")
        .await
        .unwrap();
    assert_eq!(regal.list_price, dec("89.9900"));

    // Formulas: the OC-17 row is reachable by its deterministic id
    // (uuid_from_seed over "formula:{row checksum}").
    let oc17_checksum = by_kind["formulas"].rows[0].sha256.clone();
    let oc17_id = uuid_from_seed(&format!("formula:{oc17_checksum}"));
    let formula = df_db::get_tint_formula(&pool, oc17_id).await.unwrap();
    assert_eq!(formula.color_code_snapshot, "OC-17");
    assert_eq!(formula.color_name_snapshot, "White Dove");
    assert_eq!(formula.base_product_id, regal.id);
    assert_eq!(formula.customer_id, Some(harbourview.id));
    assert_eq!(formula.container_volume_ml, 3785);
    assert_eq!(formula.total_tint_cost, dec("2.4000"));
    assert_eq!(
        formula.colorants_json,
        serde_json::json!([{"code": "Y3", "shots": 14}, {"code": "R1", "shots": 2}])
    );

    // AR: C-1001 owes 400 + 300 = 700; C-1002 paid 150 (negative).
    let entries_c1001 = df_db::list_ar_entries_for_customer(&pool, harbourview.id)
        .await
        .unwrap();
    assert_eq!(entries_c1001.len(), 2);
    let balance_c1001: Decimal = entries_c1001.iter().map(|entry| entry.amount).sum();
    assert_eq!(balance_c1001, dec("700.0000"));
    assert!(
        entries_c1001
            .iter()
            .all(|entry| entry.reference_type == "migration")
    );
    let seaside = df_db::get_customer_by_account(&pool, org_id, "C-1002")
        .await
        .unwrap();
    let entries_c1002 = df_db::list_ar_entries_for_customer(&pool, seaside.id)
        .await
        .unwrap();
    assert_eq!(entries_c1002.len(), 1);
    assert_eq!(entries_c1002[0].amount, dec("-150.0000"));
    assert_eq!(entries_c1002[0].entry_type, "payment");

    // Inventory: 24 gal of Regal on hand at MAIN.
    assert_eq!(
        df_db::inventory_on_hand(&pool, store_id, regal.id)
            .await
            .unwrap(),
        dec("24.0000")
    );

    // --- re-import is a no-op ---------------------------------------------------
    let rerun = df_migrator::import_directory(&pool, org_id, store_id, &fixtures_dir())
        .await
        .unwrap();
    assert_eq!(rerun.total_rows_read, 24);
    assert_eq!(rerun.total_imported, 0);
    assert_eq!(rerun.total_skipped, 19);
    assert_eq!(rerun.total_failed, 5);
    // State unchanged.
    assert_eq!(
        df_db::list_customers_for_org(&pool, org_id)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        df_db::inventory_on_hand(&pool, store_id, regal.id)
            .await
            .unwrap(),
        dec("24.0000")
    );
    assert_eq!(
        df_db::list_ar_entries_for_customer(&pool, harbourview.id)
            .await
            .unwrap()
            .len(),
        2
    );

    // --- an unknown file is reported, not silently ignored ----------------------
    let stray = fixtures_dir().join("notes.txt");
    std::fs::write(&stray, "not a csv\n").unwrap();
    let manifest = df_migrator::import_directory(&pool, org_id, store_id, &fixtures_dir())
        .await
        .unwrap();
    std::fs::remove_file(&stray).unwrap();
    let unknown = manifest
        .files
        .iter()
        .find(|file| file.kind == "unknown")
        .expect("unknown file is in the manifest");
    assert_eq!(unknown.file_name, "notes.txt");
    assert!(
        unknown
            .file_error
            .as_deref()
            .unwrap()
            .contains("unknown file kind")
    );
    assert_eq!(unknown.rows_read, 0);
    // The stray file's checksum is still recorded.
    assert_eq!(
        unknown.file_sha256.as_deref(),
        Some(sha256_hex(b"not a csv\n").as_str())
    );
}
