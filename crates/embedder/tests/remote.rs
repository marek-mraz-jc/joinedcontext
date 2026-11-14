//! The client against a service that is down, failing or running another model: each is a
//! refusal the caller sees, never vectors.

mod common;

use jc_door::testing::Issuer;
use jc_embedder::client::{Remote, RemoteConfig};
use jc_embedder::{DIMENSIONS, Error, Kind, MODEL};
use serde_json::json;
use std::sync::Arc;

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
