//! The pipeline templates store against a real PostgreSQL (API/01 §42, T-3976): the migration,
//! the per-project limit inside the insert, newest first, and a delete that stays in its project.
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; otherwise
//! it says so and returns, like the other database suites.

use joinedcontext_portal::ops::pipeline_templates::{
    CreateTemplate, TemplateError, TemplateStore, MAX_PER_PROJECT,
};
use serde_json::json;

fn template(title: &str) -> CreateTemplate {
    CreateTemplate {
        title: title.into(),
        manifest: json!({
            "kind": "Pipeline",
            "spec": { "class": "batch", "secretRefs": [{ "name": "db", "key": "password" }] }
        }),
    }
}

#[tokio::test]
async fn templates_round_trip_through_postgres() {
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
    let store = TemplateStore::new(Some(pool));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let (project, other) = (format!("tpl-{nanos}"), format!("tpl-other-{nanos}"));

    for i in 0..MAX_PER_PROJECT {
        store
            .create(&project, "jana", template(&format!("t{i}")))
            .await
            .expect("saved");
    }
    assert!(matches!(
        store.create(&project, "jana", template("one more")).await,
        Err(TemplateError::Limit)
    ));
    let listed = store.list(&project).await.expect("listed");
    assert_eq!(listed.len(), MAX_PER_PROJECT);
    assert_eq!(listed[0].title, format!("t{}", MAX_PER_PROJECT - 1));
    assert_eq!(listed[0].spec, json!({ "class": "batch" }));
    assert!(store.list(&other).await.expect("listed").is_empty());
    assert!(matches!(
        store.delete(&other, &listed[0].id).await,
        Err(TemplateError::NotFound)
    ));
    store
        .delete(&project, &listed[0].id)
        .await
        .expect("deleted");
    assert_eq!(
        store.list(&project).await.expect("listed").len(),
        MAX_PER_PROJECT - 1
    );
}
