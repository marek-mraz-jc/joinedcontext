#![forbid(unsafe_code)]
//! The memory store's boot check (R8) and its Organization directory, the reference the Postgres
//! store's organizations tests are held to: a role with BYPASSRLS or a table without forced RLS
//! refuses boot, a name or a number is held by one live Organization, and a retired name is free.

use daiya_model::{DaiyaStore, InstallationId, OrgId};
use daiya_store_memory::MemoryStore;

const INSTALLATION: InstallationId = InstallationId::from_u128(1);

#[tokio::test]
async fn boot_refuses_a_role_with_bypassrls_until_it_is_dropped() {
    let store = MemoryStore::new();
    store
        .verify_rls_enforced()
        .await
        .expect("a fresh store boots");

    store.simulate_bypassrls_role(true);
    let err = store.verify_rls_enforced().await.unwrap_err();
    assert_eq!(err.slug(), "cr.rls_not_enforced");
    assert!(err.to_string().contains("BYPASSRLS"), "{err}");

    store.simulate_bypassrls_role(false);
    store
        .verify_rls_enforced()
        .await
        .expect("boots again without it");

    store.simulate_table_without_forced_rls("cr_dashboard");
    let err = store.verify_rls_enforced().await.unwrap_err();
    assert!(err.to_string().contains("'cr_dashboard'"), "{err}");
}

#[tokio::test]
async fn a_name_or_number_is_held_by_one_live_organization() {
    let store = MemoryStore::new();
    let (first, second) = (OrgId::from_u128(0xa1), OrgId::from_u128(0xb2));
    let helsinki = store
        .create_org(INSTALLATION, "helsinki", Some(4), first)
        .await
        .unwrap();
    assert_eq!((helsinki.number, helsinki.org_id), (4, first));

    // The same name at its own number answers the row it holds (a repeated import).
    let again = store
        .create_org(INSTALLATION, "helsinki", Some(4), second)
        .await
        .unwrap();
    assert_eq!(again, helsinki);

    let conflict = |name: &'static str, number, id| {
        let store = &store;
        async move {
            store
                .create_org(INSTALLATION, name, number, id)
                .await
                .unwrap_err()
                .status_code()
        }
    };
    assert_eq!(
        conflict("zilina", Some(4), second).await,
        409,
        "a held number"
    );
    assert_eq!(conflict("helsinki", None, second).await, 409, "a live name");
    assert_eq!(
        conflict("zilina", None, first).await,
        409,
        "an id another name holds"
    );

    let zilina = store
        .create_org(INSTALLATION, "zilina", None, second)
        .await
        .unwrap();
    assert_eq!(zilina.number, 5, "the next number after the highest");

    assert!(store.retire_org(INSTALLATION, 4).await.unwrap());
    assert!(
        !store.retire_org(INSTALLATION, 4).await.unwrap(),
        "retired once"
    );
    assert!(
        !store.retire_org(INSTALLATION, 99).await.unwrap(),
        "never held"
    );
    assert_eq!(
        conflict("helsinki", Some(4), second).await,
        409,
        "a retired number is never given again"
    );

    // A retired name is free, under a new id: the old id stays with the retired row.
    let reborn = store
        .create_org(INSTALLATION, "helsinki", None, first)
        .await
        .unwrap();
    assert_eq!(reborn.number, 6);
    assert_ne!(reborn.org_id, first);
    assert!(!reborn.retired);
}
