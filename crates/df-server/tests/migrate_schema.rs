//! `df-server migrate schema` applies the migrations to a fresh
//! SQLite file and is a no-op on the second run (which also proves
//! the `DATABASE_URL` fallback: the second run passes no `--db`).

use std::process::Command;

fn run_schema(db: &str, use_env: bool) -> (i32, String, String) {
    let exe = env!("CARGO_BIN_EXE_df-server");
    let mut cmd = Command::new(exe);
    cmd.args(["migrate", "schema"]);
    if use_env {
        cmd.env("DATABASE_URL", db);
    } else {
        cmd.args(["--db", db]);
    }
    let output = cmd.output().expect("run df-server migrate schema");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn migrate_schema_applies_once_then_reports_up_to_date() {
    let path = std::env::temp_dir().join(format!(
        "decor-pos-migrate-schema-{}.db",
        std::process::id()
    ));
    let db = format!("sqlite:{}", path.display());
    let _ = std::fs::remove_file(&path);
    let (code, stdout, stderr) = run_schema(&db, false);
    assert_eq!(code, 0, "{stderr}");
    let first: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(first["status"], "migrated");
    assert_eq!(first["dialect"], "sqlite");
    assert_eq!(
        first["newly_applied"],
        serde_json::json!([
            1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19
        ])
    );
    // Second run, this time via DATABASE_URL: no changes, exit 0.
    let (code, stdout, stderr) = run_schema(&db, true);
    assert_eq!(code, 0, "{stderr}");
    let second: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(second["status"], "already_up_to_date");
    assert_eq!(second["newly_applied"], serde_json::json!([]));
    assert_eq!(second["applied"], first["applied"]);
    let _ = std::fs::remove_file(&path);
}
