//! The data views store against a real PostgreSQL (API/01 §30, T-3104): the migration, the
//! per-person limit inside the insert, versioned updates and deletes.
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to (locally:
//! `docker run -e POSTGRES_PASSWORD=… postgres:17-alpine`); otherwise it says so and returns, like
//! the other database suites, so the fast lane without a service container stays green.

use joinedcontext_portal::ops::data_views::{
    CreateView, DataViewError, DataViewStore, Owner, UpdateView, ViewConfig, MAX_PER_PERSON,
};
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
    format!("views-{nanos}")
}

fn view(mode: &str) -> CreateView {
    serde_json::from_value(json!({
        "type": "BikeHireDockingStation", "kind": "kanban", "mode": mode, "title": "Board",
        "config": { "group": "status", "colour": [{ "when": "status==\"broken\"", "colour": "danger" }], "settings": { "lanes": ["working", "broken"] } }
    }))
    .expect("a create body")
}

const JANA: Owner<'static> = Owner {
    subject: "sub-jana",
    name: "jana",
};

#[tokio::test]
async fn views_round_trip_through_postgres() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let store = DataViewStore::new(Some(pool));
    let project = fresh_project();

    let saved = store
        .create(&project, "bikes", JANA, view("collaborative"))
        .await
        .expect("saved");
    assert_eq!(
        store
            .get(&project, "bikes", &saved.id)
            .await
            .expect("read back"),
        saved
    );
    assert_eq!(saved.config.settings["lanes"], json!(["working", "broken"]));
    assert_eq!(
        store.list(&project, "bikes").await.unwrap(),
        vec![saved.clone()]
    );
    assert!(store.list(&project, "trams").await.unwrap().is_empty());

    let update = |expected: Option<i64>| UpdateView {
        kind: "grid".into(),
        mode: "locked".into(),
        title: "Locked board".into(),
        config: ViewConfig::default(),
        expected_version: expected,
    };
    let changed = store
        .update(&project, "bikes", &saved.id, update(Some(1)))
        .await
        .expect("updated");
    assert_eq!(
        (
            changed.version,
            changed.mode.as_str(),
            changed.kind.as_str()
        ),
        (2, "locked", "grid")
    );
    assert_eq!(
        store
            .update(&project, "bikes", &saved.id, update(Some(1)))
            .await,
        Err(DataViewError::Conflict(2))
    );
    assert_eq!(
        store
            .update(&project, "bikes", &saved.id, update(None))
            .await
            .unwrap()
            .version,
        3
    );
    assert_eq!(
        store
            .update(&project, "trams", &saved.id, update(None))
            .await,
        Err(DataViewError::NotFound)
    );

    store
        .delete(&project, "bikes", &saved.id)
        .await
        .expect("deleted");
    assert_eq!(
        store.delete(&project, "bikes", &saved.id).await,
        Err(DataViewError::NotFound)
    );
}

#[tokio::test]
async fn the_per_person_limit_holds_in_the_insert_itself() {
    let Some(url) = database_url() else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let store = DataViewStore::new(Some(pool));
    let project = fresh_project();
    // Saved concurrently, as two windows would: the count and the insert are one statement.
    let saves =
        (0..MAX_PER_PERSON + 5).map(|_| store.create(&project, "bikes", JANA, view("personal")));
    let results = futures_util::future::join_all(saves).await;
    let saved = results.iter().filter(|r| r.is_ok()).count() as i64;
    assert!(saved <= MAX_PER_PERSON, "{saved} saved past the limit");
    assert!(results.iter().any(|r| r == &Err(DataViewError::Limit)));
    store
        .create(
            &project,
            "bikes",
            Owner {
                subject: "sub-eva",
                name: "eva",
            },
            view("personal"),
        )
        .await
        .expect("another person");
}
