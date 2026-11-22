//! The client against a service that is down, failing or running another model: each is a
//! refusal the caller sees, never vectors.

mod common;

use jc_door::testing::Issuer;
use jc_embedder::client::{Remote, RemoteConfig};
use jc_embedder::{DIMENSIONS, Error, Kind, MODEL};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn remote(url: String) -> Remote {
    let issuer = Arc::new(Issuer::start("https://id.a.example/realms/a", "a1").await);
    let token_url = common::token_endpoint(issuer, "assistant-a", "embedder").await;
    Remote::new(RemoteConfig {
        url,
        token_url,
        client_id: "assistant-a".into(),
        client_secret: "s".into(),
    })
    .expect("client")
}

fn answering(sha: &'static str, status: u16) -> axum::Router {
    axum::Router::new().route(
        "/v1/embeddings",
        axum::routing::post(move || async move {
            let body = if status == 200 {
                json!({"model": MODEL, "model_sha256": sha,
                       "data": [{"index": 0, "embedding": vec![0.05_f32; DIMENSIONS]}]})
            } else {
                json!({"error": "the model failed on this request"})
            };
            (
                axum::http::StatusCode::from_u16(status).expect("status"),
                axum::Json(body),
            )
        }),
    )
}

/// A token endpoint like `common::token_endpoint` that counts the tokens it handed out.
async fn counted_token_endpoint(issuer: Arc<Issuer>, fetched: Arc<AtomicUsize>) -> String {
    common::serve(axum::Router::new().route(
        "/token",
        axum::routing::post(move || {
            let issuer = Arc::clone(&issuer);
            let fetched = Arc::clone(&fetched);
            async move {
                fetched.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({
                    "access_token": issuer.token("assistant-a", "embedder", 300),
                    "token_type": "Bearer",
                    "expires_in": 300,
                }))
            }
        }),
    ))
    .await
        + "/token"
}

/// A service that answers `statuses` in turn (the last one from then on), each 200 with one
/// vector per text it was sent; the requests it saw, by their text counts.
fn answering_in_turn(statuses: Vec<u16>, seen: Arc<std::sync::Mutex<Vec<usize>>>) -> axum::Router {
    axum::Router::new().route(
        "/v1/embeddings",
        axum::routing::post(move |axum::Json(req): axum::Json<serde_json::Value>| {
            let seen = Arc::clone(&seen);
            let statuses = statuses.clone();
            async move {
                let texts = req["input"].as_array().map_or(0, Vec::len);
                let mut seen = seen.lock().expect("lock");
                let status = statuses[seen.len().min(statuses.len() - 1)];
                seen.push(texts);
                let body = if status == 200 {
                    json!({"model": MODEL, "model_sha256": jc_embedder::MODEL_SHA256,
                           "data": (0..texts).map(|index| json!({"index": index,
                               "embedding": vec![0.05_f32; DIMENSIONS]})).collect::<Vec<_>>()})
                } else {
                    json!({"detail": "no error member"})
                };
                (
                    axum::http::StatusCode::from_u16(status).expect("status"),
                    axum::Json(body),
                )
            }
        }),
    )
}

#[tokio::test]
async fn a_url_that_is_not_http_is_refused_and_debug_shows_no_secret() {
    for url in ["ftp://embedder.example", "embedder.example", ""] {
        let got = Remote::new(RemoteConfig {
            url: url.into(),
            token_url: "http://127.0.0.1:1/token".into(),
            client_id: "assistant-a".into(),
            client_secret: "s3cret-value".into(),
        });
        assert!(
            matches!(&got, Err(Error::Refused(why)) if why.contains("not an http(s) URL")),
            "{url}: {got:?}"
        );
    }
    let remote = Remote::new(RemoteConfig {
        url: "https://embedder.example/".into(),
        token_url: "http://127.0.0.1:1/token".into(),
        client_id: "assistant-a".into(),
        client_secret: "s3cret-value".into(),
    })
    .expect("client");
    let shown = format!("{remote:?}");
    assert!(
        shown.contains("https://embedder.example/v1/embeddings") && shown.contains("assistant-a"),
        "{shown}"
    );
    assert!(!shown.contains("s3cret-value"), "{shown}");
}

#[tokio::test]
async fn no_token_is_a_refusal_and_no_request_goes_out() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let url = common::serve(answering_in_turn(vec![200], Arc::clone(&seen))).await;
    let closed = tokio::net::TcpSocket::new_v4().expect("socket");
    closed
        .bind("127.0.0.1:0".parse().expect("addr"))
        .expect("bind");
    let remote = Remote::new(RemoteConfig {
        url,
        token_url: format!("http://{}/token", closed.local_addr().expect("addr")),
        client_id: "assistant-a".into(),
        client_secret: "s".into(),
    })
    .expect("client");
    let got = remote.embed(Kind::Query, &["q".into()]).await;
    drop(closed);
    assert!(
        matches!(&got, Err(Error::Refused(why)) if why.contains("no token for the embedder")),
        "{got:?}"
    );
    assert!(seen.lock().expect("lock").is_empty());
}

/// One token serves every batch while it lives; a 401 drops it, so the next call signs in again.
#[tokio::test]
async fn texts_go_in_batches_on_one_token_and_a_401_fetches_a_new_one() {
    let issuer = Arc::new(Issuer::start("https://id.a.example/realms/a", "a1").await);
    let fetched = Arc::new(AtomicUsize::new(0));
    let token_url = counted_token_endpoint(issuer, Arc::clone(&fetched)).await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let url = common::serve(answering_in_turn(
        vec![200, 200, 401, 200],
        Arc::clone(&seen),
    ))
    .await;
    let remote = Remote::new(RemoteConfig {
        url,
        token_url,
        client_id: "assistant-a".into(),
        client_secret: "s".into(),
    })
    .expect("client");
    let texts: Vec<String> = (0..17).map(|i| format!("t{i}")).collect();
    let got = remote.embed(Kind::Passage, &texts).await.expect("vectors");
    assert_eq!(got.len(), 17);
    assert_eq!(
        *seen.lock().expect("lock"),
        [16, 1],
        "BATCH texts per request"
    );
    assert_eq!(
        fetched.load(Ordering::SeqCst),
        1,
        "one token for both batches"
    );
    let refused = remote.embed(Kind::Query, &["q".into()]).await;
    assert!(
        matches!(&refused, Err(Error::Refused(why)) if why.starts_with("401") && why.trim_end().ends_with("Unauthorized")),
        "an error body without an error member names only the status: {refused:?}"
    );
    assert_eq!(fetched.load(Ordering::SeqCst), 1);
    remote
        .embed(Kind::Query, &["q".into()])
        .await
        .expect("signed in again");
    assert_eq!(
        fetched.load(Ordering::SeqCst),
        2,
        "the 401 dropped the token"
    );
    assert!(
        remote
            .embed(Kind::Query, &[])
            .await
            .expect("nothing")
            .is_empty()
    );
    assert_eq!(
        seen.lock().expect("lock").len(),
        4,
        "no request for no texts"
    );
}

#[tokio::test]
async fn an_answer_that_is_not_json_or_over_the_cap_is_refused() {
    let garbage = common::serve(axum::Router::new().route(
        "/v1/embeddings",
        axum::routing::post(|| async { "not json" }),
    ))
    .await;
    let got = remote(garbage)
        .await
        .embed(Kind::Query, &["q".into()])
        .await;
    assert!(
        matches!(&got, Err(Error::Refused(why)) if why == "the answer is not an embedding answer"),
        "{got:?}"
    );
    let huge = common::serve(axum::Router::new().route(
        "/v1/embeddings",
        axum::routing::post(|| async { "x".repeat(4 * 1024 * 1024 + 1) }),
    ))
    .await;
    let got = remote(huge).await.embed(Kind::Query, &["q".into()]).await;
    assert!(
        matches!(&got, Err(Error::Unreachable(why)) if why.contains("could not be read")),
        "{got:?}"
    );
    let at_cap = common::serve(axum::Router::new().route(
        "/v1/embeddings",
        axum::routing::post(|| async { " ".repeat(4 * 1024 * 1024) }),
    ))
    .await;
    let got = remote(at_cap).await.embed(Kind::Query, &["q".into()]).await;
    assert!(
        matches!(&got, Err(Error::Refused(why)) if why == "the answer is not an embedding answer"),
        "an answer of exactly the cap is read: {got:?}"
    );
}

#[tokio::test]
async fn a_service_running_another_model_is_refused() {
    let url = common::serve(answering(
        "1111111111111111111111111111111111111111111111111111111111111111",
        200,
    ))
    .await;
    let got = remote(url).await.embed(Kind::Query, &["q".into()]).await;
    assert!(
        matches!(&got, Err(Error::Refused(why)) if why.contains("not the pinned")),
        "{got:?}"
    );
}

#[tokio::test]
async fn a_service_that_is_down_or_failing_is_a_refusal() {
    let failing = common::serve(answering(jc_embedder::MODEL_SHA256, 503)).await;
    let got = remote(failing)
        .await
        .embed(Kind::Passage, &["p".into()])
        .await;
    assert!(
        matches!(&got, Err(Error::Refused(why)) if why.contains("503")),
        "{got:?}"
    );
    // A port nothing listens on: the socket stays bound (without SO_REUSEADDR) and never
    // listens, so no parallel test's server can take the port and a connect is refused.
    let closed = tokio::net::TcpSocket::new_v4().expect("socket");
    closed
        .bind("127.0.0.1:0".parse().expect("addr"))
        .expect("bind");
    let url = format!("http://{}", closed.local_addr().expect("addr"));
    let got = remote(url).await.embed(Kind::Passage, &["p".into()]).await;
    drop(closed);
    assert!(matches!(got, Err(Error::Unreachable(_))), "{got:?}");
}

/// T-7290: a `413` names the texts as the cause, so the caller stops sending them; any other
/// refusal (503 above) is the service's and worth another try.
#[tokio::test]
async fn a_text_over_the_cap_is_too_large_not_a_passing_refusal() {
    let url = common::serve(answering(jc_embedder::MODEL_SHA256, 413)).await;
    let got = remote(url).await.embed(Kind::Passage, &["p".into()]).await;
    assert!(
        matches!(&got, Err(Error::TooLarge(why)) if why.contains("413")),
        "{got:?}"
    );
}

#[tokio::test]
async fn the_pinned_model_answers_go_through() {
    let url = common::serve(answering(jc_embedder::MODEL_SHA256, 200)).await;
    let got = remote(url)
        .await
        .embed(Kind::Query, &["q".into()])
        .await
        .expect("vectors");
    assert_eq!(got.len(), 1);
}
