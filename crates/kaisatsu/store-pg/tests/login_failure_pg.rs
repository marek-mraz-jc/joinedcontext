#![forbid(unsafe_code)]
//! Concurrent failed sign-ins against `PgStore`: every failure counts, so a burst of parallel
//! guesses meets the same lockout as sequential ones (W7). Needs `KAISATSU_TEST_DATABASE_URL`.

use kaisatsu_core::{RealmConfig, Store};
use kaisatsu_crypto::Kek;
use kaisatsu_oidc::brute_force::BruteForceProtector;
use std::sync::Arc;

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn parallel_failures_are_each_counted() {
    let Some(db) = common::test_db("parallel_failures_are_each_counted").await else {
        return;
    };
    let store: Arc<dyn Store> = Arc::new(db.store(Kek::generate_for_tests()).await);
    let realm = RealmConfig {
        id: "bf".into(),
        realm: "bf".into(),
        quick_login_check_milli_seconds: 0,
        failure_factor: 1000,
        ..RealmConfig::default()
    };
    store.upsert_realm(realm.clone()).await.expect("realm");
    let bf = Arc::new(BruteForceProtector::new(store.clone()));
    let tasks: Vec<_> = (0..24)
        .map(|_| {
            let (bf, realm) = (bf.clone(), realm.clone());
            tokio::spawn(async move { bf.record_failure(&realm, Some("u1"), None).await })
        })
        .collect();
    for t in tasks {
        t.await.expect("join").expect("record");
    }
    let rec = store
        .get_login_failure("bf", "user:u1")
        .await
        .expect("get")
        .expect("record stored");
    assert_eq!(rec.num_failures, 24);
    db.drop_db().await;
}
