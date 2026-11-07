//! The stream hash of a logged run as PostgreSQL keeps it (T-5825): a restarted Portal reads it
//! back to run a replace pipeline whose stream changed while no Portal held it.
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; otherwise
//! it says so and returns, like the other database suites.

use joinedcontext_portal::pipeline_log::LogStore;

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
    format!("p-{nanos}")
}

#[tokio::test]
async fn a_logged_run_keeps_its_stream_hash_and_the_newest_run_names_it() {
    let Some(url) = database_url() else {
        eprintln!("JC_PORTAL_TEST_DATABASE_URL is not set: the run stream hash's database half is not run");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let log = LogStore::new(Some(pool));
    let project = fresh_project();
    let p = project.as_str();
    let key = (project.clone(), "trees".to_owned());
    // A run with no row keeps nothing.
    log.ran_stream(p, "trees", "r0", "never")
        .await
        .expect("keep");
    assert!(!log.ran_streams().await.expect("read").contains_key(&key));
    // A run logged before the hash existed reads as unknown.
    log.append(p, "trees", "r1", &[]).await.expect("append");
    assert!(!log.ran_streams().await.expect("read").contains_key(&key));
    log.ran_stream(p, "trees", "r1", "h1").await.expect("keep");
    assert_eq!(log.ran_streams().await.expect("read")[&key], "h1");
    // The newest run counts: one logged after it without a hash hides the older hash.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    log.append(p, "trees", "r2", &[]).await.expect("append");
    assert!(!log.ran_streams().await.expect("read").contains_key(&key));
    log.ran_stream(p, "trees", "r2", "h2").await.expect("keep");
    assert_eq!(log.ran_streams().await.expect("read")[&key], "h2");
}
