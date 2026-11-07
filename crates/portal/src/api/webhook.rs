//! Gitea webhook receiver endpoint (CC-03, CC-08, CC-18).
//!
//! Authenticates server-to-server callbacks from Gitea via HMAC-SHA256 signatures,
//! exempt from session cookies and CSRF double-submit tokens.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::error::{ApiError, ProblemDetails};
use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

pub const SIGNATURE_HEADER: &str = "x-gitea-signature";
pub const EVENT_HEADER: &str = "x-gitea-event";

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.is_empty() || !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().as_chunks::<2>().0 {
        let high = (chunk[0] as char).to_digit(16)? as u8;
        let low = (chunk[1] as char).to_digit(16)? as u8;
        bytes.push((high << 4) | low);
    }
    Some(bytes)
}

/// Verifies the HMAC-SHA256 signature of a webhook payload against the secret.
///
/// Uses constant-time comparison via [`Mac::verify_slice`]. A non-hex or invalid
/// `presented_hex` returns `false` without panicking.
pub fn verify_signature(secret: &str, body: &[u8], presented_hex: &str) -> bool {
    let hex_str = presented_hex.trim();
    let Some(expected_bytes) = decode_hex(hex_str) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected_bytes).is_ok()
}

fn is_pr_closed_merged(payload: &Value) -> bool {
    let action_closed = payload.get("action").and_then(Value::as_str) == Some("closed");
    let pr_merged = payload
        .get("pull_request")
        .and_then(|pr| pr.get("merged"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    action_closed && pr_merged
}

fn is_default_branch_push(payload: &Value) -> bool {
    let Some(git_ref) = payload.get("ref").and_then(Value::as_str) else {
        return false;
    };
    if git_ref.starts_with("refs/tags/") {
        return false;
    }
    let branch = git_ref.strip_prefix("refs/heads/").unwrap_or(git_ref);

    let default_branch = payload
        .get("repository")
        .and_then(|r| r.get("default_branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");

    branch == default_branch
}

fn should_sync(headers: &HeaderMap, payload: &Value) -> bool {
    let event = headers
        .get(EVENT_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase());

    match event.as_deref() {
        Some("pull_request") => is_pr_closed_merged(payload),
        Some("push") => is_default_branch_push(payload),
        Some(_) => false,
        None => {
            if payload.get("pull_request").is_some() {
                is_pr_closed_merged(payload)
            } else if payload.get("ref").is_some() {
                is_default_branch_push(payload)
            } else {
                false
            }
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/webhooks/gitea",
    summary = "Forge Webhook",
    description = "Called by the forge on a push: a signed payload makes the mirror synchronize. An unsigned or wrongly signed call is refused with 401.",
    tag = "system",
    params(
        ("x-gitea-signature" = String, Header, description = "HMAC-SHA256 signature of request body"),
        ("x-gitea-event" = Option<String>, Header, description = "Gitea event type (e.g. push, pull_request)"),
    ),
    request_body(
        content = Object,
        description = "Gitea webhook event payload",
        content_type = "application/json",
        example = json!({
            "ref": "refs/heads/main",
            "after": "4f2a9c1d0e8b7a6f5e4d3c2b1a0f9e8d7c6b5a49",
            "repository": { "full_name": "hel/config" }
        }),
    ),
    responses(
        (status = 202, description = "Webhook accepted and synchronization triggered"),
        (status = 204, description = "Webhook accepted with no action needed"),
        (status = 400, description = "Invalid payload", body = ProblemDetails),
        (status = 401, description = "Missing or invalid signature", body = ProblemDetails),
        (status = 503, description = "Webhook secret not configured", body = ProblemDetails),
    )
)]
pub async fn gitea_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    // 1. Webhook secret must be configured; fail closed (503) if missing. During a rotation
    //    two are configured and both are accepted, because the forge's own hook changes in a
    //    separate step and a window where every push is refused is why nobody rotates (T-0982).
    let secrets: Vec<&str> = [
        state.config.gitea_webhook_secret.as_deref(),
        state.config.gitea_webhook_secret_previous.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if secrets.is_empty() {
        return Err(ApiError::Unavailable(
            "gitea webhook secret is not configured".into(),
        ));
    }

    // 2. Validate HMAC-SHA256 signature before deserializing payload body.
    // Order matters: missing or invalid signature returns 401 without parsing JSON.
    let presented_sig = headers
        .get(SIGNATURE_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;

    // Every configured secret is tried, and each comparison is the constant-time one: a
    // signature made with the secret being retired verifies until it is removed.
    if !secrets
        .iter()
        .any(|secret| verify_signature(secret, &body, presented_sig))
    {
        return Err(ApiError::Unauthorized);
    }

    // 3. Only parse the payload after the signature check has succeeded.
    let payload: Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?;

    if !should_sync(&headers, &payload) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    // 4. Start the mirror reconciliation and answer 202 at once: a full sync takes minutes on
    // dev, longer than the forge waits for a delivery, and a request the forge gives up on would
    // drop the sync with it (F-R2POR3-017). An error is recorded in SyncStatus.
    if let Some(syncer) = state.syncer.clone() {
        syncer.merged();
    }

    Ok(StatusCode::ACCEPTED.into_response())
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/webhooks/gitea", axum::routing::post(gitea_webhook))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::config::Config;
    use crate::git::GiteaClient;
    use crate::reconciler::Syncer;
    use crate::server;
    use crate::store::Mirror;

    fn compute_signature(secret: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("slice");
        mac.update(body);
        let result = mac.finalize().into_bytes();
        result
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    }

    #[test]
    fn verify_signature_scenarios() {
        let secret = "correct-horse-battery-staple";
        let body = b"hello from gitea webhook";
        let valid_sig = compute_signature(secret, body);

        // 1. Correct signature passes
        assert!(verify_signature(secret, body, &valid_sig));

        // Uppercase hex passes
        assert!(verify_signature(
            secret,
            body,
            &valid_sig.to_ascii_uppercase()
        ));

        // Surrounding whitespace in signature passes
        assert!(verify_signature(secret, body, &format!("  {valid_sig}  ")));

        // 2. Wrong signature fails
        let wrong_sig = format!("00{}", &valid_sig[2..]);
        assert!(!verify_signature(secret, body, &wrong_sig));

        // Different body fails
        assert!(!verify_signature(secret, b"tampered payload", &valid_sig));

        // Different secret fails
        assert!(!verify_signature("wrong-secret", body, &valid_sig));

        // 3. Non-hex string fails
        assert!(!verify_signature(
            secret,
            body,
            "this-is-not-a-valid-hex-string!"
        ));

        // 4. Empty presented value fails
        assert!(!verify_signature(secret, body, ""));
        assert!(!verify_signature(secret, body, "   "));

        // Odd length hex fails
        assert!(!verify_signature(
            secret,
            body,
            &valid_sig[..valid_sig.len() - 1]
        ));
    }

    #[tokio::test]
    async fn webhook_without_configured_secret_returns_503() {
        let app = server::app(AppState::new(Config::for_tests(), None));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, "dummy-sig")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let prob: ProblemDetails = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(prob.status, 503);
        assert!(prob.detail.unwrap().contains("webhook secret"));
    }

    #[tokio::test]
    async fn webhook_without_signature_header_returns_401() {
        let mut config = Config::for_tests();
        config.gitea_webhook_secret = Some("secret123".into());
        let app = server::app(AppState::new(config, None));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_with_wrong_signature_returns_401_without_parsing_body() {
        let mut config = Config::for_tests();
        config.gitea_webhook_secret = Some("secret123".into());
        let app = server::app(AppState::new(config, None));

        // Notice: body is completely invalid JSON. If it deserialized first, it would be 400.
        // But because signature check fails first, it must return 401.
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, "deadbeef1234")
                    .body(Body::from("this is not json at all {{{{"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn webhook_with_valid_signature_invalid_json_returns_400() {
        let secret = "secret123";
        let mut config = Config::for_tests();
        config.gitea_webhook_secret = Some(secret.into());
        let app = server::app(AppState::new(config, None));

        let body_bytes = b"not json at all {{{{";
        let sig = compute_signature(secret, body_bytes);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, sig)
                    .body(Body::from(&body_bytes[..]))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// T-0982: a webhook secret is shared with the forge, so the two sides cannot change in the
    /// same instant. Both are accepted for as long as the rotation takes, and the retired one is
    /// removed afterwards — after which its signature is refused like any other.
    #[tokio::test]
    async fn a_rotation_accepts_the_retiring_secret_until_it_is_removed() {
        let old_secret = "the-one-gitea-still-has";
        let new_secret = "the-one-written-today";
        let payload = json!({ "action": "opened" });
        let body = serde_json::to_vec(&payload).unwrap();

        let post = |config: Config, signature: String, body: Vec<u8>| async move {
            server::app(AppState::new(config, None))
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/webhooks/gitea")
                        .header(SIGNATURE_HEADER, signature)
                        .header(EVENT_HEADER, "pull_request")
                        .header(axum::http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap()
                .status()
        };

        let rotating = || {
            let mut config = Config::for_tests();
            config.gitea_webhook_secret = Some(new_secret.into());
            config.gitea_webhook_secret_previous = Some(old_secret.into());
            config
        };

        // During the overlap both sign a request the Portal accepts.
        assert_eq!(
            post(
                rotating(),
                compute_signature(new_secret, &body),
                body.clone()
            )
            .await,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            post(
                rotating(),
                compute_signature(old_secret, &body),
                body.clone()
            )
            .await,
            StatusCode::NO_CONTENT
        );

        // A secret that is neither is refused while both are configured.
        assert_eq!(
            post(
                rotating(),
                compute_signature("somebody-elses", &body),
                body.clone()
            )
            .await,
            StatusCode::UNAUTHORIZED
        );

        // Once the rotation is finished the retired secret is a stranger's.
        let mut finished = Config::for_tests();
        finished.gitea_webhook_secret = Some(new_secret.into());
        assert_eq!(
            post(finished, compute_signature(old_secret, &body), body.clone()).await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// Fail closed: with neither secret configured nothing is accepted, whatever it is signed
    /// with, and the answer says the Portal cannot judge rather than that the caller is wrong.
    #[tokio::test]
    async fn with_no_secret_at_all_every_webhook_is_refused() {
        let body = serde_json::to_vec(&json!({ "action": "opened" })).unwrap();
        let response = server::app(AppState::new(Config::for_tests(), None))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, compute_signature("anything", &body))
                    .header(EVENT_HEADER, "pull_request")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn webhook_csrf_exemption_allows_post_without_csrf_tokens() {
        let secret = "secret123";
        let mut config = Config::for_tests();
        config.gitea_webhook_secret = Some(secret.into());
        let app = server::app(AppState::new(config, None));

        let payload = json!({ "action": "opened" });
        let body = serde_json::to_vec(&payload).unwrap();
        let sig = compute_signature(secret, &body);

        // Deliberately no session cookie and no X-CSRF-Token header.
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, sig)
                    .header(EVENT_HEADER, "pull_request")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Must NOT be 403 Forbidden! Action is opened, so 204 No Content.
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn webhook_ignored_events_return_204() {
        let secret = "secret123";
        let mut config = Config::for_tests();
        config.gitea_webhook_secret = Some(secret.into());
        let app = server::app(AppState::new(config, None));

        // 1. PR closed but not merged
        let pr_unmerged = json!({
            "action": "closed",
            "pull_request": { "merged": false }
        });
        let body1 = serde_json::to_vec(&pr_unmerged).unwrap();
        let sig1 = compute_signature(secret, &body1);
        let resp1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, sig1)
                    .header(EVENT_HEADER, "pull_request")
                    .body(Body::from(body1))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::NO_CONTENT);

        // 2. Push to non-default branch
        let push_feat = json!({
            "ref": "refs/heads/feature-abc",
            "repository": { "default_branch": "main" }
        });
        let body2 = serde_json::to_vec(&push_feat).unwrap();
        let sig2 = compute_signature(secret, &body2);
        let resp2 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, sig2)
                    .header(EVENT_HEADER, "push")
                    .body(Body::from(body2))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::NO_CONTENT);

        // 3. Tag push
        let push_tag = json!({
            "ref": "refs/tags/v1.0.0",
            "repository": { "default_branch": "main" }
        });
        let body3 = serde_json::to_vec(&push_tag).unwrap();
        let sig3 = compute_signature(secret, &body3);
        let resp3 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, sig3)
                    .header(EVENT_HEADER, "push")
                    .body(Body::from(body3))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp3.status(), StatusCode::NO_CONTENT);

        // 4. Other event (e.g. issues)
        let issue_event = json!({ "action": "created" });
        let body4 = serde_json::to_vec(&issue_event).unwrap();
        let sig4 = compute_signature(secret, &body4);
        let resp4 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, sig4)
                    .header(EVENT_HEADER, "issues")
                    .body(Body::from(body4))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp4.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn webhook_pr_merged_triggers_sync_and_answers_202() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client =
            Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
        let mirror = Arc::new(Mirror::new());
        let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

        let secret = "secret123";
        let mut config = Config::for_tests();
        config.gitea_webhook_secret = Some(secret.into());

        let state = AppState::new(config, None)
            .with_gitea(client)
            .with_syncer(Arc::clone(&syncer));
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
                "commit": { "id": "rev-pr-merge" }
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/git/trees/rev-pr-merge",
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

        let manifest_content = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
        let b64 = base64::engine::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            manifest_content.as_bytes(),
        );

        Mock::given(method("GET"))
            .and(path(
                "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
            ))
            .and(query_param("ref", "rev-pr-merge"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-1",
                "content": b64
            })))
            .mount(&server)
            .await;

        let pr_merged = json!({
            "action": "closed",
            "pull_request": {
                "merged": true
            }
        });
        let body = serde_json::to_vec(&pr_merged).unwrap();
        let sig = compute_signature(secret, &body);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, sig)
                    .header(EVENT_HEADER, "pull_request")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        settled(&syncer, |status| status.revision.is_some()).await;
        assert_eq!(mirror.len(), 1);
        assert!(mirror.get("ovzdusie", "ContextSpace", "mobility").is_some());

        let status = syncer.status();
        assert_eq!(status.manifests, 1);
        assert_eq!(status.revision.as_deref(), Some("rev-pr-merge"));
        assert!(status.last_error.is_none());
    }

    #[tokio::test]
    async fn webhook_sync_error_still_answers_202_and_records_status() {
        let server = MockServer::start().await;
        let base_url = server.uri().parse().unwrap();
        let client =
            Arc::new(GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").unwrap());
        let mirror = Arc::new(Mirror::new());
        let syncer = Arc::new(Syncer::new(Arc::clone(&client), Arc::clone(&mirror)));

        let secret = "secret123";
        let mut config = Config::for_tests();
        config.gitea_webhook_secret = Some(secret.into());

        let state = AppState::new(config, None)
            .with_gitea(client)
            .with_syncer(Arc::clone(&syncer));
        let app = server::app(state);

        // Forge returns 500, slower than the forge waits for a delivery.
        Mock::given(method("GET"))
            .and(path("/api/v1/repos/test-owner/test-repo"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_string("gitea db down")
                    .set_delay(std::time::Duration::from_secs(2)),
            )
            .mount(&server)
            .await;

        let push_main = json!({
            "ref": "refs/heads/main",
            "repository": { "default_branch": "main" }
        });
        let body = serde_json::to_vec(&push_main).unwrap();
        let sig = compute_signature(secret, &body);

        let started = std::time::Instant::now();
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/webhooks/gitea")
                    .header(SIGNATURE_HEADER, sig)
                    .header(EVENT_HEADER, "push")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Crucial requirement: must return 202 so Gitea does not retry merge webhook!
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        // F-R2POR3-017: the answer does not wait for the sync.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "the webhook waited {:?} for the sync",
            started.elapsed()
        );

        // Error is recorded in syncer status
        settled(&syncer, |status| status.last_error.is_some()).await;
    }

    /// Waits up to ten seconds for the sync the webhook started to reach `done`.
    async fn settled(syncer: &Syncer, done: impl Fn(&crate::reconciler::SyncStatus) -> bool) {
        for _ in 0..200 {
            if done(&syncer.status()) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the sync did not finish: {:?}", syncer.status());
    }
}
