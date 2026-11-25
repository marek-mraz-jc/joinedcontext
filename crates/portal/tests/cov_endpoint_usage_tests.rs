//! The usage store on a database: rows are kept per project key, an unplaced project keeps none,
//! old days go, and a dead database answers an error (T-7823).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use jiff::civil::date;
use joinedcontext_portal::endpoint_usage::UsageStore;

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .expect("set JC_PORTAL_TEST_DATABASE_URL to a PostgreSQL database the Portal migrates");
    joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate")
}

fn row(
    project: &str,
    endpoint: &str,
    day: jiff::civil::Date,
    calls: u64,
) -> ((String, String, String, jiff::civil::Date, String), u64) {
    (
        (
            project.to_owned(),
            endpoint.to_owned(),
            "ngsi-ld".to_owned(),
            day,
            "agent".to_owned(),
        ),
        calls,
    )
}

#[tokio::test]
async fn calls_of_a_keyed_project_are_summed_and_read_back_per_project() {
    let pool = pool().await;
    let project = format!("use-{}", common::unique::unique());
    let other = format!("use-{}", common::unique::unique());
    let mirror = common::keyed_mirror(&pool, &[project.as_str(), other.as_str()]).await;
    let store = UsageStore::new(Some(pool), Arc::clone(&mirror));
    let today = date(2026, 10, 22);
    store
        .add(
            BTreeMap::from([
                row(&project, "air", today, 3),
                row(&other, "air", today, 100),
            ]),
            today,
        )
        .await
        .expect("kept");
    store
        .add(BTreeMap::from([row(&project, "air", today, 4)]), today)
        .await
        .expect("kept again");
    let read = store.read(&project, today, today).await.expect("read");
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].name, "air");
    assert_eq!(read[0].calls, 7);
    assert_eq!(read[0].caller, "agent");
    assert_eq!(read[0].day, today);
    let outside = store
        .read(&project, date(2026, 10, 23), date(2026, 10, 24))
        .await
        .expect("read");
    assert!(outside.is_empty());
}

#[tokio::test]
async fn days_older_than_the_keep_window_are_dropped_on_the_next_add() {
    let pool = pool().await;
    let project = format!("use-{}", common::unique::unique());
    // The test database's owner is not bound by row-level security, so a parallel add prunes this
    // old day too: only its absence after the next add is asserted.
    let state = joinedcontext_portal::state::AppState::new(
        joinedcontext_portal::config::Config::for_tests(),
        None,
    );
    let organization = format!("org-{}", common::unique::unique());
    common::keyed_project(&state, &pool, &project, &organization).await;
    let store = UsageStore::new(Some(pool), state.mirror.clone());
    let old = date(2020, 1, 1);
    store
        .add(BTreeMap::from([row(&project, "air", old, 1)]), old)
        .await
        .expect("kept");
    let today = date(2026, 10, 22);
    store
        .add(BTreeMap::from([row(&project, "air", today, 1)]), today)
        .await
        .expect("kept");
    assert!(store
        .read(&project, old, old)
        .await
        .expect("read")
        .is_empty());
}

#[tokio::test]
async fn a_project_without_a_tenancy_key_keeps_and_reads_nothing() {
    let pool = pool().await;
    let unplaced = format!("use-{}", common::unique::unique());
    let mirror = common::keyed_mirror(&pool, &[]).await;
    let store = UsageStore::new(Some(pool), mirror);
    let today = date(2026, 10, 22);
    store
        .add(BTreeMap::from([row(&unplaced, "air", today, 5)]), today)
        .await
        .expect("an unplaced project is skipped, not refused");
    assert!(store
        .read(&unplaced, today, today)
        .await
        .expect("read")
        .is_empty());
}

#[tokio::test]
async fn a_dead_database_answers_an_error_to_add_and_read() {
    let dead = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(50))
        .connect_lazy("postgres://nobody@127.0.0.1:1/none")
        .expect("a lazy pool");
    let mirror = Arc::new(joinedcontext_portal::store::Mirror::new());
    mirror.upsert(common::envelope(
        "Project",
        "p",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        serde_json::json!({ "organizationRef": "helsinki" }),
    ));
    let store = UsageStore::new(Some(dead), mirror);
    let today = date(2026, 10, 22);
    assert!(store
        .add(BTreeMap::from([row("p", "air", today, 1)]), today)
        .await
        .is_err());
    assert!(store.read("p", today, today).await.is_err());
}
