//! The release rule against a real server (T-7538): a healthy released connection stays in the
//! pool, one whose session the server ended is closed instead of handed out again.
//! `JC_POOL_TEST_DATABASE_URL` names the server; unset, the test says so and passes.
#![allow(clippy::disallowed_methods)] // The test reads its database URL; jc runs none of this.

use sqlx::Connection;
use sqlx::postgres::{PgConnection, PgPool};
use std::time::Duration;

/// The pool's size once the background release of a dropped connection settled.
async fn settled_size(pool: &PgPool, want: u32) -> u32 {
    for _ in 0..100 {
        if pool.size() == want {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    pool.size()
}

#[tokio::test]
async fn a_released_connection_whose_session_ended_is_closed_not_reused() {
    let Ok(url) = std::env::var("JC_POOL_TEST_DATABASE_URL") else {
        println!("SKIP: JC_POOL_TEST_DATABASE_URL is unset");
        return;
    };
    let pool: PgPool = jc_pool::options()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();

    let held = pool.acquire().await.unwrap();
    drop(held);
    assert_eq!(
        settled_size(&pool, 1).await,
        1,
        "a healthy connection goes back to the pool"
    );

    let mut held = pool.acquire().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let mut admin = PgConnection::connect(&url).await.unwrap();
    let ended: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(&mut admin)
        .await
        .unwrap();
    assert!(ended);
    drop(held);
    assert_eq!(
        settled_size(&pool, 0).await,
        0,
        "the dead connection is closed on release"
    );

    let fresh = pool.acquire().await.unwrap();
    drop(fresh);
    admin.close().await.unwrap();
}

#[test]
fn idle_connections_leave_after_the_shared_timeout() {
    let options: sqlx::pool::PoolOptions<sqlx::Postgres> = jc_pool::options();
    assert_eq!(options.get_idle_timeout(), Some(jc_pool::IDLE_TIMEOUT));
    assert_eq!(jc_pool::IDLE_TIMEOUT, Duration::from_secs(60));
}
