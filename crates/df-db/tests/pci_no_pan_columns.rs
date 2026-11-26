//! PCI guardrail test (spec §8 guardrail 3): the schemas contain
//! zero PAN/CVV columns, and CI fails if a forbidden column pattern
//! is ever introduced.
//!
//! This scans every migration file in both dialects — the test reads
//! the source tree via `CARGO_MANIFEST_DIR`, so it covers migrations
//! added in any later phase, not just the ones that exist today.
//! It also pins the exact column sets of the two payment tables:
//! `payments` and `portal_payments` may hold only a tender/method
//! label, a provider reference string and an approval code — never
//! card data.

use std::collections::HashSet;
use std::path::PathBuf;

/// Forbidden identifiers (lowercase, whole-token match). `pan` is
/// matched as a whole token only — `company` must not trip it.
const FORBIDDEN_TOKENS: &[&str] = &[
    "pan",
    "card_number",
    "cardnumber",
    "card_no",
    "cvv",
    "cvc",
    "card_cvv",
    "card_cvc",
    "card_expiry",
    "card_expiration",
    "cardholder",
    "track_data",
    "magstripe",
    "card_security_code",
];

fn migration_files() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files = Vec::new();
    for dialect in ["postgres", "sqlite"] {
        let dir = root.join(dialect);
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        entries.sort();
        assert!(
            !entries.is_empty(),
            "no migration files found in {}",
            dir.display()
        );
        files.extend(entries);
    }
    files
}

fn tokens(sql: &str) -> HashSet<String> {
    // Strip `--` comments first: the migrations document the guardrail
    // in their own headers ("No PAN/CVV columns"), which must not
    // trip the scan.
    let stripped: String = sql
        .lines()
        .map(|line| match line.find("--") {
            Some(idx) => &line[..idx],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n");
    stripped
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Column names declared in `CREATE TABLE <table> (...)`, lowercased.
fn table_columns(sql: &str, table: &str) -> Vec<String> {
    let lower = sql.to_lowercase();
    let marker = format!("create table {table} (");
    let Some(start) = lower.find(&marker) else {
        return Vec::new();
    };
    let body = &lower[start + marker.len()..];
    let Some(end) = body.find("\n);") else {
        return Vec::new();
    };
    body[..end]
        .lines()
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            let name = name.trim_matches(|c| c == ',' || c == '"');
            if name.is_empty()
                || name.starts_with("--")
                || ["primary", "foreign", "unique", "check", "constraint"].contains(&name)
            {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

#[test]
fn no_migration_contains_pan_or_cvv_columns() {
    let files = migration_files();
    assert!(
        files.len() >= 18,
        "expected both dialects' migrations, found {} files",
        files.len()
    );
    for file in &files {
        let sql = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        let found = tokens(&sql);
        for forbidden in FORBIDDEN_TOKENS {
            assert!(
                !found.contains(*forbidden),
                "{} contains forbidden PAN/CVV identifier `{forbidden}`",
                file.display()
            );
        }
    }
}

#[test]
fn payments_table_holds_no_card_data() {
    // `payments`: tender label + provider reference + approval code
    // only (spec §5.5, §8 guardrail 3).
    let allowed: HashSet<&str> = [
        "id",
        "transaction_id",
        "tender",
        "amount",
        "reference",
        "authorization",
        "created_at",
    ]
    .into_iter()
    .collect();
    for file in migration_files() {
        let sql = std::fs::read_to_string(&file).unwrap();
        for column in table_columns(&sql, "payments") {
            assert!(
                allowed.contains(column.as_str()),
                "{}: payments has unexpected column `{column}`",
                file.display()
            );
        }
    }
}

#[test]
fn portal_payments_table_holds_no_card_data() {
    // `portal_payments`: method label + provider reference only.
    let allowed: HashSet<&str> = [
        "idempotency_key",
        "customer_id",
        "payment_entry_id",
        "amount",
        "method",
        "provider_reference",
        "created_at",
    ]
    .into_iter()
    .collect();
    for file in migration_files() {
        let sql = std::fs::read_to_string(&file).unwrap();
        for column in table_columns(&sql, "portal_payments") {
            assert!(
                allowed.contains(column.as_str()),
                "{}: portal_payments has unexpected column `{column}`",
                file.display()
            );
        }
    }
}
