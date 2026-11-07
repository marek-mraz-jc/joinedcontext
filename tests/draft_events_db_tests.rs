//! Draft events between two Portal replicas on one PostgreSQL (OPS-51, T-1485).
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to (locally:
//! `docker run -e POSTGRES_PASSWORD=… postgres:17-alpine`); otherwise it says so and returns, like
//! the other database suites, so the fast lane without a service container stays green.

use std::time::Duration;

use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;
use serde_json::json;

fn database_url() -> Option<String> {
    std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// A project nobody else's run writes to, so tests share one database without a fixture.
fn fresh_project() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("replicas-{nanos}")
}

/// Two replicas: each its own process state and its own pool, one database.
async fn replica(url: &str) -> AppState {
    let pool = joinedcontext_portal::db::connect(url)
        .await
        .expect("the test database answers and migrates");
    AppState::new(Config::for_tests(), None).with_db(pool)
}

/// OPS-51: a draft typed through replica A is an event on replica B's stream, once, and A's own
/// stream sees it once too — its own notification is not delivered a second time.
#[tokio::test]
async fn a_draft_put_on_one_replica_is_an_event_on_the_other() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let project = fresh_project();
    let a = replica(&url).await;
    let b = replica(&url).await;
    let mut on_a = a.draft_events.subscribe(&project).await;
    let mut on_b = b.draft_events.subscribe(&project).await;
    // The listener subscribes asynchronously; give it the moment a connection takes.
    tokio::time::sleep(Duration::from_millis(500)).await;

    a.drafts
        .put(
            &project,
            "Endpoint",
            "ep-1",
            json!({ "kind": "Endpoint", "metadata": { "name": "ep-1" } }),
            None,
            "steward",
            "person",
        )
        .await
        .expect("the draft is stored");

    let seen = tokio::time::timeout(Duration::from_secs(5), on_b.recv())
        .await
        .expect("replica B hears of it within five seconds")
        .expect("an event");
    assert_eq!(
        (seen.kind.as_str(), seen.name.as_str(), seen.version),
        ("Endpoint", "ep-1", 1)
    );
    assert_eq!(seen.event, "put");

    let own = on_a.recv().await.expect("replica A's own event");
    assert_eq!(own.name, "ep-1");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        on_a.try_recv().is_err(),
        "replica A delivered its own event a second time"
    );
    assert!(on_b.try_recv().is_err(), "replica B heard of it twice");
}
