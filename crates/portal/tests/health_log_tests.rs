//! The health log on Postgres (T-5067): the same rules the in-memory store's unit tests hold,
//! since kept while a status holds, last_ok, recovery, the hourly sampling and the 90-day bound.
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; skips
//! otherwise.

use chrono::{DateTime, Duration, Utc};
use joinedcontext_portal::health_log::{Finding, HealthStore, Kind, Status, Text, KEEP};

fn finding(name: &str, status: Status) -> Finding {
    Finding::new("t5067", Kind::Pipeline, name, "runs").judged(
        status,
        Text::raw("403 no policy grants this operation"),
        Text::say("policyWrite", &[("name", name.to_owned())]),
    )
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().expect("a time")
}

#[tokio::test]
async fn the_postgres_log_keeps_since_recovers_and_bounds_its_history() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("connect + migrate");
    for clean in [
        "DELETE FROM health_check WHERE project = 't5067'",
        "DELETE FROM health_check_history WHERE project = 't5067'",
    ] {
        sqlx::query(clean).execute(&pool).await.expect("clean");
    }
    let store = HealthStore::new(Some(pool));
    let t0 = at("2026-07-01T08:00:00Z");
    let run = |now, status| {
        store.record(
            now,
            vec![finding("events", status), finding("gone", Status::Ok)],
        )
    };
    run(t0, Status::Ok).await.expect("record");
    let t1 = t0 + Duration::minutes(2);
    run(t1, Status::Error).await.expect("record");
    let t2 = t1 + Duration::minutes(2);
    store
        .record(t2, vec![finding("events", Status::Error)])
        .await
        .expect("record");
    let list = store.list(Some("t5067"), t2).await.expect("list");
    assert_eq!(list.len(), 1, "a check no longer found is gone: {list:?}");
    let held = &list[0];
    assert_eq!(held.status, Status::Error);
    assert_eq!(held.since, "2026-07-01T08:02:00Z");
    assert_eq!(held.last_ok.as_deref(), Some("2026-07-01T08:00:00Z"));
    assert_eq!(
        held.history.len(),
        2,
        "one point per change: {:?}",
        held.history
    );
    assert!(held.detail.text.contains("403"));
    assert_eq!(
        held.fix.code, "policyWrite",
        "the key the UI translates survives the round trip"
    );
    assert_eq!(
        held.fix.args.get("name").map(String::as_str),
        Some("events")
    );

    let t3 = t2 + Duration::minutes(2);
    store
        .record(t3, vec![finding("events", Status::Ok)])
        .await
        .expect("record");
    let recovered = &store.list(None, t3).await.expect("list")[0];
    assert_eq!(
        (recovered.status, recovered.since.as_str()),
        (Status::Ok, "2026-07-01T08:06:00Z")
    );
    assert_eq!(recovered.history.len(), 3);

    let later = t0 + KEEP + Duration::days(1);
    store
        .record(later, vec![finding("events", Status::Ok)])
        .await
        .expect("record");
    let bounded = &store.list(None, later).await.expect("list")[0];
    assert_eq!(bounded.history.len(), 1, "older than 90 days is gone");
    assert!(store
        .list(Some("other"), later)
        .await
        .expect("list")
        .is_empty());
}
