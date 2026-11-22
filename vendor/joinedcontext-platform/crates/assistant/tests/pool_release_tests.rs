//! The assistant's pool when a statement is cut off (T-7499, jc-prod 2026-11-22 11:25-11:49Z:
//! three of three connections held after the chunk query's statement timeouts, the chats 503).
//! The shared options every other module's pool starts from are held to the same.
//!
//! `JC_ASSISTANT_TEST_DATABASE_URL` names the server, as in store_tests.rs.

use std::time::Duration;

#[path = "../../../../../crates/shared/jc-pool/testkit/tls_front.rs"]
mod tls_front;

const POOL: usize = 3;

/// `options` at jc-prod's size over TLS, every caller gone before the server cancels its
/// statement: the pool still hands out all of its connections.
async fn gives_every_connection_back(options: sqlx::postgres::PgPoolOptions) {
    let url = std::env::var("JC_ASSISTANT_TEST_DATABASE_URL").unwrap_or_else(|_| {
        panic!("set JC_ASSISTANT_TEST_DATABASE_URL to a PostgreSQL (see store_tests.rs)")
    });
    let key = rcgen::KeyPair::generate().expect("a key");
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_owned()])
        .expect("params")
        .self_signed(&key)
        .expect("a certificate");
    let url = tls_front::tls_url(&url, cert.der().to_vec(), key.serialize_der()).await;
    let pool = options
        .max_connections(POOL as u32)
        .acquire_timeout(Duration::from_secs(3))
        .connect(&url)
        .await
        .expect("the database answers over TLS");

    let callers: Vec<_> = (0..POOL)
        .map(|_| {
            let pool = pool.clone();
            tokio::spawn(tokio::time::timeout(Duration::from_secs(1), async move {
                let mut tx = pool.begin().await?;
                sqlx::query("SET LOCAL statement_timeout = '2s'")
                    .execute(&mut *tx)
                    .await?;
                sqlx::query("SELECT pg_sleep(30)").execute(&mut *tx).await?;
                tx.commit().await
            }))
        })
        .collect();
    for caller in callers {
        let left = caller.await.expect("joined");
        assert!(left.is_err(), "the caller did not leave first: {left:?}");
    }

    // Past the server's cancel: a held connection never comes back.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let mut held = Vec::new();
        while held.len() < POOL {
            match pool.acquire().await {
                Ok(connection) => held.push(connection),
                Err(_) => break,
            }
        }
        if held.len() == POOL {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pool hands out {} of {POOL} connections",
            held.len()
        );
    }
}

#[tokio::test]
async fn the_assistants_pool_gives_back_every_cut_off_connection() {
    gives_every_connection_back(assistant::host::pool_options()).await;
}

#[tokio::test]
async fn the_shared_options_give_back_every_cut_off_connection() {
    gives_every_connection_back(jc_pool::options::<sqlx::Postgres>()).await;
}
