#![forbid(unsafe_code)]
//! Deletes take their grants along (S3-15) and membership is a row (S3-16), in both stores.

use kaisatsu_core::*;
use kaisatsu_crypto::Kek;
use std::sync::Arc;

#[path = "../../store-memory/tests/cascade_cases/mod.rs"]
mod cascade_cases;
mod common;
use cascade_cases::*;

macro_rules! postgres_store {
    ($($case:ident),* $(,)?) => {
        mod postgres {
            use super::*;
            $(#[tokio::test]
            async fn $case() {
                let Some(db) = common::test_db(stringify!($case)).await else {
                    return;
                };
                let store = Arc::new(db.store(Kek::generate_for_tests()).await);
                realm(store.as_ref()).await;
                super::$case(store.as_ref()).await;
                drop(store);
                db.drop_db().await;
            })*
        }
    };
}

postgres_store!(
    deleted_realm_role,
    deleted_client,
    deleted_group,
    deleted_person,
    stale_profile_write,
    named_path_without_membership,
    deleted_client_role,
    absent_grants_remove_cleanly,
    documents_lose_what_is_deleted,
    renamed_client,
    deleted_service_account,
    edited_group,
    subgroup_of_another_organization,
    grants_to_the_absent,
);

/// Two replicas: grant writes and a delete of the role at once leave no grant behind.
#[tokio::test]
async fn a_delete_racing_grant_writes_leaves_no_document_grant() {
    let Some(db) = common::test_db("delete_race").await else {
        return;
    };
    let store = Arc::new(db.store(Kek::generate_for_tests()).await);
    realm(store.as_ref()).await;
    role(store.as_ref(), None, "raced").await;
    for n in 0..24 {
        person(store.as_ref(), &format!("race{n}")).await;
        grant(store.as_ref(), "user", &format!("race{n}"), None, "raced").await;
    }
    let other = Arc::new(db.store(Kek::generate_for_tests()).await);
    let mut writes = tokio::task::JoinSet::new();
    for n in 0..24 {
        let store = other.clone();
        writes.spawn(async move {
            let mut u = get(store.as_ref(), &format!("race{n}")).await;
            u.first_name = Some("edited".into());
            store.update_user(u).await.expect("profile write");
        });
    }
    assert!(store.delete_role(R, None, "raced").await.expect("delete"));
    while let Some(done) = writes.join_next().await {
        done.expect("writer");
    }
    for n in 0..24 {
        let u = get(store.as_ref(), &format!("race{n}")).await;
        assert!(u.realm_roles.is_empty(), "race{n} kept the deleted role");
    }
    drop((store, other));
    db.drop_db().await;
}
