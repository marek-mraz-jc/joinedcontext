//! The unit against a live PostgreSQL: `JC_AUDIT_TEST_DATABASE_URL` names a database the test
//! may create tables in; without it each test prints `SKIP` and returns.

use jc_audit::{abort, begin, commit, scope};
use sqlx::PgPool;

async fn pool(test: &str) -> Option<(PgPool, String)> {
    // A test reads its database from the environment, as the module stores' live tests do.
    #[allow(clippy::disallowed_methods)]
    let Ok(url) = std::env::var("JC_AUDIT_TEST_DATABASE_URL") else {
        println!("SKIP {test}: JC_AUDIT_TEST_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPool::connect(&url).await.expect("connect");
    let table = format!("t_{test}");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP TABLE IF EXISTS {table}; CREATE TABLE {table} (k text PRIMARY KEY DEFERRABLE INITIALLY DEFERRED)"
    )))
    .execute(&pool)
    .await
    .expect("create table");
    Some((pool, table))
}

async fn insert(pool: &PgPool, table: &str, k: &str) -> Result<(), sqlx::Error> {
    let mut tx = begin(pool).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {table} VALUES ($1)"
    )))
    .bind(k)
    .execute(&mut *tx)
    .await?;
    commit(tx).await
}

/// A store call that writes and then returns early without committing (an error path).
async fn insert_then_drop(pool: &PgPool, table: &str, k: &str) -> Result<(), sqlx::Error> {
    let mut tx = begin(pool).await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {table} VALUES ($1)"
    )))
    .bind(k)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

async fn keys(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT k FROM {table} ORDER BY k"
    )))
    .fetch_all(pool)
    .await
    .expect("read")
}

#[tokio::test]
async fn outside_a_unit_each_call_commits_on_its_own() {
    let Some((pool, t)) = pool("outside").await else {
        return;
    };
    insert(&pool, &t, "a").await.unwrap();
    insert_then_drop(&pool, &t, "b").await.unwrap();
    assert_eq!(keys(&pool, &t).await, ["a"]);
}

#[tokio::test]
async fn a_unit_commits_every_call_together_and_only_at_its_end() {
    let Some((pool, t)) = pool("together").await else {
        return;
    };
    let out = scope(|_: &()| true, async {
        insert(&pool, &t, "change").await.unwrap();
        // Another connection sees nothing before the unit ends.
        assert!(keys(&pool, &t).await.is_empty());
        insert(&pool, &t, "audit").await.unwrap();
    })
    .await;
    assert!(out.is_ok());
    assert_eq!(keys(&pool, &t).await, ["audit", "change"]);
}

#[tokio::test]
async fn a_failed_request_rolls_back_every_call() {
    let Some((pool, t)) = pool("failed").await else {
        return;
    };
    let out = scope(|ok: &bool| *ok, async {
        insert(&pool, &t, "change").await.unwrap();
        false
    })
    .await;
    assert!(matches!(out, Ok(false)));
    assert!(keys(&pool, &t).await.is_empty());
}

#[tokio::test]
async fn a_failing_audit_write_fails_the_change() {
    let Some((pool, t)) = pool("audit_fails").await else {
        return;
    };
    let out = scope(|r: &Result<(), sqlx::Error>| r.is_ok(), async {
        insert(&pool, &t, "change").await?;
        // The audit insert errors (no such table): the request answers with that error.
        let mut tx = begin(&pool).await?;
        sqlx::query("INSERT INTO no_such_audit_table VALUES (1)")
            .execute(&mut *tx)
            .await?;
        commit(tx).await
    })
    .await;
    assert!(out.unwrap().is_err());
    assert!(keys(&pool, &t).await.is_empty());
}

#[tokio::test]
async fn a_dropped_call_undoes_only_its_own_statements_even_after_a_failed_statement() {
    let Some((pool, t)) = pool("dropped").await else {
        return;
    };
    let out = scope(|_: &()| true, async {
        insert(&pool, &t, "kept").await.unwrap();
        insert_then_drop(&pool, &t, "undone").await.unwrap();
        // A failed statement leaves the transaction aborted; the next call starts clean.
        let mut tx = begin(&pool).await.unwrap();
        assert!(sqlx::query("SELECT 1/0").execute(&mut *tx).await.is_err());
        drop(tx);
        insert(&pool, &t, "after").await.unwrap();
        // The last call is left behind too: the unit undoes it before committing.
        insert_then_drop(&pool, &t, "last").await.unwrap();
    })
    .await;
    assert!(out.is_ok());
    assert_eq!(keys(&pool, &t).await, ["after", "kept"]);
}

#[tokio::test]
async fn abort_rolls_back_a_request_that_answers_success() {
    let Some((pool, t)) = pool("abort").await else {
        return;
    };
    let out = scope(|_: &()| true, async {
        insert(&pool, &t, "change").await.unwrap();
        abort();
    })
    .await;
    assert!(out.is_ok());
    assert!(keys(&pool, &t).await.is_empty());
}

#[tokio::test]
async fn a_commit_that_fails_is_reported_and_nothing_stands() {
    let Some((pool, t)) = pool("commit_fails").await else {
        return;
    };
    // The deferred key check runs at COMMIT.
    let out = scope(|_: &()| true, async {
        insert(&pool, &t, "same").await.unwrap();
        insert(&pool, &t, "same").await.unwrap();
    })
    .await;
    assert!(
        out.unwrap_err()
            .to_string()
            .starts_with("the change was not saved: "),
    );
    assert!(keys(&pool, &t).await.is_empty());
}

/// The session ends under the unit (its statement fails): a request that failed still answers,
/// one that succeeded reports the change as not saved, and nothing stands either way.
#[tokio::test]
async fn a_unit_whose_session_died_reports_only_a_lost_success() {
    let Some((pool, t)) = pool("session_died").await else {
        return;
    };
    for succeeded in [false, true] {
        let out = scope(move |_: &()| succeeded, async {
            insert(&pool, &t, "change").await.unwrap();
            let mut tx = begin(&pool).await.unwrap();
            let killed = sqlx::query("SELECT pg_terminate_backend(pg_backend_pid())")
                .execute(&mut *tx)
                .await;
            assert!(killed.is_err());
        })
        .await;
        assert_eq!(out.is_err(), succeeded, "succeeded={succeeded}");
        assert!(keys(&pool, &t).await.is_empty());
    }
}

/// A request dropped mid-unit (a client gone) closes its connection instead of handing it back
/// to the pool inside the open transaction, so the next caller neither sees its rows nor
/// inherits its transaction.
#[tokio::test]
async fn a_cancelled_unit_closes_its_connection() {
    let Some((pool, t)) = pool("cancelled").await else {
        return;
    };
    let during = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let seen = during.clone();
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        scope(|_: &()| true, async {
            insert(&pool, &t, "change").await.unwrap();
            seen.store(pool.size(), std::sync::atomic::Ordering::SeqCst);
            std::future::pending::<()>().await;
        }),
    )
    .await;
    assert!(cancelled.is_err());
    let during = during.load(std::sync::atomic::Ordering::SeqCst);
    assert!(during > 0);
    for _ in 0..100 {
        if pool.size() < during {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        pool.size() < during,
        "the unit's connection went back to the pool"
    );
    assert!(keys(&pool, &t).await.is_empty());
}

/// A store call's transaction reads as its connection, inside a unit and outside one.
#[tokio::test]
async fn a_transaction_dereferences_to_its_connection() {
    let Some((pool, _)) = pool("deref").await else {
        return;
    };
    let version = |tx: &jc_audit::Tx| tx.server_version_num().unwrap_or(0);
    let own = begin(&pool).await.unwrap();
    assert!(version(&own) >= 100_000);
    commit(own).await.unwrap();
    let inside = scope(|_: &u32| true, async {
        let tx = begin(&pool).await.unwrap();
        let v = version(&tx);
        commit(tx).await.unwrap();
        v
    })
    .await
    .unwrap();
    assert!(inside >= 100_000);
}

#[tokio::test]
async fn concurrent_calls_of_one_unit_wait_for_each_other() {
    let Some((pool, t)) = pool("concurrent").await else {
        return;
    };
    let out = scope(|_: &()| true, async {
        let (a, b) = tokio::join!(insert(&pool, &t, "a"), insert(&pool, &t, "b"));
        a.unwrap();
        b.unwrap();
    })
    .await;
    assert!(out.is_ok());
    assert_eq!(keys(&pool, &t).await, ["a", "b"]);
}

#[tokio::test]
async fn a_unit_without_a_store_call_takes_no_connection() {
    let Some((pool, _)) = pool("idle").await else {
        return;
    };
    // The pool's size counts every connection it opened; how many sit idle depends on when the
    // setup's connection got back to the pool.
    let opened = pool.size();
    assert!(matches!(scope(|_: &u8| true, async { 7u8 }).await, Ok(7)));
    assert_eq!(pool.size(), opened);
}

#[tokio::test]
async fn a_replay_mark_stands_when_its_request_fails() {
    let Some((pool, t)) = pool("own").await else {
        return;
    };
    let out = scope(|_: &()| false, async {
        insert(&pool, &t, "change").await.unwrap();
        let mut tx = jc_audit::begin_own(&pool).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {t} VALUES ('mark')"
        )))
        .execute(&mut *tx)
        .await
        .unwrap();
        commit(tx).await.unwrap();
    })
    .await;
    assert!(out.is_ok());
    assert_eq!(keys(&pool, &t).await, ["mark"]);
}

#[tokio::test]
async fn work_after_the_commit_runs_only_when_the_unit_commits() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let ran = Arc::new(AtomicUsize::new(0));
    let mark = |ran: &Arc<AtomicUsize>| {
        let ran = ran.clone();
        async move {
            ran.fetch_add(1, Ordering::SeqCst);
        }
    };
    // Outside a unit it runs at once.
    jc_audit::after_commit(mark(&ran)).await;
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    // In a unit that wrote nothing it waits for the unit's end, then runs on success only.
    let waited = scope(|_: &usize| true, async {
        jc_audit::after_commit(mark(&ran)).await;
        ran.load(Ordering::SeqCst)
    })
    .await
    .expect("unit");
    assert_eq!((waited, ran.load(Ordering::SeqCst)), (1, 2));
    scope(|_: &()| false, jc_audit::after_commit(mark(&ran)))
        .await
        .expect("unit");
    assert_eq!(
        ran.load(Ordering::SeqCst),
        2,
        "a failed unit runs nothing after it"
    );

    let Some((pool, t)) = pool("after_commit").await else {
        return;
    };
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let watch = |pool: &PgPool, t: &str, seen: &Arc<std::sync::Mutex<Vec<Vec<String>>>>| {
        let (pool, t, seen) = (pool.clone(), t.to_owned(), seen.clone());
        async move {
            let now = keys(&pool, &t).await;
            seen.lock().expect("lock").push(now);
        }
    };
    scope(|r: &Result<(), sqlx::Error>| r.is_ok(), async {
        insert(&pool, &t, "a").await?;
        jc_audit::after_commit(watch(&pool, &t, &seen)).await;
        Ok(())
    })
    .await
    .expect("unit")
    .expect("insert");
    scope(|_: &Result<(), sqlx::Error>| false, async {
        insert(&pool, &t, "b").await?;
        jc_audit::after_commit(watch(&pool, &t, &seen)).await;
        Ok(())
    })
    .await
    .expect("unit")
    .expect("insert");
    assert_eq!(
        *seen.lock().expect("lock"),
        vec![vec!["a".to_owned()]],
        "the work sees the committed row, and a rolled-back unit runs none"
    );
}

/// Work after the commit that panics leaves the change standing and the unit answering success.
#[tokio::test]
async fn a_panic_after_the_commit_never_reports_a_saved_change_as_lost() {
    let Some((pool, t)) = pool("after_panic").await else {
        return;
    };
    let out = scope(|_: &()| true, async {
        insert(&pool, &t, "change").await.unwrap();
        jc_audit::after_commit(async { panic!("after the commit") }).await;
    })
    .await;
    assert!(out.is_ok());
    assert_eq!(keys(&pool, &t).await, ["change"]);
}

/// The middleware: a read runs outside a unit, a write commits on a 2xx and rolls back
/// otherwise, and a commit that fails answers 500 with an error a client can act on.
#[tokio::test]
async fn the_layer_runs_each_write_as_one_unit() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use tower::ServiceExt;
    let Some((pool, t)) = pool("layer").await else {
        return;
    };
    let write = |status: StatusCode, keys: &'static [&'static str]| {
        let (pool, t) = (pool.clone(), t.clone());
        move || async move {
            for k in keys {
                insert(&pool, &t, k).await.unwrap();
            }
            status
        }
    };
    let app = axum::Router::new()
        .route("/ok", post(write(StatusCode::CREATED, &["ok"])))
        .route("/no", post(write(StatusCode::BAD_REQUEST, &["no"])))
        .route("/twice", post(write(StatusCode::OK, &["dup", "dup"])))
        .route(
            "/read",
            axum::routing::get(write(StatusCode::BAD_REQUEST, &["read"])),
        )
        .layer(axum::middleware::from_fn(jc_audit::layer));
    let call = |method: &str, path: &str| {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        app.clone().oneshot(req)
    };
    assert_eq!(
        call("POST", "/ok").await.unwrap().status(),
        StatusCode::CREATED
    );
    assert_eq!(
        call("POST", "/no").await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call("GET", "/read").await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let lost = call("POST", "/twice").await.unwrap();
    assert_eq!(lost.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(lost.headers()["content-type"], "application/json");
    let body = axum::body::to_bytes(lost.into_body(), 1024).await.unwrap();
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("the change was not saved; try again")
    );
    assert_eq!(keys(&pool, &t).await, ["ok", "read"]);
}
