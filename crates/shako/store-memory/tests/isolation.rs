#![forbid(unsafe_code)]
//! Runs the shared multi-tenant store isolation and boot-check test suite against `MemoryStore` (S2).

use shako_core::ids::{InstanceId, OrgId};
use shako_core::model::InstanceRecord;
use shako_core::testkit::{run_contract_suite, run_isolation_suite};
use shako_core::{ShakoError, Store};
use shako_store_memory::MemoryStore;

#[tokio::test]
async fn isolation_suite_and_boot_rls_refusal_pass_on_memory_store() {
    let store = MemoryStore::new();
    run_isolation_suite(&store).await;
    run_contract_suite(&store).await;

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

#[tokio::test]
async fn a_table_marked_unreadable_fails_its_reads_until_cleared() {
    let store = MemoryStore::new();
    let (inst, org) = (InstanceId::new(), OrgId::new());
    store
        .upsert_instance(InstanceRecord::default_for(inst, org, "git.city.example"))
        .await
        .expect("instance");
    store.simulate_read_failure("instances", true);
    let err = store.get_instance(inst, org).await.expect_err("unreadable");
    assert!(
        matches!(&err, ShakoError::Internal(m) if m == "instances cannot be read"),
        "{err:?}"
    );
    store.simulate_read_failure("instances", false);
    let found = store.get_instance(inst, org).await.expect("readable again");
    assert_eq!(found.map(|i| i.org_id), Some(org));
}
