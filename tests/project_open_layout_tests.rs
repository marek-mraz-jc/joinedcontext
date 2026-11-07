//! Opening a project in layout 2: its own repository, seeded, and the registry entry as the
//! organization's Change; both or neither (T-2643, CC-85, PF-86, PF-87).

mod common;

use axum::http::StatusCode;
use common::{envelope, forge, mount_repository, person, send, state_on, REPO};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const NEW: &str = "/api/v1/repos/test-owner/doprava";

/// The forge of a layout 2 organization that lets anyone open a project. The repository
/// `doprava` is not there until the Portal creates it; `exists` says it already is.
async fn world(exists: bool) -> (MockServer, AppState) {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(NEW))
        .respond_with(if exists {
            ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" }))
        } else {
            ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" }))
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/orgs/test-owner/repos"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "name": "doprava" })))
        .mount(&server)
        .await;
    for (verb, route) in [
        ("POST", format!("{NEW}/contents")),
        ("POST", format!("{NEW}/branch_protections")),
        ("DELETE", NEW.to_owned()),
    ] {
        Mock::given(method(verb))
            .and(path(route))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "seed" } })),
            )
            .mount(&server)
            .await;
    }
    let state = state_on(&server);
    state.mirror.upsert(envelope(
        "Organization",
        "bb",
        joinedcontext_portal::permissions::ORG_NAMESPACE,
        json!({ "domain": "banskabystrica.sk", "projects": { "creation": "anyone" } }),
    ));
    state.mirror.set_layout(2);
    (server, state)
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

/// The files the organization's Change wrote, path and decoded content.
async fn organization_files(server: &MockServer) -> Vec<(String, String)> {
    common::committed_files(server, REPO).await
}

async fn open(state: &AppState) -> (StatusCode, String) {
    let answer = send(
        state,
        person("nobody"),
        "POST",
        "/api/v1/projects",
        Some(json!({ "name": "doprava", "displayName": "Doprava" })),
    )
    .await;
    (answer.status, answer.text)
}

/// PF-86, PF-87: the project's repository is created private, seeded on `main` with its layout,
/// its own file at 0.1.0 and CODEOWNERS, and `main` protected; the organization's Change carries
/// the registry entry and the creator's binding, and not the project's own file.
#[tokio::test]
async fn a_project_opens_into_a_repository_of_its_own() {
    let (server, state) = world(false).await;
    let (status, text) = open(&state).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");

    let created = received(&server, "POST", "/api/v1/orgs/test-owner/repos").await;
    assert_eq!(created.len(), 1);
    assert_eq!(
        (created[0]["name"].as_str(), created[0]["private"].as_bool()),
        (Some("doprava"), Some(true))
    );

    let seed = received(&server, "POST", &format!("{NEW}/contents")).await;
    assert_eq!(seed.len(), 1, "one commit");
    assert_eq!(seed[0]["branch"], "main");
    let mut seeded: Vec<&str> = seed[0]["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    seeded.sort_unstable();
    assert_eq!(
        seeded,
        [
            ".gitea/workflows/validate.yml",
            ".jc/layout",
            "CODEOWNERS",
            "project.yaml"
        ]
    );
    // CC-90: the workflow validates the project under its slug on the runner that carries
    // jcctl, and fetches no action from outside the installation.
    let workflow = seed[0]["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|file| file["path"] == ".gitea/workflows/validate.yml")
        .and_then(|file| file["content"].as_str())
        .and_then(|content| {
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, content).ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .expect("the workflow");
    let parsed: Value = serde_norway::from_str(&workflow).expect("the workflow is YAML");
    assert_eq!(parsed["jobs"]["validate"]["runs-on"], "node-22");
    assert!(
        workflow.contains("jcctl validate --project . --slug"),
        "{workflow}"
    );
    assert!(!workflow.contains("uses:"), "no action is fetched");
    assert_eq!(
        received(&server, "POST", &format!("{NEW}/branch_protections")).await[0]["rule_name"],
        "main"
    );

    let files = organization_files(&server).await;
    let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
    assert!(paths.contains(&"projects/doprava.yaml"), "{paths:?}");
    assert!(
        paths.contains(&"users/assignments/doprava-creator.yaml"),
        "{paths:?}"
    );
    assert!(
        !paths.contains(&"projects/doprava/project.yaml"),
        "{paths:?}"
    );
    let entry = &files
        .iter()
        .find(|(path, _)| path == "projects/doprava.yaml")
        .expect("entry")
        .1;
    let entry: jc_core::kinds::Project =
        jc_core::kinds::Project::from_yaml(entry).unwrap_or_else(|e| panic!("{e}: {entry}"));
    entry.validate().expect("a valid registry entry");
    assert!(entry.spec.is_registry_entry());
    assert_eq!(entry.spec.git_ref.as_deref(), Some("main"));
}

/// PF-86: a repository of that name already in the forge is refused, never adopted, and
/// nothing is written anywhere.
#[tokio::test]
async fn a_repository_that_is_already_there_is_not_adopted() {
    let (server, state) = world(true).await;
    let (status, text) = open(&state).await;
    assert_eq!(status, StatusCode::CONFLICT, "{text}");
    assert!(text.contains("doprava"), "{text}");
    assert!(received(&server, "POST", &format!("{NEW}/contents"))
        .await
        .is_empty());
    assert!(organization_files(&server).await.is_empty());
    assert!(
        received(&server, "DELETE", NEW).await.is_empty(),
        "somebody's repository stays"
    );
}

/// CC-85: when the organization's Change cannot be opened, the repository the Portal just
/// created is removed again, so no repository is left that no registry entry names.
#[tokio::test]
async fn a_failed_registration_removes_the_new_repository() {
    let (server, state) = world(false).await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "message": "down" })))
        .with_priority(1)
        .mount(&server)
        .await;
    let (status, text) = open(&state).await;
    assert!(status.is_server_error(), "{status}: {text}");
    assert_eq!(
        received(&server, "DELETE", NEW).await.len(),
        1,
        "the new repository is removed"
    );
}

/// PF-77, PF-86: deleting a project of layout 2 is the organization's Change that removes its
/// registry entry and the bindings that name it; the project's repository is not touched here
/// (the sync archives it once the entry is gone).
#[tokio::test]
async fn deleting_a_project_removes_its_registry_entry() {
    let (server, state) = world(false).await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/main")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "truncated": false,
            "tree": [
                { "path": "org.yaml", "type": "blob" },
                { "path": "projects/doprava.yaml", "type": "blob" },
                { "path": "projects/ovzdusie.yaml", "type": "blob" },
                { "path": "users/assignments/doprava-creator.yaml", "type": "blob" },
            ],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex(format!(
            "^{REPO}/contents/.*"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1", "content": "", "encoding": "base64"
        })))
        .mount(&server)
        .await;
    let org = joinedcontext_portal::permissions::ORG_NAMESPACE;
    state.mirror.upsert(envelope(
        "Project",
        "doprava",
        org,
        json!({ "organizationRef": "bb" }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        org,
        json!({ "rules": [{ "kinds": ["Project", "RoleBinding"], "verbs": ["propose", "approve", "delete", "read"] }] }),
    ));
    for (binding, scope) in [
        ("org-admins", json!({ "organization": "bb" })),
        ("doprava-creator", json!({ "project": "doprava" })),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            binding,
            org,
            json!({ "subjects": [{ "user": "admin@hel.fi" }], "role": "org-admin", "scope": scope }),
        ));
    }
    state
        .mirror
        .set_repositories(std::collections::BTreeMap::from([(
            "doprava".to_owned(),
            "doprava".to_owned(),
        )]));

    let answer = send(
        &state,
        person("admin"),
        "DELETE",
        "/api/v1/projects/doprava",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    assert_eq!(
        common::removed_in_one_commit(&server).await,
        [
            "projects/doprava.yaml",
            "users/assignments/doprava-creator.yaml",
        ],
        "the entry and the binding, and not the other project's entry nor the repository"
    );
}

// --- an organization Change keeps its own id under a project (T-2656, CC-87) ---------------

/// The paths of the pull request reads the forge received, either repository.
async fn pulls_read(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() == "GET" && r.url.path().contains("/pulls/"))
        .map(|r| r.url.path().to_owned())
        .collect()
}

/// `doprava` has its own repository, and `admin@hel.fi` reads and decides its changes.
fn registered(state: &AppState) {
    let org = joinedcontext_portal::permissions::ORG_NAMESPACE;
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        org,
        json!({ "rules": [{ "kinds": ["Project", "RoleBinding", "ContextSpace"], "verbs": ["propose", "approve", "delete", "read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "org-admins",
        org,
        json!({ "subjects": [{ "user": "admin@hel.fi" }], "role": "org-admin", "scope": { "organization": "bb" } }),
    ));
    state
        .mirror
        .set_repositories(std::collections::BTreeMap::from([(
            "doprava".to_owned(),
            "doprava".to_owned(),
        )]));
}

/// CC-87: the Change that opens a project is a merge request of the organization repository,
/// so it is named `chg-org-`, and the name still means that merge request once the project has
/// its own repository.
#[tokio::test]
async fn the_opening_change_is_named_for_the_organization_repository() {
    let (_server, state) = world(false).await;
    let (status, text) = open(&state).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    let change: Value = serde_json::from_str(&text).expect("a Change");
    assert_eq!(change["metadata"]["name"], "chg-org-00000009");
}

/// CC-87: read, approve and reject by `chg-org-` reach the organization repository after the project is
/// registered, never the project repository's merge request of the same number.
#[tokio::test]
async fn an_organization_change_is_read_and_rejected_in_the_organization_repository() {
    let (server, state) = world(false).await;
    mount_repository(&server, NEW).await;
    registered(&state);

    for (verb, uri, body) in [
        (
            "GET",
            "/api/v1/projects/doprava/changes/chg-org-00000009",
            None,
        ),
        (
            "POST",
            "/api/v1/projects/doprava/changes/chg-org-00000009/approve",
            Some(json!({ "confirm": "doprava" })),
        ),
        (
            "POST",
            "/api/v1/projects/doprava/changes/chg-org-00000009/reject",
            Some(json!({ "reason": "not yet" })),
        ),
    ] {
        send(&state, person("admin"), verb, uri, body).await;
    }

    let read = pulls_read(&server).await;
    let organization = format!("{REPO}/pulls/9");
    assert!(
        read.iter().filter(|path| **path == organization).count() >= 3,
        "each of read, approve and reject asked the organization repository: {read:?}"
    );
    assert!(!read.iter().any(|path| path.starts_with(NEW)), "{read:?}");
}

/// CC-87: a `chg-` id under a registered project is still its own repository's merge request.
#[tokio::test]
async fn a_project_change_keeps_its_id_and_its_repository() {
    let (server, state) = world(false).await;
    mount_repository(&server, NEW).await;
    registered(&state);

    send(
        &state,
        person("admin"),
        "GET",
        "/api/v1/projects/doprava/changes/chg-00000009",
        None,
    )
    .await;

    let read = pulls_read(&server).await;
    assert!(read.contains(&format!("{NEW}/pulls/9")), "{read:?}");
    assert!(
        !read
            .iter()
            .any(|path| path.starts_with(&format!("{REPO}/"))),
        "{read:?}"
    );
}

/// PF-106: the gateway's forge user reads the configuration's repositories and nothing more,
/// so a project repository the Portal opens names it as a reader; without `JC_GITEA_READER`
/// nobody is added.
#[tokio::test]
async fn the_gateway_reads_a_project_repository_the_portal_opens() {
    let (server, mut state) = world(false).await;
    Mock::given(method("PUT"))
        .and(path(format!("{NEW}/collaborators/jc-gateway")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let uri = server.uri();
    let client = joinedcontext_portal::git::GiteaClient::from_env(|key| {
        match key {
            "JC_GITEA_URL" => Some(uri.as_str()),
            "JC_GITEA_OWNER" => Some("test-owner"),
            "JC_GITEA_REPO" => Some("test-repo"),
            "JC_GITEA_TOKEN" => Some("token-xyz"),
            "JC_GITEA_READER" => Some("jc-gateway"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config")
    .expect("configured");
    state.gitea = Some(std::sync::Arc::new(client));
    let (status, text) = open(&state).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    assert_eq!(
        received(&server, "PUT", &format!("{NEW}/collaborators/jc-gateway")).await,
        vec![json!({ "permission": "read" })]
    );

    let (server, state) = world(false).await;
    let (status, text) = open(&state).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{text}");
    let added = server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.url.path().contains("/collaborators/"))
        .count();
    assert_eq!(added, 0, "a reader was added that nobody named");
}
