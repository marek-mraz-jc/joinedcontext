#![forbid(unsafe_code)]
//! Error paths and less travelled methods of both stores: WebAuthn credential update, list and
//! organization-scoped delete (K11), identity-provider links (K12), outbox backoff without a
//! given retry time (K9), sessions removed with their person, the K entity sets over OData, and
//! the PostgreSQL boot and migration refusals. Runs against `MemoryStore` always and `PgStore`
//! with `KAISATSU_TEST_DATABASE_URL`.

use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use kaisatsu_store_pg::{PgStore, verify_boot_rls};
use std::sync::Arc;

mod common;
#[path = "../../store-memory/tests/error_cases/mod.rs"]
mod error_cases;
use error_cases::*;

#[tokio::test]
async fn pg_store_error_paths() {
    let Some(db) = common::test_db("pg_store_error_paths").await else {
        return;
    };
    let store = db.store(Kek::generate_for_tests()).await;
    // Signing keys written directly are listed back (rotation path).
    let kek = Kek::generate_for_tests();
    store
        .upsert_realm(RealmConfig {
            id: "keys".into(),
            realm: "keys".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let key = kaisatsu_crypto::KeyManager::generate_es256("keys", &kek, 100, "ACTIVE").unwrap();
    store.upsert_signing_key(key.clone()).await.unwrap();
    assert!(
        store
            .list_signing_keys("keys")
            .await
            .unwrap()
            .iter()
            .any(|k| k.kid == key.kid)
    );
    contract(Arc::new(store)).await;
    db.drop_db().await;
}

#[tokio::test]
async fn pg_boot_check_and_migrations_refuse_a_dead_or_unprivileged_pool() {
    let Some(db) = common::test_db("pg_boot_check_dead_pool").await else {
        return;
    };
    // A closed pool fails the first boot query with a storage error, not a panic.
    let app = db.app_pool().await;
    app.close().await;
    let err = verify_boot_rls(&app)
        .await
        .expect_err("closed pool refused");
    assert!(matches!(err, KaisatsuError::Storage(_)), "{err}");
    // The request role may not migrate (no CREATE on the database).
    let app = db.app_pool().await;
    let err = PgStore::run_migrations(&app)
        .await
        .expect_err("kaisatsu_app cannot migrate");
    assert!(err.to_string().contains("migration failed"), "{err}");
    app.close().await;
    db.drop_db().await;
}
