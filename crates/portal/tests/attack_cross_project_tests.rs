//! Attack vector T-1682 (PF-59, R20): a person of project A reads or writes project B.
//!
//! A is `ovzdusie`, B is `doprava`, and the attacker is a steward of A alone. B goes in the path,
//! in a workspace name, in a draft key and — the step that got through — in a file of a change
//! approved through A's path.
//!
//! What the existing suites already play, so this file does not repeat it:
//! `project_read_tests::a_project_the_caller_may_not_read_is_not_there` (six read routes with B in
//! the path), `edge_changes_tests::a_change_is_not_readable_through_another_projects_path` and
//! `::a_change_is_in_the_list_of_the_project_it_changes_and_of_no_other` (a change id of B),
//! `agent_runs_tests` and `edge_agent_runs_tests` (a run id of B), `permissions_tests::the_organization_endpoint_list_answers_what_each_binding_reaches`
//! (a list filtered item by item), `access_escalation_tests::a_role_of_one_project_grants_nothing_in_another_and_nothing_at_organization_scope`
//! (a role of B reached from A). What was left is here.
//!
//! **One step of this vector is somebody else's task and was left alone.** A workspace name is
//! organization-wide, and the two wordings of the 404 (`no workspace named 'x'` for a name nobody
//! has, `…in project 'y'` for one that belongs elsewhere) tell a caller which names exist in every
//! other project. worker-1 found it first, filed it as **T-2296** with the conflict on `open` that
//! leaks the same thing, and pinned today's behaviour in
//! `edge_workspace_routes_tests::reading_a_workspace_of_another_project_is_not_there_in_two_different_wordings`
//! and `::the_name_of_a_workspace_in_another_project_comes_back_as_a_conflict`. Fixing half of it
//! here would have collided with their branch and left the other half open.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const MINE: &str = "ovzdusie";
const THEIRS: &str = "doprava";
/// The branch of merge request 12: a Pipeline of `ovzdusie`, so the change's headline resolves in
/// the attacker's own project and the page opens.
const BRANCH: &str = "portal/update-pipeline-aq-1682aaaa";

fn pipeline_yaml(project: &str, name: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: {name}\n  \
         namespace: {project}\nspec:\n  contextSpaceRef: air\n  source:\n    kind: http\n"
    )
}

/// A forge holding merge request 12 with `files` in it, each mounted on both branches.
async fn forge_with(files: &[(&str, String)]) -> MockServer {
    let gitea = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{}/pulls/12", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 12,
            "html_url": "https://gitea.example/pulls/12",
            "state": "open",
            "title": "update Pipeline aq",
            "head": { "ref": BRANCH },
            "base": { "ref": "main" },
            "created_at": "2026-09-18T09:14:22Z",
            "user": { "login": "someone.else", "full_name": "Someone Else", "email": "else@hel.fi" },
            "mergeable": true,
            "merged": false
        })))
        .mount(&gitea)
        .await;
    let listed: Vec<Value> = files
        .iter()
        .map(|(path, _)| json!({ "filename": path, "status": "modified" }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{}/pulls/12/files", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(listed))
        .mount(&gitea)
        .await;
    for (repo_path, content) in files {
        Mock::given(method("GET"))
            .and(path(format!("{}/contents/{repo_path}", common::REPO)))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "sha": "blob-1",
                "content": common::encode(content),
            })))
            .mount(&gitea)
            .await;
    }
    gitea
}

/// `steward@hel.fi` may read, propose, approve and delete Pipelines and RoleBindings — in
/// `ovzdusie` and nowhere else. That is the whole attack: a binding of one project.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(common::envelope(
        "Role",
        "pipelines-of-ovzdusie",
        ORG_NAMESPACE,
        json!({ "rules": [{
            "kinds": ["Pipeline", "RoleBinding"],
            "verbs": ["read", "propose", "approve", "delete"],
        }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "steward-of-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "steward@hel.fi" }],
            "role": "pipelines-of-ovzdusie",
            "scope": { "project": MINE },
        }),
    ));
    state
}

/// Everything the forge was asked to change: a merge, a close, a review or a write.
async fn mutated(gitea: &MockServer) -> Vec<String> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| !request.method.to_string().eq_ignore_ascii_case("get"))
        .map(|request| format!("{} {}", request.method, request.url.path()))
        .collect()
}

async fn approve(state: &AppState) -> common::Answer {
    common::send(
        state,
        common::person("steward"),
        "POST",
        &format!("/api/v1/projects/{MINE}/changes/chg-0000000c/approve"),
        Some(json!({ "confirm": "aq" })),
    )
    .await
}

/// PF-59, R20: the approval of a change is made with the caller's rights **in the project of the
/// path**, so a file living in another project's directory would be written on a binding that
/// never reached it. The steward of `ovzdusie` approves a bundle headed by their own Pipeline and
/// carrying a Pipeline of `doprava`: refused, the refusal names the file and the project it
/// belongs to, and the forge is asked to merge nothing.
#[tokio::test]
async fn a_bundle_carrying_another_projects_manifest_is_not_approved_from_this_one() {
    let foreign = format!("projects/{THEIRS}/pipelines/bikes/pipeline.yaml");
    let gitea = forge_with(&[
        (
            "projects/ovzdusie/pipelines/aq/pipeline.yaml",
            pipeline_yaml(MINE, "aq"),
        ),
        (foreign.as_str(), pipeline_yaml(THEIRS, "bikes")),
    ])
    .await;
    let state = state_with(&gitea);
    let answer = approve(&state).await;
    assert_eq!(
        answer.status,
        StatusCode::FORBIDDEN,
        "a steward of {MINE} approved a manifest of {THEIRS}: {}",
        answer.text
    );
    assert!(
        answer.text.contains(THEIRS) && answer.text.contains("Pipeline"),
        "the refusal has to name the kind and the project whose grants were asked for: {}",
        answer.text
    );
    let _ = &foreign;
    assert!(
        mutated(&gitea).await.is_empty(),
        "the refusal came after the forge was asked to write: {:?}",
        mutated(&gitea).await
    );
}

/// The same bundle inside one project is approved and merged, so the case above refuses the
/// foreign project and not bundles, and this one fails if the check is written too wide.
#[tokio::test]
async fn a_bundle_inside_the_project_is_approved_and_merged() {
    let gitea = forge_with(&[
        (
            "projects/ovzdusie/pipelines/aq/pipeline.yaml",
            pipeline_yaml(MINE, "aq"),
        ),
        (
            "projects/ovzdusie/pipelines/no2/pipeline.yaml",
            pipeline_yaml(MINE, "no2"),
        ),
    ])
    .await;
    let state = state_with(&gitea);
    let answer = approve(&state).await;
    assert_eq!(
        answer.status,
        StatusCode::ACCEPTED,
        "the steward's own project: {}",
        answer.text
    );
    assert!(
        mutated(&gitea)
            .await
            .iter()
            .any(|call| call.ends_with("/pulls/12/merge")),
        "nothing was merged: {:?}",
        mutated(&gitea).await
    );
}

/// A file of the organization is not a file of another project: `users/assignments/` is where a
/// RoleBinding lives whatever project proposes it, and the rights that decide it are its own
/// (PF-52, `within_own_rights`), not this refusal. The bundle merges.
#[tokio::test]
async fn an_organization_file_still_travels_with_a_projects_change() {
    let binding = "apiVersion: joinedcontext.com/v1alpha1\nkind: RoleBinding\nmetadata:\n  \
                   name: steward-of-ovzdusie\n  namespace: org\nspec:\n  subjects:\n  - user: \
                   steward@hel.fi\n  role: pipelines-of-ovzdusie\n  scope:\n    project: \
                   ovzdusie\n";
    let gitea = forge_with(&[
        (
            "projects/ovzdusie/pipelines/aq/pipeline.yaml",
            pipeline_yaml(MINE, "aq"),
        ),
        (
            "users/assignments/steward-of-ovzdusie.yaml",
            binding.to_owned(),
        ),
    ])
    .await;
    let state = state_with(&gitea);
    let answer = approve(&state).await;
    assert_eq!(
        answer.status,
        StatusCode::ACCEPTED,
        "an organization file of the caller's own rights: {}",
        answer.text
    );
}

/// PF-59: a draft is keyed by its project, so the same kind and name in `doprava` is a different
/// draft and `ovzdusie`'s path reaches none of it — neither the manifest nor the fact that it is
/// there.
#[tokio::test]
async fn a_draft_of_another_project_is_not_reachable_through_this_ones_path() {
    let gitea = common::forge().await;
    let state = state_with(&gitea);
    let manifest = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Pipeline",
        "metadata": { "name": "bikes", "namespace": THEIRS },
        "spec": { "contextSpaceRef": "air" },
    });
    state
        .drafts
        .put(
            THEIRS,
            "Pipeline",
            "bikes",
            manifest,
            None,
            "steward@hel.fi",
            "person",
        )
        .await
        .expect("a draft of the other project");
    let answer = common::send(
        &state,
        common::person("steward"),
        "GET",
        &format!("/api/v1/projects/{MINE}/drafts/Pipeline/bikes"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);
    assert!(
        !answer.text.contains(THEIRS),
        "the 404 named the project the draft really belongs to: {}",
        answer.text
    );
}
