//! The Portal's pool when a statement is cut off (T-7494, the jc-prod outage of 2026-11-22): the
//! runner stops waiting after 5 s, the request's future is dropped, and Postgres cancels the trim
//! later. sqlx 0.9.0 hung closing such a connection over TLS and never gave its slot back, so the
//! database here is reached over TLS. The other statement that starved that pool is tested below.
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; otherwise
//! it says so and returns, like the other database suites.

use std::sync::Arc;
use std::time::Duration;

use joinedcontext_portal::pipeline_log::{LogStore, NewLine, Outcome, FORGET_ROWS, LINES_KEPT};
use joinedcontext_portal::pipeline_outcomes::{Reason, RejectedStore};
use p256::pkcs8::EncodePrivateKey;

mod common;
#[path = "../../shared/jc-pool/testkit/tls_front.rs"]
mod tls_front;
use common::unique;

const POOL: usize = 5;

/// The test database behind the shared TLS front. The key is a test constant of no secret; the
/// certificate is its self-signed one.
async fn tls_url(url: &str) -> String {
    let key = p256::SecretKey::from_slice(&[7; 32])
        .expect("a scalar")
        .to_pkcs8_der()
        .expect("an encoded key")
        .as_bytes()
        .to_vec();
    let cert = include_bytes!("fixtures/db-relay.cert.der").to_vec();
    tls_front::tls_url(url, cert, key).await
}

/// Red on e942ebb8: the five connections stay held after the server cancelled their trims, and
/// the history the trims' transactions carried is lost.
#[tokio::test]
async fn a_trim_that_times_out_after_the_caller_left_leaves_the_pool_at_full_size() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("JC_PORTAL_TEST_DATABASE_URL is not set: the pool's database half is not run");
        return;
    };
    let direct = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let project = format!("p-{}", unique::unique());
    let mirror = common::keyed_mirror(&direct, &[&project]).await;

    // One line over the kept ones per pipeline, the oldest locked: each trim waits on it.
    let mut blocker = direct.begin().await.expect("begin");
    let key = joinedcontext_portal::project_ids::key_of(&direct, &mirror, &project)
        .await
        .expect("read")
        .expect("placed");
    joinedcontext_portal::db::enter(&mut blocker, key)
        .await
        .expect("keyed");
    for n in 0..POOL {
        sqlx::query(
            "INSERT INTO pipeline_log (project, pipeline, run, outcome) \
             SELECT $1, $2, 'seed', 'sent' FROM generate_series(1, $3)",
        )
        .bind(&project)
        .bind(format!("held-{n}"))
        .bind(LINES_KEPT as i32 + 1)
        .execute(&mut *blocker)
        .await
        .expect("seeded");
    }
    blocker.commit().await.expect("commit");
    let mut blocker = direct.begin().await.expect("begin");
    joinedcontext_portal::db::enter(&mut blocker, key)
        .await
        .expect("keyed");
    sqlx::query("SELECT id FROM pipeline_log WHERE project = $1 FOR UPDATE")
        .bind(&project)
        .execute(&mut *blocker)
        .await
        .expect("locked");

    let pool = joinedcontext_portal::db::connect(&tls_url(&url).await)
        .await
        .expect("the database answers over TLS");
    let log = Arc::new(LogStore::new(Some(pool.clone()), mirror));
    let reports: Vec<_> = (0..POOL)
        .map(|n| {
            let (log, project) = (log.clone(), project.clone());
            tokio::spawn(async move {
                let pipeline = format!("held-{n}");
                // The runner's own timeout is 5 s; its closed request drops this future.
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    log.append(&project, &pipeline, "r1", &[]),
                )
                .await;
            })
        })
        .collect();
    for report in reports {
        report.await.expect("reported");
    }

    // Past the longest a trim may run on the server, with the blocker still holding its lock.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let mut held = Vec::new();
        while held.len() < POOL {
            match pool.acquire().await {
                Ok(connection) => held.push(connection),
                Err(_) => break,
            }
        }
        if held.len() == POOL {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pool hands out {} of {POOL} connections",
            held.len()
        );
        drop(held);
    }
    blocker.rollback().await.expect("unlocked");

    for n in 0..POOL {
        let runs = log
            .runs(&project, &format!("held-{n}"), 10)
            .await
            .expect("read");
        assert!(
            runs.iter().any(|run| run.run == "r1"),
            "held-{n} lost its history row to the failed trim"
        );
    }
}

/// One project's failed delete keeps that project's rows and the pass goes on to the next one, in
/// the run logs and in the refused records alike (T-7497).
#[tokio::test]
async fn one_projects_failed_cleanup_leaves_the_projects_after_it_cleaned() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("JC_PORTAL_TEST_DATABASE_URL is not set: cleanup's database half is not run");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let id = unique::unique();
    // A pass takes the projects by name: the one that fails comes first.
    let (failing, next) = (format!("a-{id}"), format!("b-{id}"));
    let mirror = common::keyed_mirror(&pool, &[&failing, &next]).await;
    let log = LogStore::new(Some(pool.clone()), mirror.clone());
    let rejected = RejectedStore::new(Some(pool.clone()), mirror.clone());
    for project in [&failing, &next] {
        log.append(project, "gone", "r", &[line()])
            .await
            .expect("kept");
        rejected
            .reject(project, "gone", &serde_json::json!({}), &Reason::default())
            .await
            .expect("kept");
    }
    let trigger = format!("refuse_{id}");
    let refuse = format!(
        "CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'cleanup refused by the test'; END $$; \
         CREATE TRIGGER {trigger} BEFORE DELETE ON pipeline_runs FOR EACH ROW \
         WHEN (OLD.project = '{failing}') EXECUTE FUNCTION {trigger}(); \
         CREATE TRIGGER {trigger} BEFORE DELETE ON pipeline_rejected FOR EACH ROW \
         WHEN (OLD.project = '{failing}') EXECUTE FUNCTION {trigger}();"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(refuse))
        .execute(&pool)
        .await
        .expect("the failure is set");

    let loaded = std::collections::BTreeMap::from([
        (failing.clone(), Default::default()),
        (next.clone(), Default::default()),
    ]);
    let logs_forgotten = log.forget_gone(&loaded).await;
    let rejected_forgotten = rejected.forget_gone(&loaded).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP FUNCTION {trigger}() CASCADE"
    )))
    .execute(&pool)
    .await
    .expect("the failure is lifted");

    assert!(logs_forgotten.is_err(), "the failure is still reported");
    assert!(rejected_forgotten.is_err(), "the failure is still reported");
    let kept = log.runs(&failing, "gone", 10).await;
    assert_eq!(kept.expect("runs").len(), 1);
    let cleaned = log.lines(&next, "gone", "r", 10, None).await;
    assert!(
        cleaned.expect("lines").is_empty(),
        "the next project's log was not cleaned"
    );
    assert_eq!(rejected.count(&failing, "gone").await.expect("count"), 1);
    assert_eq!(
        rejected.count(&next, "gone").await.expect("count"),
        0,
        "the next project's refused records were not cleaned"
    );
}

/// `forget_gone` drops exactly the pipelines a project no longer holds, whichever table still has
/// their rows, and no other project's (T-7497: it steps the index by pipeline name).
#[tokio::test]
async fn forgetting_gone_pipelines_drops_theirs_alone_in_every_table() {
    let Some(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("JC_PORTAL_TEST_DATABASE_URL is not set: forgetting's database half is not run");
        return;
    };
    let pool = joinedcontext_portal::db::connect(&url)
        .await
        .expect("the test database answers and migrates");
    let (project, other) = (
        format!("p-{}", unique::unique()),
        format!("p-{}", unique::unique()),
    );
    let mirror = common::keyed_mirror(&pool, &[&project, &other]).await;
    let log = LogStore::new(Some(pool.clone()), mirror.clone());
    let written = [("urn:a".to_owned(), "h".to_owned())];
    // The first, the middle and the last name of the walk go; names differing in case stay apart.
    for (project, pipeline) in [
        (&project, "A-gone"),
        (&project, "a-kept"),
        (&project, "m-gone"),
        (&project, "n-kept"),
        (&project, "z-gone"),
        (&other, "m-gone"),
    ] {
        log.append(project, pipeline, "r", &[line()])
            .await
            .expect("kept");
        log.record_changes(project, pipeline, "r", &written)
            .await
            .expect("hashed");
    }
    // More than one statement's worth, some of it a second pipeline's.
    let many: Vec<(String, String)> = (0..2 * FORGET_ROWS + 7)
        .map(|n| (format!("urn:{n}"), "h".to_owned()))
        .collect();
    log.record_changes(&project, "m-gone", "r", &many)
        .await
        .expect("hashed");
    let held = |project: &str| {
        let (pool, mirror, project) = (pool.clone(), mirror.clone(), project.to_owned());
        async move {
            let key = joinedcontext_portal::project_ids::key_of(&pool, &mirror, &project)
                .await
                .expect("read")
                .expect("placed");
            let mut tx = joinedcontext_portal::db::tenant_tx(&pool, key)
                .await
                .expect("keyed");
            let mut tables = Vec::new();
            for table in ["pipeline_log", "pipeline_runs", "pipeline_record_hashes"] {
                let names: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT DISTINCT pipeline FROM {table} WHERE project = $1"
                )))
                .bind(&project)
                .fetch_all(&mut *tx)
                .await
                .expect("read");
                tables.push(names.into_iter().collect::<std::collections::BTreeSet<_>>());
            }
            tables
        }
    };
    let loaded = |names: &[&str]| {
        std::collections::BTreeMap::from([(
            project.clone(),
            names.iter().map(|name| (*name).to_owned()).collect(),
        )])
    };
    let names = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();

    log.forget_gone(&loaded(&["a-kept", "n-kept", "never-ran"]))
        .await
        .expect("forgotten");
    let kept: std::collections::BTreeSet<String> = names(&["a-kept", "n-kept"]);
    assert_eq!(held(&project).await, vec![kept; 3]);
    let others: std::collections::BTreeSet<String> = names(&["m-gone"]);
    assert_eq!(held(&other).await, vec![others.clone(); 3]);

    log.forget_gone(&loaded(&[])).await.expect("forgotten");
    assert!(held(&project).await.iter().all(|table| table.is_empty()));
    assert_eq!(held(&other).await, vec![others; 3]);
}

fn line() -> NewLine {
    NewLine {
        record_id: "urn:a".to_owned(),
        step: None,
        outcome: Outcome::Sent,
        message: String::new(),
    }
}
