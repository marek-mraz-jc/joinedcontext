//! API/01 §31, T-3107: the trash on PostgreSQL keeps each keeper's copies apart, newest first,
//! and forgets only the keeper's own. Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a
//! PostgreSQL the test may write to; otherwise it says so and returns, like the other database
//! suites.

use joinedcontext_portal::entity_trash::TrashStore;
use serde_json::json;

#[tokio::test]
async fn copies_are_kept_per_keeper_newest_first_and_forgotten_by_their_keeper_only() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let store = TrashStore::new(Some(pool));
    let project = format!(
        "trash-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let mine = (project.as_str(), "air", "f:1:sami");
    let theirs = (project.as_str(), "air", "f:1:anna");
    let entity = |n: u32| json!({ "id": format!("urn:ngsi-ld:T:x:{n}"), "type": "T" });

    let first = store
        .keep(mine, "urn:ngsi-ld:T:x:1", "T", &entity(1))
        .await
        .expect("kept");
    store
        .keep(mine, "urn:ngsi-ld:T:x:2", "T", &entity(2))
        .await
        .expect("kept");
    store
        .keep(theirs, "urn:ngsi-ld:T:x:3", "T", &entity(3))
        .await
        .expect("kept");

    let listed = store.list(mine).await.expect("listed");
    assert_eq!(
        listed
            .iter()
            .map(|item| item.urn.as_str())
            .collect::<Vec<_>>(),
        ["urn:ngsi-ld:T:x:2", "urn:ngsi-ld:T:x:1"]
    );
    assert_eq!(listed[1].entity, entity(1));
    assert!(!store.forget(theirs, first.id).await.expect("asked"));
    assert!(store.forget(mine, first.id).await.expect("asked"));
    assert_eq!(store.list(mine).await.expect("listed").len(), 1);
    assert_eq!(store.list(theirs).await.expect("listed").len(), 1);
}
