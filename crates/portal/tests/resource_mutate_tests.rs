use std::sync::Arc;

mod common;
use common::{mirror_holding, CheckFirst};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use joinedcontext_portal::api::mutate::branch_name;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{Change, ChangePhase, Lane, Operation};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;

use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_CSRF_TOKEN: &str = "test-csrf-token-12345";

fn make_session_cookie(config: &Config) -> String {
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
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        let pair = raw.split(';').next().unwrap_or_default();
        parts.push(pair.to_string());
    }
    parts.join("; ")
}

fn session_and_csrf_cookies(config: &Config) -> String {
    let session = make_session_cookie(config);
    format!("{session}; {CSRF_COOKIE}={TEST_CSRF_TOKEN}")
}

/// A new space's Change carries its empty draft model beside the manifest (T-2699, DM-61, DM-62):
/// the forge of a space create answers the model's manifest and its LinkML source, and each is
/// written exactly once.
async fn mount_space_model(server: &MockServer) {
    for file in ["mobility.yaml", "mobility.linkml.yaml"] {
        let model = format!(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/datamodels/{file}"
        );
        Mock::given(method("GET"))
            .and(path(model.as_str()))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })),
            )
            .mount(server)
            .await;
        Mock::given(method("PUT"))
            .and(path(model.as_str()))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(json!({ "commit": { "sha": "commit-sha-model" } })),
            )
            .expect(1)
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn create_returns_202_with_change_and_commits_to_gitea() {
    let server = MockServer::start().await;
    mount_space_model(&server).await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "not found"
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-created" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 42,
            "html_url": "https://gitea.example.sk/pulls/42",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change: Change = serde_json::from_slice(&body_bytes).expect("deserialize Change");
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(
        change.status.merge_request.as_deref(),
        Some("https://gitea.example.sk/pulls/42")
    );
    assert_eq!(change.status.lane, Lane::Green);

    let requests = server.received_requests().await.expect("received requests");

    let branch_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "POST"
                && r.url.path() == "/api/v1/repos/test-owner/test-repo/branches"
        })
        .expect("branch creation request");
    let branch_body: serde_json::Value =
        serde_json::from_slice(&branch_req.body).expect("branch request body");
    assert!(branch_body["new_branch_name"]
        .as_str()
        .expect("new branch name")
        .starts_with("portal/create-contextspace-mobility-"));

    let put_req = requests
        .iter()
        .find(|r| {
            r.method.as_str() == "PUT"
                && r.url
                    .path()
                    .starts_with("/api/v1/repos/test-owner/test-repo/contents/")
        })
        .expect("PUT contents request");
    let put_body: serde_json::Value =
        serde_json::from_slice(&put_req.body).expect("PUT request body");
    assert_eq!(put_body["author"]["name"], "Demo Steward");

    let content_b64 = put_body["content"].as_str().expect("base64 content");
    let decoded_bytes = STANDARD.decode(content_b64).expect("valid base64 content");
    let yaml_str = String::from_utf8(decoded_bytes).expect("valid UTF-8 YAML string");
    assert!(!yaml_str.contains("status:"));
    // The commit holds the one layout `jcctl fmt` writes, as a hand edit would (T-5052).
    assert_eq!(
        jc_core::yaml_fmt::format(&yaml_str).as_deref(),
        Ok(yaml_str.as_str())
    );
    assert!(yaml_str.starts_with("apiVersion: "), "{yaml_str}");

    assert!(requests.iter().any(|r| {
        r.method.as_str() == "POST" && r.url.path() == "/api/v1/repos/test-owner/test-repo/pulls"
    }));
}

#[tokio::test]
async fn create_invalid_manifest_bodies_return_400() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    // 1. Foreign namespace
    let foreign_ns_body = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "foreign-project"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let resp_foreign = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&foreign_ns_body).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_foreign.status(), StatusCode::BAD_REQUEST);
    let bytes_foreign = resp_foreign
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_foreign: ProblemDetails =
        serde_json::from_slice(&bytes_foreign).expect("ProblemDetails");
    assert_eq!(problem_foreign.status, 400);
    assert!(problem_foreign
        .detail
        .as_deref()
        .expect("detail")
        .contains("foreign-project"));

    // 2. Status block in body (MF-04)
    let with_status_body = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        },
        "status": {
            "phase": "Live"
        }
    });

    let resp_status = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&with_status_body).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_status.status(), StatusCode::BAD_REQUEST);
    let bytes_status = resp_status
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_status: ProblemDetails =
        serde_json::from_slice(&bytes_status).expect("ProblemDetails");
    assert_eq!(problem_status.status, 400);
    assert!(problem_status
        .detail
        .as_deref()
        .expect("detail")
        .contains("status is computed by the platform"));

    // 3. Literal token string (MF-24)
    let with_token_body = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "auth": {
                "token": "ghp_super_secret_token_literal"
            }
        }
    });

    let resp_token = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&with_token_body).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_token.status(), StatusCode::BAD_REQUEST);
    let bytes_token = resp_token
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem_token: ProblemDetails =
        serde_json::from_slice(&bytes_token).expect("ProblemDetails");
    assert_eq!(problem_token.status, 400);
    assert!(problem_token
        .detail
        .as_deref()
        .expect("detail")
        .contains("literal secret in field 'token' is forbidden"));
}

/// T-0412, MF-37: the kind's own invariants run at write time. An inline Bloblang mapping on a
/// `mapping` step is what jc-core refuses (PL-41); the Portal refuses it too, before a branch.
#[tokio::test]
async fn create_with_a_spec_the_kind_refuses_returns_400_naming_the_field() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None));
    let body = json!({
        "apiVersion": API_VERSION,
        "kind": "Pipeline",
        "metadata": { "name": "aq-derived", "namespace": "ovzdusie" },
        "spec": {
            "class": "resident",
            "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt-mesto" } },
            "compute": { "kind": "mapping", "bloblang": "root = this" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air"
        }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/pipelines")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("Pipeline") && detail.contains("bloblang"),
        "{detail}"
    );
}

/// MF-37: a field the kind does not define is refused with a 400 that names it, before any
/// branch or Change exists: the forge sees no request at all.
#[tokio::test]
async fn create_with_a_field_the_kind_does_not_define_is_400_and_writes_nothing() {
    let forge = MockServer::start().await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("valid mock server url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None).with_gitea(Arc::new(client)));
    let body = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true, "colour": "red" }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("colour"),
        "the failing field is named: {detail}"
    );
    let calls = forge.received_requests().await.unwrap_or_default();
    assert!(calls.is_empty(), "the forge was reached: {calls:?}");
}

#[tokio::test]
async fn create_with_unknown_plural_returns_404() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {}
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/unknownplurals")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 404);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/resource-not-found"
    );
    assert!(problem
        .detail
        .as_deref()
        .expect("detail")
        .contains("unknownplurals"));
}

#[tokio::test]
async fn replace_with_name_mismatch_returns_400() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "body-space-name",
            "namespace": "ovzdusie"
        },
        "spec": {}
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/spaces/path-space-name")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 400);
    assert!(problem
        .detail
        .as_deref()
        .expect("detail")
        .contains("metadata.name 'body-space-name' does not match path 'path-space-name'"));
}

#[tokio::test]
async fn patch_unsupported_media_type_returns_415() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let patch_payload = json!({
        "spec": {
            "isSandbox": false
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&patch_payload).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 415);
    assert!(problem
        .detail
        .as_deref()
        .expect("detail")
        .contains("application/merge-patch+json"));
}

#[tokio::test]
async fn patch_merge_patch_json_changes_field_and_returns_202() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-space-old",
            "content": "YXBpVmVyc2lvbjogeW91"
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "commit": { "sha": "commit-sha-patched" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 101,
            "html_url": "https://gitea.example.sk/pulls/101",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: "mobility".to_string(),
            namespace: Some("ovzdusie".to_string()),
            ..Default::default()
        },
        spec: json!({ "isSandbox": false }),
        status: None,
    });

    let app = server::app(state);

    let patch_payload = json!({
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot_checked(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/projects/ovzdusie/spaces/mobility")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/merge-patch+json")
                .body(Body::from(
                    serde_json::to_vec(&patch_payload).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change: Change = serde_json::from_slice(&bytes).expect("deserialize Change");
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(change.status.plan.update, 1);
    assert_eq!(
        change.status.merge_request.as_deref(),
        Some("https://gitea.example.sk/pulls/101")
    );
}

#[tokio::test]
async fn lane_classification_public_endpoint_red_and_sandbox_space_green() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/endpoints/public-air.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "message": "not found"
        })))
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/endpoints/public-air.yaml",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-endpoint" }
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 88,
            "html_url": "https://gitea.example.sk/pulls/88",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    // The space this manifest names is in the project, as it has to be: a manifest naming a
    // resource that is not there is refused by the check (MF-13, T-2233).
    let state = AppState::new(config.clone(), None)
        .with_gitea(Arc::new(client))
        .with_mirror(mirror_holding("ovzdusie", &[("ContextSpace", "mobility")]));
    let app = server::app(state);

    // 1. Public Endpoint -> Red Lane
    let public_endpoint = json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": {
            "name": "public-air",
            "namespace": "ovzdusie",
            "labels": {
                "joinedcontext.com/space": "mobility"
            }
        },
        "spec": {
            "contextSpaceRef": "mobility",
            "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }
    });

    let resp_endpoint = app
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/endpoints")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&public_endpoint).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(resp_endpoint.status(), StatusCode::ACCEPTED);
    let bytes_ep = resp_endpoint
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let change_ep: Change = serde_json::from_slice(&bytes_ep).expect("deserialize Change");
    assert_eq!(change_ep.status.lane, Lane::Red);
}

#[tokio::test]
async fn create_without_forge_returns_503() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 503);
    assert_eq!(
        problem.r#type,
        "https://joinedcontext.com/errors/service-unavailable"
    );
    assert!(problem
        .detail
        .as_deref()
        .expect("detail")
        .contains("git forge is not configured"));
}

#[tokio::test]
async fn create_without_csrf_header_returns_403() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": {
            "name": "mobility",
            "namespace": "ovzdusie"
        },
        "spec": {
            "isSandbox": true
        }
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, make_session_cookie(&config))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&bytes).expect("ProblemDetails");
    assert_eq!(problem.status, 403);
    assert_eq!(problem.r#type, "https://joinedcontext.com/errors/forbidden");
}

/// A second proposal for a resource whose change is still open (T-0883, CC-34, PF-52): refused
/// before a byte reaches the branch, the open change named, nothing of the forge's own words.
#[tokio::test]
async fn a_second_proposal_while_a_change_is_open_names_it_and_writes_nothing() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    let open_branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Create);
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 175,
                "html_url": "https://gitea.example.sk/pulls/175",
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": open_branch },
                "base": { "ref": "main" },
                "created_at": "2026-09-15T20:00:00Z",
                "user": { "login": "jana.kovacova", "full_name": "Jana Kováčová" },
                "mergeable": true,
                "merged": false
            }
        ])))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = app
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&body).expect("problem");
    let detail = problem.detail.expect("detail");
    assert_eq!(
        detail,
        "a change for ContextSpace 'mobility' is already open: chg-000000af; approve or reject it first"
    );
    assert!(!detail.contains("pull request"), "no forge words: {detail}");

    let requests = server.received_requests().await.expect("received requests");
    assert!(
        !requests.iter().any(|r| r.method.as_str() != "GET"),
        "nothing was written to the forge"
    );
}

/// A change that a retry opened on a suffixed branch (T-0887) is the same resource: the second
/// proposal is refused and names it.
#[tokio::test]
async fn an_open_change_on_a_suffixed_branch_still_blocks_a_proposal() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    let open_branch = format!(
        "{}_0badf00d",
        branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Create)
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 176,
                "html_url": "https://gitea.example.sk/pulls/176",
                "state": "open",
                "title": "create ContextSpace mobility",
                "head": { "ref": open_branch },
                "base": { "ref": "main" },
                "created_at": "2026-09-15T20:00:00Z",
                "user": { "login": "jana.kovacova", "full_name": "Jana Kováčová" },
                "mergeable": true,
                "merged": false
            }
        ])))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = server::app(state)
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    let problem: ProblemDetails = serde_json::from_slice(&body).expect("problem");
    assert_eq!(
        problem.detail.expect("detail"),
        "a change for ContextSpace 'mobility' is already open: chg-000000b0; approve or reject it first"
    );
    let requests = server.received_requests().await.expect("received requests");
    assert!(
        !requests.iter().any(|r| r.method.as_str() != "GET"),
        "nothing was written to the forge"
    );
}

/// An open change on another resource is nobody's business here: the proposal goes through.
#[tokio::test]
async fn an_open_change_elsewhere_does_not_block_a_proposal() {
    let server = MockServer::start().await;
    mount_space_model(&server).await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "number": 9,
                "html_url": "https://gitea.example.sk/pulls/9",
                "state": "open",
                "head": { "ref": branch_name("ovzdusie", "ContextSpace", "parking", Operation::Create) },
                "base": { "ref": "main" },
                "merged": false
            }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 43,
            "html_url": "https://gitea.example.sk/pulls/43",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let app = server::app(state);
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = app
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

/// A proposal's branch left by an earlier attempt is recreated from main (T-0886), so what the
/// approver reviews is this proposal alone.
#[tokio::test]
async fn a_stale_proposal_branch_is_recreated_from_main() {
    let server = MockServer::start().await;
    mount_space_model(&server).await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(409).set_body_string("branch already exists"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    let branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Create);
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/branches/{branch}"
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 44,
            "html_url": "https://gitea.example.sk/pulls/44",
            "state": "open",
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = server::app(state)
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&payload).expect("json bytes"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    let names: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/branches"))
        .map(|r| {
            r.body_json::<serde_json::Value>().expect("branch body")["new_branch_name"]
                .as_str()
                .expect("name")
                .to_string()
        })
        .collect();
    assert_eq!(names[0], branch, "the deterministic name is tried first");
    assert!(
        names[1].starts_with(&format!("{branch}_")) && names[1] != branch,
        "the retry opens on a fresh name: {}",
        names[1]
    );
    let heads: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
        .map(|r| {
            r.body_json::<serde_json::Value>().expect("pull body")["head"]
                .as_str()
                .expect("head")
                .to_string()
        })
        .collect();
    assert_eq!(
        heads,
        vec![names[1].clone()],
        "the pull request opens on the fresh name"
    );
}

#[tokio::test]
async fn a_kind_the_platform_cannot_load_is_refused_before_anything_is_written() {
    // T-0833: `Entity` is declared in Architecture/06 and jc-core does not define it: a seed is a
    // plain NGSI-LD `.json`, never a manifest. Every loader refuses an unknown kind and refuses
    // the whole repository with it, so one such file committed here would stop configuration
    // reaching every endpoint. `Subscription` used to be the other one, until T-0913.
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None));
    let body = json!({
        "apiVersion": API_VERSION,
        "kind": "Entity",
        "metadata": { "name": "air-alerts", "namespace": "ovzdusie" },
        "spec": { "type": "AirQualityObserved" }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/entities")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("Entity") && detail.contains("not defined"),
        "{detail}"
    );
}

/// T-0913: the kind the Portal used to refuse is written like any other now, and the file lands
/// in the space its `contextSpaceRef` names rather than in a folder named after the project.
#[tokio::test]
async fn a_subscription_is_proposed_into_the_space_it_watches() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    let file = "projects/ovzdusie/spaces/air-quality/subscriptions/air-alerts.yaml";

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{file}"
        )))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{file}"
        )))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-created" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 7,
            "html_url": "https://gitea.example.sk/pulls/7",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    // The space this manifest names is in the project, as it has to be: a manifest naming a
    // resource that is not there is refused by the check (MF-13, T-2233).
    let state = AppState::new(config.clone(), None)
        .with_gitea(Arc::new(client))
        .with_mirror(mirror_holding(
            "ovzdusie",
            &[("ContextSpace", "air-quality")],
        ));
    let app = server::app(state);

    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "Subscription",
        "metadata": { "name": "air-alerts", "namespace": "ovzdusie" },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air-quality" },
            "entities": [{ "type": "AirQualityObserved" }],
            "notification": {
                "endpoint": { "uri": "https://alerts.example.fi/hooks/air-quality" }
            }
        }
    });
    let response = app
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/subscriptions")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let requests = server.received_requests().await.expect("received requests");
    assert!(
        requests
            .iter()
            .any(|r| r.method.as_str() == "PUT" && r.url.path().ends_with(file)),
        "the subscription is written to {file}"
    );
}

/// T-0913, MF-31: the platform calls the notification address, so a credential written into it
/// is refused at the form rather than committed and found by whoever reads the repository.
#[tokio::test]
async fn a_subscription_carrying_a_literal_credential_is_refused() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None));
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "Subscription",
        "metadata": { "name": "air-alerts", "namespace": "ovzdusie" },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air-quality" },
            "entities": [{ "type": "AirQualityObserved" }],
            "notification": {
                "endpoint": {
                    "uri": "https://alerts.example.fi/hooks/air-quality?token=s3cr3t-value"
                }
            }
        }
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/subscriptions")
                .header(header::COOKIE, session_and_csrf_cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem json");
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("secretRef"), "{detail}");
    assert!(
        !detail.contains("s3cr3t-value"),
        "the refusal repeated the secret: {detail}"
    );
}

/// The Endpoint the ovzdusie pipelines write through, so no write they propose is one nobody
/// can make (PL-111).
fn public_air() -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Endpoint".into(),
        metadata: ObjectMeta::new("public-air", "ovzdusie"),
        spec: json!({ "contextSpaceRef": "ovzdusie", "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa", "audience": "public" }),
        status: None,
    }
}

/// PF-73, PF-74: the write that would put the project over a quota is refused before a change
/// exists, and the refusal names the count and the limit. The dry run says the same, so a person
/// learns it from the form and not from the merge request.
#[tokio::test]
async fn the_pipeline_above_the_quota_is_refused_with_the_count_and_the_limit() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let org = |spec: serde_json::Value| ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Organization".into(),
        metadata: ObjectMeta::new("bb", joinedcontext_portal::permissions::ORG_NAMESPACE),
        spec,
        status: None,
    };
    state.mirror.upsert(org(
        json!({ "domain": "banskabystrica.sk", "projects": { "quota": { "residentPipelines": 1 } } }),
    ));
    let pipeline = |name: &str| {
        json!({
            "apiVersion": API_VERSION,
            "kind": "Pipeline",
            "metadata": { "name": name, "namespace": "ovzdusie" },
            "spec": {
                "class": "resident",
                "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt-mesto" } },
                "compute": { "kind": "bloblang", "bloblang": "root = this" },
                "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air"
            }
        })
    };
    // An MQTT feed waits for pushes, so its pipeline stays resident (PL-78).
    state.mirror.upsert(
        serde_json::from_value(json!({
            "apiVersion": API_VERSION, "kind": "DataSource",
            "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
            "spec": { "type": "mqtt", "mqtt": { "urls": ["tcp://mqtt.banskabystrica.sk:1883"], "topics": ["air/#"] } }
        }))
        .expect("a data source"),
    );
    state.mirror.upsert(public_air());
    state
        .mirror
        .upsert(serde_json::from_value::<ResourceEnvelope>(pipeline("first")).expect("a pipeline"));

    let post = |uri: &str, body: serde_json::Value| {
        let app = server::app(state.clone());
        let config = config.clone();
        let uri = uri.to_owned();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header(header::COOKIE, session_and_csrf_cookies(&config))
                        .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&body).expect("json")))
                        .expect("request"),
                )
                .await
                .expect("response");
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
    };

    let (status, body) = post("/api/v1/projects/ovzdusie/pipelines", pipeline("second")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("residentPipelines 2 of 1"),
        "the refusal names the count and the limit: {body}"
    );

    // The same answer on the door a form uses, before anything is proposed.
    let (status, body) = post(
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        pipeline("second"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("residentPipelines 2 of 1"), "{body}");

    // The project's own quota is what counts, and it may be raised there.
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Project".into(),
        metadata: ObjectMeta::new("ovzdusie", joinedcontext_portal::permissions::ORG_NAMESPACE),
        spec: json!({ "organizationRef": { "name": "bb" }, "quotas": { "residentPipelines": 5 } }),
        status: None,
    });
    let (status, body) = post(
        "/api/v1/projects/ovzdusie/pipelines?dryRun=All",
        pipeline("second"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// PF-73: a `Project` that raises a quota above the organization's default is a red-lane change,
/// so an org-admin decides it and not the project's own steward.
#[tokio::test]
async fn a_project_that_raises_its_quota_is_a_red_lane_change() {
    use joinedcontext_portal::change::classify;

    let spec = json!({ "organizationRef": { "name": "bb" }, "quotas": { "residentPipelines": 9 } });
    assert_eq!(classify("Project", Operation::Create, &spec), Lane::Red);
    assert_eq!(classify("Project", Operation::Update, &spec), Lane::Red);
}

/// T-0905, DM-39: a Mapping names two golden-test documents it cannot carry inside itself. The
/// proposal carries them as `files`, and the change commits all three to one branch and one
/// merge request, the files before the manifest that names them.
#[tokio::test]
async fn a_proposal_commits_the_files_its_manifest_names() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/test-owner/test-repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/test-owner/test-repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "commit": { "sha": "commit-sha-created" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 51,
            "html_url": "https://gitea.example.sk/pulls/51",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    // The space this manifest names is in the project, as it has to be: a manifest naming a
    // resource that is not there is refused by the check (MF-13, T-2233).
    let state = AppState::new(config.clone(), None)
        .with_gitea(Arc::new(client))
        .with_mirror(mirror_holding("ovzdusie", &[("ContextSpace", "mobility")]));
    let app = server::app(state);

    let response = app
        .oneshot_checked(mapping_proposal(
            &config,
            json!({
                "./tests/air-to-partner-air.input.json": "{\n  \"id\": \"urn:ngsi-ld:AirQualityObserved:x\"\n}\n",
                "./tests/air-to-partner-air.expect.json": "{\n  \"id\": \"urn:ngsi-ld:AirQualityObserved:x\"\n}\n"
            }),
        ))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);

    let requests = server.received_requests().await.expect("received requests");
    let written: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(written.len(), 3, "manifest and both documents: {written:?}");
    assert!(
        written[2].ends_with("/mappings/air-to-partner-air.yaml"),
        "the manifest is written last, after the files it names: {written:?}"
    );
    for name in ["input", "expect"] {
        assert!(
            written
                .iter()
                .any(|p| p.ends_with(&format!("/mappings/tests/air-to-partner-air.{name}.json"))),
            "the {name} document is committed beside the manifest: {written:?}"
        );
    }
}

/// A path that climbs out of the manifest's folder is refused, and nothing is written: the body
/// says what a change contains, so `..` would let a Mapping's propose permission rewrite a
/// Policy or a role binding.
#[tokio::test]
async fn a_file_outside_the_manifests_folder_is_refused_and_writes_nothing() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    for bad in [
        json!({ "../../users/roles/steward.yaml": "rules: []\n" }),
        json!({ "/etc/passwd": "root\n" }),
        json!({ "": "nothing\n" }),
        json!({ "./tests/a.json": 7 }),
    ] {
        let app = server::app(state.clone());
        let response = app
            .oneshot(mapping_proposal(&config, bad.clone()))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad}");
    }

    let requests = server.received_requests().await.expect("received requests");
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "PUT"),
        "a refused proposal wrote a file"
    );
}

/// One body carries a proposal's worth of files, not a repository's: more than that is an import.
#[tokio::test]
async fn too_many_or_too_large_files_are_refused() {
    let server = MockServer::start().await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));

    let many: serde_json::Map<String, serde_json::Value> = (0..17)
        .map(|n| (format!("./tests/{n}.json"), json!("{}")))
        .collect();
    let huge = json!({ "./tests/big.json": "x".repeat(256 * 1024 + 1) });

    for body in [serde_json::Value::Object(many), huge] {
        let app = server::app(state.clone());
        let response = app
            .oneshot(mapping_proposal(&config, body))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

/// The Mapping proposal every files test sends, with the `files` member under test.
fn mapping_proposal(config: &Config, files: serde_json::Value) -> Request<Body> {
    let payload = json!({
        "apiVersion": API_VERSION,
        "kind": "Mapping",
        "metadata": { "name": "air-to-partner-air", "namespace": "ovzdusie" },
        "spec": {
            "contextSpaceRef": "mobility",
            "source": { "name": "air", "version": "1" },
            "target": { "name": "partner-air", "version": "2" },
            "transformation": { "class_derivations": { "PartnerAir": { "populated_from": "Air" } } },
            "tests": [{
                "input": "./tests/air-to-partner-air.input.json",
                "expect": "./tests/air-to-partner-air.expect.json"
            }]
        },
        "files": files
    });
    Request::builder()
        .method("POST")
        .uri("/api/v1/projects/ovzdusie/mappings")
        .header(header::COOKIE, session_and_csrf_cookies(config))
        .header(CSRF_HEADER, TEST_CSRF_TOKEN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&payload).expect("json bytes"),
        ))
        .expect("request")
}

// --- T-2855: a kind filed once per scope (DS-07) ---------------------------------------------

const PARTICIPANT_PATH: &str =
    "/api/v1/repos/test-owner/test-repo/contents/dataspace/participant.yaml";

fn participant(name: &str, connector: &str) -> serde_json::Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "DataSpaceParticipant",
        "metadata": { "name": name, "namespace": "org" },
        "spec": {
            "did": "did:web:banskabystrica.sk",
            "credentialIssuer": "https://auth.banskabystrica.sk/realms/organization",
            "connectorUrl": connector,
            "engine": "rainbow"
        }
    })
}

/// The forge of these cases: main, and a branch create that must never be reached by a refusal.
async fn participant_forge(branches: u64, on_main: Option<&serde_json::Value>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(branches)
        .mount(&server)
        .await;
    let file = match on_main {
        Some(manifest) => ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-participant",
            "content": STANDARD.encode(serde_norway::to_string(manifest).expect("yaml")),
        })),
        None => ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })),
    };
    Mock::given(method("GET"))
        .and(path(PARTICIPANT_PATH))
        .respond_with(file)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(PARTICIPANT_PATH))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 7, "html_url": "https://gitea.example.sk/pulls/7",
            "state": "open", "mergeable": true, "merged": false
        })))
        .mount(&server)
        .await;
    server
}

fn participant_state(
    server: &MockServer,
    held: Option<&serde_json::Value>,
) -> (Config, axum::Router) {
    let client = GiteaClient::new(
        server.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    if let Some(held) = held {
        state
            .mirror
            .upsert(serde_json::from_value(held.clone()).expect("an envelope"));
    }
    (config, server::app(state))
}

fn participant_request(
    config: &Config,
    method_name: &str,
    uri: &str,
    body: &serde_json::Value,
) -> Request<Body> {
    Request::builder()
        .method(method_name)
        .uri(uri)
        .header(header::COOKIE, session_and_csrf_cookies(config))
        .header(CSRF_HEADER, TEST_CSRF_TOKEN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).expect("json")))
        .expect("request")
}

async fn detail_of(response: axum::response::Response) -> (StatusCode, String) {
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("bytes")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// DS-07, T-2855: the organization has one participant file. Creating another name is refused
/// naming the one there, in the dry run and in the proposal, and no branch is made.
#[tokio::test]
async fn a_second_participant_is_refused_naming_the_one_the_mirror_holds_and_no_branch_is_made() {
    let held = participant("a", "https://connector.banskabystrica.sk");
    let server = participant_forge(0, Some(&held)).await;
    let (config, app) = participant_state(&server, Some(&held));
    let second = participant("b", "https://evil.example");
    for uri in [
        "/api/v1/projects/org/dataspaceparticipants?dryRun=All",
        "/api/v1/projects/org/dataspaceparticipants",
    ] {
        let response = app
            .clone()
            .oneshot(participant_request(&config, "POST", uri, &second))
            .await
            .expect("response");
        let (status, body) = detail_of(response).await;
        assert_eq!(status, StatusCode::CONFLICT, "{uri}: {body}");
        assert!(body.contains("DataSpaceParticipant 'a'"), "{uri}: {body}");
        assert!(body.contains("dataspace/participant.yaml"), "{uri}: {body}");
    }
}

/// The mirror may not have read main yet: the file on main is what decides, before a branch.
#[tokio::test]
async fn a_create_over_a_file_main_already_holds_is_refused_before_a_branch() {
    let on_main = participant("a", "https://connector.banskabystrica.sk");
    let server = participant_forge(0, Some(&on_main)).await;
    let (config, app) = participant_state(&server, None);
    let response = app
        .oneshot_checked(participant_request(
            &config,
            "POST",
            "/api/v1/projects/org/dataspaceparticipants",
            &participant("b", "https://evil.example"),
        ))
        .await
        .expect("response");
    let (status, body) = detail_of(response).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("DataSpaceParticipant 'a'"), "{body}");
}

/// The participant there is still changed the ordinary way.
#[tokio::test]
async fn the_participant_there_is_still_updated() {
    let held = participant("a", "https://connector.banskabystrica.sk");
    let server = participant_forge(1, Some(&held)).await;
    let (config, app) = participant_state(&server, Some(&held));
    let response = app
        .oneshot_checked(participant_request(
            &config,
            "PUT",
            "/api/v1/projects/org/dataspaceparticipants/a",
            &participant("a", "https://connector2.banskabystrica.sk"),
        ))
        .await
        .expect("response");
    let (status, body) = detail_of(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

/// T-3972, PL-15: a credential field of a runner step names the pipeline's own secret as
/// `${VAR}` and passes the literal-secret door; a name the pipeline does not declare is refused
/// by the kind, naming it, and a literal is still refused as one.
#[tokio::test]
async fn a_step_names_its_own_secret_as_a_variable_and_nothing_else() {
    let config = Config::for_tests();
    let post = |password: &str| {
        let body = json!({
            "apiVersion": "joinedcontext.com/v1alpha2",
            "kind": "Pipeline",
            "metadata": { "name": "feed", "namespace": "ovzdusie" },
            "spec": {
                "class": "resident",
                "period": "5m",
                "secretRefs": [{ "name": "feed", "key": "pw", "envVar": "FEED_PW" }],
                "sources": [{ "dataSourceRef": { "kind": "DataSource", "name": "mqtt-mesto" } }],
                "steps": [{ "processor": { "http": {
                    "url": "https://example.org/extra",
                    "verb": "GET",
                    "basic_auth": { "enabled": true, "username": "u", "password": password }
                } } }],
                "outputs": [{ "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:ovzdusie:public-air" }]
            }
        });
        Request::builder()
            .method("POST")
            .uri("/api/v1/projects/ovzdusie/pipelines")
            .header(header::COOKIE, session_and_csrf_cookies(&config))
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(&body).expect("json")))
            .expect("request")
    };
    async fn answer(config: &Config, request: Request<Body>) -> (StatusCode, String) {
        let state = AppState::new(config.clone(), None);
        state.mirror.upsert(public_air());
        let app = server::app(state);
        let response = app.oneshot(request).await.expect("response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        let detail = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|problem| problem["detail"].as_str().map(str::to_owned))
            .unwrap_or_default();
        (status, detail)
    }

    // Past the literal-secret door and the kind's checks, the write stops only at the next one:
    // the DataSource this test never proposed.
    let (status, detail) = answer(&config, post("${FEED_PW}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");
    assert!(
        detail.contains("DataSource 'mqtt-mesto'") && !detail.contains("secret"),
        "the pipeline's own variable is refused: {detail}"
    );
    let (status, detail) = answer(&config, post("${PORTAL_DB_PASSWORD}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");
    assert!(detail.contains("${PORTAL_DB_PASSWORD}"), "{detail}");
    let (status, detail) = answer(&config, post("hunter2")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{detail}");
    assert!(
        detail.contains("literal secret in field 'password'") && !detail.contains("hunter2"),
        "{detail}"
    );
}

/// T-4595, EP-127: Project settings → General writes `spec.catalog` into project.yaml. The Portal
/// reads it with the jc-core it pins, so a pin from before the catalogue refused the field as
/// unknown; it checks now, and a publisher without its registry IRI is refused naming the field.
#[tokio::test]
async fn a_project_catalogue_is_checked_with_the_project_and_its_iri_is_required() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None);
    let project = |catalog: serde_json::Value| ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "Project".into(),
        metadata: ObjectMeta::new(
            "banskabystrica",
            joinedcontext_portal::permissions::ORG_NAMESPACE,
        ),
        spec: json!({ "organizationRef": "bb", "catalog": catalog }),
        status: None,
    };
    state.mirror.upsert(ResourceEnvelope {
        spec: json!({ "organizationRef": "bb" }),
        ..project(json!(null))
    });
    let put = |body: ResourceEnvelope| {
        let app = server::app(state.clone());
        let config = config.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method("PUT")
                        .uri("/api/v1/projects/org/projects/banskabystrica?dryRun=All")
                        .header(header::COOKIE, session_and_csrf_cookies(&config))
                        .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::to_vec(&body).expect("json")))
                        .expect("request"),
                )
                .await
                .expect("response");
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
    };

    let (status, body) = put(project(json!({
        "publisher": {
            "name": { "sk": "Mesto Banská Bystrica" },
            "uri": "https://data.gov.sk/id/legal-subject/00313271"
        },
        "contactPoint": { "name": "Oddelenie otvorených dát", "email": "opendata@banskabystrica.sk" }
    })))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"valid\":true"), "{body}");

    let (status, body) = put(project(json!({
        "publisher": { "name": { "sk": "Mesto Banská Bystrica" } }
    })))
    .await;
    // A dry run answers what a proposal would be refused for as its verdict.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"valid\":false"), "{body}");
    assert!(body.contains("spec.catalog.publisher.uri"), "{body}");
}

/// F-POR2-006: a patch by a person who may not read the manifest is answered exactly as a patch
/// of a name that does not exist, so the answer discloses nothing.
#[tokio::test]
async fn a_patch_of_an_unreadable_manifest_answers_as_if_it_were_absent() {
    let forge = MockServer::start().await;
    let state = common::state_on(&forge);
    state.mirror.upsert(common::envelope(
        "Endpoint",
        "air",
        "ovzdusie",
        json!({ "contextSpaceRef": "mobility", "audience": "project" }),
    ));
    let patch = Some(json!({ "spec": { "audience": "public" } }));
    let held = common::send(
        &state,
        common::person("stranger"),
        "PATCH",
        "/api/v1/projects/ovzdusie/endpoints/air",
        patch.clone(),
    )
    .await;
    let absent = common::send(
        &state,
        common::person("stranger"),
        "PATCH",
        "/api/v1/projects/ovzdusie/endpoints/nope",
        patch,
    )
    .await;
    assert_eq!(held.status, StatusCode::NOT_FOUND, "{}", held.text);
    assert_eq!(held.text.replace("air", "nope"), absent.text);
}

/// F-POR2-012: the branch name of a change is stable across builds. An open change is found by
/// this name (CC-34); a toolchain whose hasher renames it would orphan every open change and let
/// a second one open. A failure here is that toolchain bump: decide the migration then.
#[test]
fn the_branch_name_of_a_change_is_pinned() {
    assert_eq!(
        branch_name("ovzdusie", "Endpoint", "air", Operation::Update),
        "portal/update-endpoint-air-878e627a"
    );
}

/// The forge's open pull requests: none until the first proposal opens one on `branch`.
struct OpenPulls {
    opened: Arc<std::sync::atomic::AtomicBool>,
    branch: String,
}

impl wiremock::Respond for OpenPulls {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        if !self.opened.load(std::sync::atomic::Ordering::SeqCst) {
            return ResponseTemplate::new(200).set_body_json(json!([]));
        }
        ResponseTemplate::new(200).set_body_json(json!([{
            "number": 44,
            "html_url": "https://gitea.example.sk/pulls/44",
            "state": "open",
            "title": "create ContextSpace mobility",
            "head": { "ref": self.branch },
            "base": { "ref": "main" },
            "created_at": "2026-10-25T20:00:00Z",
            "user": { "login": "jana.kovacova", "full_name": "Jana Kováčová" },
            "mergeable": true,
            "merged": false
        }]))
    }
}

struct OpenPull(Arc<std::sync::atomic::AtomicBool>);

impl wiremock::Respond for OpenPull {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        ResponseTemplate::new(201).set_body_json(json!({
            "number": 44,
            "html_url": "https://gitea.example.sk/pulls/44",
            "state": "open",
            "merged": false
        }))
    }
}

/// F-POR2-011: two proposals of one resource at once. The second waits for the first one's pull
/// request and is refused naming it; before, it found the branch without a pull request yet and
/// deleted it under the first one's writes.
#[tokio::test]
async fn two_proposals_of_one_resource_at_once_open_one_change_and_delete_no_branch() {
    let server = MockServer::start().await;
    mount_space_model(&server).await;
    let base_url = server.uri().parse().expect("valid mock server url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");
    let branch = branch_name("ovzdusie", "ContextSpace", "mobility", Operation::Create);
    let opened = Arc::new(std::sync::atomic::AtomicBool::new(false));
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .and(query_param("state", "open"))
        .respond_with(OpenPulls {
            opened: opened.clone(),
            branch: branch.clone(),
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(409).set_body_string("branch already exists"))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/branches/{branch}"
        )))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&server)
        .await;
    // The first proposal's write is slow, so the second arrives while it is under way.
    Mock::given(method("PUT"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/space.yaml",
        ))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({ "commit": { "sha": "c1" } }))
                .set_delay(std::time::Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(OpenPull(opened))
        .mount(&server)
        .await;

    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None).with_gitea(Arc::new(client)));
    let propose = || {
        Request::builder()
            .method("POST")
            .uri("/api/v1/projects/ovzdusie/spaces")
            .header(header::COOKIE, session_and_csrf_cookies(&config))
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({
                    "apiVersion": API_VERSION,
                    "kind": "ContextSpace",
                    "metadata": { "name": "mobility", "namespace": "ovzdusie" },
                    "spec": { "isSandbox": true }
                }))
                .expect("json bytes"),
            ))
            .expect("request")
    };
    let (first, second) = tokio::join!(
        app.clone().oneshot_checked(propose()),
        app.clone().oneshot_checked(propose())
    );
    let mut statuses = vec![
        first.expect("response").status(),
        second.expect("response").status(),
    ];
    statuses.sort();
    assert_eq!(statuses, vec![StatusCode::ACCEPTED, StatusCode::CONFLICT]);
    let requests = server.received_requests().await.expect("received requests");
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method.as_str() == "DELETE")
            .count(),
        0,
        "no branch is deleted under a proposal still writing"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
            .count(),
        1,
        "one change opens"
    );
}
