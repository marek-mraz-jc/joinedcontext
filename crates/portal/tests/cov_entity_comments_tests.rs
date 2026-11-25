//! Entity comments on a database: the per-entity limit, and a project with no tenancy key (T-7823).

mod common;

use joinedcontext_portal::entity_comments::{Author, CommentStore, NewComment, MAX_PER_ENTITY};

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

const WHO: Author<'static> = Author {
    id: "ann",
    name: "Ann",
    subject: "sub-ann",
};

#[tokio::test]
async fn an_entity_holding_the_most_comments_takes_no_more() {
    let pool = pool().await;
    let project = format!("cm-{}", common::unique::unique());
    let mirror = common::keyed_mirror(&pool, &[project.as_str()]).await;
    let store = CommentStore::new(Some(pool), mirror);
    let new = || NewComment {
        project: &project,
        space: "s",
        urn: "urn:ngsi-ld:T:1",
        text: "hi",
        mentions: &[],
    };
    for _ in 0..MAX_PER_ENTITY {
        assert!(store.add(WHO, new()).await.expect("added").is_some());
    }
    assert!(store.add(WHO, new()).await.expect("answered").is_none());
    let listed = store
        .list(&project, "s", "urn:ngsi-ld:T:1")
        .await
        .expect("listed");
    assert_eq!(listed.len() as i64, MAX_PER_ENTITY);
}

#[tokio::test]
async fn a_project_without_a_tenancy_key_lists_and_removes_nothing_and_adds_an_error() {
    let pool = pool().await;
    let project = format!("cm-{}", common::unique::unique());
    let mirror = common::keyed_mirror(&pool, &[]).await;
    let store = CommentStore::new(Some(pool), mirror);
    assert!(store
        .list(&project, "s", "urn:ngsi-ld:T:1")
        .await
        .expect("listed")
        .is_empty());
    assert!(!store
        .remove(&project, "s", "sub-ann", 1)
        .await
        .expect("removed"));
    let refused = store
        .add(
            WHO,
            NewComment {
                project: &project,
                space: "s",
                urn: "urn:ngsi-ld:T:1",
                text: "hi",
                mentions: &[],
            },
        )
        .await;
    assert!(matches!(refused, Err(sqlx::Error::RowNotFound)));
}
