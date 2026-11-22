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
    assert!(out.is_err());
    assert!(keys(&pool, &t).await.is_empty());
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
    let idle = pool.num_idle();
    assert!(matches!(scope(|_: &u8| true, async { 7u8 }).await, Ok(7)));
    assert_eq!(pool.num_idle(), idle);
}
