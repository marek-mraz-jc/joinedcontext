//! The fetch and packages routes against an https upstream (AG-50, AG-65, T-7795).
//!
//! Both routes reach https alone, so their answers were struck as untestable in the per-route
//! suites. Here a local TLS server with a certificate made for the test answers as a documentation
//! host would: a page, something that runs, an answer past the run's cap, one that breaks off, a
//! redirect loop and a redirect to no URL. The proxy's egress client is the suite's, resolving the
//! one listed name to the local server and trusting its certificate, and nothing else.

mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::{app, authed, body_of, sample_run, state, Bases};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;
use tower::ServiceExt;

const HOST: &str = "docs.example.test";
const CAP: u64 = 64;
const BUDGET: u64 = 10_000;

/// What the documentation host answers each path with.
fn answer(path: &str) -> Vec<u8> {
    let whole = |status: &str, headers: &str, body: &str| {
        format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    };
    match path {
        "/doc" => whole("200 OK", "Content-Type: text/markdown; charset=utf-8\r\n", "# serde"),
        "/bin" => whole("200 OK", "Content-Type: application/x-sh\r\n", "rm -rf /"),
        "/plain" => whole("200 OK", "", "untyped"),
        "/big" => whole("200 OK", "Content-Type: text/plain\r\n", &"x".repeat(100)),
        "/cut" => b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 50\r\nConnection: close\r\n\r\npart".to_vec(),
        "/loop" => whole("302 Found", "Location: /loop\r\n", ""),
        "/nowhere" => whole("302 Found", "Location: https://[::1\r\n", ""),
        "/api/v1/crates/serde/1.0.0/download" => {
            whole("200 OK", "Content-Type: application/gzip\r\n", "crate-bytes")
        }
        _ => whole("404 Not Found", "", ""),
    }
}

/// The local https host and a client that reaches it by its listed name alone.
async fn upstream() -> reqwest::Client {
    let key = rcgen::KeyPair::generate().expect("a key");
    let cert = rcgen::CertificateParams::new(vec![HOST.to_owned()])
        .expect("params")
        .self_signed(&key)
        .expect("a certificate");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
    )
    .expect("the test certificate");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let addr = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut head = Vec::new();
                let mut chunk = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&chunk[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&head).into_owned();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let _ = tls.write_all(&answer(&path)).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .resolve(HOST, addr)
        .add_root_certificate(reqwest::Certificate::from_der(cert.der()).expect("the root"))
        .build()
        .expect("a client")
}

async fn proxy() -> axum::Router {
    let mut run = sample_run(false);
    run.allowed_hosts = vec![HOST.to_owned()];
    run.max_response_bytes = CAP;
    run.max_egress_bytes_per_run = BUDGET;
    let mut proxy_state = (*state(run, Bases::default())).clone();
    proxy_state.egress = upstream().await;
    agent_proxy::router(Arc::new(proxy_state))
}

async fn fetch(proxy: &axum::Router, path: &str) -> axum::http::Response<Body> {
    let url = format!("https://{HOST}{path}");
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("url", &url)
        .finish();
    proxy
        .clone()
        .oneshot(
            authed("GET", &format!("/v1/fetch?{query}"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer")
}

/// A page is served with its own type and the budget left after it; an untyped answer is served
/// as bytes; the budget shrinks by what each answer weighed.
#[tokio::test]
async fn a_readable_page_is_served_with_its_type_and_the_budget_left() {
    let proxy = proxy().await;
    let page = fetch(&proxy, "/doc").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        page.headers()["content-type"],
        "text/markdown; charset=utf-8"
    );
    assert_eq!(
        page.headers()["x-jc-egress-remaining"],
        (BUDGET - 7).to_string().as_str()
    );
    assert_eq!(body_of(page).await, "# serde");
}

/// Something that runs is refused after its headers and before its body; an answer with no type
/// at all is refused the same way, and says so.
#[tokio::test]
async fn a_type_that_runs_or_no_type_is_refused_before_the_body() {
    let proxy = proxy().await;
    let script = fetch(&proxy, "/bin").await;
    assert_eq!(script.status(), StatusCode::FORBIDDEN);
    assert!(body_of(script)
        .await
        .contains("'application/x-sh' is not a readable content type"));
    let untyped = fetch(&proxy, "/plain").await;
    assert_eq!(untyped.status(), StatusCode::FORBIDDEN);
    assert!(body_of(untyped).await.contains("'(none)'"));
}

/// An answer past the run's `maxResponseBytes` is a `413` naming the cap; one that breaks off is
/// the host's `502` naming the host, never a short page.
#[tokio::test]
async fn an_answer_past_the_cap_or_broken_off_is_refused_with_why() {
    let proxy = proxy().await;
    let big = fetch(&proxy, "/big").await;
    assert_eq!(big.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(body_of(big)
        .await
        .contains(&format!("maxResponseBytes ({CAP})")));
    let cut = fetch(&proxy, "/cut").await;
    assert_eq!(cut.status(), StatusCode::BAD_GATEWAY);
    assert!(body_of(cut)
        .await
        .contains(&format!("{HOST} stopped answering")));
}

/// A redirect chain past five hops is refused; a redirect to something that is not a URL is the
/// host's fault, named with the location it gave.
#[tokio::test]
async fn a_redirect_loop_or_a_redirect_to_no_url_is_refused() {
    let proxy = proxy().await;
    let looped = fetch(&proxy, "/loop").await;
    assert_eq!(looped.status(), StatusCode::FORBIDDEN);
    assert!(body_of(looped).await.contains("more than 5 redirects"));
    let nowhere = fetch(&proxy, "/nowhere").await;
    assert_eq!(nowhere.status(), StatusCode::BAD_GATEWAY);
    assert!(body_of(nowhere).await.contains("is not a URL"));
}

/// A package download from a listed registry is relayed as it came, with its status.
#[tokio::test]
async fn a_package_download_from_a_listed_registry_is_relayed() {
    let proxy = proxy().await;
    let download = proxy
        .clone()
        .oneshot(
            authed(
                "GET",
                &format!("/v1/packages/{HOST}/api/v1/crates/serde/1.0.0/download"),
            )
            .body(Body::empty())
            .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(body_of(download).await, "crate-bytes");
    let missing = proxy
        .oneshot(
            authed("GET", &format!("/v1/packages/{HOST}/no/such/crate"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

/// The listed host over plain http is refused before any connection is made: the deployment's
/// own wiring is enough to prove it, no upstream needed.
#[tokio::test]
async fn the_listed_host_over_plain_http_is_refused_before_a_connection() {
    let mut run = sample_run(false);
    run.allowed_hosts = vec![HOST.to_owned()];
    run.max_egress_bytes_per_run = BUDGET;
    let answer = app(run, Bases::default())
        .oneshot(
            authed("GET", &format!("/v1/fetch?url=http%3A%2F%2F{HOST}%2Fdoc"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::FORBIDDEN);
    assert!(body_of(answer).await.contains("scheme 'http' is refused"));
}
