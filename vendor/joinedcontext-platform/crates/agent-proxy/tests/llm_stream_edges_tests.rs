//! How a streamed model call ends when it does not end well (ADR-N-032, AG-41, T-7795): the
//! caller leaves halfway, the provider breaks the stream off, or the provider falls silent. Each
//! is counted as a stream without usage, the most it was allowed, so leaving early is never
//! cheaper than reading to the end. And a provider that is not Anthropic gets the key as a bearer.

mod common;

use axum::body::Body;
use axum::http::StatusCode;
use common::{app, authed, body_of, sample_run, state, Bases, RUN_ID};
use http_body_util::BodyExt;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

/// The run's budget, and what one streamed call asks for.
const BUDGET: u64 = 100_000;
const ASKED: u64 = 50;

/// A provider that answers with `head` and the stream's first event, then does what `then` says.
#[derive(Clone, Copy)]
enum Then {
    /// Holds the connection open and sends nothing more.
    Hold,
    /// Closes the connection inside a chunk.
    BreakOff,
}

async fn provider(then: Then) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let addr = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut chunk = [0u8; 4096];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&chunk[..n]),
                    }
                }
                let event = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{event}\r\n",
                    event.len()
                );
                if socket.write_all(answer.as_bytes()).await.is_err() {
                    return;
                }
                match then {
                    Then::Hold => std::future::pending::<()>().await,
                    Then::BreakOff => {
                        let _ = socket.write_all(b"40\r\ndata: {\"cho").await;
                    }
                }
            });
        }
    });
    format!("http://{addr}")
}

fn streamed() -> Body {
    Body::from(format!(
        r#"{{"model":"m","stream":true,"max_tokens":{ASKED},"messages":[]}}"#
    ))
}

/// What the run has spent after a call that reported no usage.
async fn spent(state: &agent_proxy::ProxyState) -> u64 {
    for _ in 0..100 {
        let left = state.limits.tokens_left(RUN_ID, BUDGET).await;
        if left < BUDGET {
            return BUDGET - left;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    0
}

fn proxy_state(model: &str) -> Arc<agent_proxy::ProxyState> {
    let mut run = sample_run(false);
    run.max_tokens = BUDGET;
    state(
        run,
        Bases {
            model: model.to_owned(),
            ..Bases::default()
        },
    )
}

/// A caller that reads the first event and leaves is counted as the most the call was allowed;
/// the relay stops with it.
#[tokio::test]
async fn a_caller_that_leaves_halfway_pays_for_the_whole_call() {
    let model = provider(Then::Hold).await;
    let proxy_state = proxy_state(&model);
    let answer = agent_proxy::router(Arc::clone(&proxy_state))
        .oneshot(
            authed("POST", "/v1/llm/v1/chat/completions")
                .body(streamed())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::OK);
    let mut body = answer.into_body();
    let first = body
        .frame()
        .await
        .expect("a frame")
        .expect("the first event");
    assert!(first
        .into_data()
        .is_ok_and(|data| data.starts_with(b"data:")));
    drop(body);
    assert!(
        spent(&proxy_state).await >= ASKED,
        "leaving early is never cheaper"
    );
}

/// A stream the provider breaks off reaches the caller as an error, never as a clean end, and
/// is counted as the most it was allowed.
#[tokio::test]
async fn a_stream_the_provider_breaks_off_ends_in_an_error_and_is_counted_whole() {
    let model = provider(Then::BreakOff).await;
    let proxy_state = proxy_state(&model);
    let answer = agent_proxy::router(Arc::clone(&proxy_state))
        .oneshot(
            authed("POST", "/v1/llm/v1/chat/completions")
                .body(streamed())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::OK);
    let read = answer.into_body().collect().await;
    assert!(read.is_err(), "a broken stream read as a whole one");
    assert!(spent(&proxy_state).await >= ASKED);
}

/// A provider silent for two minutes is a hung provider: the caller gets an error naming the
/// silence and the call is counted as the most it was allowed. The clock is paused, so the two
/// minutes pass as soon as nothing else can happen.
#[tokio::test(start_paused = true)]
async fn a_provider_silent_for_two_minutes_ends_the_stream_with_why() {
    let model = provider(Then::Hold).await;
    let proxy_state = proxy_state(&model);
    let answer = agent_proxy::router(Arc::clone(&proxy_state))
        .oneshot(
            authed("POST", "/v1/llm/v1/chat/completions")
                .body(streamed())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::OK);
    let read = answer.into_body().collect().await;
    let error = read.expect_err("the silence ended the stream");
    assert!(
        format!("{error:?}").contains("sent nothing for 120 s"),
        "{error:?}"
    );
    assert!(spent(&proxy_state).await >= ASKED);
}

/// A provider that is not Anthropic gets the key as a bearer and no Anthropic header.
#[tokio::test]
async fn an_openai_compatible_provider_gets_the_key_as_a_bearer() {
    let provider = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer mock-model-key",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&provider)
        .await;
    let mut openai = (*proxy_state(&provider.uri())).clone();
    let mut config = (*openai.config).clone();
    config.model_provider = "openai".to_owned();
    openai.config = Arc::new(config);
    let answer = agent_proxy::router(Arc::new(openai))
        .oneshot(
            authed("POST", "/v1/llm/v1/chat/completions")
                .body(Body::from(r#"{"model":"m","messages":[]}"#))
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::OK);
    let sent = provider.received_requests().await.unwrap_or_default();
    assert!(sent.iter().all(|r| !r.headers.contains_key("x-api-key")));
}

/// A stream asked of a provider that is not there is the provider's `502`, never an empty stream.
#[tokio::test]
async fn a_stream_asked_of_a_provider_that_is_not_there_is_a_502() {
    let answer = app(sample_run(false), Bases::default())
        .oneshot(
            authed("POST", "/v1/llm/v1/chat/completions")
                .body(streamed())
                .expect("a request"),
        )
        .await
        .expect("an answer");
    assert_eq!(answer.status(), StatusCode::BAD_GATEWAY);
    assert!(body_of(answer).await.contains("model provider"));
}
