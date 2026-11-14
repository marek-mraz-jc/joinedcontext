//! A proxy named in the environment never carries a harvest fetch: the proxy would resolve the
//! host itself, past the pinned, checked address. Its own test binary, because it sets the
//! process environment.

use dataroom_harvest::{FetchResponse, HttpFetcher, OutboundFetcher, ValidatedTarget};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn answering(body: &'static str) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let answer = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(answer.as_bytes()).await;
        }
    });
    (addr, task)
}

#[tokio::test]
async fn harvest_fetch_ignores_proxy_env() {
    let (proxy, proxy_task) = answering("from the proxy").await;
    let (pinned, pinned_task) = answering("from the pinned address").await;
    for var in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
    ] {
        // Edition 2021 and one test in this binary: nothing reads the environment concurrently.
        std::env::set_var(var, format!("http://{proxy}"));
    }
    std::env::remove_var("NO_PROXY");
    std::env::remove_var("no_proxy");
    let target = ValidatedTarget {
        url: format!("http://harvest.test:{}/feed", pinned.port())
            .parse()
            .unwrap(),
        host: "harvest.test".into(),
        pinned_addrs: vec![pinned],
    };
    let got = HttpFetcher::new()
        .fetch_hop(&target, "text/turtle", 1 << 20, Duration::from_secs(5))
        .await
        .unwrap();
    proxy_task.abort();
    pinned_task.abort();
    match got {
        FetchResponse::Ok { body, .. } => assert_eq!(body, "from the pinned address"),
        other => panic!("{other:?}"),
    }
}
