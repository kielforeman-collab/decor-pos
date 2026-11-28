//! CLI integration test for `df-server tokens create|list|revoke`.

use std::process::Command;
use uuid::Uuid;

fn run_tokens(args: &[&str]) -> (i32, String, String) {
    let exe = env!("CARGO_BIN_EXE_df-server");
    let output = Command::new(exe)
        .arg("tokens")
        .args(args)
        .output()
        .expect("run df-server tokens");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn tokens_cli_create_list_revoke_round_trip() {
    let path = std::env::temp_dir().join(format!("decor-pos-tokens-cli-{}.db", std::process::id()));
    let db = format!("sqlite:{}", path.display());
    let _ = std::fs::remove_file(&path);

    let org_id = Uuid::now_v7().to_string();
    let store_id = Uuid::now_v7().to_string();

    // Create token
    let (code, stdout, stderr) = run_tokens(&[
        "create",
        "--db",
        &db,
        "--org-id",
        &org_id,
        "--store-id",
        &store_id,
        "--label",
        "store-1-edge",
        "--ttl-days",
        "90",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let created: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let raw_token = created["token"].as_str().unwrap();
    let id = created["id"].as_str().unwrap();
    assert!(raw_token.starts_with("dfs_"));
    assert_eq!(created["label"], "store-1-edge");
    assert_eq!(created["scopes"], serde_json::json!(["sync:ingest"]));

    // List tokens — must not expose raw token or token_hash
    let (code, stdout, stderr) = run_tokens(&[
        "list",
        "--db",
        &db,
        "--org-id",
        &org_id,
        "--store-id",
        &store_id,
    ]);
    assert_eq!(code, 0, "{stderr}");
    let listed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);
    assert!(arr[0].get("token").is_none());
    assert!(arr[0].get("token_hash").is_none());
    assert!(arr[0]["revoked_at"].is_null());

    // Revoke token
    let (code, stdout, stderr) = run_tokens(&["revoke", "--db", &db, "--id", id]);
    assert_eq!(code, 0, "{stderr}");
    let revoked: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(revoked["revoked"], true);

    // Second revoke exits 1 (already revoked)
    let (code, _, _) = run_tokens(&["revoke", "--db", &db, "--id", id]);
    assert_eq!(code, 1);

    let _ = std::fs::remove_file(&path);
}
