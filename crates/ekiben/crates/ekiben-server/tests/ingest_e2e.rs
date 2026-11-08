//! E4 end to end: a streams-mode server with the shared ingest listener, an `http_server` input
//! posted through the API and answered by a `sync_response` output.
//!
//! Streams mode and the ingest listener are process-wide, so they live in this test binary alone.

use std::time::Duration;

use ekiben_core::{OrgId, ResourceManager};
use ekiben_server::{build_router, ServerAuthConfig, ServerState, StreamManager};
use ekiben_testkit::full_registry;
use reqwest::StatusCode;
use tokio::net::TcpListener;

struct Servers {
    api: String,
    ingest: String,
}

async fn start() -> Servers {
    ekiben_io_http::http_server::set_streams_mode();
    let ingest = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ingest_addr = ingest.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = ekiben_io_http::ingest::serve(ingest).await;
    });
    let manager = StreamManager::new(full_registry(), ResourceManager::new(), None, false);
    let auth = ServerAuthConfig {
        org_tokens: vec![
            ("tok-a".to_string(), OrgId::new("org-a")),
            ("tok-b".to_string(), OrgId::new("org-b")),
        ],
        ..Default::default()
    };
    let app = build_router(ServerState::new(manager, auth));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // Let the ingest task mark itself active before a stream connects.
    tokio::time::sleep(Duration::from_millis(50)).await;
    Servers {
        api: format!("http://{addr}"),
        ingest: format!("http://{ingest_addr}"),
    }
}

async fn post_until_routed(req: impl Fn() -> reqwest::RequestBuilder) -> reqwest::Response {
    for _ in 0..100 {
        let res = req().send().await.unwrap();
        if res.status() != StatusCode::NOT_FOUND {
            return res;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the route never appeared");
}

const UPPER: &str = r#"
input:
  http_server:
    path: /in
    auth:
      bearer: ingest-e4-token
pipeline:
  processors:
    - mapping: 'root = content().uppercase()'
output:
  sync_response: {}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ingest_request_is_answered_by_the_sync_response_output() {
    let s = start().await;
    let c = reqwest::Client::new();
    let res = c
        .post(format!("{}/streams/upper", s.api))
        .bearer_auth("tok-a")
        .body(UPPER)
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "{}",
        res.text().await.unwrap()
    );

    let url = format!("{}/org-a/upper/in", s.ingest);
    let res = post_until_routed(|| c.post(&url).bearer_auth("ingest-e4-token").body("hello")).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.text().await.unwrap(),
        "HELLO",
        "acked only after the output answered"
    );

    let res = c.post(&url).body("hello").send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = c
        .post(format!("{}/org-b/upper/in", s.ingest))
        .bearer_auth("ingest-e4-token")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "another organization's path"
    );

    // Deleting the stream takes its route away.
    let res = c
        .delete(format!("{}/streams/upper", s.api))
        .bearer_auth("tok-a")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut status = StatusCode::OK;
    for _ in 0..50 {
        status = c
            .post(&url)
            .bearer_auth("ingest-e4-token")
            .body("x")
            .send()
            .await
            .unwrap()
            .status();
        if status == StatusCode::NOT_FOUND {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A config without auth, or with its own listener, does not lint in streams mode.
    for (id, conf) in [
        ("noauth", "input:\n  http_server:\n    path: /in\noutput:\n  sync_response: {}\n"),
        (
            "ownaddr",
            "input:\n  http_server:\n    address: 0.0.0.0:4195\n    auth:\n      public: true\noutput:\n  sync_response: {}\n",
        ),
    ] {
        let res = c
            .post(format!("{}/streams/{id}", s.api))
            .bearer_auth("tok-a")
            .body(conf)
            .send()
            .await
            .unwrap();
        assert!(!res.status().is_success(), "{id}");
        let body = res.text().await.unwrap();
        assert!(body.contains("http_server"), "{id}: {body}");
    }
}
