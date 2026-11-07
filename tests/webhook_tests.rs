use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use joinedcontext_portal::api::webhook::{EVENT_HEADER, SIGNATURE_HEADER};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::reconciler::Syncer;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::json;
use sha2::Sha256;
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

type HmacSha256 = Hmac<Sha256>;

fn compute_signature(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(body);
    let result = mac.finalize().into_bytes();
    result
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

#[tokio::test]
async fn pr_merged_event_accepted_and_triggers_sync() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

    let secret = "webhook-secret-key";
    let mut config = Config::for_tests();
    config.gitea_webhook_secret = Some(secret.to_string());

    let state = AppState::new(config, None)
        .with_gitea(client)
        .with_syncer(syncer);
    let app = server::app(state);

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches/main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "main",
            "commit": { "id": "rev-head-pr" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/git/trees/rev-head-pr",
        ))
        .and(query_param("recursive", "true"))
        .and(query_param("per_page", "1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "tree-sha-1",
            "truncated": false,
            "tree": [
                {
                    "path": "projects/ovzdusie/spaces/mobility/space.yaml",
                    "type": "blob"
                }
            ]
        })))
        .mount(&server)
        .await;

    let manifest = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
    let b64 = STANDARD.encode(manifest.as_bytes());

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .and(query_param("ref", "rev-head-pr"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": b64
        })))
        .mount(&server)
        .await;

    let pr_payload = json!({
        "action": "closed",
        "pull_request": {
            "merged": true
        }
    });
    let body_bytes = serde_json::to_vec(&pr_payload).unwrap();
    let signature = compute_signature(secret, &body_bytes);

    // Note: neither cookie header nor x-csrf-token header are sent
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/gitea")
                .header(header::CONTENT_TYPE, "application/json")
                .header(SIGNATURE_HEADER, signature)
                .header(EVENT_HEADER, "pull_request")
                .body(Body::from(body_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    // The answer does not wait for the sync it starts (F-R2POR3-017).
    for _ in 0..200 {
        if mirror.len() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let requests = server.received_requests().await.unwrap();
    let default_branch_calls = requests
        .iter()
        .filter(|r| {
            r.method.as_str() == "GET" && r.url.path() == "/api/v1/repos/test-owner/test-repo"
        })
        .count();
    assert_eq!(default_branch_calls, 1);
    assert_eq!(mirror.len(), 1);
    assert!(mirror.get("ovzdusie", "ContextSpace", "mobility").is_some());
}

#[tokio::test]
async fn wrong_signature_returns_401_and_calls_no_gitea_endpoints() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

    let secret = "webhook-secret-key";
    let mut config = Config::for_tests();
    config.gitea_webhook_secret = Some(secret.to_string());

    let state = AppState::new(config, None)
        .with_gitea(client)
        .with_syncer(syncer);
    let app = server::app(state);

    let pr_payload = json!({
        "action": "closed",
        "pull_request": {
            "merged": true
        }
    });
    let body_bytes = serde_json::to_vec(&pr_payload).unwrap();
    let wrong_signature = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/gitea")
                .header(header::CONTENT_TYPE, "application/json")
                .header(SIGNATURE_HEADER, wrong_signature)
                .header(EVENT_HEADER, "pull_request")
                .body(Body::from(body_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let requests = server.received_requests().await.unwrap();
    assert!(requests.is_empty());
}

#[tokio::test]
async fn missing_signature_header_returns_401() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

    let secret = "webhook-secret-key";
    let mut config = Config::for_tests();
    config.gitea_webhook_secret = Some(secret.to_string());

    let state = AppState::new(config, None)
        .with_gitea(client)
        .with_syncer(syncer);
    let app = server::app(state);

    let pr_payload = json!({
        "action": "closed",
        "pull_request": {
            "merged": true
        }
    });
    let body_bytes = serde_json::to_vec(&pr_payload).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/gitea")
                .header(header::CONTENT_TYPE, "application/json")
                .header(EVENT_HEADER, "pull_request")
                .body(Body::from(body_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let requests = server.received_requests().await.unwrap();
    assert!(requests.is_empty());
}

#[tokio::test]
async fn invalid_hex_signature_returns_401() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

    let secret = "webhook-secret-key";
    let mut config = Config::for_tests();
    config.gitea_webhook_secret = Some(secret.to_string());

    let state = AppState::new(config, None)
        .with_gitea(client)
        .with_syncer(syncer);
    let app = server::app(state);

    let pr_payload = json!({
        "action": "closed",
        "pull_request": {
            "merged": true
        }
    });
    let body_bytes = serde_json::to_vec(&pr_payload).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/gitea")
                .header(header::CONTENT_TYPE, "application/json")
                .header(SIGNATURE_HEADER, "not-a-valid-hex-string!")
                .header(EVENT_HEADER, "pull_request")
                .body(Body::from(body_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let requests = server.received_requests().await.unwrap();
    assert!(requests.is_empty());
}

#[tokio::test]
async fn unconfigured_webhook_secret_returns_503() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config, None));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/gitea")
                .header(header::CONTENT_TYPE, "application/json")
                .header(SIGNATURE_HEADER, "deadbeef1234")
                .header(EVENT_HEADER, "pull_request")
                .body(Body::from(r#"{"action":"closed"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(problem.status, 503);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/service-unavailable"
    );
    assert!(problem
        .detail
        .as_deref()
        .unwrap()
        .contains("webhook secret"));
}

#[tokio::test]
async fn unhandled_event_returns_204_and_triggers_no_sync() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().unwrap();
    let client =
        Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
    let mirror = Arc::new(Mirror::new());
    let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

    let secret = "webhook-secret-key";
    let mut config = Config::for_tests();
    config.gitea_webhook_secret = Some(secret.to_string());

    let state = AppState::new(config, None)
        .with_gitea(client)
        .with_syncer(syncer);
    let app = server::app(state);

    let opened_payload = json!({
        "action": "opened",
        "pull_request": {
            "merged": false
        }
    });
    let body_bytes = serde_json::to_vec(&opened_payload).unwrap();
    let signature = compute_signature(secret, &body_bytes);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/gitea")
                .header(header::CONTENT_TYPE, "application/json")
                .header(SIGNATURE_HEADER, signature)
                .header(EVENT_HEADER, "pull_request")
                .body(Body::from(body_bytes))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let requests = server.received_requests().await.unwrap();
    assert!(requests.is_empty());
}
