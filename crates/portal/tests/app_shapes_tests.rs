//! `POST /api/v1/projects/{project}/apps/rename-shapes` against a mocked forge (AP-124, T-2940).
//!
//! One click on a project's Apps page proposes one Change, in the clicking person's name, that
//! writes `ui` for `static` and `ui-rust` for `fullstack` in every App still carrying an old
//! name, and changes nothing else in those files: no provenance stamp, no status, no other App.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "banskabystrica";
const CSRF: &str = "test-csrf-token-12345";

fn app(name: &str, kind: &str) -> String {
    format!(
        r#"apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata:
  name: {name}
  namespace: {PROJECT}
  title: {{ en: "{name}" }}
spec:
  kind: {kind}
  source:
    path: apps/{name}
  build:
    node: "22"
  visibility: project
  dataNeeds:
    - contextSpaceRef: ovzdusie
      types: [AirQualityObserved]
      operations: [queryEntity]
"#
    )
}

async fn forge() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/bb/org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/branches"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "name": "portal/rename" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/contents"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "content": { "sha": "b1ob" },
            "commit": { "sha": "c0mm1t" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/bb/org/pulls"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "number": 413,
            "html_url": "https://git.example.sk/bb/org/pulls/413",
            "title": "rename",
            "state": "open",
            "head": { "ref": "portal/rename-app-shapes" },
            "base": { "ref": "main" }
        })))
        .mount(&server)
        .await;
    server
}

fn cookie(config: &Config, groups: &[&str]) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:jana.kovacova".into(),
            username: "jana.kovacova".into(),
            email: Some("jana.kovacova@banskabystrica.sk".into()),
            name: None,
            roles: Vec::new(),
            groups: groups.iter().map(|g| (*g).to_owned()).collect(),
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
    let session = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("; ");
    format!("{session}; {CSRF_COOKIE}={CSRF}")
}

/// The project with `apps`; `groups` decide who calls (`portal-approver` is the bootstrap
/// administrator of `Config::for_tests`), `org` adds the organization's roles and bindings.
fn state(
    server: &MockServer,
    apps: &[String],
    groups: &[&str],
    org: Vec<Value>,
) -> (AppState, String) {
    let config = Config::for_tests();
    let cookie = cookie(&config, groups);
    let gitea = Arc::new(
        GiteaClient::new(
            server.uri().parse().expect("forge url"),
            "bb",
            "org",
            "token",
        )
        .expect("gitea client"),
    );
    let mirror = Arc::new(Mirror::new());
    for yaml in apps {
        let envelope: ResourceEnvelope = serde_norway::from_str(yaml).expect("manifest");
        mirror.upsert(envelope);
    }
    for manifest in [
        json!({ "apiVersion": API_VERSION, "kind": "Organization", "metadata": { "name": "bb", "namespace": ORG_NAMESPACE }, "spec": { "displayName": { "en": "Banska Bystrica" }, "domain": "banskabystrica.sk" } }),
        json!({ "apiVersion": API_VERSION, "kind": "Project", "metadata": { "name": PROJECT, "namespace": ORG_NAMESPACE }, "spec": { "organizationRef": "bb" } }),
        json!({ "apiVersion": API_VERSION, "kind": "ContextSpace", "metadata": { "name": "ovzdusie", "namespace": PROJECT }, "spec": { "defaultLocale": "sk" } }),
    ]
    .into_iter()
    .chain(org)
    {
        mirror.upsert(serde_json::from_value(manifest).expect("organization manifest"));
    }
    (
        AppState::new(config, None)
            .with_gitea(gitea)
            .with_mirror(mirror),
        cookie,
    )
}

async fn rename(state: AppState, cookie: &str, project: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/projects/{project}/apps/rename-shapes"))
        .header(header::COOKIE, cookie)
        .header(CSRF_HEADER, CSRF)
        .body(Body::empty())
        .expect("request");
    let response = server::app(state).oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The files the forge was asked to write, as `(path, decoded YAML)`.
async fn written(server: &MockServer) -> Vec<(String, String)> {
    common::committed_files(server, "/api/v1/repos/bb/org").await
}

#[tokio::test]
async fn one_click_proposes_one_change_renaming_every_old_shape_and_nothing_else() {
    let server = forge().await;
    let apps = [
        app("air-map", "static"),
        app("bus-desk", "fullstack"),
        app("bikes", "ui"),
    ];
    let (state, cookie) = state(&server, &apps, &["portal-approver"], vec![]);
    let (status, body) = rename(state, &cookie, PROJECT).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");

    let files = written(&server).await;
    assert_eq!(files.len(), 2, "the ui App is not touched: {files:?}");
    let by_app = |name: &str| {
        files
            .iter()
            .find(|(p, _)| p.contains(name))
            .map(|(_, c)| c.clone())
            .expect(name)
    };
    let air: Value = serde_norway::from_str(&by_app("air-map")).expect("yaml");
    let bus: Value = serde_norway::from_str(&by_app("bus-desk")).expect("yaml");
    assert_eq!(air["spec"]["kind"], "ui");
    assert_eq!(bus["spec"]["kind"], "ui-rust");
    for file in [&air, &bus] {
        assert!(file.get("status").is_none(), "{file}");
        assert!(
            file["metadata"]
                .get("annotations")
                .is_none_or(|a| a.get("joinedcontext.com/imported-from").is_none()),
            "no provenance stamp: {file}"
        );
        assert_eq!(
            file["spec"]["visibility"], "project",
            "the rest of the App as it was"
        );
    }

    let requests = server.received_requests().await.unwrap_or_default();
    let pulls: Vec<Value> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect();
    assert_eq!(pulls.len(), 1, "one Change for the project");
    assert_eq!(
        pulls[0]["title"],
        format!("rename the App shapes of {PROJECT} (AP-124)")
    );
    assert!(
        pulls[0]["head"]
            .as_str()
            .is_some_and(|h| h.starts_with(&format!("portal/rename-app-shapes-{PROJECT}-"))),
        "{}",
        pulls[0]
    );
    let branches: Vec<Value> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/branches"))
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect();
    assert!(
        branches.iter().all(|b| b["new_branch_name"]
            .as_str()
            .is_some_and(|n| n.starts_with("portal/rename-app-shapes-"))),
        "{branches:?}"
    );
}

#[tokio::test]
async fn nothing_to_rename_is_said_and_nothing_reaches_the_forge() {
    let server = forge().await;
    let (state, cookie) = state(&server, &[app("bikes", "ui")], &["portal-approver"], vec![]);
    let (status, body) = rename(state, &cookie, PROJECT).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("nothing to rename")),
        "{body}"
    );
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_person_who_may_not_propose_an_app_is_refused_before_anything_is_read() {
    let server = forge().await;
    // A viewer of the project: reads everything, proposes nothing (PF-61).
    let org = vec![
        json!({ "apiVersion": API_VERSION, "kind": "Role", "metadata": { "name": "viewer", "namespace": ORG_NAMESPACE }, "spec": { "rules": [{ "kinds": ["App", "ContextSpace"], "verbs": ["read"] }] } }),
        json!({ "apiVersion": API_VERSION, "kind": "RoleBinding", "metadata": { "name": "viewers", "namespace": ORG_NAMESPACE }, "spec": { "subjects": [{ "user": "jana.kovacova@banskabystrica.sk" }], "role": "viewer", "scope": { "project": PROJECT } } }),
    ];
    let (state, cookie) = state(&server, &[app("air-map", "static")], &[], org);
    let (status, body) = rename(state, &cookie, PROJECT).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(written(&server).await.is_empty());
}

#[tokio::test]
async fn a_project_the_caller_may_not_read_answers_not_found() {
    let server = forge().await;
    let (state, cookie) = state(&server, &[app("air-map", "static")], &[], vec![]);
    let (status, _) = rename(state.clone(), &cookie, "nowhere").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = rename(state, &cookie, "Not_A_Name").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(written(&server).await.is_empty());
}
