//! T-0292: `DataSource` on the resource API (MF-11, MF-12, MF-35, CC-06).
//!
//! The kind needs no route of its own: `/api/v1/projects/{project}/{plural}` is generic over the
//! catalogue, so what these tests prove is that the catalogue row is right. The path a write
//! commits to, the namespace it is scoped by and the refusal of a literal credential are all
//! consequences of that one row, and each of them is a defect nobody would see until a merge
//! request landed in the wrong folder.

mod common;
use common::CheckFirst;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{Change, ChangePhase};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ProblemDetails;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, Phase, ResourceEnvelope, Status, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TEST_CSRF_TOKEN: &str = "test-csrf-token-12345";
const MANIFEST_PATH: &str = "projects/ovzdusie/datasources/mqtt-mesto.yaml";

fn cookies(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
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
    let jar = session::store(jar, &session).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts = Vec::new();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let raw = value.to_str().expect("cookie header");
        parts.push(raw.split(';').next().unwrap_or_default().to_string());
    }
    parts.push(format!("{CSRF_COOKIE}={TEST_CSRF_TOKEN}"));
    parts.join("; ")
}

fn seeded_mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "DataSource".into(),
        metadata: ObjectMeta {
            name: "mqtt-mesto".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        },
        spec: json!({
            "type": "mqtt",
            "mqtt": {
                "urls": ["tls://mqtt.banskabystrica.sk:8883"],
                "topics": ["sensors/aq/+/reading"],
                "username": "bb-collector",
                "passwordRef": { "name": "mqtt-mesto", "key": "password" }
            }
        }),
        status: Some(Status {
            phase: Phase::Live,
            observed_revision: None,
            source_url: None,
            conditions: Vec::new(),
            build: None,
            shard: None,
            domain_verification: None,
            jobs: Vec::new(),
            notifications: None,
        }),
    });
    // Another project's source, to prove the list is scoped and not merely filtered by kind.
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "DataSource".into(),
        metadata: ObjectMeta {
            name: "mhd-vehicles".into(),
            namespace: Some("doprava".into()),
            ..Default::default()
        },
        spec: json!({ "type": "gtfs-rt", "gtfsRt": { "url": "https://gtfs.example.sk/v.pb", "feed": "vehiclePositions" } }),
        status: None,
    });
    mirror
}

async fn body_json(response: axum::response::Response) -> Value {
    body_of(response).await
}

async fn body_of<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("the body deserializes")
}

#[tokio::test]
async fn the_collection_lists_only_this_projects_sources() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None).with_mirror(seeded_mirror()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/datasources")
                .header(header::COOKIE, cookies(&config))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    let items = body["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "the other project's source is not here");
    assert_eq!(items[0]["metadata"]["name"], "mqtt-mesto");
    assert_eq!(items[0]["kind"], "DataSource");
}

#[tokio::test]
async fn one_source_comes_back_with_its_reference_and_no_credential() {
    let config = Config::for_tests();
    let app = server::app(AppState::new(config.clone(), None).with_mirror(seeded_mirror()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/ovzdusie/datasources/mqtt-mesto")
                .header(header::COOKIE, cookies(&config))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["spec"]["mqtt"]["passwordRef"]["name"], "mqtt-mesto");
    assert!(
        body["spec"]["mqtt"].get("password").is_none(),
        "a manifest never carries the credential itself"
    );
}

#[tokio::test]
async fn a_write_becomes_a_merge_request_at_the_kinds_own_path() {
    let gitea = MockServer::start().await;
    let base_url = gitea.uri().parse().expect("mock url");
    let client =
        GiteaClient::new(base_url, "test-owner", "test-repo", "token-xyz").expect("client");

    Mock::given(method("GET"))
        .and(path("/api/v1/repos/test-owner/test-repo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{MANIFEST_PATH}"
        )))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&gitea)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "/api/v1/repos/test-owner/test-repo/contents/{MANIFEST_PATH}"
        )))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "commit-1" } })),
        )
        .mount(&gitea)
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
        .mount(&gitea)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_gitea(Arc::new(client));
    let response = server::app(state)
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/datasources")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "DataSource",
                        "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
                        "spec": {
                            "type": "mqtt",
                            "mqtt": {
                                "urls": ["tls://mqtt.banskabystrica.sk:8883"],
                                "topics": ["sensors/aq/+/reading"],
                                "passwordRef": { "name": "mqtt-mesto", "key": "password" }
                            }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let change: Change = body_of(response).await;
    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    // The mocks answer only `MANIFEST_PATH`: a row with the wrong template would have missed
    // them and the write would have failed instead of reaching a merge request.
    assert!(change.status.merge_request.is_some());
}

/// CC-06, MF-24: a credential written as a value is refused before a branch exists, so the
/// secret never reaches Git even as an abandoned commit.
#[tokio::test]
async fn a_literal_credential_is_refused_before_anything_is_written() {
    let config = Config::for_tests();
    let response = server::app(AppState::new(config.clone(), None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/datasources")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "DataSource",
                        "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
                        "spec": {
                            "type": "mqtt",
                            "mqtt": {
                                "urls": ["tls://mqtt.banskabystrica.sk:8883"],
                                "topics": ["sensors/aq/+/reading"],
                                "password": "hunter2"
                            }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let problem: ProblemDetails = body_of(response).await;
    assert!(
        problem.detail.unwrap_or_default().contains("password"),
        "the refusal names the field so the author can fix it"
    );
}

/// T-2238, MF-35: the *name* of a secret is checked too. A person who has the token and not the
/// store pastes it into "Secret name", where nothing looked at it: the check answered green and the
/// plan wrote the token into the manifest, so an approval would have committed it in the clear.
#[tokio::test]
async fn a_token_pasted_into_a_secret_name_is_refused_and_never_echoed() {
    let config = Config::for_tests();
    let token = "glpat-not-a-real-token";
    let response = server::app(AppState::new(config.clone(), None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/datasources?dryRun=All")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "DataSource",
                        "metadata": { "name": "mqtt-mesto", "namespace": "ovzdusie" },
                        "spec": {
                            "type": "mqtt",
                            "mqtt": {
                                "urls": ["tls://mqtt.banskabystrica.sk:8883"],
                                "topics": ["sensors/aq/+/reading"],
                                "passwordRef": { "name": token, "key": "password" }
                            }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    // A check that rejects the manifest answers the red verdict that says why (T-2234); what must
    // never happen is a check that passes this manifest.
    assert_eq!(response.status(), StatusCode::OK);
    let answer: Value = body_of(response).await;
    assert_eq!(
        answer["verdict"]["ok"],
        Value::Bool(false),
        "a check that answers green here is a token on its way to Git: {answer}"
    );
    let said = answer["verdict"]["findings"].to_string();
    assert!(
        said.contains("passwordRef"),
        "the refusal names the field: {said}"
    );
    assert!(
        !said.contains(token),
        "the refusal repeats the credential: {said}"
    );
}

/// T-2239, MF-24: an OAuth bearer typed into a runner input. Its key is `access_token`, which the
/// detector did not know, and the runner's catalog did not call the field a secret either — so the
/// value was accepted, committed and left in Git.
#[tokio::test]
async fn an_access_token_in_a_runner_input_is_refused_by_its_key() {
    let config = Config::for_tests();
    let response = server::app(AppState::new(config.clone(), None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/datasources?dryRun=All")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, TEST_CSRF_TOKEN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "DataSource",
                        "metadata": { "name": "aq-poll", "namespace": "ovzdusie" },
                        "spec": {
                            "type": "http_client",
                            "input": {
                                "url": "https://opendata.banskabystrica.sk/aq.json",
                                "oauth": { "enabled": true, "access_token": "not-a-real-bearer" }
                            }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let answer: Value = body_of(response).await;
    assert_eq!(answer["verdict"]["ok"], Value::Bool(false), "{answer}");
    let detail = answer["verdict"]["findings"].to_string();
    assert!(
        detail.contains("access_token"),
        "the refusal names the key so the author knows what to reference: {detail}"
    );
    assert!(
        !detail.contains("not-a-real-bearer"),
        "the refusal repeats the credential: {detail}"
    );
}

#[tokio::test]
async fn the_kind_is_addressable_by_its_plural_and_scoped_to_a_project() {
    let info = joinedcontext_portal::resource::by_plural("datasources").expect("catalogued");
    assert_eq!(info.kind, "DataSource");
    assert_eq!(
        info.repo_path("ovzdusie", "", "mqtt-mesto"),
        MANIFEST_PATH,
        "a project-scoped kind needs no space in its path"
    );
}

/// T-3167 (PL-07, PL-16): jc-core's runner-target and runner-variable refusals reach the Portal's
/// door: a source pointing at the runner's own stream API, or a URL that reads another project's
/// credential out of the runner, is refused before anything is written, and a public feed is not.
#[tokio::test]
async fn a_source_aimed_at_the_runner_or_its_environment_is_refused_at_the_door() {
    let config = Config::for_tests();
    let propose = |url: &str| {
        let body = serde_json::to_vec(&json!({
            "apiVersion": API_VERSION,
            "kind": "DataSource",
            "metadata": { "name": "feed", "namespace": "ovzdusie" },
            "spec": { "type": "http", "http": { "url": url } }
        }))
        .expect("json");
        Request::builder()
            .method("POST")
            .uri("/api/v1/projects/ovzdusie/datasources")
            .header(header::COOKIE, cookies(&config))
            .header(CSRF_HEADER, TEST_CSRF_TOKEN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("request")
    };
    for (url, rule) in [
        ("http://127.0.0.1:4195/streams/x", "PL-07"),
        ("http://localhost:4180/token", "PL-07"),
        ("https://feed.example/?k=${JC_CLIENT_SECRET_BBSK}", "PL-16"),
    ] {
        let response = server::app(AppState::new(config.clone(), None))
            .oneshot(propose(url))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{url}");
        let problem: ProblemDetails = body_of(response).await;
        let detail = problem.detail.unwrap_or_default();
        assert!(detail.contains(rule), "{url}: {detail}");
        assert!(!detail.contains("JC_CLIENT_SECRET_BBSK"), "{url}: {detail}");
    }
}
