//! The trash on a database: a project with no tenancy key keeps and forgets nothing, and a
//! keeper reads only their own copies (T-7823).

mod common;

use std::sync::Arc;

use joinedcontext_portal::entity_trash::TrashStore;
use serde_json::json;

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

#[tokio::test]
async fn a_project_without_a_tenancy_key_keeps_lists_and_forgets_nothing() {
    let pool = pool().await;
    let project = format!("tr-{}", common::unique::unique());
    let mirror = common::keyed_mirror(&pool, &[]).await;
    let store = TrashStore::new(Some(pool), mirror);
    let keeper = (project.as_str(), "s", "ann");
    let kept = store
        .keep(keeper, "urn:ngsi-ld:T:1", "T", &json!({}))
        .await
        .expect("answered");
    assert!(kept.is_none());
    assert!(store.list(keeper).await.expect("listed").is_empty());
    assert!(!store.forget(keeper, 1).await.expect("answered"));
}

#[tokio::test]
async fn a_keeper_forgets_only_their_own_copy() {
    let pool = pool().await;
    let project = format!("tr-{}", common::unique::unique());
    let mirror = common::keyed_mirror(&pool, &[project.as_str()]).await;
    let store = TrashStore::new(Some(pool), Arc::clone(&mirror));
    let ann = (project.as_str(), "s", "ann");
    let bob = (project.as_str(), "s", "bob");
    let kept = store
        .keep(ann, "urn:ngsi-ld:T:1", "T", &json!({ "a": 1 }))
        .await
        .expect("kept")
        .expect("a copy");
    assert_eq!(store.list(ann).await.expect("listed").len(), 1);
    assert!(store.list(bob).await.expect("listed").is_empty());
    assert!(!store.forget(bob, kept.id).await.expect("not bob's"));
    assert!(store.forget(ann, kept.id).await.expect("forgotten"));
    assert!(store.list(ann).await.expect("listed").is_empty());
}
