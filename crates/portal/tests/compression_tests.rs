//! gzip on the Portal's answers (T-5598): a space's grid read 480 KB of JSON uncompressed, about
//! three seconds of a phone's Slow 4G before the first row. A client that asks gets gzip, one that
//! does not gets the bytes as they are, and an event stream is never buffered by an encoder.

use std::io::Read;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;

async fn get(accept_encoding: Option<&str>) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut request = Request::builder().uri("/api/v1/openapi.json");
    if let Some(value) = accept_encoding {
        request = request.header(header::ACCEPT_ENCODING, value);
    }
    let response = server::app(AppState::new(Config::for_tests(), None))
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let encoding = response
        .headers()
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, encoding, body.to_vec())
}

#[tokio::test]
async fn an_answer_is_gzipped_for_a_client_that_asks() {
    let (status, encoding, body) = get(Some("gzip, deflate, br, zstd")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(encoding.as_deref(), Some("gzip"));
    let mut plain = String::new();
    flate2::read::GzDecoder::new(body.as_slice())
        .read_to_string(&mut plain)
        .expect("a gzip body");
    let (_, _, identity) = get(None).await;
    assert_eq!(plain.as_bytes(), identity.as_slice());
    assert!(
        body.len() * 3 < identity.len(),
        "{} gzip bytes against {} plain",
        body.len(),
        identity.len()
    );
}

#[tokio::test]
async fn an_answer_stays_plain_for_a_client_that_does_not_ask() {
    let (status, encoding, body) = get(None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(encoding, None);
    assert!(body.starts_with(b"{"), "{:?}", &body[..body.len().min(16)]);
}
