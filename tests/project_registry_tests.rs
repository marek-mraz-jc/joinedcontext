//! A project's registry entry of layout 2: read with the declarations at its ref and the
//! repository's tags, and repointed to another ref or other values only through one red-lane
//! Change on the organization repository (T-3432, PF-86, CC-88).

mod common;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{encode, envelope, forge, person, send, state_on, Answer, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT_REPO: &str = "/api/v1/repos/test-owner/ovzdusie";
const ENTRY: &str = "projects/ovzdusie.yaml";
const BRANCH: &str = "portal/update-project-ovzdusie-registry";

const ENTRY_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  \
                          name: ovzdusie\n  namespace: org\nspec:\n  organizationRef: bb\n  \
                          repository:\n    name: ovzdusie\n  ref: main\n  parameters:\n    \
                          audience: public\n";

/// `project.yaml` of a release: `audience` everywhere, `city` only from `v0.2.0` on.
fn project_yaml(with_city: bool) -> String {
    let city = if with_city {
        "\n    city: { type: string, default: Banska Bystrica }"
    } else {
        ""
    };
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nmetadata:\n  name: ovzdusie\n  \
         namespace: org\nspec:\n  organizationRef: bb\n  version: 0.2.0\n  parameters:\n    \
         audience: {{ type: string, default: public, enum: [public, organization] }}{city}\n"
    )
}

async fn file(server: &MockServer, repo: &str, file: &str, git_ref: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{repo}/contents/{file}")))
        .and(query_param("ref", git_ref))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": format!("blob-{git_ref}"), "content": encode(body)
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

/// A layout 2 organization: `ovzdusie` registered at `main` of its own repository, which holds
/// `project.yaml` on `main` and at the tag `v0.2.0` and nothing at any other ref. `vlasta` may
/// propose in the organization, `petra` in the project alone, `cyril` only read the project, and
/// nobody else holds anything.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    file(&server, REPO, ENTRY, "main", ENTRY_YAML).await;
    file(
        &server,
        PROJECT_REPO,
        "project.yaml",
        "main",
        &project_yaml(false),
    )
    .await;
    file(
        &server,
        PROJECT_REPO,
        "project.yaml",
        "v0.2.0",
        &project_yaml(true),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT_REPO}/contents/project.yaml")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({ "message": "not found" })))
        .with_priority(5)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJECT_REPO}/tags")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "name": "v0.2.0", "commit": { "sha": "c0ffee2" } },
            { "name": "v0.1.0", "commit": { "sha": "c0ffee1" } }
        ])))
        .mount(&server)
        .await;

    let state = state_on(&server);
    state.mirror.upsert(envelope(
        "Organization",
        "bb",
        ORG_NAMESPACE,
        json!({ "domain": "banskabystrica.sk" }),
    ));
    state.mirror.upsert(envelope(
        "Project",
        "ovzdusie",
        ORG_NAMESPACE,
        json!({ "organizationRef": "bb", "repository": { "name": "ovzdusie" }, "ref": "main" }),
    ));
    for (role, verbs) in [
        ("reader", json!(["read"])),
        ("proposer", json!(["read", "propose"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["*", "Project"], "verbs": verbs }] }),
        ));
    }
    for (who, role, scope) in [
        ("vlasta", "proposer", json!({ "organization": "bb" })),
        ("cyril", "reader", json!({ "project": "ovzdusie" })),
        ("petra", "proposer", json!({ "project": "ovzdusie" })),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{role}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": scope }),
        ));
    }
    state.mirror.set_layout(2);
    state.mirror.set_repositories(BTreeMap::from([(
        "ovzdusie".to_owned(),
        "ovzdusie".to_owned(),
    )]));
    (server, state)
}

async fn repoint(state: &AppState, who: &str, body: Value) -> Answer {
    send(
        state,
        person(who),
        "PUT",
        "/api/v1/projects/ovzdusie/registry",
        Some(body),
    )
    .await
}

/// The registry entry every write to the organization repository put there, decoded.
async fn written_entries(server: &MockServer) -> Vec<(String, Value)> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| {
            r.method.as_str() == "PUT" && r.url.path() == format!("{REPO}/contents/{ENTRY}")
        })
        .map(|r| {
            let body: Value = serde_json::from_slice(&r.body).expect("json body");
            let content = STANDARD
                .decode(body["content"].as_str().unwrap_or_default())
                .expect("base64");
            let manifest: Value =
                serde_norway::from_slice(&content).expect("the written entry is yaml");
            (
                body["branch"].as_str().unwrap_or_default().to_owned(),
                manifest,
            )
        })
        .collect()
}

/// Every write that reached the forge, by anyone, as `VERB path`.
async fn writes(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("requests")
        .into_iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect()
}

fn refused(answer: &Answer, status: StatusCode, words: &str) {
    assert_eq!(answer.status, status, "{}", answer.text);
    assert!(
        answer.text.contains(words),
        "expected «{words}» in {}",
        answer.text
    );
}

/// PF-86, CC-88: a release is pinned by one red-lane Change on the organization repository that
/// changes `spec.ref` of the entry and nothing else.
#[tokio::test]
async fn a_tag_is_pinned_by_one_red_lane_organization_change() {
    let (server, state) = world().await;
    let answer = repoint(&state, "vlasta", json!({ "ref": "v0.2.0" })).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let change: Value = serde_json::from_str(&answer.text).expect("change");
    assert_eq!(change["status"]["lane"], "red", "{change}");

    let written = written_entries(&server).await;
    assert_eq!(written.len(), 1, "{written:?}");
    let (branch, manifest) = &written[0];
    assert_eq!(branch, BRANCH);
    let mut expected: Value = serde_norway::from_str(ENTRY_YAML).expect("entry");
    expected["spec"]["ref"] = json!("v0.2.0");
    assert_eq!(manifest, &expected, "only spec.ref changes");
    assert!(
        writes(&server)
            .await
            .iter()
            .all(|w| !w.contains(PROJECT_REPO)),
        "the project repository is only read"
    );
}

/// A ref the repository does not hold is refused by name before anything is written, and so is a
/// ref git would read as an option.
#[tokio::test]
async fn a_ref_the_repository_does_not_hold_is_refused_by_name() {
    let (server, state) = world().await;
    let answer = repoint(&state, "vlasta", json!({ "ref": "v9.9.9" })).await;
    refused(
        &answer,
        StatusCode::BAD_REQUEST,
        "'v9.9.9' is no tag, branch or commit",
    );
    let answer = repoint(&state, "vlasta", json!({ "ref": "--upload-pack=x" })).await;
    refused(&answer, StatusCode::BAD_REQUEST, "spec.ref");
    assert_eq!(writes(&server).await, Vec::<String>::new());
}

/// CC-88: values are checked against the declarations of the release the entry will pin: `city`
/// exists only from `v0.2.0` on, and an enum takes only its values.
#[tokio::test]
async fn values_are_checked_against_the_release_the_entry_will_pin() {
    let (_forge, state) = world().await;
    let answer = repoint(
        &state,
        "vlasta",
        json!({ "parameters": { "city": "Zvolen" } }),
    )
    .await;
    refused(&answer, StatusCode::BAD_REQUEST, "city");
    let answer = repoint(
        &state,
        "vlasta",
        json!({ "parameters": { "audience": "everyone" } }),
    )
    .await;
    refused(&answer, StatusCode::BAD_REQUEST, "audience");

    let (server, state) = world().await;
    let answer = repoint(
        &state,
        "vlasta",
        json!({ "ref": "v0.2.0", "parameters": { "city": "Zvolen", "audience": "organization" } }),
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let (_, manifest) = &written_entries(&server).await[0];
    assert_eq!(
        manifest["spec"]["parameters"],
        json!({ "city": "Zvolen", "audience": "organization" })
    );
}

/// An empty set returns every parameter to its default: the entry loses `spec.parameters`.
#[tokio::test]
async fn an_empty_set_returns_every_parameter_to_its_default() {
    let (server, state) = world().await;
    let answer = repoint(&state, "vlasta", json!({ "parameters": {} })).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let (_, manifest) = &written_entries(&server).await[0];
    assert!(manifest["spec"].get("parameters").is_none(), "{manifest}");
    assert_eq!(manifest["spec"]["ref"], "main");
}

/// A reader or a steward of the project alone may not repoint, a stranger meets a project that is not there (R20), an empty or
/// unknown body is refused, and a body that changes nothing proposes nothing.
#[tokio::test]
async fn who_may_repoint_and_what_is_refused() {
    let (server, state) = world().await;
    refused(
        &repoint(&state, "cyril", json!({ "ref": "v0.2.0" })).await,
        StatusCode::FORBIDDEN,
        "propose on Project",
    );
    // A steward of the project alone does not decide what the organization runs of it.
    refused(
        &repoint(&state, "petra", json!({ "ref": "v0.2.0" })).await,
        StatusCode::FORBIDDEN,
        "propose on Project",
    );
    refused(
        &repoint(&state, "stranger", json!({ "ref": "v0.2.0" })).await,
        StatusCode::NOT_FOUND,
        "ovzdusie",
    );
    refused(
        &repoint(&state, "vlasta", json!({})).await,
        StatusCode::BAD_REQUEST,
        "name the ref",
    );
    let unknown = repoint(&state, "vlasta", json!({ "ref": "v0.2.0", "secret": "x" })).await;
    assert!(unknown.status.is_client_error(), "{}", unknown.text);
    refused(
        &repoint(&state, "vlasta", json!({ "ref": "main" })).await,
        StatusCode::CONFLICT,
        "already runs that",
    );
    assert_eq!(writes(&server).await, Vec::<String>::new());
}

/// Layout 1 has no registry entry to repoint.
#[tokio::test]
async fn layout_one_has_no_registry_entry() {
    let (_forge, state) = world().await;
    state.mirror.set_layout(1);
    refused(
        &repoint(&state, "vlasta", json!({ "ref": "v0.2.0" })).await,
        StatusCode::CONFLICT,
        "layout 1",
    );
}

/// One repoint at a time (CC-34): a second one while the first is open is refused naming it.
#[tokio::test]
async fn a_second_repoint_waits_for_the_open_one() {
    let (server, state) = world().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "number": 41, "html_url": "https://gitea.example/pulls/41", "state": "open",
            "title": "repoint project ovzdusie", "head": { "ref": BRANCH }, "base": { "ref": "main" },
            "created_at": "2026-10-10T09:00:00Z", "mergeable": true, "merged": false
        }])))
        .with_priority(1)
        .mount(&server)
        .await;
    refused(
        &repoint(&state, "vlasta", json!({ "ref": "v0.2.0" })).await,
        StatusCode::CONFLICT,
        "already proposed",
    );
    assert_eq!(writes(&server).await, Vec::<String>::new());
}

/// The form's read: what runs, the values, the knobs of that release and the releases to pin.
#[tokio::test]
async fn the_entry_is_read_with_its_declarations_and_the_tags() {
    let (_forge, state) = world().await;
    let answer = send(
        &state,
        person("cyril"),
        "GET",
        "/api/v1/projects/ovzdusie/registry",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let entry: Value = serde_json::from_str(&answer.text).expect("entry");
    assert_eq!(entry["repository"], json!({ "name": "ovzdusie" }));
    assert_eq!(entry["ref"], "main");
    assert_eq!(entry["parameters"], json!({ "audience": "public" }));
    assert_eq!(entry["declarations"]["audience"]["type"], "string");
    assert!(
        entry["declarations"].get("city").is_none(),
        "main declares no city: {entry}"
    );
    assert_eq!(
        entry["tags"],
        json!([{ "name": "v0.2.0", "commit": "c0ffee2" }, { "name": "v0.1.0", "commit": "c0ffee1" }])
    );

    let stranger = send(
        &state,
        person("stranger"),
        "GET",
        "/api/v1/projects/ovzdusie/registry",
        None,
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);
}

/// The organization's change list shows the repoint as the Project it changes, red, so an
/// administrator can approve it from the Portal (PF-86, PF-58).
#[tokio::test]
async fn the_repoint_is_listed_among_the_organization_changes() {
    let (server, state) = world().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "number": 41, "html_url": "https://gitea.example/pulls/41", "state": "open",
            "title": "repoint project ovzdusie", "head": { "ref": BRANCH }, "base": { "ref": "main" },
            "created_at": "2026-10-10T09:00:00Z",
            "user": { "login": "vlasta", "full_name": "vlasta", "email": "vlasta@hel.fi" },
            "mergeable": true, "merged": false
        }])))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/41/files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "filename": ENTRY, "status": "modified" }
        ])))
        .mount(&server)
        .await;
    file(
        &server,
        REPO,
        ENTRY,
        BRANCH,
        &ENTRY_YAML.replace("ref: main", "ref: v0.2.0"),
    )
    .await;
    state.mirror.upsert(envelope(
        "Role",
        "admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["*", "Project"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "olga-admin",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "olga@hel.fi" }], "role": "admin", "scope": { "organization": "bb" } }),
    ));

    let answer = send(
        &state,
        person("olga"),
        "GET",
        "/api/v1/projects/org/changes",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let list: Value = serde_json::from_str(&answer.text).expect("list");
    let items = list["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["status"]["lane"], "red", "{list}");
    assert!(answer.text.contains("ovzdusie"), "{list}");
}
