//! The manifests a merged change wrote (T-3975, API/01 §5): read at the change's last commit, for
//! the editor's "Restore this version"; a kind the caller does not read, a native file and a
//! deleted file are left out, nothing is written, and a change not merged is refused.

mod common;

use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{encode, envelope, forge, person, send, state_on, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";
const PIPELINE: &str = "projects/ovzdusie/spaces/air/pipelines/air-ingest/pipeline.yaml";
const NATIVE: &str = "projects/ovzdusie/spaces/air/pipelines/air-ingest/bento.yaml";
const ENDPOINT: &str = "projects/ovzdusie/spaces/air/endpoints/air-all/endpoint.yaml";
const GONE: &str = "projects/ovzdusie/spaces/air/pipelines/old/pipeline.yaml";
const WROTE: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: air-ingest\n  namespace: ovzdusie\nspec:\n  class: resident\n  secretRefs:\n    - name: mqtt\n      key: password\n";
const ENDPOINT_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: air-all\n  namespace: ovzdusie\nspec: {}\n";

fn pull(number: u64, merged: bool) -> Value {
    json!({
        "number": number,
        "html_url": format!("https://gitea.example/pulls/{number}"),
        "state": if merged { "closed" } else { "open" },
        "title": "update",
        "head": { "ref": format!("portal/update-pipeline-air-ingest-{number:08x}"), "sha": format!("head-{number}") },
        "base": { "ref": "main" },
        "merged": merged,
        "merge_base": "base-1",
        "user": { "login": "jana" }
    })
}

/// Change 6 is merged and wrote the pipeline, its bento file and an endpoint and deleted another
/// pipeline; change 8 is open.
async fn world() -> (MockServer, AppState) {
    let gitea = forge().await;
    let files = json!([
        { "filename": PIPELINE, "status": "modified" },
        { "filename": NATIVE, "status": "modified" },
        { "filename": ENDPOINT, "status": "added" },
        { "filename": GONE, "status": "deleted" },
        { "filename": "users/someone.yaml", "status": "modified" }
    ]);
    for (number, merged) in [(6, true), (8, false)] {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/pulls/{number}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(pull(number, merged)))
            .mount(&gitea)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/pulls/{number}/files")))
            .respond_with(ResponseTemplate::new(200).set_body_json(files.clone()))
            .mount(&gitea)
            .await;
    }
    for (file, content) in [
        (PIPELINE, WROTE),
        (NATIVE, "input:\n  generate: {}\n"),
        (ENDPOINT, ENDPOINT_YAML),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/contents/{file}")))
            .and(query_param("ref", "head-6"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "sha": "blob", "content": encode(content) })),
            )
            .mount(&gitea)
            .await;
    }
    let state = state_on(&gitea);
    for (role, kinds) in [
        ("pipelines", json!(["Pipeline"])),
        ("both", json!(["Pipeline", "Endpoint"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{role}-readers"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{role}@hel.fi") }], "role": role, "scope": { "project": PROJECT } }),
        ));
    }
    (gitea, state)
}

async fn manifests(state: &AppState, who: &str, project: &str, id: &str) -> (u16, Value) {
    let answer = send(
        state,
        person(who),
        "GET",
        &format!("/api/v1/projects/{project}/changes/{id}/manifests"),
        None,
    )
    .await;
    (
        answer.status.as_u16(),
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

fn names(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    format!(
                        "{}/{}",
                        item["kind"].as_str().unwrap_or(""),
                        item["metadata"]["name"].as_str().unwrap_or("")
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_merged_change_answers_the_manifests_it_wrote_at_its_last_commit() {
    let (gitea, state) = world().await;
    let (status, body) = manifests(&state, "both", PROJECT, "chg-00000006").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["kind"], json!("List"));
    // The bento file is no manifest, the deleted pipeline is no version, the user file no project file.
    assert_eq!(
        names(&body),
        vec!["Pipeline/air-ingest", "Endpoint/air-all"],
        "{body}"
    );
    assert_eq!(
        body["items"][0]["spec"]["secretRefs"][0],
        json!({ "name": "mqtt", "key": "password" })
    );
    let written = gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| request.method.as_str() != "GET")
        .count();
    assert_eq!(written, 0, "a read writes nothing");
}

#[tokio::test]
async fn a_kind_the_caller_does_not_read_is_left_out() {
    let (_gitea, state) = world().await;
    let (status, body) = manifests(&state, "pipelines", PROJECT, "chg-00000006").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(names(&body), vec!["Pipeline/air-ingest"], "{body}");
}

#[tokio::test]
async fn an_open_change_is_refused_in_words() {
    let (_gitea, state) = world().await;
    let (status, body) = manifests(&state, "both", PROJECT, "chg-00000008").await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["detail"].as_str().unwrap_or("").contains("not merged"),
        "{body}"
    );
}

#[tokio::test]
async fn another_project_or_no_binding_is_not_found() {
    let (_gitea, state) = world().await;
    let (status, _) = manifests(&state, "stranger", PROJECT, "chg-00000006").await;
    assert_eq!(status, 404);
    let (status, _) = manifests(&state, "both", "elsewhere", "chg-00000006").await;
    assert_eq!(status, 404);
    let (status, _) = manifests(&state, "both", PROJECT, "not-a-change").await;
    assert_eq!(status, 400);
}

/// T-5698: a reader bound to one context space reads the manifests of that space alone; the
/// Endpoint the change wrote names no space, so a reader of space `water` is not handed it.
#[tokio::test]
async fn a_reader_of_one_space_is_not_handed_another_spaces_manifest() {
    let (_gitea, state) = world().await;
    state
        .mirror
        .upsert(envelope("ContextSpace", "water", PROJECT, json!({})));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "water-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "water@hel.fi" }], "role": "both", "scope": { "contextSpace": "water" } }),
    ));
    let (status, body) = manifests(&state, "water", PROJECT, "chg-00000006").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(names(&body), Vec::<String>::new(), "{body}");
}
