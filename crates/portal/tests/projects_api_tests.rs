use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

fn make_session_cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        access_expires_at: now + 3600,
        refresh_token: None,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

fn manifest(project: &str, kind: &str, name: &str) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(project.into()),
            ..Default::default()
        },
        spec: serde_json::json!({}),
        status: None,
    }
}

/// What the dev repository holds: `projects/banskabystrica/` and `projects/helsinki/`.
fn two_project_mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(manifest("helsinki", "ContextSpace", "helsinki"));
    mirror.upsert(manifest("helsinki", "Endpoint", "helsinki-bikes"));
    mirror.upsert(manifest("banskabystrica", "ContextSpace", "ovzdusie"));
    mirror.upsert(manifest("banskabystrica", "Endpoint", "public-air"));
    mirror
}

#[tokio::test]
async fn anonymous_call_returns_401_and_discloses_nothing() {
    let app =
        server::app(AppState::new(Config::for_tests(), None).with_mirror(two_project_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(!String::from_utf8_lossy(&body).contains("helsinki"));
}

#[tokio::test]
async fn lists_every_project_of_the_mirror_once_sorted() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let app = server::app(AppState::new(config, None).with_mirror(two_project_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["kind"], "List");
    assert_eq!(list["apiVersion"], API_VERSION);
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["banskabystrica", "helsinki"]);
}

/// T-0974, PF-59: a project the caller has no grant in answers `404` everywhere else, so the
/// list does not name it either. Otherwise anyone with a session reads the organization's
/// internal project and department list.
#[tokio::test]
async fn a_caller_with_no_grant_is_shown_no_project() {
    let config = Config::for_tests();
    // A live session in no group the organization knows: not the bootstrap group, no roles.
    let now = session::now_unix();
    let outsider = Session {
        identity: Identity {
            client: None,
            subject: "f:1:passer.by".into(),
            username: "passer.by".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        access_expires_at: now + 3600,
        refresh_token: None,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
    };
    use axum::response::IntoResponse;
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &outsider).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let cookie = response
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or_default().to_owned())
        .expect("a session cookie");

    let app = server::app(AppState::new(config, None).with_mirror(two_project_mirror()));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let names: Vec<&str> = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["name"].as_str())
        .collect();
    assert!(
        names.is_empty(),
        "a caller with no grant learns no project name: {names:?}"
    );
}

#[tokio::test]
async fn empty_repository_lists_no_project() {
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let app = server::app(AppState::new(config, None).with_mirror(Arc::new(Mirror::new())));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Opening a project (PF-65, PF-66, PF-67, T-0869)
// ---------------------------------------------------------------------------

mod common;

use common::{envelope, person, REPO};
use serde_json::{json, Value};
use wiremock::matchers::{method as http_method, path as url_path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A forge that takes the two files and opens the merge request, and answers the open-pull list
/// the name check reads (PF-67).
async fn forge_with_no_open_projects() -> MockServer {
    let gitea = common::forge().await;
    Mock::given(http_method("GET"))
        .and(url_path(format!("{REPO}/pulls")))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&gitea)
        .await;
    gitea
}

/// An organization whose `projects.creation` is `creation`, with `banskabystrica` already open.
fn organization(state: &AppState, creation: &str) {
    state.mirror.upsert(envelope(
        "Organization",
        "bb",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "domain": "banskabystrica.sk", "projects": { "creation": creation } }),
    ));
}

async fn open(state: &AppState, who: Identity, body: Value) -> (StatusCode, String) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects")
                .header(header::COOKIE, common::cookie(&state.config, who))
                .header(joinedcontext_portal::auth::csrf::CSRF_HEADER, common::CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
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

/// The files the merge request carries, by path: one `POST /contents` commit (T-6462).
async fn written(gitea: &MockServer) -> Vec<String> {
    common::committed_files(gitea, common::REPO)
        .await
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// The body of each file the merge request wrote, decoded.
async fn bodies(gitea: &MockServer) -> Vec<String> {
    common::committed_files(gitea, common::REPO)
        .await
        .into_iter()
        .map(|(_, body)| body)
        .collect()
}

#[tokio::test]
async fn anyone_may_open_a_project_and_gets_steward_on_it_and_nothing_else() {
    let gitea = forge_with_no_open_projects().await;
    let state = common::state_on(&gitea);
    organization(&state, "anyone");

    let (status, body) = open(
        &state,
        person("nobody"),
        json!({ "name": "doprava", "displayName": "Doprava" }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let change: Value = serde_json::from_str(&body).expect("a change");
    assert_eq!(change["status"]["lane"], "yellow", "{body}");

    let paths = written(&gitea).await;
    assert!(
        paths.iter().any(|p| p == "projects/doprava/project.yaml"),
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p == "users/assignments/doprava-creator.yaml"),
        "{paths:?}"
    );

    let binding = bodies(&gitea)
        .await
        .into_iter()
        .find(|body| body.contains("kind: RoleBinding"))
        .expect("the creator's binding");
    // Steward, on their own project, and nothing wider (PF-66, PF-52).
    assert!(binding.contains("role: steward"), "{binding}");
    assert!(binding.contains("project: doprava"), "{binding}");
    assert!(!binding.contains("organization:"), "{binding}");
    assert!(binding.contains("nobody@hel.fi"), "{binding}");

    // The project file is one the loader takes (T-2643): `organizationRef` as jc-core reads it.
    let project = bodies(&gitea)
        .await
        .into_iter()
        .find(|body| body.contains("kind: Project"))
        .expect("the project's own file");
    let parsed =
        jc_core::kinds::Project::from_yaml(&project).unwrap_or_else(|e| panic!("{e}: {project}"));
    parsed.validate().expect("a valid project");
}

#[tokio::test]
async fn org_admin_is_the_default_and_a_person_without_propose_on_project_is_refused() {
    let gitea = forge_with_no_open_projects().await;
    let state = common::state_on(&gitea);
    // No `projects.creation` at all: the default is `org-admin` (PF-65).
    state.mirror.upsert(envelope(
        "Organization",
        "bb",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "domain": "banskabystrica.sk" }),
    ));

    let (status, body) = open(&state, person("nobody"), json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("propose on Project"), "{body}");
    assert!(written(&gitea).await.is_empty(), "nothing was written");
}

#[tokio::test]
async fn a_group_setting_refuses_a_person_the_group_manifest_does_not_name() {
    let gitea = forge_with_no_open_projects().await;
    let state = common::state_on(&gitea);
    organization(&state, "group:city-leads");
    state.mirror.upsert(envelope(
        "Group",
        "city-leads",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "members": [{ "user": "lead@hel.fi" }] }),
    ));

    let (status, body) = open(&state, person("nobody"), json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("city-leads"), "{body}");

    let (status, body) = open(&state, person("lead"), json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn a_name_that_is_taken_or_not_a_label_is_refused_before_anything_is_written() {
    let gitea = forge_with_no_open_projects().await;
    let state = common::state_on(&gitea);
    organization(&state, "anyone");
    state
        .mirror
        .upsert(manifest("helsinki", "ContextSpace", "helsinki"));

    let (status, body) = open(&state, person("nobody"), json!({ "name": "helsinki" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let (status, body) = open(&state, person("nobody"), json!({ "name": "Doprava Mesta" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("DNS-1123"), "{body}");

    // `org` is the organization's own namespace, never a project (PF-67).
    let (status, body) = open(&state, person("nobody"), json!({ "name": "org" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    assert!(written(&gitea).await.is_empty(), "nothing was written");
}

#[tokio::test]
async fn a_name_another_open_change_already_reserved_is_refused() {
    let gitea = common::forge().await;
    Mock::given(http_method("GET"))
        .and(url_path(format!("{REPO}/pulls")))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "number": 4,
            "html_url": "https://gitea.example/pulls/4",
            "state": "open",
            "title": "open project doprava",
            "head": { "ref": "portal/create-project-doprava-0000000a" },
            "base": { "ref": "main" },
            "created_at": "2026-09-16T09:00:00Z",
            "user": { "login": "someone", "full_name": "Someone", "email": "someone@hel.fi" },
            "mergeable": true,
            "merged": false
        }])))
        .mount(&gitea)
        .await;
    let state = common::state_on(&gitea);
    organization(&state, "anyone");

    let (status, body) = open(&state, person("nobody"), json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("waiting for approval"), "{body}");
}

#[tokio::test]
async fn the_door_opens_a_project_and_nothing_else() {
    let gitea = forge_with_no_open_projects().await;
    let state = common::state_on(&gitea);
    organization(&state, "anyone");

    // The person who may open a project by the setting still proposes nothing else: the
    // resource routes ask for a binding, which they do not have (PF-65, PF-50).
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/doprava/pipelines")
                .header(
                    header::COOKIE,
                    common::cookie(&state.config, person("nobody")),
                )
                .header(joinedcontext_portal::auth::csrf::CSRF_HEADER, common::CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "apiVersion": API_VERSION,
                        "kind": "Pipeline",
                        "metadata": { "name": "aq", "namespace": "doprava" },
                        "spec": {
                            "class": "resident",
                            "targetEndpoint": "urn:ngsi-ld:Endpoint:banskabystrica.sk:doprava:air"
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn anyone_plus_lax_merges_the_project_at_once_and_the_message_says_who_did() {
    use joinedcontext_portal::git::GiteaClient;

    let gitea = forge_with_no_open_projects().await;
    let branding = std::env::temp_dir().join(format!("jc-branding-{}.yaml", std::process::id()));
    std::fs::write(&branding, "instanceName: \"Test\"\nvalidation: lax\n").expect("branding file");
    let mut config = Config::for_tests();
    config.branding_file = Some(branding.to_string_lossy().into_owned());
    let client = GiteaClient::new(
        gitea.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(config, None).with_gitea(Arc::new(client));
    organization(&state, "anyone");

    let (status, body) = open(&state, person("nobody"), json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    // Nobody is waiting for a person, and the answer says so: the UI opens the project itself
    // instead of a change nobody will approve (PF-66, T-0870).
    let change: Value = serde_json::from_str(&body).expect("a change");
    assert_eq!(change["status"]["phase"], "Merged", "{body}");

    let merges: Vec<String> = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path().ends_with("/merge"))
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect();
    assert_eq!(merges.len(), 1, "the platform merged it once: {merges:?}");
    assert!(
        merges[0].contains("Merged by the platform"),
        "{}",
        merges[0]
    );
    assert!(merges[0].contains("lax"), "{}", merges[0]);

    let _ = std::fs::remove_file(&branding);
}

#[tokio::test]
async fn anyone_on_a_strict_installation_waits_for_a_person() {
    let gitea = forge_with_no_open_projects().await;
    let state = common::state_on(&gitea);
    organization(&state, "anyone");

    let (status, body) = open(&state, person("nobody"), json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let change: Value = serde_json::from_str(&body).expect("a change");
    assert_eq!(change["status"]["phase"], "PendingApproval", "{body}");
    assert!(
        !gitea
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|r| r.url.path().ends_with("/merge")),
        "strict waits for a person (PF-57)"
    );
}

/// PF-75: the project carries what it holds of each quota, so a person sees the limit before the
/// verdict does; a project no binding of the caller covers is not there at all (PF-59, R20).
#[tokio::test]
async fn the_project_carries_its_usage_against_the_quota_in_force() {
    let gitea = common::forge().await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Organization",
        "bb",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "domain": "banskabystrica.sk", "projects": { "quota": { "contextSpaces": 3 } } }),
    ));
    state.mirror.upsert(envelope(
        "Project",
        "ovzdusie",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "organizationRef": { "name": "bb" } }),
    ));
    state.mirror.upsert(envelope(
        "ContextSpace",
        "vzduch",
        "ovzdusie",
        json!({ "isSandbox": false }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "viewer",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Project"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "viewers",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({
            "role": "viewer",
            "subjects": [{ "user": "reader@hel.fi" }],
            "scope": { "project": "ovzdusie" }
        }),
    ));

    let answer = common::send(
        &state,
        person("reader"),
        "GET",
        "/api/v1/projects/ovzdusie",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let detail: Value = serde_json::from_str(&answer.text).expect("the project");
    assert_eq!(detail["status"]["usage"]["contextSpaces"]["used"], 1);
    assert_eq!(detail["status"]["usage"]["contextSpaces"]["limit"], 3);
    // A dimension no quota limits is listed with its count and no limit.
    assert_eq!(detail["status"]["usage"]["apps"]["used"], 0);
    assert!(
        detail["status"]["usage"]["apps"]["limit"].is_null(),
        "{}",
        answer.text
    );

    let stranger = common::send(
        &state,
        person("nobody"),
        "GET",
        "/api/v1/projects/ovzdusie",
        None,
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
}

// ---------------------------------------------------------------------------
// Deleting a project (PF-77, PF-78)
// ---------------------------------------------------------------------------

/// The repository as the forge holds it: the two projects and the organization's assignments.
const TREE: &[&str] = &[
    "org.yaml",
    "projects/banskabystrica/project.yaml",
    "projects/banskabystrica/spaces/ovzdusie/space.yaml",
    "projects/banskabystrica/spaces/ovzdusie/model.linkml.yaml",
    "projects/banskabystrica/endpoints/public-air.yaml",
    "projects/helsinki/project.yaml",
    "users/assignments/banskabystrica-creator.yaml",
    "users/assignments/ovzdusie-reader.yaml",
    "users/assignments/helsinki-creator.yaml",
    "users/assignments/org-admins.yaml",
    "users/roles/org-admin.yaml",
];

/// A forge that answers the tree, every blob and the open-pull list the deletion reads.
async fn forge_with_a_tree(commits: Value) -> MockServer {
    let gitea = forge_with_no_open_projects().await;
    Mock::given(http_method("GET"))
        .and(url_path(format!("{REPO}/git/trees/main")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "truncated": false,
            "tree": TREE.iter().map(|path| json!({ "path": path, "type": "blob" })).collect::<Vec<_>>(),
        })))
        .mount(&gitea)
        .await;
    Mock::given(http_method("GET"))
        .and(wiremock::matchers::path_regex(format!(
            "^{REPO}/contents/.*"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1", "content": "", "encoding": "base64"
        })))
        .mount(&gitea)
        .await;
    Mock::given(http_method("GET"))
        .and(url_path(format!("{REPO}/commits")))
        .respond_with(ResponseTemplate::new(200).set_body_json(commits))
        .mount(&gitea)
        .await;
    Mock::given(http_method("POST"))
        .and(url_path(format!("{REPO}/contents")))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
        )
        .mount(&gitea)
        .await;
    gitea
}

/// The project, its spaces and endpoints, the roles that delete a project and the bindings that
/// name it — the world a deletion has to sweep.
fn world_to_delete(state: &AppState) {
    organization(state, "org-admin");
    state.mirror.upsert(envelope(
        "Project",
        "banskabystrica",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "organizationRef": { "name": "bb" } }),
    ));
    state.mirror.upsert(envelope(
        "ContextSpace",
        "ovzdusie",
        "banskabystrica",
        json!({ "isSandbox": false }),
    ));
    state.mirror.upsert(envelope(
        "Endpoint",
        "public-air",
        "banskabystrica",
        json!({ "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa", "audience": "public" }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "ContextSpace", "Endpoint", "Role", "RoleBinding"], "verbs": ["propose", "approve", "delete", "read"] }] }),
    ));
    for (binding, scope) in [
        ("org-admins", json!({ "organization": "bb" })),
        (
            "banskabystrica-creator",
            json!({ "project": "banskabystrica" }),
        ),
        ("ovzdusie-reader", json!({ "contextSpace": "ovzdusie" })),
        ("helsinki-creator", json!({ "project": "helsinki" })),
    ] {
        let subject = if binding == "org-admins" {
            "admin@hel.fi"
        } else {
            "steward@hel.fi"
        };
        state.mirror.upsert(envelope(
            "RoleBinding",
            binding,
            joinedcontext_portal::permissions::ORG_NAMESPACE,
            json!({ "subjects": [{ "user": subject }], "role": "org-admin", "scope": scope }),
        ));
    }
}

async fn delete_project(state: &AppState, who: Identity, project: &str) -> (StatusCode, String) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/projects/{project}"))
                .header(header::COOKIE, common::cookie(&state.config, who))
                .header(joinedcontext_portal::auth::csrf::CSRF_HEADER, common::CSRF)
                .body(Body::empty())
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

/// The paths the merge request removed, in its one commit (F-R2POR3-019).
async fn removed(gitea: &MockServer) -> Vec<String> {
    let commits: Vec<Value> = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/contents"))
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect();
    if commits.is_empty() {
        return Vec::new();
    }
    assert_eq!(commits.len(), 1, "one commit removes every file");
    let mut paths: Vec<String> = commits[0]["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|file| file["operation"] == "delete")
        .filter_map(|file| file["path"].as_str().map(str::to_owned))
        .collect();
    paths.sort();
    paths
}

#[tokio::test]
async fn deleting_a_project_removes_its_whole_tree_and_every_binding_that_names_it() {
    let gitea = forge_with_a_tree(json!([])).await;
    let state = common::state_on(&gitea);
    world_to_delete(&state);

    let (status, body) = delete_project(&state, person("admin"), "banskabystrica").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let change: Value = serde_json::from_str(&body).expect("a change");
    assert_eq!(change["status"]["lane"], "red", "{body}");
    assert_eq!(change["status"]["plan"]["delete"], 6, "{body}");

    assert_eq!(
        removed(&gitea).await,
        vec![
            "projects/banskabystrica/endpoints/public-air.yaml",
            "projects/banskabystrica/project.yaml",
            "projects/banskabystrica/spaces/ovzdusie/model.linkml.yaml",
            "projects/banskabystrica/spaces/ovzdusie/space.yaml",
            // The grants written for the project and for one of its spaces go with it, so
            // nothing outlives the project it was written for (PF-77).
            "users/assignments/banskabystrica-creator.yaml",
            "users/assignments/ovzdusie-reader.yaml",
        ],
        "another project's files and the organization's own bindings stay",
    );
}

#[tokio::test]
async fn a_share_from_another_project_refuses_the_deletion_and_names_it() {
    let gitea = forge_with_a_tree(json!([])).await;
    let state = common::state_on(&gitea);
    world_to_delete(&state);
    state.mirror.upsert(envelope(
        "SharedSpaceReference",
        "bb-air",
        "helsinki",
        json!({ "endpointSlug": "k7m2qz4tv6xh3n5jb2ryd3wcfa", "alias": "bb-air" }),
    ));

    let (status, body) = delete_project(&state, person("admin"), "banskabystrica").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("helsinki/bb-air"), "{body}");
    assert!(removed(&gitea).await.is_empty(), "nothing was written");
}

#[tokio::test]
async fn a_share_by_endpoint_ref_refuses_the_deletion_too() {
    let gitea = forge_with_a_tree(json!([])).await;
    let state = common::state_on(&gitea);
    world_to_delete(&state);
    // The in-organization form names the project, not the slug (EP-77).
    state.mirror.upsert(envelope(
        "SharedSpaceReference",
        "bb-air",
        "helsinki",
        json!({ "endpointRef": { "project": "banskabystrica", "name": "public-air" }, "alias": "bb-air" }),
    ));

    let (status, body) = delete_project(&state, person("admin"), "banskabystrica").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("helsinki/bb-air"), "{body}");
    assert!(removed(&gitea).await.is_empty(), "nothing was written");
}

#[tokio::test]
async fn a_project_nobody_bound_the_caller_to_is_not_there_and_one_they_only_read_is_refused() {
    let gitea = forge_with_a_tree(json!([])).await;
    let state = common::state_on(&gitea);
    world_to_delete(&state);
    // A reader of the project: bound, so the project is there, and refused, because reading is
    // not deleting (PF-59, PF-77).
    state.mirror.upsert(envelope(
        "Role",
        "viewer",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-reads-bb",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "viewer",
            "scope": { "project": "banskabystrica" },
        }),
    ));

    let (status, body) = delete_project(&state, person("jana"), "banskabystrica").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // A stranger is told the project is not there, never that it is not theirs (R20).
    let (status, body) = delete_project(&state, person("nobody"), "banskabystrica").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(removed(&gitea).await.is_empty(), "nothing was written");
}

/// PF-78: the name of a deleted project stays reserved for the organization's cooling period,
/// counted from the commit that removed it.
#[tokio::test]
async fn the_name_of_a_deleted_project_is_refused_until_the_cooling_period_passes() {
    let day_ago = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    let gitea = forge_with_a_tree(json!([{
        "sha": "c0ffee", "commit": { "message": "delete project mobilita",
        "author": { "name": "admin", "email": "admin@hel.fi", "date": day_ago } }
    }]))
    .await;
    let state = common::state_on(&gitea);
    organization(&state, "anyone");

    let (status, body) = open(&state, person("jana"), json!({ "name": "mobilita" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("reserved until"), "{body}");

    // The same name once the period has passed: a project again, nothing written before.
    let long_ago = (chrono::Utc::now() - chrono::Duration::days(60)).to_rfc3339();
    let gitea = forge_with_a_tree(json!([{
        "sha": "c0ffee", "commit": { "message": "delete project mobilita",
        "author": { "name": "admin", "email": "admin@hel.fi", "date": long_ago } }
    }]))
    .await;
    let state = common::state_on(&gitea);
    organization(&state, "anyone");
    let (status, body) = open(&state, person("jana"), json!({ "name": "mobilita" })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

/// PF-78: `nameCooldownDays: 0` is the organization saying it wants no reservation at all.
#[tokio::test]
async fn an_organization_may_set_no_cooling_period_at_all() {
    let day_ago = (chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339();
    let gitea = forge_with_a_tree(json!([{
        "sha": "c0ffee", "commit": { "message": "delete project mobilita",
        "author": { "name": "admin", "email": "admin@hel.fi", "date": day_ago } }
    }]))
    .await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Organization",
        "bb",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({
            "domain": "banskabystrica.sk",
            "projects": { "creation": "anyone", "nameCooldownDays": 0 },
        }),
    ));

    let (status, body) = open(&state, person("jana"), json!({ "name": "mobilita" })).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

/// A `Project` manifest of the organization namespace, with `labels`.
fn project_manifest(name: &str, labels: &[(&str, &str)]) -> ResourceEnvelope {
    let mut project = manifest("org", "Project", name);
    project.metadata.labels = labels
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    project
}

async fn listed(state: AppState, cookie: &str) -> Vec<(String, bool)> {
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    list["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            (
                item["name"].as_str().expect("name").to_owned(),
                item["sample"].as_bool().expect("sample is a boolean"),
            )
        })
        .collect()
}

/// PF-109 (T-3234): the project labelled `joinedcontext.com/sample: "true"` is listed as the
/// sample, any other value or none is not, and a caller who may read no project is shown no
/// sample either: the label grants nothing.
#[tokio::test]
async fn the_labelled_project_is_listed_as_the_sample_to_those_who_may_read_it() {
    let mirror = two_project_mirror();
    mirror.upsert(project_manifest(
        "banskabystrica",
        &[("joinedcontext.com/sample", "true")],
    ));
    mirror.upsert(project_manifest(
        "helsinki",
        &[("joinedcontext.com/sample", "yes")],
    ));
    mirror.upsert(manifest("zilina", "ContextSpace", "zilina"));
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    assert_eq!(
        listed(
            AppState::new(config.clone(), None).with_mirror(mirror.clone()),
            &cookie
        )
        .await,
        [
            ("banskabystrica".to_owned(), true),
            ("helsinki".to_owned(), false),
            ("zilina".to_owned(), false),
        ]
    );

    use axum::response::IntoResponse;
    let now = session::now_unix();
    let outsider = Session {
        identity: Identity {
            client: None,
            subject: "f:1:passer.by".into(),
            username: "passer.by".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        access_expires_at: now + 3600,
        refresh_token: None,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &outsider)
        .expect("store session");
    let outsider_cookie = (jar, StatusCode::OK)
        .into_response()
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or_default().to_owned())
        .expect("a session cookie");
    assert_eq!(
        listed(
            AppState::new(config, None).with_mirror(mirror),
            &outsider_cookie
        )
        .await,
        []
    );
}

/// T-4243: each item carries its Project manifest's title as the manifest wrote it, a legacy
/// language map or one string, and none when the manifest names none or there is no manifest.
#[tokio::test]
async fn each_project_is_listed_with_the_title_its_manifest_gives_it() {
    let mirror = two_project_mirror();
    let mut city = project_manifest("banskabystrica", &[]);
    city.metadata.title = Some(jc_core::i18n::Text::Localized(
        serde_json::from_value(serde_json::json!({
            "sk": "Mesto Banská Bystrica",
            "en": "City of Banská Bystrica"
        }))
        .expect("a language map"),
    ));
    mirror.upsert(city);
    let mut region = project_manifest("helsinki", &[]);
    region.metadata.title = Some(jc_core::i18n::Text::Plain("Helsinki <b>".into()));
    mirror.upsert(region);
    mirror.upsert(manifest("zilina", "ContextSpace", "zilina"));
    mirror.upsert(project_manifest("zilina", &[]));
    let config = Config::for_tests();
    let cookie = make_session_cookie(&config);
    let response = server::app(AppState::new(config, None).with_mirror(mirror))
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let titles: Vec<(&str, Option<&serde_json::Value>)> = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| (item["name"].as_str().expect("name"), item.get("title")))
        .collect();
    assert_eq!(
        titles,
        [
            (
                "banskabystrica",
                Some(
                    &serde_json::json!({"sk": "Mesto Banská Bystrica", "en": "City of Banská Bystrica"})
                )
            ),
            ("helsinki", Some(&serde_json::json!("Helsinki <b>"))),
            ("zilina", None),
        ]
    );
}
