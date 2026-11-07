//! Duplicating a project of layout 2: its repository copied with its history under a new slug,
//! remounted, and the registry entry as the organization's Change; the origin only read
//! (T-2643, PF-89, PF-83, EP-02).

mod common;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{envelope, forge, person, send, state_on, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ORIGIN: &str = "/api/v1/repos/test-owner/ovzdusie";
const COPY: &str = "/api/v1/repos/test-owner/doprava";
const SLUG: &str = "mluyob4nz52lok3ssk7pgn5vwt";

const PROJECT: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  \
                       name: ovzdusie\n  namespace: org\nspec:\n  organizationRef: bb\n  \
                       parameters:\n    audience: { type: string, default: public }\n";

fn endpoint() -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air\n  \
         namespace: ovzdusie\nspec:\n  contextSpaceRef: ovzdusie\n  slug: {SLUG}\n  audience: \
         public\n"
    )
}

/// Files of one repository at `main`: its tree and each file's contents.
async fn repository(server: &MockServer, base: &str, files: &[(&str, String)]) {
    let tree: Vec<_> = files
        .iter()
        .map(|(file, _)| json!({ "path": file, "type": "blob", "sha": format!("blob-{file}") }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{base}/git/trees/main")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "tree", "truncated": false, "tree": tree
        })))
        .mount(server)
        .await;
    for (file, body) in files {
        Mock::given(method("GET"))
            .and(path(format!("{base}/contents/{file}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": format!("blob-{file}"), "content": STANDARD.encode(body)
            })))
            .mount(server)
            .await;
    }
}

/// A layout 2 organization that lets anyone open a project, `ovzdusie` in its own repository
/// and readable by `nobody@hel.fi`; `doprava` is not in the forge until the migration makes it,
/// unless `exists`.
async fn world(exists: bool) -> (MockServer, AppState) {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(COPY))
        .respond_with(if exists {
            ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" }))
        } else {
            ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" }))
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/migrate"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "name": "doprava" })))
        .mount(&server)
        .await;
    for (verb, route) in [
        ("POST", format!("{COPY}/contents")),
        ("POST", format!("{COPY}/branch_protections")),
        ("DELETE", COPY.to_owned()),
    ] {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
            )
            .mount(&server)
            .await;
    }
    repository(&server, ORIGIN, &[("project.yaml", PROJECT.to_owned())]).await;
    // What the migration copied: the origin's files, still naming the origin.
    repository(
        &server,
        COPY,
        &[
            (".jc/layout", "2\n".to_owned()),
            ("project.yaml", PROJECT.to_owned()),
            ("CODEOWNERS", "* @test-owner/ovzdusie-writers\n".to_owned()),
            ("spaces/ovzdusie/endpoints/air.yaml", endpoint()),
            // The origin's credential, encrypted for the Portal (CC-06).
            (
                "secrets/feed.enc.yaml",
                "feed:\n  password: ENC[AES256_GCM,data:x]\nsops: {}\n".to_owned(),
            ),
        ],
    )
    .await;

    let state = state_on(&server);
    state.mirror.upsert(envelope(
        "Organization",
        "bb",
        ORG_NAMESPACE,
        json!({ "domain": "banskabystrica.sk", "projects": { "creation": "anyone" } }),
    ));
    state.mirror.upsert(envelope(
        "Project",
        "ovzdusie",
        ORG_NAMESPACE,
        json!({ "organizationRef": "bb" }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["*"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "nobody-reads",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "nobody@hel.fi" }], "role": "reader", "scope": { "project": "ovzdusie" } }),
    ));
    state.mirror.set_layout(2);
    state.mirror.set_repositories(BTreeMap::from([(
        "ovzdusie".to_owned(),
        "ovzdusie".to_owned(),
    )]));
    (server, state)
}

async fn duplicate(state: &AppState, who: &str, body: Value) -> (StatusCode, String) {
    let answer = send(
        state,
        person(who),
        "POST",
        "/api/v1/projects/ovzdusie/duplicate",
        Some(body),
    )
    .await;
    (answer.status, answer.text)
}

async fn received(server: &MockServer, verb: &str, route: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path() == route)
        .map(|r| serde_json::from_slice(&r.body).unwrap_or_default())
        .collect()
}

/// Every write that reached the origin's repository.
async fn origin_writes(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() != "GET" && r.url.path().starts_with(ORIGIN))
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

/// PF-89, PF-83, EP-02: the copy is migrated from the forge itself with its history, remounted
/// onto the new slug with fresh endpoint slugs and its own writers, `main` protected; the
/// organization's Change carries the entry with the given parameters; the origin is only read.
#[tokio::test]
async fn a_project_is_duplicated_into_a_repository_of_its_own() {
    let (server, state) = world(false).await;
    let (status, text) = duplicate(
        &state,
        "nobody",
        json!({ "name": "doprava", "parameters": { "audience": "organization" } }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");

    let migrated = received(&server, "POST", "/api/v1/repos/migrate").await;
    assert_eq!(migrated.len(), 1);
    let clone_addr = migrated[0]["clone_addr"].as_str().unwrap_or_default();
    assert!(
        clone_addr.ends_with("/test-owner/ovzdusie.git"),
        "{clone_addr}"
    );
    assert_eq!(
        (
            migrated[0]["repo_name"].as_str(),
            migrated[0]["private"].as_bool(),
            migrated[0]["mirror"].as_bool(),
        ),
        (Some("doprava"), Some(true), Some(false))
    );

    let commits = received(&server, "POST", &format!("{COPY}/contents")).await;
    assert_eq!(commits.len(), 1, "one remount commit");
    assert_eq!(commits[0]["branch"], "main");
    let files = commits[0]["files"].as_array().expect("files");
    let deleted: Vec<&str> = files
        .iter()
        .filter(|file| file["operation"] == "delete")
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert_eq!(
        deleted,
        ["secrets/feed.enc.yaml"],
        "the origin's secrets stay behind (F-R2POR3-001)"
    );
    let written: BTreeMap<String, String> = files
        .iter()
        .filter(|file| file["operation"] == "upload")
        .map(|file| {
            let content = STANDARD
                .decode(file["content"].as_str().unwrap_or_default())
                .unwrap_or_default();
            (
                file["path"].as_str().unwrap_or_default().to_owned(),
                String::from_utf8(content).unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        written.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "CODEOWNERS",
            "project.yaml",
            "spaces/ovzdusie/endpoints/air.yaml"
        ],
        ".jc/layout is left as it is"
    );
    assert_eq!(written["CODEOWNERS"], "* @test-owner/doprava-writers\n");
    let project: Value = serde_norway::from_str(&written["project.yaml"]).expect("yaml");
    assert_eq!(project["metadata"]["name"], "doprava");
    let endpoint: Value =
        serde_norway::from_str(&written["spaces/ovzdusie/endpoints/air.yaml"]).expect("yaml");
    assert_eq!(endpoint["metadata"]["namespace"], "doprava");
    let slug = endpoint["spec"]["slug"].as_str().unwrap_or_default();
    assert_ne!(slug, SLUG, "the copy does not answer at the origin's URL");
    jc_core::kinds::EndpointSlug::new(slug).expect("an EP-02 slug");
    assert_eq!(
        received(&server, "POST", &format!("{COPY}/branch_protections")).await[0]["rule_name"],
        "main"
    );

    let (_, yaml) = common::committed_files(&server, REPO)
        .await
        .into_iter()
        .find(|(path, _)| path == "projects/doprava.yaml")
        .expect("the registry entry");
    let entry = jc_core::kinds::Project::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e}: {yaml}"));
    assert!(entry.spec.is_registry_entry());
    let entry: Value = serde_norway::from_str(&yaml).expect("yaml");
    assert_eq!(entry["spec"]["repository"]["name"], "doprava");
    assert_eq!(entry["spec"]["parameters"]["audience"], "organization");

    assert!(
        origin_writes(&server).await.is_empty(),
        "{:?}",
        origin_writes(&server).await
    );
    assert!(received(&server, "DELETE", COPY).await.is_empty());
}

/// CC-88: a value for a parameter the origin does not declare is refused before anything is
/// copied.
#[tokio::test]
async fn an_undeclared_parameter_copies_nothing() {
    let (server, state) = world(false).await;
    let (status, text) = duplicate(
        &state,
        "nobody",
        json!({ "name": "doprava", "parameters": { "colour": "red" } }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("colour"), "{text}");
    assert!(received(&server, "POST", "/api/v1/repos/migrate")
        .await
        .is_empty());
}

/// PF-89: a repository of the new name already in the forge is refused, never adopted or
/// removed.
#[tokio::test]
async fn a_repository_that_is_already_there_is_not_overwritten() {
    let (server, state) = world(true).await;
    let (status, text) = duplicate(&state, "nobody", json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(received(&server, "POST", "/api/v1/repos/migrate")
        .await
        .is_empty());
    assert!(received(&server, "DELETE", COPY).await.is_empty());
}

/// CC-85: when the organization's Change cannot be opened, the copy is removed again.
#[tokio::test]
async fn a_failed_registration_removes_the_copy() {
    let (server, state) = world(false).await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "message": "down" })))
        .with_priority(1)
        .mount(&server)
        .await;
    let (status, text) = duplicate(&state, "nobody", json!({ "name": "doprava" })).await;
    assert!(status.is_server_error(), "{status}: {text}");
    assert_eq!(received(&server, "DELETE", COPY).await.len(), 1);
    assert!(origin_writes(&server).await.is_empty());
}

/// PF-59, R20: a caller no binding lets read the origin gets the same `404` as for a project
/// that is not there, and nothing is copied.
#[tokio::test]
async fn a_project_the_caller_may_not_read_is_not_duplicated() {
    let (server, state) = world(false).await;
    let (status, text) = duplicate(&state, "stranger", json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    assert!(received(&server, "POST", "/api/v1/repos/migrate")
        .await
        .is_empty());
}

/// PF-89: layout 1 has no project repository to copy; the answer names the import instead.
#[tokio::test]
async fn layout_one_answers_with_the_import() {
    let (server, state) = world(false).await;
    state.mirror.set_layout(1);
    let (status, text) = duplicate(&state, "nobody", json!({ "name": "doprava" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(text.contains("MF-45"), "{text}");
    assert!(received(&server, "POST", "/api/v1/repos/migrate")
        .await
        .is_empty());
}

/// PF-67: the new slug passes the checks opening does; a taken one is refused.
#[tokio::test]
async fn a_taken_name_is_refused() {
    let (server, state) = world(false).await;
    let (status, text) = duplicate(&state, "nobody", json!({ "name": "ovzdusie" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(received(&server, "POST", "/api/v1/repos/migrate")
        .await
        .is_empty());
}
