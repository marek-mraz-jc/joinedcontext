//! The activity trim over PostgreSQL for a project with nothing past the retention window.

use jc_time::TimestampTz;
use joinedcontext_portal::activity::ActivityStore;

mod common;
use common::unique::unique;

/// A keyed project whose events are all inside the window: its one batch deletes nothing and
/// counts no batch.
#[tokio::test]
async fn a_project_with_nothing_past_the_window_trims_nothing() {
    let Ok(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL") else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    let project = format!("act-{}", unique());
    let mirror = common::keyed_mirror(&pool, &[&project]).await;
    let store = ActivityStore::new(Some(pool), mirror);
    let trimmed = store.trim(TimestampTz::now()).await.expect("trimmed");
    assert_eq!((trimmed.rows, trimmed.batches), (0, 0));
}
