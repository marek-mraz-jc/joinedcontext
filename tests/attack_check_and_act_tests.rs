//! Attack vector T-1683 (PF-57): the check is made on one value and the action taken on another.
//!
//! The step this file exists for is the third one: **approve a Change whose branch moved after
//! review**. The approval used to read every file of the merge request at the branch *name* and
//! then ask the forge to merge that branch *name* — two reads of a moving target. A push landing
//! between them merged content nobody checked, past every per-file rule `approve_every_file`
//! walks. Now one commit decides both: the review reads the commit the forge reported and the
//! merge is pinned to it, so the forge itself refuses a branch that has moved.
//!
//! What the existing suites already play, so this file does not repeat it:
//! `mutate::tests::propose_validation_rejects_foreign_namespace_and_fills_absent` and
//! `::propose_rejects_name_mismatch_on_replace` (a body whose `metadata.name`/`namespace` differs
//! from the path — the path wins or the request is refused),
//! `edge_verdict_gate_tests::only_a_green_verdict_of_this_exact_manifest_in_this_project_opens_the_gate`
//! and `strict_gate_tests::a_changed_or_refused_manifest_is_not_proposed_on_an_earlier_check` (a
//! Verdict binds the digest of the body it checked, so a draft swapped after its Verdict is
//! refused), `ops::verdict::tests::digest_is_order_independent` (the digest itself).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "ovzdusie";
const BRANCH: &str = "portal/update-pipeline-aq-1683aaaa";
/// The commit the approver's page was rendered from and the review reads.
const REVIEWED: &str = "1683aaaabbbbccccddddeeeeffff000011112222";
const MANIFEST_PATH: &str = "projects/ovzdusie/pipelines/aq/pipeline.yaml";

fn pipeline_yaml(body: &str) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: aq\n  \
         namespace: {PROJECT}\nspec:\n  contextSpaceRef: air\n  source:\n    kind: http\n{body}"
    )
}

/// A forge holding merge request 13 on `BRANCH` at commit `REVIEWED`. The manifest is mounted at
/// the commit and **not** at the branch name, so a review that still read the branch would find
/// nothing — which is how this fixture proves which ref the review uses.
///
/// `merge_status` is what the forge answers the merge with: 200 for a branch that has not moved,
/// 409 for Gitea's refusal of a `head_commit_id` that is no longer the head.
async fn forge_at(merge_status: u16) -> MockServer {
    let gitea = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(common::REPO))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "main" })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/pulls/13", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "number": 13,
            "html_url": "https://gitea.example/pulls/13",
            "state": "open",
            "title": "update Pipeline aq",
            "head": { "ref": BRANCH, "sha": REVIEWED },
            "base": { "ref": "main" },
            "created_at": "2026-09-18T09:14:22Z",
            "user": { "login": "someone.else", "full_name": "Someone Else", "email": "else@hel.fi" },
            "mergeable": true,
            "merged": false
        })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/pulls/13/files", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "filename": MANIFEST_PATH, "status": "modified" }
        ])))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/contents/{MANIFEST_PATH}", common::REPO)))
        .and(query_param("ref", REVIEWED))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-reviewed",
            "content": common::encode(&pipeline_yaml("  rate: 60\n")),
        })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/contents/{MANIFEST_PATH}", common::REPO)))
        .and(query_param("ref", "main"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-base",
            "content": common::encode(&pipeline_yaml("  rate: 30\n")),
        })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/commits", common::REPO)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&gitea)
        .await;
    let body = if merge_status == 409 {
        json!({ "message": "head_commit_id does not match the head of the branch" })
    } else {
        json!({})
    };
    Mock::given(method("POST"))
        .and(path(format!("{}/pulls/13/merge", common::REPO)))
        .respond_with(ResponseTemplate::new(merge_status).set_body_json(body))
        .mount(&gitea)
        .await;
    gitea
}

/// `steward@hel.fi` may approve Pipelines in `ovzdusie`.
fn state_with(gitea: &MockServer) -> AppState {
    let state = common::state_on(gitea);
    state.mirror.upsert(common::envelope(
        "Role",
        "approves-pipelines",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(common::envelope(
        "RoleBinding",
        "steward-of-ovzdusie",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "steward@hel.fi" }],
            "role": "approves-pipelines",
            "scope": { "project": PROJECT },
        }),
    ));
    state
}

async fn approve(state: &AppState) -> common::Answer {
    common::send(
        state,
        common::person("steward"),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/changes/chg-0000000d/approve"),
        None,
    )
    .await
}

/// The body of the merge the forge received, if it received one.
async fn merge_payload(gitea: &MockServer) -> Option<Value> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|request| {
            request.method.to_string().eq_ignore_ascii_case("post")
                && request.url.path().ends_with("/pulls/13/merge")
        })
        .and_then(|request| serde_json::from_slice(&request.body).ok())
}

/// PF-57, T-1683: the approval merges the commit it reviewed, by name. The forge is told which
/// commit, so a push that lands between the review and the merge is the forge's refusal and not
/// a race this Portal can lose.
#[tokio::test]
async fn the_approval_pins_the_merge_to_the_commit_it_reviewed() {
    let gitea = forge_at(200).await;
    let state = state_with(&gitea);
    let answer = approve(&state).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let payload = merge_payload(&gitea)
        .await
        .expect("the merge was asked for");
    assert_eq!(
        payload.get("head_commit_id").and_then(Value::as_str),
        Some(REVIEWED),
        "the merge was not pinned to the reviewed commit: {payload}"
    );
}

/// PF-57, T-1683: the review reads the commit and not the branch. Every file of this fixture is
/// mounted at the commit alone, so an approval that still read `BRANCH` would find no manifest
/// and answer 404 — this case is green only while the two are the same read.
#[tokio::test]
async fn every_file_of_the_review_is_read_at_that_same_commit() {
    let gitea = forge_at(200).await;
    let state = state_with(&gitea);
    let answer = common::send(
        &state,
        common::person("steward"),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/changes/chg-0000000d"),
        None,
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::OK,
        "the change was read at the branch, not at the commit: {}",
        answer.text
    );
    assert!(
        answer.text.contains("spec.rate"),
        "the diff of the reviewed commit is what the page shows: {}",
        answer.text
    );
}

/// T-1683, CC-80: the forge refuses to merge a commit that is no longer the head, and the person
/// at the button is told what happened and what to do — not handed the forge's own JSON.
#[tokio::test]
async fn a_branch_that_moved_after_the_review_is_refused_in_words_a_person_can_act_on() {
    let gitea = forge_at(409).await;
    let state = state_with(&gitea);
    let answer = approve(&state).await;
    assert_eq!(
        answer.status,
        StatusCode::CONFLICT,
        "a branch that moved was merged: {}",
        answer.text
    );
    for phrase in ["reviewed", "Open the change again", "chg-0000000d"] {
        assert!(
            answer.text.contains(phrase),
            "the refusal has to say '{phrase}': {}",
            answer.text
        );
    }
    assert!(
        !answer.text.contains("head_commit_id"),
        "the forge's own words reached the person: {}",
        answer.text
    );
}

/// The head the forge's update gives the branch once `main` is merged into it.
const UPDATED: &str = "1683ddddeeeeffff000011112222aaaabbbbcccc";

/// `forge_at(200)` whose branch another merge overtook: the first merge is refused as behind
/// its base (PF-105), the update answers `update_status`, and the manifest's blob at the updated
/// head is `blob-reviewed` when `main` left it alone.
async fn behind_forge(update_status: u16, manifest_after: &str) -> MockServer {
    let gitea = forge_at(200).await;
    Mock::given(method("POST"))
        .and(path(format!("{}/pulls/13/merge", common::REPO)))
        .respond_with(
            ResponseTemplate::new(405)
                .set_body_json(json!({ "message": "The head branch is behind the base branch" })),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&gitea)
        .await;
    let update = if update_status == 409 {
        json!({ "message": "merge failed because of conflict" })
    } else {
        json!({})
    };
    Mock::given(method("POST"))
        .and(path(format!("{}/pulls/13/update", common::REPO)))
        .and(query_param("style", "merge"))
        .respond_with(ResponseTemplate::new(update_status).set_body_json(update))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{}/branches/{BRANCH}", common::REPO)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": UPDATED } })),
        )
        .mount(&gitea)
        .await;
    for (commit, blob) in [(REVIEWED, "blob-reviewed"), (UPDATED, manifest_after)] {
        Mock::given(method("GET"))
            .and(path(format!("{}/git/trees/{commit}", common::REPO)))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "truncated": false,
                "tree": [
                    { "path": MANIFEST_PATH, "type": "blob", "sha": blob },
                    { "path": "README.md", "type": "blob", "sha": format!("readme-{commit}") }
                ]
            })))
            .mount(&gitea)
            .await;
    }
    gitea
}

async fn merges_asked(gitea: &MockServer) -> Vec<Value> {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|request| {
            request.method.to_string().eq_ignore_ascii_case("post")
                && request.url.path().ends_with("/pulls/13/merge")
        })
        .filter_map(|request| serde_json::from_slice(&request.body).ok())
        .collect()
}

/// PF-105, PF-57: a Change another merge overtook is brought up to date and merged at the
/// updated head, because every file it changes reads the same there as at the reviewed commit.
/// A file of `main` it does not touch (`README.md`) moving is no reason to send it back.
#[tokio::test]
async fn a_branch_that_fell_behind_is_updated_and_merged_when_its_own_files_are_unchanged() {
    let gitea = behind_forge(200, "blob-reviewed").await;
    let state = state_with(&gitea);
    let answer = approve(&state).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let merges = merges_asked(&gitea).await;
    assert_eq!(
        merges.len(),
        2,
        "one refused merge, one after the update: {merges:?}"
    );
    assert_eq!(
        merges[1].get("head_commit_id").and_then(Value::as_str),
        Some(UPDATED),
        "the second merge was not pinned to the updated head: {:?}",
        merges[1]
    );
}

/// PF-105, PF-57: when `main` changed a file of the Change meanwhile, the approval no longer
/// covers what would merge. Nothing merges, and the reviewer is told which file and what to do.
#[tokio::test]
async fn a_branch_whose_own_file_main_changed_goes_back_to_its_reviewer() {
    let gitea = behind_forge(200, "blob-main-changed-it").await;
    let state = state_with(&gitea);
    let answer = approve(&state).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    for phrase in [MANIFEST_PATH, "Open the change again", "chg-0000000d"] {
        assert!(
            answer.text.contains(phrase),
            "the refusal has to say '{phrase}': {}",
            answer.text
        );
    }
    assert_eq!(
        merges_asked(&gitea).await.len(),
        1,
        "a merge followed the refusal"
    );
}

/// PF-105, CC-80: `main` that does not merge into the branch is the same answer as a branch
/// that moved, in words, never the forge's own.
#[tokio::test]
async fn a_branch_main_does_not_merge_into_is_refused_in_words_a_person_can_act_on() {
    let gitea = behind_forge(409, "blob-reviewed").await;
    let state = state_with(&gitea);
    let answer = approve(&state).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer.text.contains("Open the change again"),
        "{}",
        answer.text
    );
    assert!(
        !answer.text.contains("merge failed"),
        "the forge's words reached the person: {}",
        answer.text
    );
    assert_eq!(
        merges_asked(&gitea).await.len(),
        1,
        "a merge followed the refusal"
    );
}
