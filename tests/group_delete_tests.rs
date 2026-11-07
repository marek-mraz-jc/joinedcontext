//! PF-95: deleting a `Group` is one Change that also takes the group out of every `RoleBinding`
//! and every `App.spec.access` entry naming it; a binding or an entry left naming nobody goes
//! with it. A reference in a project's own repository (layout 2) cannot ride in that Change and
//! refuses the deletion, and so does a cascade that would leave no administrator (PF-03).

mod common;

use axum::http::StatusCode;
use base64::Engine;
use serde_json::{json, Value};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, forge, person, send, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const GROUP: &str = "/api/v1/projects/org/groups/stewards";

/// `ada@hel.fi` administers the organization; `stewards` is named by two bindings and one App.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    let mirror = &state.mirror;
    mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{
            "kinds": ["Group", "RoleBinding", "Role", "App"],
            "verbs": ["propose", "approve", "delete"],
        }]}),
    ));
    mirror.upsert(envelope(
        "Role",
        "steward",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["propose", "approve"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "admins",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "ada@hel.fi" }],
            "role": "org-admin",
            "scope": { "organization": "hel" },
        }),
    ));
    mirror.upsert(envelope(
        "Group",
        "stewards",
        ORG_NAMESPACE,
        json!({ "members": [{ "user": "jana@hel.fi" }] }),
    ));
    mirror.upsert(envelope(
        "Group",
        "editors",
        ORG_NAMESPACE,
        json!({ "members": [] }),
    ));
    // Names the group and a person: the person stays.
    mirror.upsert(envelope(
        "RoleBinding",
        "mixed",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "stewards" }, { "user": "jana@hel.fi" }],
            "role": "steward",
            "scope": { "project": "ovzdusie" },
        }),
    ));
    // Names the group alone: nobody is left, so the binding goes.
    mirror.upsert(envelope(
        "RoleBinding",
        "stewards-only",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "stewards" }],
            "role": "steward",
            "scope": { "project": "ovzdusie" },
        }),
    ));
    // Names another group: untouched.
    mirror.upsert(envelope(
        "RoleBinding",
        "editors",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "editors" }],
            "role": "steward",
            "scope": { "project": "ovzdusie" },
        }),
    ));
    mirror.upsert(envelope(
        "App",
        "alerts",
        "ovzdusie",
        json!({
            "kind": "static",
            "roles": [{ "name": "viewer" }, { "name": "steward" }],
            "access": [
                { "role": "viewer", "subjects": [{ "group": "stewards" }, { "group": "editors" }] },
                { "role": "steward", "subjects": [{ "group": "stewards" }] },
            ],
        }),
    ));
    state
}

/// Every manifest the forge holds reads back as a blob with a sha.
async fn files_exist(gitea: &MockServer) {
    Mock::given(method("GET"))
        .and(path_regex(format!("^{REPO}/contents/.+")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": base64::engine::general_purpose::STANDARD.encode("kind: Group\n"),
        })))
        .with_priority(5)
        .mount(gitea)
        .await;
}

/// The files of the one commit the deletion made: `(operation, path, decoded content)`.
async fn committed(gitea: &MockServer) -> Vec<(String, String, String)> {
    let requests = gitea.received_requests().await.unwrap_or_default();
    let commits: Vec<Value> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/contents"))
        .filter_map(|r| serde_json::from_slice(&r.body).ok())
        .collect();
    assert_eq!(commits.len(), 1, "one commit for the whole removal");
    commits[0]["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| {
            let content = file["content"]
                .as_str()
                .and_then(|c| base64::engine::general_purpose::STANDARD.decode(c).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default();
            (
                file["operation"].as_str().unwrap_or_default().to_owned(),
                file["path"].as_str().unwrap_or_default().to_owned(),
                content,
            )
        })
        .collect()
}

fn file<'a>(
    files: &'a [(String, String, String)],
    suffix: &str,
) -> Option<&'a (String, String, String)> {
    files.iter().find(|(_, path, _)| path.ends_with(suffix))
}

#[tokio::test]
async fn a_group_leaves_every_binding_and_access_entry_in_the_same_change() {
    let gitea = forge().await;
    files_exist(&gitea).await;
    let state = state_with(&gitea);

    let answer = send(
        &state,
        person("ada"),
        "DELETE",
        &format!("{GROUP}?confirm=stewards"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);

    let files = committed(&gitea).await;
    let (op, _, _) = file(&files, "groups/stewards.yaml").expect("the group is removed");
    assert_eq!(op, "delete");
    let (op, _, _) = file(&files, "stewards-only.yaml").expect("the emptied binding goes");
    assert_eq!(op, "delete");

    let (op, _, mixed) = file(&files, "mixed.yaml").expect("the mixed binding is edited");
    assert_eq!(op, "upload");
    assert!(mixed.contains("jana@hel.fi"), "the person stays: {mixed}");
    assert!(!mixed.contains("stewards"), "the group is gone: {mixed}");

    let (op, _, app) = file(&files, "apps/alerts/app.yaml").expect("the App is edited");
    assert_eq!(op, "upload");
    assert!(!app.contains("stewards"), "{app}");
    assert!(
        app.contains("editors"),
        "another group's access stays: {app}"
    );
    assert!(
        !app.contains("role: steward"),
        "an entry left naming nobody is dropped: {app}"
    );

    assert!(
        file(&files, "rolebindings/editors.yaml").is_none(),
        "a binding that never named the group is not touched: {files:?}"
    );
    assert!(file(&files, "admins.yaml").is_none(), "{files:?}");
}

/// Layout 2: an App in a project with its own repository cannot ride in the organization's Change.
#[tokio::test]
async fn a_reference_in_a_projects_own_repository_refuses_the_deletion_naming_it() {
    let gitea = forge().await;
    files_exist(&gitea).await;
    let state = state_with(&gitea);
    state
        .mirror
        .set_repositories(std::collections::BTreeMap::from([(
            "ovzdusie".to_owned(),
            "ovzdusie".to_owned(),
        )]));

    let answer = send(
        &state,
        person("ada"),
        "DELETE",
        &format!("{GROUP}?confirm=stewards"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer.text.contains("App alerts in project ovzdusie"),
        "{}",
        answer.text
    );
    let wrote = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .any(|r| r.method.as_str() != "GET");
    assert!(!wrote, "nothing is written for a refused deletion");
}

/// PF-03: the administrators' binding names the group alone; deleting the group would empty it.
#[tokio::test]
async fn a_cascade_that_leaves_no_administrator_is_refused() {
    let gitea = forge().await;
    files_exist(&gitea).await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "RoleBinding",
        "admins",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "group": "stewards" }],
            "role": "org-admin",
            "scope": { "organization": "hel" },
        }),
    ));
    let mut ada = person("ada");
    ada.groups = vec!["stewards".to_owned()];

    let answer = send(
        &state,
        ada,
        "DELETE",
        &format!("{GROUP}?confirm=stewards"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("PF-03"), "{}", answer.text);
}

/// The cascade needs the rights each of its edits needs: removing a group is not a way to change
/// a binding or an App the deleter may not change.
#[tokio::test]
async fn deleting_a_group_needs_the_rights_its_cascade_needs() {
    let gitea = forge().await;
    files_exist(&gitea).await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "group-keeper",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Group"], "verbs": ["propose", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "keepers",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "kim@hel.fi" }],
            "role": "group-keeper",
            "scope": { "organization": "hel" },
        }),
    ));

    let answer = send(
        &state,
        person("kim"),
        "DELETE",
        &format!("{GROUP}?confirm=stewards"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(
        answer.text.contains("RoleBinding") || answer.text.contains("App"),
        "{}",
        answer.text
    );
}

/// A group nothing names is removed alone, as before.
#[tokio::test]
async fn a_group_nothing_names_is_removed_alone() {
    let gitea = forge().await;
    files_exist(&gitea).await;
    let state = state_with(&gitea);
    state.mirror.upsert(envelope(
        "Group",
        "lonely",
        ORG_NAMESPACE,
        json!({ "members": [] }),
    ));

    let answer = send(
        &state,
        person("ada"),
        "DELETE",
        "/api/v1/projects/org/groups/lonely?confirm=lonely",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let files = committed(&gitea).await;
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].1.ends_with("groups/lonely.yaml"));
}
