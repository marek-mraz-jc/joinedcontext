//! Tests for reading and saving DataModel LinkML source and compiled schema artifacts (DM-01, DM-02, DM-22, DM-24, DM-56).

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{Change, ChangePhase, Lane};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::json;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_CSRF_TOKEN: &str = "csrf-token-12345";

const PUBLISHED_LINKML: &str = r#"id: https://example.org/models/air-quality
name: air-quality
prefixes:
  aq: https://example.org/aq/
default_prefix: aq
imports:
  - linkml:types
classes:
  AirQualityObserved:
    class_uri: https://example.org/aq/AirQualityObserved
    slots:
      - dateObserved
      - pm10
slots:
  dateObserved:
    range: string
    required: true
  pm10:
    range: integer
    required: false
"#;

const COMPILED_ARTIFACTS: &str = r##"{
  "jsonSchema": {"title": "AirQualityObserved"},
  "context": {"@context": {"pm10": "https://example.org/aq/pm10"}},
  "docs": "# AirQualityObserved Documentation",
  "example": {"id": "urn:ngsi-ld:AirQualityObserved:01", "type": "AirQualityObserved"},
  "generatorVersion": "linkml-1.11.1"
}"##;

fn session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("steward@banskabystrica.sk".into()),
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
    parts.push(format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"));
    parts.join("; ")
}

fn seed_datamodel(state: &AppState, project: &str, name: &str, linkml_path: &str, version: &str) {
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some(project.to_string()),
            ..Default::default()
        },
        spec: json!({
            "contextSpaceRef": "mobility",
            "linkml": linkml_path,
            "version": version,
            "lifecycle": "published",
            "classes": ["AirQualityObserved"]
        }),
        status: None,
    });
}

#[tokio::test]
async fn get_source_answers_file_content_and_yaml_content_type() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/yaml; charset=utf-8"
    );
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert_eq!(body_str, PUBLISHED_LINKML);
}

/// DM-13, T-1483: the forge client hands back the file decoded once; a source whose text also
/// happens to be valid base64 is served as the person wrote it, not decoded a second time.
#[tokio::test]
async fn a_model_source_that_is_valid_base64_is_returned_as_written() {
    const WRITTEN: &str = "aGVsbG8=";
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(WRITTEN)
        })))
        .mount(&forge)
        .await;
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        String::from_utf8_lossy(&body),
        WRITTEN,
        "not decoded a second time into 'hello'"
    );
}

#[tokio::test]
async fn path_traversal_and_absolute_paths_are_400_and_forge_is_not_called() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));

    seed_datamodel(
        &state,
        "ovzdusie",
        "escape-relative",
        "../secrets.linkml.yaml",
        "1.0.0",
    );
    seed_datamodel(
        &state,
        "ovzdusie",
        "escape-absolute",
        "/etc/passwd.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    for name in ["escape-relative", "escape-absolute"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/projects/ovzdusie/datamodels/{name}/source"
                    ))
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    assert_eq!(forge.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn other_project_or_absent_model_is_404() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None);
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/other-project/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn anonymous_request_returns_401() {
    let config = Config::for_tests();
    let state = AppState::new(config, None);
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn put_breaking_change_under_same_major_is_400_naming_removed_slot() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    // Remove slot pm10 -> breaking change
    let breaking_source = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
slots:
  dateObserved:
    range: string
    required: true
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source?version=1.0.1")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(breaking_source))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("breaking change"), "{detail}");
    let errors = problem["errors"].as_array().unwrap();
    assert!(
        errors.iter().any(|e| e.to_string().contains("pm10")),
        "errors must name removed slot pm10"
    );
}

#[tokio::test]
async fn put_source_that_does_not_compile_is_400_with_tool_messages() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"errors":["slot 'temperature' has no range declared"]}"#,
            "application/json",
        ))
        .mount(&tools)
        .await;

    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let candidate = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
      - temperature
slots:
  dateObserved:
    range: string
  temperature: {}
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(candidate))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let detail = problem["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("temperature"),
        "detail must contain tool error: {detail}"
    );
}

#[tokio::test]
async fn put_dry_run_all_returns_200_and_proposes_nothing() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(COMPILED_ARTIFACTS, "application/json"),
        )
        .mount(&tools)
        .await;

    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let additive_source = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
      - pm10
      - co2
slots:
  dateObserved:
    range: string
    required: true
  pm10:
    range: integer
    required: false
  co2:
    range: float
    required: false
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source?dryRun=All")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(additive_source))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let res: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(res["severity"], "additive");
    assert_eq!(res["version"], "1.1.0");
    assert_eq!(
        res["artifacts"]["docs"],
        "# AirQualityObserved Documentation"
    );

    // Proposes nothing
    let forge_reqs = forge.received_requests().await.unwrap();
    assert!(!forge_reqs
        .iter()
        .any(|r| r.method.as_str() == "POST" && r.url.path().contains("/pulls")));
}

#[tokio::test]
async fn put_creates_change_and_writes_manifest_source_and_four_artifacts() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main"
        })))
        .mount(&forge)
        .await;

    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;

    // Handle GET content and PUT content for files
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&forge)
        .await;

    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&forge)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/contents"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&forge)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 88,
            "html_url": "https://forge.example.sk/pulls/88",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&forge)
        .await;

    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(COMPILED_ARTIFACTS, "application/json"),
        )
        .mount(&tools)
        .await;

    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );

    let app = server::app(state);

    let additive_source = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
      - pm10
      - co2
slots:
  dateObserved:
    range: string
    required: true
  pm10:
    range: integer
    required: false
  co2:
    range: float
    required: false
"#;

    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(additive_source))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let change: Change = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(change.status.lane, Lane::Yellow);
    assert_eq!(
        change.status.merge_request.as_deref(),
        Some("https://forge.example.sk/pulls/88")
    );

    // Verify all 6 files written to the branch
    let requests = forge.received_requests().await.unwrap();
    let put_paths: Vec<String> = requests
        .iter()
        .filter(|r| {
            r.method.as_str() == "POST" && r.url.path() == "/api/v1/repos/owner/repo/contents"
        })
        .flat_map(|r| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or_default();
            body["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f["path"].as_str().map(|p| format!("/{p}")))
                .collect::<Vec<_>>()
        })
        .collect();

    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/air-quality.yaml")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/air-quality.linkml.yaml")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/json-schema/air-quality.v1.json")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/context/air-quality.v1.jsonld")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/docs/air-quality.md")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/examples/air-quality.example.jsonld")));
}

/// The space a created model belongs to; without it there is no folder to write into (DM-57).
fn seed_space(state: &AppState, project: &str, name: &str) {
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "ContextSpace".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some(project.to_string()),
            ..Default::default()
        },
        spec: json!({ "visibility": "project" }),
        status: None,
    });
}

const NEW_MODEL: &str = r#"id: https://example.org/models/bikes
name: bikes
classes:
  BikeHireDockingStation:
    slots:
      - availableBikeNumber
slots:
  availableBikeNumber:
    range: integer
"#;

/// Creates `bikes` through `uri` against a forge that holds nothing of it, and answers the paths
/// the one commit wrote.
async fn create_bikes_through(uri: &str) -> Vec<String> {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&forge)
        .await;
    // Nothing of this model is in the repository yet.
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&forge)
        .await;
    Mock::given(method("PUT"))
        .and(wiremock::matchers::path_regex(
            r"^/api/v1/repos/owner/repo/contents/.*",
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&forge)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/contents"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&forge)
        .await;

    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 91,
            "html_url": "https://forge.example.sk/pulls/91",
            "state": "open",
            "mergeable": true,
            "merged": false
        })))
        .mount(&forge)
        .await;

    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(COMPILED_ARTIFACTS, "application/json"),
        )
        .mount(&tools)
        .await;

    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_space(&state, "ovzdusie", "mobility");

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(uri)
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(NEW_MODEL))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let change: Change = serde_json::from_slice(&body_bytes).unwrap();
    // A model that did not exist breaks nothing, so the lane is the one a draft gets.
    assert_eq!(change.status.lane, Lane::Green);
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert_eq!(change.status.plan.create, 6);

    let requests = forge.received_requests().await.unwrap();
    let put_paths: Vec<String> = requests
        .iter()
        .filter(|r| {
            r.method.as_str() == "POST" && r.url.path() == "/api/v1/repos/owner/repo/contents"
        })
        .flat_map(|r| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap_or_default();
            body["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f["path"].as_str().map(|p| format!("/{p}")))
                .collect::<Vec<_>>()
        })
        .collect();

    put_paths
}

#[tokio::test]
async fn put_for_a_name_the_project_does_not_hold_creates_the_model_in_its_space() {
    let put_paths =
        create_bikes_through("/api/v1/projects/ovzdusie/datamodels/bikes/source?space=mobility")
            .await;
    // The Change carries the manifest as well as the source, which is what makes the model exist.
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/projects/ovzdusie/spaces/mobility/datamodels/bikes.yaml")));
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/projects/ovzdusie/spaces/mobility/datamodels/bikes.linkml.yaml")));
    // A new model starts at 0.1.0, so its artifacts are the v0 ones.
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/datamodels/json-schema/bikes.v0.json")));
}

/// DM-57, DM-75 (T-6162): the editor names no space, so a create without one is a project
/// model in its own folder, with no `contextSpaceRef`; a space takes it by its `dataModelRef`.
#[tokio::test]
async fn put_for_an_unknown_model_without_a_space_creates_a_project_model() {
    let put_paths = create_bikes_through("/api/v1/projects/ovzdusie/datamodels/bikes/source").await;

    assert!(
        put_paths
            .iter()
            .any(|p| p.ends_with("/projects/ovzdusie/datamodels/bikes/bikes.yaml")),
        "{put_paths:?}"
    );
    assert!(put_paths
        .iter()
        .any(|p| p.ends_with("/projects/ovzdusie/datamodels/bikes/bikes.linkml.yaml")));
    assert!(!put_paths.iter().any(|p| p.contains("/spaces/")));
}

/// T-1139, DM-56: a source past the limit is refused by the route, before the body is read into
/// a generator call and before the forge is touched. The ceiling is the one the layer enforces,
/// so a caller learns it from the answer rather than from a closed connection.
#[tokio::test]
async fn put_of_a_source_past_the_limit_is_refused_and_the_forge_is_not_called() {
    let forge = MockServer::start().await;
    let forge_url = forge.uri().parse().expect("valid forge url");
    let gitea = GiteaClient::new(forge_url, "owner", "repo", "token").expect("client");

    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_space(&state, "ovzdusie", "mobility");

    // One byte over: the refusal is the size, not the content.
    let huge = "#".repeat(512 * 1024 + 1);
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes/source?space=mobility")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(huge))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        response.status() == StatusCode::BAD_REQUEST
            || response.status() == StatusCode::PAYLOAD_TOO_LARGE,
        "a source past the limit is refused, not accepted: {}",
        response.status()
    );
    assert!(forge.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn put_naming_a_space_the_project_does_not_hold_is_404() {
    let config = Config::for_tests();
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None);

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/bikes/source?space=nowhere")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(NEW_MODEL))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A Role in the organization that reads `kinds`, bound to `who@hel.fi` on the project `ovzdusie`.
fn grant_read(state: &AppState, who: &str, kinds: serde_json::Value) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    state.mirror.upsert(common::envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "reader-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": format!("{who}@hel.fi") }],
            "role": "reader",
            "scope": { "project": "ovzdusie" },
        }),
    ));
}

/// PF-59, R20 (T-1367): the source is a read of the DataModel, so a person who may not read the
/// project's models is answered as if the model were not there, and the forge is never asked.
#[tokio::test]
async fn get_source_without_read_on_datamodel_is_404_and_the_forge_is_not_asked() {
    let forge = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "default_branch": "main",
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .expect(0)
        .mount(&forge)
        .await;
    let state = common::state_on(&forge);
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );
    grant_read(&state, "pipelines-only", json!(["Pipeline"]));
    let uri = "/api/v1/projects/ovzdusie/datamodels/air-quality/source";

    for who in ["stranger", "pipelines-only"] {
        let answer = common::send(&state, common::person(who), "GET", uri, None).await;
        assert_eq!(
            answer.status,
            StatusCode::NOT_FOUND,
            "{who}: {}",
            answer.text
        );
        assert!(
            !answer.text.contains("AirQualityObserved"),
            "{who} read the source"
        );
        // The same answer as a model that does not exist, so the name discloses nothing.
        let absent = common::send(
            &state,
            common::person(who),
            "GET",
            "/api/v1/projects/ovzdusie/datamodels/no-such-model/source",
            None,
        )
        .await;
        assert_eq!(
            answer.text.replace("air-quality", "no-such-model"),
            absent.text,
            "{who}: the refusal differs from an absent model"
        );
    }
}

/// The refusal's other half: a person whose role reads DataModel gets the source.
#[tokio::test]
async fn get_source_with_read_on_datamodel_answers_the_source() {
    let forge = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/test-owner/test-repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": STANDARD.encode(PUBLISHED_LINKML)
        })))
        .mount(&forge)
        .await;
    let state = common::state_on(&forge);
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );
    grant_read(&state, "modeller", json!(["DataModel"]));

    let answer = common::send(
        &state,
        common::person("modeller"),
        "GET",
        "/api/v1/projects/ovzdusie/datamodels/air-quality/source",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    assert_eq!(answer.text, PUBLISHED_LINKML);
}

/// An organization model (DM-75): `namespace: org`, no space, its source in its own folder.
const STATIONS_LINKML: &str = "id: https://example.org/models/stations\nname: stations\nimports: [linkml:types]\nclasses:\n  Station:\n    attributes:\n      capacity: { range: integer }\n";

fn seed_spaceless(state: &AppState, namespace: &str, name: &str, version: &str, lifecycle: &str) {
    state.mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.to_string(),
        kind: "DataModel".to_string(),
        metadata: ObjectMeta {
            name: name.to_string(),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        spec: json!({
            "linkml": format!("./{name}.linkml.yaml"),
            "version": version,
            "lifecycle": lifecycle,
            "classes": ["Station"]
        }),
        status: None,
    });
}

/// A forge holding the space model's published source and the organization model's, and a Model
/// Tools that compiles anything; one repository serves both, as `forge_for("org")` does here.
async fn importing_world() -> (MockServer, MockServer, AppState, String) {
    let forge = MockServer::start().await;
    let gitea = GiteaClient::new(forge.uri().parse().expect("url"), "owner", "repo", "token")
        .expect("client");
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    for (file, text) in [
        (
            "projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
            PUBLISHED_LINKML,
        ),
        ("datamodels/stations/stations.linkml.yaml", STATIONS_LINKML),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/repos/owner/repo/contents/{file}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "sha-1", "content": STANDARD.encode(text)
            })))
            .mount(&forge)
            .await;
    }
    let tools = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/generate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(COMPILED_ARTIFACTS, "application/json"),
        )
        .mount(&tools)
        .await;
    let config = Config {
        model_tools_url: Some(tools.uri()),
        ..Config::for_tests()
    };
    let cookie = session_cookie(&config);
    let state = AppState::new(config, None).with_gitea(Arc::new(gitea));
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );
    seed_spaceless(&state, "org", "stations", "1.3.0", "published");
    seed_spaceless(&state, "espoo", "bikes", "1.0.0", "published");
    (forge, tools, state, cookie)
}

async fn dry_run(
    state: &AppState,
    cookie: &str,
    source: String,
) -> (StatusCode, serde_json::Value) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datamodels/air-quality/source?dryRun=All")
                .header(header::COOKIE, cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "text/yaml")
                .body(Body::from(source))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

/// DM-76: a space model importing an organization model compiles with that model's source,
/// which the Portal reads and hands Model Tools itself.
#[tokio::test]
async fn a_space_model_importing_an_organization_model_compiles_with_its_source() {
    let (_forge, tools, state, cookie) = importing_world().await;
    let source =
        PUBLISHED_LINKML.replace("  - linkml:types", "  - linkml:types\n  - org.stations.v1");

    let (status, body) = dry_run(&state, &cookie, source).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sent: Vec<serde_json::Value> = tools
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter_map(|request| serde_json::from_slice(&request.body).ok())
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0]["imports"],
        json!({ "org.stations.v1": STATIONS_LINKML })
    );
}

/// DM-76, R20: another project's model has no import name, a pin to another major and a draft
/// are refused naming why, and none of them reaches Model Tools.
#[tokio::test]
async fn an_import_of_another_projects_model_or_the_wrong_major_is_refused() {
    let (_forge, tools, state, cookie) = importing_world().await;
    let importing = |entry: &str| {
        PUBLISHED_LINKML.replace(
            "  - linkml:types",
            &format!("  - linkml:types\n  - {entry}"),
        )
    };

    let (status, body) = dry_run(&state, &cookie, importing("project.bikes.v1")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.to_string()
            .contains("project 'ovzdusie' has no model 'bikes'"),
        "{body}"
    );

    let (status, body) = dry_run(&state, &cookie, importing("org.stations.v2")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.to_string()
            .contains("is at version 1.3.0, and the import pins major 2"),
        "{body}"
    );

    let (status, body) = dry_run(&state, &cookie, importing("org.stations")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    seed_spaceless(&state, "org", "stations", "1.3.0", "draft");
    let (status, body) = dry_run(&state, &cookie, importing("org.stations.v1")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("is draft"), "{body}");

    assert!(
        tools.received_requests().await.unwrap().is_empty(),
        "an unresolved import reached Model Tools"
    );
}

/// DM-76: an organization model a space imports is not deleted; the refusal names the importer.
#[tokio::test]
async fn deleting_an_imported_organization_model_is_refused_naming_its_importers() {
    let (forge, _tools, state, cookie) = importing_world().await;
    let importing =
        PUBLISHED_LINKML.replace("  - linkml:types", "  - linkml:types\n  - org.stations.v1");
    forge.reset().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&forge)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/repos/owner/repo/contents/projects/ovzdusie/spaces/mobility/datamodels/air-quality.linkml.yaml",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-1", "content": STANDARD.encode(importing)
        })))
        .mount(&forge)
        .await;

    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/projects/org/datamodels/stations")
                .header(header::COOKIE, &cookie)
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&bytes);
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("imported by ovzdusie/air-quality"), "{body}");
    assert!(
        !forge
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.method.as_str() == "POST"),
        "a refused delete opened a change"
    );
}

/// A Role that proposes DataModels, bound to `who@hel.fi` in context space `space` only.
fn grant_propose_in_space(state: &AppState, who: &str, space: &str) {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    state.mirror.upsert(common::envelope(
        "Role",
        "modeller",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["DataModel"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        &format!("modeller-{who}"),
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": format!("{who}@hel.fi") }],
            "role": "modeller",
            "scope": { "contextSpace": space },
        }),
    ));
}

/// F-POR1-002: the propose check judges the whole manifest, so a grant bound to the model's own
/// space admits the save and a grant bound to another space refuses it.
#[tokio::test]
async fn put_source_is_judged_by_the_space_the_model_lives_in() {
    let forge = MockServer::start().await;
    let state = common::state_on(&forge);
    seed_space(&state, "ovzdusie", "mobility");
    seed_space(&state, "ovzdusie", "parking");
    seed_datamodel(
        &state,
        "ovzdusie",
        "air-quality",
        "./air-quality.linkml.yaml",
        "1.0.0",
    );
    grant_propose_in_space(&state, "mobility-steward", "mobility");
    grant_propose_in_space(&state, "parking-steward", "parking");
    let uri = "/api/v1/projects/ovzdusie/datamodels/air-quality/source?dryRun=All";

    let outsider = common::send_body(
        &state,
        common::person("parking-steward"),
        "PUT",
        uri,
        "application/yaml",
        axum::body::Body::from(NEW_MODEL),
    )
    .await;
    assert_eq!(outsider.status, StatusCode::FORBIDDEN, "{}", outsider.text);

    let steward = common::send_body(
        &state,
        common::person("mobility-steward"),
        "PUT",
        uri,
        "application/yaml",
        axum::body::Body::from(NEW_MODEL),
    )
    .await;
    assert_ne!(steward.status, StatusCode::FORBIDDEN, "{}", steward.text);
}
