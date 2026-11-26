//! Phase 8 (v1.2) exit tests (a)–(d): live PostgreSQL HQ.
//!
//! (a) `df-server migrate schema` against a fresh live Postgres
//!     applies all migrations; a second run is a no-op.
//! (b) `df-server hq` starts against live Postgres, `GET /health`
//!     → 200, a protected HQ route without a token → 401. (No
//!     fixture rows are seeded for (b): /health needs no database
//!     and the auth gate answers 401 before any DB query.)
//!
//! NOTE (Nov 26, 2026): the df-db Postgres placeholder gap that
//! first blocked (c)/(d) is closed — Worker 2 rewrote `?` → `$n`
//! across df-db (sqlx 0.8.6 `Any` passes `?` through verbatim and
//! Postgres rejects it; `$n` also works on SQLite). The HTTP sync
//! ingest (`POST /api/v1/sync/ingest`, `df_sync::DbHq` +
//! `df_sync::RemoteHq`) has landed too, so (c) now pushes through
//! the live endpoint; the `DbHq` fallback runs only if the endpoint
//! does not answer with an `ApplyReport`, and the test prints which
//! path ran.
//! (c) Live round-trip: an offline edge sale on SQLite → sync push
//!     to the HQ → HQ (Postgres) inventory/AR match the edge
//!     exactly; replaying the same outbox is a no-op.
//! (d) Portal + AR reads against the Postgres HQ return the same
//!     figures as the SQLite HQ path for the same fixture data.
//!
//! The push in (c) prefers the HTTP sync ingest
//! (`POST /api/v1/sync/ingest` on the live HQ server) when it
//! answers with an `ApplyReport`; otherwise it applies the batch
//! through the canonical DB-backed sink (`df_sync::DbHq`, the same
//! sink the endpoint itself uses). Which path ran is printed, so a
//! run never silently claims the HTTP ingest was exercised when it
//! was not.
//!
//! Every test skips cleanly (passes, with a printed reason) when no
//! live Postgres is reachable: CI has no Postgres. The URL comes
//! from `/tmp/decor-pos-pg-url.txt` (written by the provisioning
//! step) or the `DECOR_POS_PG_URL` env var. Each test uses its own
//! freshly dropped + recreated database, so runs are re-runnable
//! and parallel-safe.
//!
//! Expected figures reuse the Phase 1 parity fixture exactly (same
//! UUIDs, same $100 product, same 20%-off matrix rule, US/CA store):
//! * cash sale, 2 gal: subtotal 160.0000, eco 1.5000, tax 11.7088,
//!   grand 173.2088, doc TX-000001, edge inventory −2.
//! * charge sale, 1 gal: grand 86.6044, edge AR 86.6044, HQ
//!   inventory −3, HQ AR 86.6044.

use chrono::Utc;
use df_auth::{Role, hash_password};
use df_db::auth::{NewUser, create_user, login_with_password};
use df_db::models::{
    ArLedgerEntry, Customer, InventoryLedgerEntry, Organization, PortalPaymentRow, PricingRuleRow,
    Product, Store, Transaction,
};
use df_domain::{Currency, Money};
use df_edge::{Edge, EdgeConfig, MockTerminal, SaleInput, SaleLineInput, TenderInput, TenderKind};
use df_sync::{DbHq, DeltaBatch, HqSink, Outbox};
use rust_decimal::Decimal;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Output};
use std::str::FromStr;
use std::time::{Duration, Instant};
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;
const PRODUCT_ID: u128 = 0x3000;
const CUSTOMER_ID: u128 = 0x4000;

/// sqlx `_sqlx_migrations` versions for the 0001–0013 set (there is
/// no 0008 — the numbering jumps 0007 → 0009).
const EXPECTED_MIGRATION_VERSIONS: [i64; 12] = [1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13];

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn money(s: &str) -> Money {
    Money::from_str_amount(s, Currency::Usd).unwrap()
}

fn store_id() -> Uuid {
    Uuid::from_u128(STORE_ID)
}

fn org_id() -> Uuid {
    Uuid::from_u128(ORG_ID)
}

fn product_id() -> Uuid {
    Uuid::from_u128(PRODUCT_ID)
}

fn customer_id() -> Uuid {
    Uuid::from_u128(CUSTOMER_ID)
}

/// Live Postgres base URL, or `None` (caller skips with a reason).
fn pg_base_url() -> Option<String> {
    if let Ok(url) = std::env::var("DECOR_POS_PG_URL")
        && !url.trim().is_empty()
    {
        return Some(url.trim().to_string());
    }
    std::fs::read_to_string("/tmp/decor-pos-pg-url.txt")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Replace the database name in a `postgres://.../dbname` URL.
fn url_with_db(base: &str, db: &str) -> String {
    match base.rfind('/') {
        Some(idx) => format!("{}/{db}", &base[..idx]),
        None => base.to_string(),
    }
}

/// `true` when a pool opens against the URL and answers `SELECT 1`
/// within 10 s.
async fn pg_reachable(url: &str) -> bool {
    let attempt = async {
        let pool = df_db::connect(url).await.ok()?;
        let row: (i64,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await.ok()?;
        Some(row.0 == 1)
    };
    tokio::time::timeout(Duration::from_secs(10), attempt)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// Drop + recreate a scratch database on the live server, and return
/// its URL. Requires the `decorpos` role to have CREATEDB (granted
/// at provisioning).
async fn recreate_db(base_url: &str, db: &str) -> String {
    let maint_url = url_with_db(base_url, "postgres");
    let pool = df_db::connect(&maint_url)
        .await
        .expect("connect to maintenance database");
    // Identifiers are test-controlled constants, never user input.
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{db}\""))
        .execute(&pool)
        .await
        .expect("drop scratch database");
    sqlx::query(&format!("CREATE DATABASE \"{db}\""))
        .execute(&pool)
        .await
        .expect("create scratch database");
    url_with_db(base_url, db)
}

fn df_server_exe() -> String {
    env!("CARGO_BIN_EXE_df-server").to_string()
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(df_server_exe())
        .args(args)
        .output()
        .expect("run df-server")
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Raw HTTP request over a plain TCP socket (no extra deps).
/// Returns `(status_code, body)`.
fn http_request(
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(500),
    )
    .expect("connect to server");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(body) = body {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        request.push_str("\r\n");
        request.push_str(body);
    } else {
        request.push_str("\r\n");
    }
    stream.write_all(request.as_bytes()).expect("write request");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    let status: u16 = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

/// A spawned `df-server hq`, killed on drop.
struct HqServer {
    child: Child,
    port: u16,
}

impl HqServer {
    fn spawn(db_url: &str, org: Uuid, store: Option<Uuid>) -> Self {
        let port = free_port();
        let mut args = vec![
            "hq".to_string(),
            "--db".to_string(),
            db_url.to_string(),
            "--listen".to_string(),
            format!("127.0.0.1:{port}"),
            "--org-id".to_string(),
            org.to_string(),
        ];
        if let Some(store) = store {
            args.push("--store-id".to_string());
            args.push(store.to_string());
        }
        let child = Command::new(df_server_exe())
            .args(&args)
            .spawn()
            .expect("spawn df-server hq");
        let server = Self { child, port };
        server.wait_for_health();
        server
    }

    fn wait_for_health(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(mut stream) = TcpStream::connect_timeout(
                &format!("127.0.0.1:{}", self.port).parse().unwrap(),
                Duration::from_millis(200),
            ) {
                stream
                    .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                    .ok();
                let mut buf = String::new();
                stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                if stream.read_to_string(&mut buf).is_ok() && buf.contains("200") {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("df-server hq did not answer /health in time");
    }
}

impl Drop for HqServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The Phase 1 parity fixture: org + store + one $100 paint product,
/// a 20%-off matrix rule, and a charge-enabled customer. Identical on
/// every pool it seeds, so figures compare across dialects.
async fn seed_fixture(pool: &df_db::AnyPool) {
    df_db::insert_organization(
        pool,
        &Organization {
            id: org_id(),
            name: "Test Paint Co".to_string(),
            default_currency: "USD".to_string(),
            enabled_modules: serde_json::json!({"core_pos": true}),
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
            id: product_id(),
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
            list_price: dec("100.0000"),
            standard_cost: dec("50.0000"),
            avg_cost: dec("50.0000"),
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
    df_db::insert_pricing_rule(
        pool,
        &PricingRuleRow {
            id: Uuid::from_u128(0x5000),
            org_id: org_id(),
            priority: 1,
            customer_id: None,
            job_id: None,
            price_level_id: None,
            product_id: Some(product_id()),
            product_line_id: None,
            class_id: None,
            department_id: None,
            vendor_id: None,
            min_qty: dec("1"),
            calc_method: "pct_off_list".to_string(),
            calc_value: dec("20"),
            min_margin_floor: None,
            effective_from: None,
            effective_to: None,
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
            customer_type: "contractor".to_string(),
            company_name: Some("Acme Painting".to_string()),
            first_name: None,
            last_name: None,
            email: None,
            phone: None,
            price_level_id: None,
            salesperson_id: None,
            tax_exempt_cert: None,
            tax_exempt_expires: None,
            charge_enabled: true,
            credit_limit: dec("1000.0000"),
            payment_terms: "NET30".to_string(),
            terms_days: None,
            finance_rate_pct_monthly: None,
            credit_hold: false,
            require_po_number: false,
            require_job_name: false,
            portal_enabled: true,
        },
    )
    .await
    .unwrap();
}

/// Seed an Admin and return a live Bearer token for it (same login
/// path the HTTP gate validates).
async fn seed_admin_token(pool: &df_db::AnyPool) -> String {
    create_user(
        pool,
        &NewUser {
            org_id: org_id(),
            email: "admin@example.invalid".to_string(),
            display_name: "Test Admin".to_string(),
            password_hash: Some(hash_password("password-123").unwrap()),
            counter_pin_hash: None,
            badge_token_hash: None,
            role: Role::Admin,
            portal_customer_id: None,
            active: true,
        },
    )
    .await
    .unwrap();
    login_with_password(pool, "admin@example.invalid", "password-123", false)
        .await
        .unwrap()
        .token
}

/// Outcome of pushing one batch to the HQ, however it got there.
struct PushOutcome {
    applied: usize,
    duplicates: usize,
    via_http_ingest: bool,
}

/// Push a batch to the live HQ. Prefers the HTTP sync ingest when
/// the server answers it with an `ApplyReport`; otherwise applies
/// through the canonical DB-backed sink (`df_sync::DbHq`, same
/// `sync_applied_entries` idempotency the endpoint itself uses).
/// When `edge` is given, the first push goes through `edge.sync_now`
/// (which marks the entries synced) and an HTTP push is followed by
/// an explicit `mark_synced` of exactly the ids in the report's
/// `applied_ids`; replays pass `None` and apply the snapshot batch
/// directly. Prints which path ran.
async fn push_batch(
    hq: &HqServer,
    db_hq: &DbHq,
    edge: Option<&Edge>,
    batch: &DeltaBatch,
    admin_token: &str,
) -> PushOutcome {
    let body = serde_json::to_string(batch).expect("batch serialises");
    let (status, resp) = http_request(
        hq.port,
        "POST",
        "/api/v1/sync/ingest",
        Some(admin_token),
        Some(&body),
    );
    if status == 200
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&resp)
        && json.get("applied").and_then(|v| v.as_u64()).is_some()
        && let Some(applied_ids) = json
            .get("applied_ids")
            .and_then(|v| v.as_array())
            .map(|ids| {
                ids.iter()
                    .filter_map(|v| v.as_str())
                    .filter_map(|s| Uuid::parse_str(s).ok())
                    .collect::<Vec<Uuid>>()
            })
    {
        let applied = json["applied"].as_u64().unwrap() as usize;
        let duplicates = json.get("duplicates").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        println!(
            "push_batch: via HTTP ingest (POST /api/v1/sync/ingest), applied={applied} duplicates={duplicates}"
        );
        if let Some(edge) = edge {
            // The HTTP path applies the batch but never touches the
            // edge outbox; mark exactly the ids HQ resolved.
            Outbox::mark_synced(edge.pool(), &applied_ids)
                .await
                .unwrap();
        }
        return PushOutcome {
            applied,
            duplicates,
            via_http_ingest: true,
        };
    }
    println!("push_batch: HTTP ingest unavailable (status {status}); applying via DbHq sink");
    let report = match edge {
        Some(edge) => edge.sync_now(db_hq).await.expect("edge sync_now"),
        None => db_hq.apply_batch(batch).await.expect("DbHq apply_batch"),
    };
    PushOutcome {
        applied: report.applied,
        duplicates: report.duplicates,
        via_http_ingest: false,
    }
}

/// Counts of the HQ ledger rows the round-trip writes, for the
/// no-duplicate-rows assertions.
async fn hq_row_counts(pool: &df_db::AnyPool) -> (i64, i64, i64) {
    let inv: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_ledger WHERE store_id = $1 AND product_id = $2",
    )
    .bind(store_id().to_string())
    .bind(product_id().to_string())
    .fetch_one(pool)
    .await
    .unwrap();
    let ar: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ar_ledger WHERE customer_id = $1")
        .bind(customer_id().to_string())
        .fetch_one(pool)
        .await
        .unwrap();
    let tx: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE store_id = $1")
        .bind(store_id().to_string())
        .fetch_one(pool)
        .await
        .unwrap();
    (inv, ar, tx)
}

fn sale_input(qty: &str, tender: TenderKind, customer: Option<Uuid>) -> SaleInput {
    SaleInput {
        customer_id: customer,
        job_id: None,
        lines: vec![SaleLineInput {
            product_id: product_id(),
            sku: "BM-REGAL-1G".to_string(),
            description: "Regal Select Interior Gallon".to_string(),
            qty: dec(qty),
            uom: "gal".to_string(),
            unit_price: money("100.0000"),
            unit_cost: money("50.0000"),
        }],
        tenders: vec![TenderInput {
            kind: tender,
            amount: None,
        }],
        po_number: None,
    }
}

/// Edge on a temp SQLite file, seeded with the parity fixture.
async fn seeded_edge(db_path: &std::path::Path) -> Edge {
    let config = EdgeConfig {
        store_id: store_id(),
        org_id: org_id(),
        db_url: format!("sqlite:{}", db_path.display()),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    let edge = Edge::open(config).await.unwrap();
    seed_fixture(edge.pool()).await;
    edge
}

/// (d) fixture rows, identical on both pools: one −3 inventory
/// delta, one 86.6044 AR charge, one 150.00 portal payment, one
/// TX-000001 sale of 86.6044 for the customer.
async fn seed_read_fixture(pool: &df_db::AnyPool) {
    seed_fixture(pool).await;
    // Fixed instant (not Utc::now()): the two pools must hold
    // byte-identical rows or the cross-dialect row comparisons
    // below fail on created_at alone.
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-26T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let tx_id = Uuid::from_u128(0x7000);
    df_db::insert_inventory_delta(
        pool,
        &InventoryLedgerEntry {
            id: Uuid::from_u128(0x7001),
            org_id: org_id(),
            store_id: store_id(),
            product_id: product_id(),
            lot_number: None,
            delta_on_hand: dec("-3"),
            delta_committed: Decimal::ZERO,
            delta_on_order: Decimal::ZERO,
            delta_in_transit: Decimal::ZERO,
            unit_cost: dec("50.0000"),
            reason_type: "sale".to_string(),
            reference_id: tx_id,
            created_at: now,
        },
    )
    .await
    .unwrap();
    df_db::insert_ar_entry(
        pool,
        &ArLedgerEntry {
            id: Uuid::from_u128(0x7002),
            org_id: org_id(),
            store_id: store_id(),
            customer_id: customer_id(),
            job_id: None,
            entry_type: "charge".to_string(),
            reference_type: "transaction".to_string(),
            reference_id: tx_id,
            amount: dec("86.6044"),
            due_date: None,
            statement_id: None,
            created_at: now,
        },
    )
    .await
    .unwrap();
    df_db::insert_portal_payment(
        pool,
        &PortalPaymentRow {
            idempotency_key: "pay-rt-1".to_string(),
            customer_id: customer_id(),
            payment_entry_id: Uuid::from_u128(0x7002),
            amount: dec("150.0000"),
            method: "card".to_string(),
            provider_reference: "ch_test_1".to_string(),
            created_at: now,
        },
    )
    .await
    .unwrap();
    df_db::insert_transaction(
        pool,
        &Transaction {
            id: tx_id,
            org_id: org_id(),
            store_id: store_id(),
            till_session_id: None,
            doc_number: "TX-000001".to_string(),
            doc_type: "sale".to_string(),
            status: "completed".to_string(),
            customer_id: Some(customer_id()),
            job_id: None,
            po_number: None,
            salesperson_id: None,
            cashier_id: None,
            currency: "USD".to_string(),
            subtotal: dec("80.0000"),
            tax_total: dec("5.8544"),
            eco_fee_total: dec("0.7500"),
            pos_fee_total: dec("0.0000"),
            grand_total: dec("86.6044"),
            deposit_paid: Decimal::ZERO,
            fulfillment_mode: "counter".to_string(),
            scheduled_at: None,
            completed_at: Some(now),
            created_at: now,
        },
    )
    .await
    .unwrap();
}

// --- (a) ---------------------------------------------------------------

#[tokio::test]
async fn pg_migrate_schema_applies_all_then_noop() {
    let Some(base) = pg_base_url() else {
        println!(
            "SKIP: no live Postgres — set DECOR_POS_PG_URL or write /tmp/decor-pos-pg-url.txt"
        );
        return;
    };
    if !pg_reachable(&base).await {
        println!(
            "SKIP: live Postgres at {base} is not reachable",
            base = redact_url(&base)
        );
        return;
    }
    let db_url = recreate_db(&base, "decorpos_hq_rt_a").await;

    let first = run_cli(&["migrate", "schema", "--db", &db_url]);
    assert!(
        first.status.success(),
        "migrate schema failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("migrate schema prints JSON");
    println!("migrate schema (first run): {report}");
    assert_eq!(report["dialect"], "postgres");
    assert_eq!(report["status"], "migrated");
    let applied: Vec<i64> = serde_json::from_value(report["applied"].clone()).unwrap();
    assert_eq!(applied, EXPECTED_MIGRATION_VERSIONS);
    let newly: Vec<i64> = serde_json::from_value(report["newly_applied"].clone()).unwrap();
    assert_eq!(newly, EXPECTED_MIGRATION_VERSIONS);

    let second = run_cli(&["migrate", "schema", "--db", &db_url]);
    assert!(second.status.success());
    let report2: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("second migrate prints JSON");
    println!("migrate schema (second run): {report2}");
    assert_eq!(report2["status"], "already_up_to_date");
    let newly2: Vec<i64> = serde_json::from_value(report2["newly_applied"].clone()).unwrap();
    assert!(newly2.is_empty(), "second run applied {newly2:?}");
    let applied2: Vec<i64> = serde_json::from_value(report2["applied"].clone()).unwrap();
    assert_eq!(applied2, EXPECTED_MIGRATION_VERSIONS);
}

fn redact_url(url: &str) -> String {
    match url.find("://") {
        Some(idx) => match url[idx + 3..].find('@') {
            Some(at) => format!("{}://***@{}", &url[..idx], &url[idx + 3 + at + 1..]),
            None => url.to_string(),
        },
        None => url.to_string(),
    }
}

// --- (b) ---------------------------------------------------------------

#[tokio::test]
async fn pg_hq_serves_health_and_gates_protected_routes() {
    let Some(base) = pg_base_url() else {
        println!(
            "SKIP: no live Postgres — set DECOR_POS_PG_URL or write /tmp/decor-pos-pg-url.txt"
        );
        return;
    };
    if !pg_reachable(&base).await {
        println!(
            "SKIP: live Postgres at {} is not reachable",
            redact_url(&base)
        );
        return;
    }
    let db_url = recreate_db(&base, "decorpos_hq_rt_b").await;
    let migrate = run_cli(&["migrate", "schema", "--db", &db_url]);
    assert!(migrate.status.success(), "migrate failed");

    // No fixture rows are seeded: /health needs no database and the
    // auth gate answers 401 before any DB query when no token is
    // presented, so this test proves the live server + gate without
    // depending on any df-db query working on Postgres.
    let hq = HqServer::spawn(&db_url, org_id(), Some(store_id()));
    let (status, body) = http_request(hq.port, "GET", "/health", None, None);
    println!("GET /health -> {status} {body}");
    assert_eq!(status, 200);
    assert!(body.contains("\"status\":\"ok\""), "{body}");
    assert!(body.contains("\"version\":\"1.2.0\""), "{body}");

    for path in [
        format!("/api/v1/inventory/balances?store_id={}", store_id()),
        format!("/api/v1/ar/customers/{}/summary", customer_id()),
        format!("/api/v1/ar/aging?store_id={}", store_id()),
    ] {
        let (status, _) = http_request(hq.port, "GET", &path, None, None);
        assert_eq!(
            status, 401,
            "unauthenticated GET {path} must be 401, got {status}"
        );
    }
    println!("protected HQ routes without a token: 401 (3/3)");
}

// --- (c) ---------------------------------------------------------------

#[tokio::test]
async fn pg_live_round_trip_edge_sqlite_to_hq_postgres() {
    let Some(base) = pg_base_url() else {
        println!(
            "SKIP: no live Postgres — set DECOR_POS_PG_URL or write /tmp/decor-pos-pg-url.txt"
        );
        return;
    };
    if !pg_reachable(&base).await {
        println!(
            "SKIP: live Postgres at {} is not reachable",
            redact_url(&base)
        );
        return;
    }
    let db_url = recreate_db(&base, "decorpos_hq_rt_c").await;
    let migrate = run_cli(&["migrate", "schema", "--db", &db_url]);
    assert!(
        migrate.status.success(),
        "migrate failed: {}",
        String::from_utf8_lossy(&migrate.stderr)
    );

    // HQ pool + fixture + admin token, then the live HQ server.
    let pg_pool = df_db::connect(&db_url).await.unwrap();
    seed_fixture(&pg_pool).await;
    let admin_token = seed_admin_token(&pg_pool).await;
    let hq = HqServer::spawn(&db_url, org_id(), Some(store_id()));
    let db_hq = DbHq::new(pg_pool.clone());

    // --- OFFLINE: two sales on a SQLite edge, no HQ contact. ---
    let edge_path =
        std::env::temp_dir().join(format!("decor-pos-rt-edge-{}-c.db", std::process::id()));
    let _ = std::fs::remove_file(&edge_path);
    let edge = seeded_edge(&edge_path).await;
    let terminal = MockTerminal::new();
    edge.open_till(Uuid::from_u128(0x6000), money("150.0000"))
        .await
        .unwrap();

    let sale = edge
        .checkout(sale_input("2", TenderKind::Cash, None), &terminal)
        .await
        .unwrap();
    assert_eq!(sale.subtotal, money("160.0000"));
    assert_eq!(sale.eco_fee_total, money("1.5000"));
    assert_eq!(sale.tax_total, money("11.7088"));
    assert_eq!(sale.grand_total, money("173.2088"));
    assert_eq!(sale.doc_number, "TX-000001");

    let charge_sale = edge
        .checkout(
            sale_input("1", TenderKind::ChargeAccount, Some(customer_id())),
            &terminal,
        )
        .await
        .unwrap();
    assert_eq!(charge_sale.grand_total, money("86.6044"));
    assert_eq!(charge_sale.doc_number, "TX-000002");

    assert_eq!(
        df_db::inventory_on_hand(edge.pool(), store_id(), product_id())
            .await
            .unwrap(),
        dec("-3")
    );
    let edge_ar = df_db::ar_balance_for_customer(edge.pool(), customer_id())
        .await
        .unwrap();
    assert_eq!(edge_ar, dec("86.6044"));

    // Snapshot the batch so the identical batch can be replayed.
    let pending = Outbox::pending(edge.pool(), store_id(), 1000)
        .await
        .unwrap();
    assert_eq!(pending.len(), 5, "2 entries (sale 1) + 3 entries (sale 2)");
    let batch = DeltaBatch::new(store_id(), pending);

    // --- PUSH: edge outbox → live Postgres HQ. ---
    let pushed = push_batch(&hq, &db_hq, Some(&edge), &batch, &admin_token).await;
    assert_eq!(pushed.applied, 5, "first push applies all 5 entries");
    assert_eq!(pushed.duplicates, 0);
    // The point of this test is the live HTTP round-trip: the first
    // push must go through POST /api/v1/sync/ingest, not silently
    // fall back to the in-process DbHq sink. (This also keeps the
    // `via_http_ingest` field read, so clippy's dead_code lint
    // stays quiet.)
    assert!(
        pushed.via_http_ingest,
        "first push must go via HTTP ingest, not the DbHq fallback"
    );
    assert_eq!(edge.pending_outbox_count().await.unwrap(), 0);

    // HQ (Postgres) inventory/AR match the edge exactly.
    let hq_on_hand = df_db::inventory_on_hand(&pg_pool, store_id(), product_id())
        .await
        .unwrap();
    assert_eq!(hq_on_hand, dec("-3"));
    let hq_ar = df_db::ar_balance_for_customer(&pg_pool, customer_id())
        .await
        .unwrap();
    assert_eq!(hq_ar, dec("86.6044"));
    assert_eq!(hq_ar, edge_ar);
    let hq_txs = df_db::list_transactions_for_customer(&pg_pool, customer_id())
        .await
        .unwrap();
    assert_eq!(hq_txs.len(), 1);
    assert_eq!(hq_txs[0].grand_total, dec("86.6044"));
    assert_eq!(hq_txs[0].doc_number, "TX-000002");
    let (inv_rows, ar_rows, tx_rows) = hq_row_counts(&pg_pool).await;
    assert_eq!((inv_rows, ar_rows, tx_rows), (2, 1, 2));
    println!(
        "round-trip figures: HQ inventory {hq_on_hand} (edge -3), HQ AR {hq_ar} (edge 86.6044), rows inv={inv_rows} ar={ar_rows} tx={tx_rows}"
    );

    // --- REPLAY: the identical batch changes NOTHING. ---
    let replayed = push_batch(&hq, &db_hq, None, &batch, &admin_token).await;
    assert_eq!(replayed.applied, 0, "replay applies nothing");
    assert_eq!(replayed.duplicates, 5, "replay reports all 5 as duplicates");
    assert_eq!(
        df_db::inventory_on_hand(&pg_pool, store_id(), product_id())
            .await
            .unwrap(),
        dec("-3")
    );
    assert_eq!(
        df_db::ar_balance_for_customer(&pg_pool, customer_id())
            .await
            .unwrap(),
        dec("86.6044")
    );
    assert_eq!(hq_row_counts(&pg_pool).await, (2, 1, 2));

    // A second drain with an empty outbox is also a no-op.
    let drain = edge.sync_now(&db_hq).await.unwrap();
    assert_eq!(drain.applied, 0);
    assert_eq!(drain.duplicates, 0);
    assert_eq!(hq_row_counts(&pg_pool).await, (2, 1, 2));

    let _ = std::fs::remove_file(&edge_path);
}

// --- (d) ---------------------------------------------------------------

#[tokio::test]
async fn pg_portal_and_ar_reads_match_sqlite() {
    let Some(base) = pg_base_url() else {
        println!(
            "SKIP: no live Postgres — set DECOR_POS_PG_URL or write /tmp/decor-pos-pg-url.txt"
        );
        return;
    };
    if !pg_reachable(&base).await {
        println!(
            "SKIP: live Postgres at {} is not reachable",
            redact_url(&base)
        );
        return;
    }

    // Same fixture on both dialects.
    let sqlite_pool = df_db::connect("sqlite::memory:").await.unwrap();
    df_db::migrate(&sqlite_pool, df_db::Dialect::Sqlite)
        .await
        .unwrap();
    seed_read_fixture(&sqlite_pool).await;

    let pg_url = recreate_db(&base, "decorpos_hq_rt_d").await;
    let migrate = run_cli(&["migrate", "schema", "--db", &pg_url]);
    assert!(migrate.status.success(), "migrate failed");
    let pg_pool = df_db::connect(&pg_url).await.unwrap();
    seed_read_fixture(&pg_pool).await;

    // Repo-level reads: identical figures on both pools.
    let pg_balances = df_db::inventory_balances(&pg_pool, org_id(), store_id())
        .await
        .unwrap();
    let lite_balances = df_db::inventory_balances(&sqlite_pool, org_id(), store_id())
        .await
        .unwrap();
    assert_eq!(pg_balances, lite_balances);
    let pg_balance = pg_balances
        .iter()
        .find(|b| b.product_id == product_id())
        .expect("product balance");
    assert_eq!(pg_balance.on_hand, dec("-3"));
    assert_eq!(pg_balance.list_price, dec("100.0000"));

    let pg_ar = df_db::ar_balance_for_customer(&pg_pool, customer_id())
        .await
        .unwrap();
    let lite_ar = df_db::ar_balance_for_customer(&sqlite_pool, customer_id())
        .await
        .unwrap();
    assert_eq!(pg_ar, lite_ar);
    assert_eq!(pg_ar, dec("86.6044"));

    let pg_payments = df_db::list_portal_payments_for_customer(&pg_pool, customer_id())
        .await
        .unwrap();
    let lite_payments = df_db::list_portal_payments_for_customer(&sqlite_pool, customer_id())
        .await
        .unwrap();
    assert_eq!(pg_payments.len(), 1);
    assert_eq!(pg_payments, lite_payments);
    assert_eq!(pg_payments[0].amount, dec("150.0000"));
    assert_eq!(pg_payments[0].idempotency_key, "pay-rt-1");

    let pg_invoices = df_db::list_transactions_for_customer(&pg_pool, customer_id())
        .await
        .unwrap();
    let lite_invoices = df_db::list_transactions_for_customer(&sqlite_pool, customer_id())
        .await
        .unwrap();
    assert_eq!(pg_invoices.len(), 1);
    assert_eq!(pg_invoices, lite_invoices);
    assert_eq!(pg_invoices[0].grand_total, dec("86.6044"));
    println!(
        "repo reads match: inventory on_hand {} / AR {} / portal payment {} / invoice {}",
        pg_balance.on_hand, pg_ar, pg_payments[0].amount, pg_invoices[0].grand_total
    );

    // HTTP-level reads: the same `df-server hq` binary over a
    // Postgres pool and over a SQLite file pool, same fixture.
    let sqlite_path =
        std::env::temp_dir().join(format!("decor-pos-rt-hq-lite-{}-d.db", std::process::id()));
    let _ = std::fs::remove_file(&sqlite_path);
    let sqlite_file_url = format!("sqlite:{}", sqlite_path.display());
    let lite_file_pool = df_db::connect(&sqlite_file_url).await.unwrap();
    df_db::migrate(&lite_file_pool, df_db::Dialect::Sqlite)
        .await
        .unwrap();
    seed_read_fixture(&lite_file_pool).await;
    let lite_token = seed_admin_token(&lite_file_pool).await;
    let pg_token = seed_admin_token(&pg_pool).await;

    let pg_hq = HqServer::spawn(&pg_url, org_id(), Some(store_id()));
    let lite_hq = HqServer::spawn(&sqlite_file_url, org_id(), Some(store_id()));

    let (pg_status, pg_body) = http_request(
        pg_hq.port,
        "GET",
        &format!("/api/v1/inventory/balances?store_id={}", store_id()),
        Some(&pg_token),
        None,
    );
    let (lite_status, lite_body) = http_request(
        lite_hq.port,
        "GET",
        &format!("/api/v1/inventory/balances?store_id={}", store_id()),
        Some(&lite_token),
        None,
    );
    assert_eq!(pg_status, 200, "{pg_body}");
    assert_eq!(lite_status, 200, "{lite_body}");
    let pg_json: serde_json::Value = serde_json::from_str(&pg_body).unwrap();
    let lite_json: serde_json::Value = serde_json::from_str(&lite_body).unwrap();
    assert_eq!(pg_json, lite_json, "inventory balances differ by dialect");
    assert_eq!(pg_json[0]["on_hand"], "-3");
    assert_eq!(pg_json[0]["list_price"], "100.0000");

    let (pg_status, pg_body) = http_request(
        pg_hq.port,
        "GET",
        &format!("/api/v1/ar/customers/{}/summary", customer_id()),
        Some(&pg_token),
        None,
    );
    let (lite_status, lite_body) = http_request(
        lite_hq.port,
        "GET",
        &format!("/api/v1/ar/customers/{}/summary", customer_id()),
        Some(&lite_token),
        None,
    );
    assert_eq!(pg_status, 200, "{pg_body}");
    assert_eq!(lite_status, 200, "{lite_body}");
    let pg_json: serde_json::Value = serde_json::from_str(&pg_body).unwrap();
    let lite_json: serde_json::Value = serde_json::from_str(&lite_body).unwrap();
    assert_eq!(pg_json, lite_json, "AR summaries differ by dialect");
    assert_eq!(pg_json["balance"], "86.6044");
    assert_eq!(pg_json["entry_count"], 1);
    println!("HTTP reads match: balances on_hand -3, AR balance 86.6044, entry_count 1");

    let _ = std::fs::remove_file(&sqlite_path);
}
