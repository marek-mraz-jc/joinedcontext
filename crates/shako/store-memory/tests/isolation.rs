#![forbid(unsafe_code)]
//! Runs the shared multi-tenant store isolation and boot-check test suite against `MemoryStore` (S2).

use shako_core::Store;
use shako_core::testkit::run_isolation_suite;
use shako_store_memory::MemoryStore;

#[tokio::test]
async fn isolation_suite_and_boot_rls_refusal_pass_on_memory_store() {
    let store = MemoryStore::new();
    run_isolation_suite(&store).await;

    // Boot check refuses when a table lacks forced RLS
    let bad_table_store = MemoryStore::new();
    bad_table_store.simulate_table_without_forced_rls("shako.rogue_table");
    let err = bad_table_store
        .verify_rls_enforced()
        .await
        .expect_err("must refuse table without forced RLS");
    assert_eq!(err.slug(), "shako.rls_not_enforced");

    // Boot check refuses when request role has BYPASSRLS
    let bypass_store = MemoryStore::new();
    bypass_store.simulate_bypassrls_role(true);
    let err = bypass_store
        .verify_rls_enforced()
        .await
        .expect_err("must refuse BYPASSRLS role");
    assert_eq!(err.slug(), "shako.rls_not_enforced");
}
