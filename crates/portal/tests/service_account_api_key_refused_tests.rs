//! T-4618 (PF-36, PF-104): the platform keeps no API keys (T-4362), so a ServiceAccount that
//! declares an `api-key` credential is no manifest the Portal takes: its apply answers 400, its
//! dry run is invalid and red, both name the credential kind, and nothing reaches Git.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::MockServer;

const CSRF: &str = "test-csrf-api-key-token";

fn cookies(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar =
        session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("session");
    let response = (jar, StatusCode::OK).into_response();
    let session: Vec<_> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    format!("{}; {CSRF_COOKIE}={CSRF}", session.join("; "))
}

fn account(kind: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "ServiceAccount",
        "metadata": { "name": "vendorx", "namespace": "ovzdusie" },
        "spec": {
            "owner": { "user": "jana.kovacova" },
            "purpose": "VendorX pushes ParkingSpot updates",
            "roles": [],
            "credentials": [{ "kind": kind, "name": "legacy-push" }]
        }
    })
}

/// The status and body of one POST of `payload`, and how many requests Gitea received.
async fn post(query: &str, payload: &Value) -> (StatusCode, String, usize) {
    send(
        "POST",
        &format!("/api/v1/projects/ovzdusie/serviceaccounts{query}"),
        payload,
    )
    .await
}

/// One request against a Portal whose mirror already holds `vendorx` with an OAuth client.
async fn send(method: &str, uri: &str, payload: &Value) -> (StatusCode, String, usize) {
    let gitea = MockServer::start().await;
    let client =
        GiteaClient::new(gitea.uri().parse().expect("url"), "o", "r", "token-xyz").expect("client");
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let existing = account("oauth-client");
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ServiceAccount".to_string(),
        metadata: ObjectMeta {
            name: "vendorx".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: existing["spec"].clone(),
        status: None,
    });
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let sent = gitea.received_requests().await.expect("requests").len();
    (status, String::from_utf8_lossy(&body).into_owned(), sent)
}

#[tokio::test]
async fn an_api_key_credential_is_refused_400_by_the_apply() {
    let (status, body, sent) = post("", &account("api-key")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("api-key"), "the error names the kind: {body}");
    assert_eq!(sent, 0, "a refused manifest reaches no Git");
}

/// A dry run answers 200 for every manifest and carries its verdict (CC-29): here invalid, red,
/// with one error finding that names the credential kind.
#[tokio::test]
async fn the_dry_run_of_an_api_key_credential_is_invalid_and_names_the_kind() {
    let (status, body, sent) = post("?dryRun=All", &account("api-key")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result: Value = serde_json::from_str(&body).expect("a dry-run result");
    assert_eq!(result["valid"], json!(false), "{body}");
    assert_eq!(result["lane"], json!("red"), "{body}");
    let findings = result["verdict"]["findings"].as_array().expect("findings");
    assert!(
        findings.iter().any(|f| f["level"] == "error"
            && f["message"]
                .as_str()
                .is_some_and(|m| m.contains("`api-key`"))),
        "{body}"
    );
    assert_eq!(sent, 0);
}

/// The refusal is the kind, not the account: the same manifest with an OAuth client passes the
/// dry run.
#[tokio::test]
async fn the_same_account_with_an_oauth_client_passes_the_dry_run() {
    let (status, body, sent) = post("?dryRun=All", &account("oauth-client")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(sent, 0);
}

/// Edge cases: a key hidden behind a valid credential, an update rather than a create, and the
/// kind in other letters are each refused, and none reaches Git.
#[tokio::test]
async fn an_api_key_is_refused_second_in_the_list_on_an_update_and_in_any_case() {
    let mut second = account("oauth-client");
    second["metadata"]["name"] = json!("vendory");
    second["spec"]["credentials"]
        .as_array_mut()
        .expect("credentials")
        .push(json!({ "kind": "api-key", "name": "legacy-push-2" }));
    let (status, body, sent) = post("", &second).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "second credential: {body}");
    assert!(body.contains("api-key"), "{body}");
    assert_eq!(sent, 0);

    let uri = "/api/v1/projects/ovzdusie/serviceaccounts/vendorx";
    let (status, body, sent) = send("PUT", uri, &account("api-key")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "update: {body}");
    assert!(body.contains("api-key"), "{body}");
    assert_eq!(sent, 0);

    for kind in ["API-Key", "api_key", "apiKey"] {
        let mut other = account(kind);
        other["metadata"]["name"] = json!("vendorz");
        let (status, body, sent) = post("", &other).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{kind}: {body}");
        assert_eq!(sent, 0, "{kind}");
    }
}
