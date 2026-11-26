//! `Edge::ensure_configured_store`: a fresh in-memory edge has no
//! org/store rows; the ensure seeds them from the config, is a
//! no-op on the second call, and never overwrites existing details.

use df_domain::Currency;
use df_edge::{Edge, EdgeConfig};
use uuid::Uuid;

const STORE_ID: u128 = 0x1000;
const ORG_ID: u128 = 0x2000;

async fn fresh_edge() -> Edge {
    let config = EdgeConfig {
        store_id: Uuid::from_u128(STORE_ID),
        org_id: Uuid::from_u128(ORG_ID),
        db_url: "sqlite::memory:".to_string(),
        store_name: "Test Paint Co".to_string(),
        currency: Currency::Usd,
        surcharge_rules: EdgeConfig::default_surcharge_rules(Currency::Usd),
    };
    Edge::open(config).await.unwrap()
}

#[tokio::test]
async fn ensure_seeds_fresh_db_and_is_idempotent() {
    let edge = fresh_edge().await;
    // Nothing seeded by `Edge::open`.
    assert!(
        df_db::get_store(edge.pool(), Uuid::from_u128(STORE_ID))
            .await
            .is_err()
    );
    assert!(
        df_db::get_organization(edge.pool(), Uuid::from_u128(ORG_ID))
            .await
            .is_err()
    );

    // First call seeds both rows.
    assert!(edge.ensure_configured_store().await.unwrap());
    let store = df_db::get_store(edge.pool(), Uuid::from_u128(STORE_ID))
        .await
        .unwrap();
    assert_eq!(store.code, "MAIN");
    assert_eq!(store.name, "Test Paint Co");
    assert_eq!(store.org_id, Uuid::from_u128(ORG_ID));
    assert_eq!(store.currency, "USD");
    let org = df_db::get_organization(edge.pool(), Uuid::from_u128(ORG_ID))
        .await
        .unwrap();
    assert_eq!(org.default_currency, "USD");

    // Second call is a no-op and keeps the existing rows untouched.
    assert!(!edge.ensure_configured_store().await.unwrap());
    assert_eq!(
        df_db::get_store(edge.pool(), Uuid::from_u128(STORE_ID))
            .await
            .unwrap()
            .code,
        "MAIN"
    );
}
