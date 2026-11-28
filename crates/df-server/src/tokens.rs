//! Store service token administration CLI (`df-server tokens ...`).
//!
//! Issues, lists, and revokes long-lived, store-scoped `dfs_` bearer
//! tokens for unattended edge -> HQ sync daemons (migration 0016).
//! The raw token is printed once at creation and is never persisted;
//! `tokens list` prints metadata only (never `token_hash`).

use chrono::Duration;
use df_auth::SCOPE_SYNC_INGEST;
use serde_json::json;
use uuid::Uuid;

use crate::db::{open_migrated_pool, print_json};

pub(crate) async fn run_tokens_create(
    db: String,
    org_id: Uuid,
    store_id: Uuid,
    label: String,
    ttl_days: Option<i64>,
) -> ! {
    let ttl = match ttl_days {
        Some(days) if days <= 0 => {
            eprintln!("df-server tokens create: --ttl-days must be a positive integer");
            std::process::exit(2);
        }
        Some(days) => Some(Duration::days(days)),
        None => None,
    };
    let pool = open_migrated_pool(&db).await;
    match df_db::create_service_token(
        &pool,
        org_id,
        store_id,
        &label,
        vec![SCOPE_SYNC_INGEST.to_string()],
        ttl,
    )
    .await
    {
        Ok((raw, row)) => {
            eprintln!(
                "df-server tokens create: SAVE THIS TOKEN NOW — it is stored only as a SHA-256 hash and cannot be retrieved again."
            );
            print_json(json!({
                "token": raw,
                "id": row.id,
                "org_id": row.org_id,
                "store_id": row.store_id,
                "label": row.label,
                "scopes": row.scopes,
                "created_at": row.created_at.to_rfc3339(),
                "expires_at": row.expires_at.map(|d| d.to_rfc3339()),
            }));
        }
        Err(err) => {
            eprintln!("df-server tokens create: {err}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn run_tokens_list(db: String, org_id: Uuid, store_id: Option<Uuid>) -> ! {
    let pool = open_migrated_pool(&db).await;
    match df_db::list_service_tokens(&pool, org_id, store_id).await {
        Ok(rows) => {
            let summaries: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "org_id": r.org_id,
                        "store_id": r.store_id,
                        "label": r.label,
                        "scopes": r.scopes,
                        "created_at": r.created_at.to_rfc3339(),
                        "expires_at": r.expires_at.map(|d| d.to_rfc3339()),
                        "revoked_at": r.revoked_at.map(|d| d.to_rfc3339()),
                        "last_used_at": r.last_used_at.map(|d| d.to_rfc3339()),
                    })
                })
                .collect();
            print_json(serde_json::Value::Array(summaries));
        }
        Err(err) => {
            eprintln!("df-server tokens list: {err}");
            std::process::exit(1);
        }
    }
}

pub(crate) async fn run_tokens_revoke(db: String, id: Uuid) -> ! {
    let pool = open_migrated_pool(&db).await;
    match df_db::revoke_service_token(&pool, id).await {
        Ok(true) => print_json(json!({ "id": id, "revoked": true })),
        Ok(false) => {
            eprintln!("df-server tokens revoke: active service token {id} not found");
            std::process::exit(1);
        }
        Err(err) => {
            eprintln!("df-server tokens revoke: {err}");
            std::process::exit(1);
        }
    }
}
