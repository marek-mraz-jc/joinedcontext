// SPDX-License-Identifier: EUPL-1.2
//! T-6820: a pooled connection gives back the buffer a large statement grew.
//! sqlx keeps a connection's write and read buffers at the largest size they
//! ever reached, so every pooled connection that once encoded a pipeline's
//! batch upsert held that batch's bytes for the rest of its life: 118-218 MiB
//! of `encode_with` on the prod rehearsal profile (2026-11-11, 20 + reserve
//! slots). The release hook shrinks them back to sqlx's 8 KiB default.
//! Skips loudly without ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use std::time::Duration;

use tikv_jemalloc_ctl::{epoch, stats};

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Bytes the process has allocated and not freed, as jemalloc counts them.
fn live() -> usize {
    epoch::advance().expect("epoch");
    stats::allocated::read().expect("stats.allocated")
}

#[tokio::test(flavor = "current_thread")]
async fn a_released_connection_keeps_no_large_statement_buffer() {
    let Ok(url) = std::env::var("ANTARES_TEST_DATABASE_URL") else {
        eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
        return;
    };
    let pool = antares_sql::store::pg::connect_reserve(&url, 1, Duration::from_secs(30))
        .await
        .expect("pool");
    const BIG: usize = 32 << 20;
    // warm: the one connection is open and its statement cached
    let _: i32 = sqlx::query_scalar("SELECT length($1)::int")
        .bind("x")
        .fetch_one(&pool)
        .await
        .expect("warm");
    let before = live();
    {
        let text = "x".repeat(BIG);
        let n: i32 = sqlx::query_scalar("SELECT length($1)::int")
            .bind(&text)
            .fetch_one(&pool)
            .await
            .expect("big statement");
        assert_eq!(n as usize, BIG);
    }
    // the release hook runs after the statement returned; a second acquire
    // waits for it, since the pool has one slot
    let _: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(&pool)
        .await
        .expect("after");
    let grown = live().saturating_sub(before);
    assert!(
        grown < 4 << 20,
        "{} MiB still live after a 32 MiB statement's connection went back to the pool",
        grown >> 20
    );
    assert_eq!(pool.size(), 1, "the connection was kept, not replaced");
}
