//! T-2764, UI-50: an edit that writes one string over a stored legacy title map keeps every
//! other language, whatever door it comes through; only a map sent without a language drops it.

mod common;
use common::CheckFirst;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::Engine;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF: &str = "test-csrf-token-12345";
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
    let mut parts: Vec<String> = response
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
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

/// A source whose title and description are the legacy `{en, fi}` maps.
fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    let metadata: ObjectMeta = serde_json::from_value(json!({
        "name": "mqtt-mesto",
        "namespace": "ovzdusie",
        "title": { "en": "City MQTT", "fi": "Kaupungin MQTT" },
        "description": { "en": "Sensors of the city", "fi": "Kaupungin anturit" }
    }))
    .expect("metadata");
    mirror.upsert(ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: "DataSource".into(),
        metadata,
        spec: spec(),
        status: None,
    });
    mirror
}

fn spec() -> Value {
    json!({
        "type": "mqtt",
        "mqtt": {
            "urls": ["tls://mqtt.banskabystrica.sk:8883"],
            "topics": ["sensors/aq/+/reading"],
            "passwordRef": { "name": "mqtt-mesto", "key": "password" }
        }
    })
}

/// Proposes `metadata` over the stored source through the REST route and answers the manifest
/// the forge was asked to commit.
async fn committed(metadata: Value) -> Value {
    let gitea = MockServer::start().await;
    let client = GiteaClient::new(
        gitea.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "t",
    )
    .expect("client");
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
    let contents = format!("/api/v1/repos/test-owner/test-repo/contents/{MANIFEST_PATH}");
    Mock::given(method("GET"))
        .and(path(contents.clone()))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .mount(&gitea)
        .await;
    Mock::given(method("PUT"))
        .and(path(contents.clone()))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path(contents.clone()))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&gitea)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/test-owner/test-repo/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 7, "html_url": "https://gitea.example.sk/pulls/7",
            "state": "open", "mergeable": true, "merged": false
        })))
        .mount(&gitea)
        .await;

    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None)
        .with_mirror(mirror())
        .with_gitea(Arc::new(client));
    let response = server::app(state)
        .oneshot_checked(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/projects/ovzdusie/datasources/mqtt-mesto")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "DataSource",
                        "metadata": metadata,
                        "spec": spec()
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let written = gitea
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .find(|r| r.url.path() == contents && r.method.as_str() != "GET")
        .expect("the manifest was committed");
    let body: Value = serde_json::from_slice(&written.body).expect("contents body");
    let yaml = base64::engine::general_purpose::STANDARD
        .decode(body["content"].as_str().expect("content"))
        .expect("base64");
    serde_norway::from_slice(&yaml).expect("yaml")
}

#[tokio::test]
async fn one_string_over_a_title_map_replaces_its_entry_and_keeps_the_other_languages() {
    let manifest = committed(json!({
        "name": "mqtt-mesto",
        "namespace": "ovzdusie",
        "title": "City broker",
        "description": "Sensors of the whole city"
    }))
    .await;
    assert_eq!(
        manifest["metadata"]["title"],
        json!({ "en": "City broker", "fi": "Kaupungin MQTT" })
    );
    assert_eq!(
        manifest["metadata"]["description"],
        json!({ "en": "Sensors of the whole city", "fi": "Kaupungin anturit" })
    );
}

#[tokio::test]
async fn a_map_sent_without_a_language_is_what_the_person_wrote() {
    let manifest = committed(json!({
        "name": "mqtt-mesto",
        "namespace": "ovzdusie",
        "title": { "en": "City broker" }
    }))
    .await;
    assert_eq!(
        manifest["metadata"]["title"],
        json!({ "en": "City broker" })
    );
    assert!(
        manifest["metadata"].get("description").is_none(),
        "a description left out is removed, as it always was"
    );
}
