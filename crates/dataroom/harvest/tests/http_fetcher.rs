//! The production fetcher against a plain-HTTP server on 127.0.0.1. The guard refuses http:// and
//! loopback, so these tests build the [`ValidatedTarget`] by hand: they cover the transport (the
//! pinned address, redirects, status, byte caps, timeouts), not the guard.

use dataroom_harvest::{FetchResponse, HttpFetcher, OutboundFetcher, ValidatedTarget};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves one connection: reads the request head, writes `answer`, then keeps the socket open
/// for `hold` before closing it. Returns the request head it read.
async fn serve_once(
    answer: Vec<u8>,
    hold: Duration,
) -> (SocketAddr, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if sock.read(&mut byte).await.unwrap_or(0) == 0 {
                break;
            }
            head.push(byte[0]);
        }
        let _ = sock.write_all(&answer).await;
        tokio::time::sleep(hold).await;
        String::from_utf8_lossy(&head).into_owned()
    });
    (addr, task)
}

/// A target on a name nothing resolves, pinned to the test server.
fn target(addr: SocketAddr) -> ValidatedTarget {
    ValidatedTarget {
        url: format!("http://harvest.test:{}/feed?page=1", addr.port())
            .parse()
            .unwrap(),
        host: "harvest.test".into(),
        pinned_addrs: vec![addr],
    }
}

async fn fetch(
    answer: &str,
    max: usize,
    timeout: Duration,
    hold: Duration,
) -> (dataroom_core::Result<FetchResponse>, String) {
    let (addr, server) = serve_once(answer.as_bytes().to_vec(), hold).await;
    let got = HttpFetcher::new()
        .fetch_hop(&target(addr), "text/turtle", max, timeout)
        .await;
    server.abort();
    let head = server.await.unwrap_or_default();
    (got, head)
}

const SEC: Duration = Duration::from_secs(5);
const NOW: Duration = Duration::ZERO;

#[tokio::test]
async fn a_200_answer_comes_back_with_its_content_type_from_the_pinned_address() {
    let (addr, server) = serve_once(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/turtle\r\nContent-Length: 5\r\n\r\nhello".to_vec(),
        NOW,
    )
    .await;
    let got = HttpFetcher::new()
        .fetch_hop(&target(addr), "text/turtle", 100, SEC)
        .await
        .expect("fetch");
    let head = server.await.unwrap();
    match got {
        FetchResponse::Ok { content_type, body } => {
            assert_eq!(content_type.as_deref(), Some("text/turtle"));
            assert_eq!(body, "hello");
        }
        other => panic!("{other:?}"),
    }
    assert!(head.starts_with("GET /feed?page=1 HTTP/1.1\r\n"), "{head}");
    let lower = head.to_ascii_lowercase();
    assert!(lower.contains("accept: text/turtle\r\n"), "{head}");
    assert!(
        lower.contains("user-agent: dataroom-harvest/1.0\r\n"),
        "{head}"
    );
    assert!(
        lower.contains(&format!("host: harvest.test:{}\r\n", addr.port())),
        "{head}"
    );
}

#[tokio::test]
async fn a_redirect_is_handed_back_for_the_engine_to_check() {
    let (got, _) = fetch(
        "HTTP/1.1 302 Found\r\nLocation: https://elsewhere.example/x\r\nContent-Length: 0\r\n\r\n",
        100,
        SEC,
        NOW,
    )
    .await;
    match got.expect("redirect") {
        FetchResponse::Redirect { status, location } => {
            assert_eq!(
                (status, location.as_str()),
                (302, "https://elsewhere.example/x")
            );
        }
        other => panic!("{other:?}"),
    }
    let (got, _) = fetch(
        "HTTP/1.1 301 Moved\r\nContent-Length: 0\r\n\r\n",
        100,
        SEC,
        NOW,
    )
    .await;
    let err = got.unwrap_err().to_string();
    assert!(err.contains("redirect 301 without Location"), "{err}");
}

#[tokio::test]
async fn an_error_status_fails_the_fetch() {
    for (answer, code) in [
        (
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4\r\n\r\noops",
            "500",
        ),
        ("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n", "404"),
        ("HTTP/1.1 204 No Content\r\n\r\n", ""),
    ] {
        let (got, _) = fetch(answer, 100, SEC, NOW).await;
        if code.is_empty() {
            assert!(
                matches!(got, Ok(FetchResponse::Ok { ref body, .. }) if body.is_empty()),
                "{got:?}"
            );
        } else {
            let err = got.unwrap_err().to_string();
            assert!(err.contains(&format!("answered HTTP {code}")), "{err}");
        }
    }
}

#[tokio::test]
async fn a_body_over_the_cap_is_refused_by_length_or_while_streaming() {
    let (got, _) = fetch(
        "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world",
        10,
        SEC,
        NOW,
    )
    .await;
    let err = got.unwrap_err().to_string();
    assert!(
        err.contains("Content-Length 11 exceeds cap of 10 bytes"),
        "{err}"
    );

    // No Content-Length: the cap holds while the chunks arrive.
    let (got, _) = fetch(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\nhello \r\n5\r\nworld\r\n0\r\n\r\n",
        10,
        SEC,
        NOW,
    )
    .await;
    let err = got.unwrap_err().to_string();
    assert!(err.contains("body exceeded cap of 10 bytes"), "{err}");

    // Exactly the cap passes.
    let (got, _) = fetch(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n",
        10,
        SEC,
        NOW,
    )
    .await;
    assert!(
        matches!(got, Ok(FetchResponse::Ok { ref body, .. }) if body == "helloworld"),
        "{got:?}"
    );
}

#[tokio::test]
async fn a_body_cut_short_or_an_answer_that_never_comes_fails() {
    // The server promises 100 bytes, sends 5 and closes.
    let (got, _) = fetch(
        "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nhello",
        1000,
        SEC,
        NOW,
    )
    .await;
    let err = got.unwrap_err().to_string();
    assert!(err.contains("read http://harvest.test"), "{err}");

    // The server reads the request and never answers.
    let started = std::time::Instant::now();
    let (got, _) = fetch(
        "",
        1000,
        Duration::from_millis(300),
        Duration::from_secs(30),
    )
    .await;
    let err = got.unwrap_err().to_string();
    assert!(err.contains("fetch http://harvest.test"), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );

    // Nothing listens on the pinned address.
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = free.local_addr().unwrap();
    drop(free);
    let err = HttpFetcher::new()
        .fetch_hop(&target(addr), "*/*", 10, SEC)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("fetch http://harvest.test"), "{err}");
}

#[tokio::test]
async fn a_target_without_a_checked_address_is_never_fetched() {
    let mut t = target("127.0.0.1:9".parse().unwrap());
    t.pinned_addrs.clear();
    let err = HttpFetcher::new()
        .fetch_hop(&t, "*/*", 10, SEC)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("harvest.test has no checked address; refusing an unpinned fetch"),
        "{err}"
    );
}
