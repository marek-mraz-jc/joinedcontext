//! The branch watch of project_ids.rs on a database of its own, in a test binary of its own: the
//! branch table is a process-wide static and the sweep deletes, so neither touches another test.

use std::time::Duration;

use jc_types::ProjectId;
use joinedcontext_portal::project_ids;

/// Each pass after the first read reads the table again and sweeps; a pass that succeeds goes on.
#[tokio::test]
async fn the_branch_watch_reads_and_sweeps_on_every_pass() {
    let Ok(url) = std::env::var("JC_PORTAL_TEST_DATABASE_URL") else {
        eprintln!("skipped: JC_PORTAL_TEST_DATABASE_URL is not set");
        return;
    };
    let name = format!("cov2_watch_{}", ProjectId::mint().path_segment());
    let admin = sqlx::PgPool::connect(&url)
        .await
        .expect("the test database");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&admin)
        .await
        .expect("a fresh database");
    let mut own = url::Url::parse(&url).expect("url");
    own.set_path(&format!("/{name}"));
    let pool = joinedcontext_portal::db::connect(own.as_str())
        .await
        .expect("connect + migrate");

    project_ids::watch_branches_every(pool.clone(), Duration::from_millis(50)).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(project_ids::sweep_expired_branches(&pool)
        .await
        .expect("sweep")
        .is_empty());

    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE {name} WITH (FORCE)"
    )))
    .execute(&admin)
    .await
    .expect("drop");
}
