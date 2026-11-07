// SPDX-License-Identifier: EUPL-1.2
//! T-4298: a connection the pool hands back is tested within a bound, so a
//! stalled stream costs the pool nothing. sqlx tests a released connection
//! with a ping it never times out; through a mesh proxy, which accepts the
//! bytes, a stream whose far side stalled answers that ping never, and each
//! one held its pool slot for hours (dev 2026-10-16: 20 of 20 slots taken,
//! the database showing 20 idle sessions, the oldest 2 h 09 min old).
//! Skips loudly without ANTARES_TEST_DATABASE_URL (see tests/pg.rs recipe).

#![cfg(feature = "postgres")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

macro_rules! require_db {
    () => {
        match std::env::var("ANTARES_TEST_DATABASE_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("SKIP: ANTARES_TEST_DATABASE_URL not set");
                return;
            }
        }
    };
}

/// A TCP relay to the database whose open streams can be frozen: it keeps
/// both sockets open and stops forwarding, as a stalled mesh stream does.
/// Streams opened after the freeze relay normally.
struct Relay {
    url: String,
    open: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
}

impl Relay {
    async fn start(url: &str) -> Self {
        // postgres://user[:password]@host:port/db: the authority sits between the
        // last `@` before the path and the path's `/`.
        let rest = url.split_once("://").expect("scheme").1;
        let (authority, path) = rest.split_once('/').expect("path");
        let (user, upstream) = authority.rsplit_once('@').expect("user@host");
        let upstream = if upstream.contains(':') {
            upstream.to_owned()
        } else {
            format!("{upstream}:5432")
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let through = format!("postgres://{user}@127.0.0.1:{port}/{path}");
        let open: Arc<Mutex<Vec<Arc<AtomicBool>>>> = Arc::default();
        let streams = open.clone();
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(server) = TcpStream::connect(&upstream).await else {
                    continue;
                };
                let frozen = Arc::new(AtomicBool::new(false));
                streams.lock().expect("lock").push(frozen.clone());
                let (cr, cw) = client.into_split();
                let (sr, sw) = server.into_split();
                tokio::spawn(pipe(cr, sw, frozen.clone()));
                tokio::spawn(pipe(sr, cw, frozen));
            }
        });
        Self { url: through, open }
    }

    fn freeze(&self) {
        for frozen in self.open.lock().expect("lock").iter() {
            frozen.store(true, Ordering::SeqCst);
        }
    }
}

async fn pipe(
    mut from: tokio::net::tcp::OwnedReadHalf,
    mut to: tokio::net::tcp::OwnedWriteHalf,
    frozen: Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let Ok(n) = from.read(&mut buf).await else {
            return;
        };
        if n == 0 {
            return;
        }
        if frozen.load(Ordering::SeqCst) {
            // Holds both halves open and forwards nothing, ever.
            std::future::pending::<()>().await;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

#[tokio::test]
async fn cancelled_queries_give_their_connection_back() {
    let url = require_db!();
    let pool = antares_sql::store::pg::connect_with(&url, 2, Duration::from_secs(30))
        .await
        .expect("pool");
    for round in 0..40 {
        let p = pool.clone();
        let cancelled = tokio::time::timeout(Duration::from_millis(30), async move {
            if round % 2 == 0 {
                sqlx::query("SELECT pg_sleep(0.5)")
                    .execute(&p)
                    .await
                    .map(|_| ())
            } else {
                let mut tx = p.begin().await?;
                sqlx::query("SELECT pg_sleep(0.5)")
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await
            }
        })
        .await;
        assert!(cancelled.is_err(), "round {round} finished inside 30 ms");
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let a = pool
        .begin()
        .await
        .expect("first connection after the cancellations");
    let b = pool
        .begin()
        .await
        .expect("second connection after the cancellations");
    drop((a, b));
}

#[tokio::test]
async fn a_stalled_stream_does_not_keep_its_pool_slot() {
    let url = require_db!();
    let relay = Relay::start(&url).await;
    let pool = antares_sql::store::pg::connect_with(&relay.url, 2, Duration::from_secs(30))
        .await
        .expect("pool through the relay");
    // Two requests are reading when their streams stall; their callers give
    // up, as an edge does at its timeout, and drop them mid-read.
    let reads: Vec<_> = (0..2)
        .map(|_| {
            let p = pool.clone();
            tokio::spawn(tokio::time::timeout(
                Duration::from_millis(1500),
                async move { sqlx::query("SELECT pg_sleep(1)").execute(&p).await },
            ))
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(300)).await;
    relay.freeze();
    for read in reads {
        assert!(
            read.await.expect("joined").is_err(),
            "a stalled stream answered"
        );
    }
    // Within a few seconds both slots serve again, over fresh streams.
    let started = std::time::Instant::now();
    let p = pool.clone();
    let served = tokio::time::timeout(Duration::from_secs(15), async move {
        let mut a = p.begin().await?;
        let mut b = p.begin().await?;
        sqlx::query("SELECT 1").execute(&mut *a).await?;
        sqlx::query("SELECT 1").execute(&mut *b).await?;
        Ok::<_, sqlx::Error>(())
    })
    .await;
    assert!(
        matches!(served, Ok(Ok(()))),
        "the pool did not recover from two stalled streams: {served:?} after {:?} (size {}, idle {})",
        started.elapsed(),
        pool.size(),
        pool.num_idle()
    );
}

/// T-6000: prod held 13 of 13 sessions idle after `ROLLBACK` for up to 11 h
/// while every acquire timed out. The server ends a session idle past
/// `IDLE_SESSION`, and the pool closes its own idle ones well before that,
/// so the bound never cuts a connection the pool still serves from.
#[tokio::test]
async fn the_server_ends_a_session_left_idle_past_the_pool() {
    let url = require_db!();
    let pool = antares_sql::store::pg::connect_with(&url, 1, Duration::from_secs(30))
        .await
        .expect("pool");
    let set: String = sqlx::query_scalar("SELECT current_setting('idle_session_timeout')")
        .fetch_one(&pool)
        .await
        .expect("setting");
    assert_eq!(set, "10min", "a broker session carries the idle bound");
    let pool_idle = pool.options().get_idle_timeout().expect("idle timeout");
    assert!(
        pool_idle * 2 < antares_sql::store::pg::IDLE_SESSION,
        "the pool reaps an idle connection (within twice {pool_idle:?}) before the server bound"
    );
}

/// T-6277: a count with a `q` over dev's 67 742 Helsinki Trees read 56 819
/// scattered heap blocks one at a time (PostgreSQL 16 defaults
/// `effective_io_concurrency` to 1) and passed the gateway's 10 s; read ahead,
/// the same statement took 1.6 s. Every broker session reads ahead.
#[tokio::test]
async fn a_broker_session_reads_heap_blocks_ahead() {
    let url = require_db!();
    let pool = antares_sql::store::pg::connect_with(&url, 1, Duration::from_secs(30))
        .await
        .expect("pool");
    let set: String = sqlx::query_scalar("SELECT current_setting('effective_io_concurrency')")
        .fetch_one(&pool)
        .await
        .expect("setting");
    assert_eq!(
        set,
        antares_sql::store::pg::EFFECTIVE_IO_CONCURRENCY.to_string(),
        "a broker session prefetches its bitmap heap scans"
    );
}

/// Requests dropped at every stage of a transaction (mid-stream, after a
/// failed statement, mid-send of a large bind, inside a savepoint) leave
/// every pool slot usable again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transactions_cancelled_anywhere_give_every_slot_back() {
    use futures_util::TryStreamExt;
    use sqlx::Acquire;
    async fn step(p: sqlx::PgPool, k: u32) -> Result<(), sqlx::Error> {
        let mut tx = p.begin().await?;
        match k % 4 {
            0 => {
                let mut rows =
                    sqlx::query("SELECT g FROM generate_series(1, 20000) g").fetch(&mut *tx);
                while rows.try_next().await?.is_some() {}
            }
            1 => {
                sqlx::query("SELECT 1/0").execute(&mut *tx).await?;
            }
            2 => {
                sqlx::query("SELECT length($1)")
                    .bind("y".repeat(2_000_000))
                    .execute(&mut *tx)
                    .await?;
            }
            _ => {
                let mut sp = (&mut tx).begin().await?;
                sqlx::query("SELECT pg_sleep(0.005)")
                    .execute(&mut *sp)
                    .await?;
                sp.commit().await?;
            }
        }
        tx.commit().await
    }
    let url = require_db!();
    let pool = antares_sql::store::pg::connect_with(&url, 3, Duration::from_secs(30))
        .await
        .expect("pool");
    let workers: Vec<_> = (0..12u32)
        .map(|w| {
            let p = pool.clone();
            tokio::spawn(async move {
                for i in 0..100u32 {
                    let k = w.wrapping_mul(7919).wrapping_add(i.wrapping_mul(104_729));
                    let wall = Duration::from_millis(u64::from(k % 25));
                    let _ = tokio::time::timeout(wall, step(p.clone(), k / 25)).await;
                }
            })
        })
        .collect();
    for w in workers {
        w.await.expect("joined");
    }
    let p = pool.clone();
    let served = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut held = Vec::new();
        for _ in 0..3 {
            let mut tx = p.begin().await?;
            sqlx::query("SELECT 1").execute(&mut *tx).await?;
            held.push(tx);
        }
        Ok::<_, sqlx::Error>(())
    })
    .await;
    assert!(
        matches!(served, Ok(Ok(()))),
        "not every slot came back: {served:?} (size {}, idle {})",
        pool.size(),
        pool.num_idle()
    );
}

/// With every slot taken, /q/ready's store ping answers inside the probe's
/// 3 s instead of waiting out the 5 s acquire timeout.
#[tokio::test]
async fn the_readiness_ping_answers_while_every_slot_is_held() {
    let url = require_db!();
    let pool = antares_sql::store::pg::connect_with(&url, 1, Duration::from_secs(30))
        .await
        .expect("pool");
    let store = antares_sql::store::any::AnyStore::Pg(antares_sql::store::any::PgBackend::new(
        pool.clone(),
    ));
    store.ping().await.expect("ready with a free slot");
    let held = pool.acquire().await.expect("the only slot");
    let started = std::time::Instant::now();
    assert!(store.ping().await.is_err(), "not ready without a slot");
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "the ping waited {took:?}, past the probe timeout"
    );
    drop(held);
}

/// T-6000: a request dropped while its statement runs, the statement then
/// failing. The release ping reads that ErrorResponse; sqlx answered it by
/// closing the connection, and over TLS sqlx's close waits for bytes the
/// idle server never sends, so the slot was held for good (prod
/// 2026-10-28: 13 of 13 slots, each session idle after `ROLLBACK`). The
/// release hook now drains the failed statement and keeps the connection.
/// The hang needs TLS: without it the test passes either way, so it says so.
#[tokio::test]
async fn a_statement_failing_after_its_request_left_gives_the_slot_back() {
    let url = require_db!();
    let pool = antares_sql::store::pg::connect_with(&url, 1, Duration::from_secs(30))
        .await
        .expect("pool");
    let tls: bool = sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .expect("pg_stat_ssl");
    if !tls {
        eprintln!("NOTE: the server offers no TLS; the close hang this test guards cannot occur");
    }
    for round in 0..3 {
        let p = pool.clone();
        let dropped = tokio::time::timeout(Duration::from_millis(50), async move {
            let mut tx = p.begin().await?;
            sqlx::query("SELECT n / 0 FROM (SELECT count(*)::int AS n FROM pg_sleep(0.2)) s")
                .execute(&mut *tx)
                .await?;
            tx.commit().await
        })
        .await;
        assert!(dropped.is_err(), "round {round} finished inside 50 ms");
        let p = pool.clone();
        let served = tokio::time::timeout(Duration::from_secs(8), async move {
            let mut tx = p.begin().await?;
            sqlx::query("SELECT 1").execute(&mut *tx).await?;
            tx.commit().await
        })
        .await;
        assert!(
            matches!(served, Ok(Ok(()))),
            "round {round}: the slot did not come back: {served:?} (size {}, idle {})",
            pool.size(),
            pool.num_idle()
        );
    }
}

/// T-6182: N requests dropped at once while their statements run (and then
/// fail), and right after, N requests at once each get a slot within the
/// pool's acquire timeout. Prod (2026-10-30, an image without T-6000's
/// release fix) held 13 of 13 slots for 16 h this way. Run it with ssl=on:
/// without TLS the close hang cannot occur and it passes either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn n_requests_dropped_mid_statement_leave_n_slots_to_acquire() {
    const N: u32 = 4;
    let url = require_db!();
    let pool = antares_sql::store::pg::connect_with(&url, N, Duration::from_secs(30))
        .await
        .expect("pool");
    let acquire = pool.options().get_acquire_timeout();
    // The hang shows from the second drop on a reused connection.
    for round in 0..3 {
        let dropped: Vec<_> = (0..N)
            .map(|_| {
                let p = pool.clone();
                tokio::spawn(tokio::time::timeout(
                    Duration::from_millis(50),
                    async move {
                        let mut tx = p.begin().await?;
                        sqlx::query(
                            "SELECT n / 0 FROM (SELECT count(*)::int AS n FROM pg_sleep(0.2)) s",
                        )
                        .execute(&mut *tx)
                        .await?;
                        tx.commit().await
                    },
                ))
            })
            .collect();
        for d in dropped {
            assert!(
                d.await.expect("joined").is_err(),
                "round {round}: a request finished inside 50 ms"
            );
        }
        let started = std::time::Instant::now();
        let fresh: Vec<_> = (0..N)
            .map(|_| {
                let p = pool.clone();
                tokio::spawn(async move {
                    let mut tx = p.begin().await?;
                    sqlx::query("SELECT 1").execute(&mut *tx).await?;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    tx.commit().await
                })
            })
            .collect();
        for f in fresh {
            f.await
                .expect("joined")
                .unwrap_or_else(|e| panic!("round {round}: a fresh acquire after the drops: {e}"));
        }
        assert!(
            started.elapsed() < acquire,
            "round {round}: {N} acquires took {:?}, past the {acquire:?} acquire timeout",
            started.elapsed()
        );
    }
}

/// T-6182: liveness fails only for a leaked pool. With every slot held past
/// the bound and the database answering a fresh connection, the store is
/// wedged; once a slot is back, readiness and liveness recover.
#[tokio::test]
async fn a_pool_held_past_the_bound_is_wedged_until_a_slot_returns() {
    use antares_sql::store::any::{AnyStore, PgBackend};
    let url = require_db!();
    let pool = antares_sql::store::pg::connect_with(&url, 1, Duration::from_secs(30))
        .await
        .expect("pool");
    let store = AnyStore::Pg(PgBackend::new(pool.clone()).with_wedged_after(Duration::ZERO));
    store.ping().await.expect("ready with a free slot");
    assert!(!store.wedged().await, "a serving pool is not wedged");
    let held = pool.acquire().await.expect("the only slot");
    assert!(store.ping().await.is_err(), "not ready without a slot");
    assert!(store.wedged().await, "every slot held past the bound");
    drop(held);
    tokio::time::timeout(Duration::from_secs(8), async {
        while store.ping().await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("ready again once the slot returns");
    assert!(!store.wedged().await, "a slot came back: not wedged");
}

/// A lost database is not a leak: a restart fixes nothing, so liveness
/// stays green however long readiness fails. Needs no database.
#[tokio::test]
async fn a_lost_database_never_reads_as_wedged() {
    use antares_sql::store::any::{AnyStore, PgBackend};
    // Bound and never listening: every connect is refused.
    let sock = tokio::net::TcpSocket::new_v4().expect("socket");
    sock.bind("127.0.0.1:0".parse().expect("addr"))
        .expect("bind");
    let port = sock.local_addr().expect("addr").port();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect_lazy(&format!("postgres://antares@127.0.0.1:{port}/antares"))
        .expect("lazy pool");
    let store = AnyStore::Pg(PgBackend::new(pool).with_wedged_after(Duration::ZERO));
    for _ in 0..3 {
        assert!(store.ping().await.is_err(), "no database, not ready");
        assert!(!store.wedged().await, "no database, still live");
    }
    drop(sock);
}
