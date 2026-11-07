//! The source ledger of a snapshot pipeline as PostgreSQL keeps it (PL-97, PL-100, T-4891).
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; otherwise
//! it says so and returns, like the other database suites.

use joinedcontext_portal::pipeline_source_deletes::{Cause, Hold, HoldReason, Ledger};

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
async fn the_ledger_keeps_ids_misses_and_the_hold_per_pipeline() {
    let Some(url) = database_url() else {
        eprintln!("JC_PORTAL_TEST_DATABASE_URL is not set: the ledger's database half is not run");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let ledger = Ledger::new(Some(pool));
    let project = fresh_project();
    let p = project.as_str();
    let ids = ["urn:a", "urn:b", "not an id"].map(str::to_owned);
    ledger.seen(p, "x", "r1", &ids, true).await.expect("seen");
    // A record the model refused keeps a known row alive and adds no new one.
    let refused = ["urn:a", "urn:never-written"].map(str::to_owned);
    ledger
        .seen(p, "x", "r2", &refused, false)
        .await
        .expect("seen");
    let (rows, hold) = ledger.book(p, "x").await.expect("book");
    assert_eq!(rows.keys().collect::<Vec<_>>(), ["urn:a", "urn:b"]);
    assert_eq!(rows["urn:a"].last_run, "r2");
    assert_eq!(rows["urn:b"].last_run, "r1");
    assert!(hold.is_none());

    let held = Hold {
        run: "r2".into(),
        reason: HoldReason::ShareExceeded,
        would: 1,
        ledger: 2,
    };
    ledger
        .settle(p, "x", &[("urn:b".into(), 2)], Some(&held))
        .await
        .expect("settle");
    let (rows, hold) = ledger.book(p, "x").await.expect("book");
    assert_eq!(rows["urn:b"].misses, 2);
    assert_eq!(hold, Some(held));
    assert!(ledger
        .holds()
        .await
        .expect("holds")
        .iter()
        .any(|(project, pipeline, _)| project == p && pipeline == "x"));

    ledger.keep(p, "x").await.expect("keep");
    let (rows, hold) = ledger.book(p, "x").await.expect("book");
    assert!(rows.values().all(|row| row.misses == 0));
    assert!(hold.is_none());

    ledger
        .forget(p, "x", &["urn:a".into()])
        .await
        .expect("forget");
    assert_eq!(ledger.book(p, "x").await.expect("book").0.len(), 1);
    assert!(ledger.book(p, "y").await.expect("book").0.is_empty());
}

/// T-4892: the last complete run and the removals, per pipeline and per space, newest first.
#[tokio::test]
async fn the_report_keeps_the_last_complete_run_and_the_removals() {
    use joinedcontext_portal::pipeline_source_deletes::RemovalsOf;
    let Some(url) = database_url() else {
        eprintln!("JC_PORTAL_TEST_DATABASE_URL is not set: the report's database half is not run");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let ledger = Ledger::new(Some(pool));
    let project = fresh_project();
    let p = project.as_str();
    assert!(ledger.last_complete(p, "x").await.expect("read").is_none());
    ledger.completed(p, "x", "r1").await.expect("complete");
    ledger.completed(p, "x", "r2").await.expect("complete");
    let (run, at) = ledger
        .last_complete(p, "x")
        .await
        .expect("read")
        .expect("run");
    assert_eq!(run, "r2");
    assert!(at.ends_with('Z'), "{at}");

    let ids = ["urn:a", "urn:b"].map(str::to_owned);
    ledger
        .removed(p, "open", "x", ("r2", Cause::Snapshot), &ids)
        .await
        .expect("removed");
    ledger
        .removed(p, "other", "y", ("r7", Cause::Event), &["urn:c".to_owned()])
        .await
        .expect("removed");
    let of_x = ledger
        .removals(p, RemovalsOf::Pipeline("x"), 10)
        .await
        .expect("removals");
    assert_eq!(
        of_x.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["urn:a", "urn:b"]
    );
    assert_eq!(of_x[0].reason, "source-snapshot: pipeline x run r2");
    let in_other = ledger
        .removals(p, RemovalsOf::Space("other"), 10)
        .await
        .expect("removals");
    assert_eq!(in_other.len(), 1);
    assert_eq!(
        (in_other[0].pipeline.as_str(), in_other[0].run.as_str()),
        ("y", "r7")
    );
    assert_eq!(in_other[0].reason, "source-event: pipeline y run r7");
}

/// PL-98, PL-100 (T-5360): marks hold only ledger ids, go per run, oldest run first, and leave
/// the ledger marked no more once taken; each written id has its write time.
#[tokio::test]
async fn marks_are_taken_per_run_and_only_for_ledger_ids() {
    let Some(url) = database_url() else {
        eprintln!("JC_PORTAL_TEST_DATABASE_URL is not set: the marks' database half is not run");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let ledger = Ledger::new(Some(pool));
    let project = fresh_project();
    let p = project.as_str();
    let ids = ["urn:a", "urn:b", "urn:c"].map(str::to_owned);
    ledger.seen(p, "ev", "r0", &ids, true).await.expect("seen");
    let held = ledger
        .mark(p, "ev", "r2", &["urn:a".to_owned(), "urn:z".to_owned()])
        .await
        .expect("mark");
    assert_eq!(held, ["urn:a"]);
    ledger
        .mark(p, "ev", "r1", &["urn:b".to_owned(), "urn:c".to_owned()])
        .await
        .expect("mark");
    assert!(ledger
        .marking()
        .await
        .expect("marking")
        .contains(&(p.to_owned(), "ev".to_owned())));
    let first = ledger.take_marked(p, "ev").await.expect("take");
    assert_eq!(
        first,
        Some((
            "r1".to_owned(),
            vec!["urn:b".to_owned(), "urn:c".to_owned()]
        ))
    );
    let second = ledger.take_marked(p, "ev").await.expect("take");
    assert_eq!(second, Some(("r2".to_owned(), vec!["urn:a".to_owned()])));
    assert_eq!(ledger.take_marked(p, "ev").await.expect("take"), None);
    let written = ledger.written(p, "ev", &ids).await.expect("written");
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert_eq!(written.len(), 3);
    assert!(
        written.values().all(|at| (now - at).abs() < 600),
        "{written:?}"
    );
}
