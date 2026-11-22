//! The Portal's pool when a statement is cut off (T-7494, the jc-prod outage of 2026-11-22): the
//! runner stops waiting after 5 s, the request's future is dropped, and Postgres cancels the trim
//! later. sqlx 0.9.0 hung closing such a connection over TLS and never gave its slot back, so the
//! database here is reached over TLS.
//!
//! Runs when `JC_PORTAL_TEST_DATABASE_URL` points at a PostgreSQL the test may write to; otherwise
//! it says so and returns, like the other database suites.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use joinedcontext_portal::pipeline_log::{LogStore, LINES_KEPT};
use p256::pkcs8::EncodePrivateKey;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;

mod common;
use common::unique;

const POOL: usize = 5;

/// A TLS front of the test database, which has none in CI, and the URL through it. The key is a
/// test constant of no secret; the certificate is its self-signed one.
async fn tls_url(url: &str) -> String {
    let options = sqlx::postgres::PgConnectOptions::from_str(url).expect("a URL");
    let server = (options.get_host().to_owned(), options.get_port());
    let key = p256::SecretKey::from_slice(&[7; 32])
        .expect("a scalar")
        .to_pkcs8_der()
        .expect("an encoded key")
        .as_bytes()
        .to_vec();
    let cert = include_bytes!("fixtures/db-relay.cert.der").to_vec();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.into()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(key).into(),
    )
    .expect("the test certificate");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let front = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = front.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((mut client, _)) = front.accept().await {
            let (acceptor, server) = (acceptor.clone(), server.clone());
            tokio::spawn(async move {
                // Postgres's SSLRequest, answered `S` before the handshake.
                let mut request = [0; 8];
                client.read_exact(&mut request).await?;
                client.write_all(b"S").await?;
                let client = acceptor.accept(client).await?;
                let server = tokio::net::TcpStream::connect(server).await?;
                let (mut from_client, mut to_client) = tokio::io::split(client);
                let (mut from_server, mut to_server) = server.into_split();
                let up = async {
                    tokio::io::copy(&mut from_client, &mut to_server).await?;
                    to_server.shutdown().await
                };
                let down = async {
                    let mut answer = [0; 16384];
                    loop {
                        let mut read = from_server.read(&mut answer).await?;
                        if read == 0 {
                            return to_client.shutdown().await;
                        }
                        // An error and the answers queued behind it go out as one record: the
                        // client has then read all there is, which is when the close hung.
                        if answer[0] == b'E' {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            read += from_server.try_read(&mut answer[read..]).unwrap_or(0);
                        }
                        to_client.write_all(&answer[..read]).await?;
                        to_client.flush().await?;
                    }
                };
                // Either side hanging up ends the connection, as a real server's exit does.
                tokio::select! {
                    done = up => done,
                    done = down => done,
                }
            });
        }
    });
    let mut through = url::Url::parse(url).expect("a URL");
    through.set_host(Some("127.0.0.1")).expect("a host");
    through.set_port(Some(port)).expect("a port");
    through.query_pairs_mut().append_pair("sslmode", "require");
    through.into()
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
