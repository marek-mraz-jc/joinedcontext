//! A manifest naming what an open change creates (MF-48, MF-13, PF-57; owner decision T-2235).
//!
//! What is worth a test: the check lets such a reference through as waiting and names the change,
//! the proposal records it in its merge request, a reference nothing creates is still refused, a
//! proposal sent after the awaited change was rejected is refused by the strict gate's own check,
//! and the approval refuses to merge before the awaited change, flagging a rejected one.

mod common;

use axum::http::StatusCode;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use common::{envelope, REPO};
use joinedcontext_portal::api::mutate::branch_name;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::change::Operation;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "ovzdusie";

/// The bootstrap group of `Config::for_tests`: past every permission check, so what is judged
/// here is the reference alone.
fn steward() -> Identity {
    Identity {
        client: None,
        subject: "sub-steward".into(),
        username: "steward".into(),
        email: Some("steward@hel.fi".into()),
        name: Some("Steward".into()),
        roles: vec!["portal-approver".into()],
        groups: Vec::new(),
        second_factor: false,
    }
}

/// An endpoint naming the context space `air`, which the project does not hold.
fn endpoint() -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Endpoint",
        "metadata": { "name": "public-air", "namespace": PROJECT },
        "spec": {
            "contextSpaceRef": "air",
            "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa",
            "audience": "public",
            "enabledRepresentations": ["ngsi-ld"]
        }
    })
}

fn pull(number: u64, branch: &str, state: &str, merged: bool, body: &str) -> Value {
    json!({
        "number": number,
        "html_url": format!("https://gitea.example/pulls/{number}"),
        "state": state,
        "title": format!("pull {number}"),
        "body": body,
        "head": { "ref": branch },
        "base": { "ref": "main" },
        "created_at": "2026-09-25T09:00:00Z",
        "user": { "login": "author", "full_name": "Author", "email": "author@hel.fi" },
        "mergeable": true,
        "merged": merged
    })
}

/// A forge with change #7 open, proposing the context space `air`.
async fn forge_creating_air() -> (MockServer, AppState) {
    let gitea = common::forge().await;
    let branch = branch_name(PROJECT, "ContextSpace", "air", Operation::Create);
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .and(query_param("state", "open"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([pull(7, &branch, "open", false, "")])),
        )
        .mount(&gitea)
        .await;
    let state = common::state_on(&gitea);
    (gitea, state)
}

async fn check(state: &AppState, manifest: Value) -> Value {
    let answer = common::send(
        state,
        steward(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/endpoints?dryRun=All"),
        Some(manifest),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    serde_json::from_str(&answer.text).expect("a dry-run result")
}

/// MF-48: the reference passes as waiting, the verdict names the change and says why, and the
/// proposal records the change in its merge request.
#[tokio::test]
async fn a_reference_to_a_resource_in_an_open_change_is_allowed_and_says_why() {
    let (gitea, state) = forge_creating_air().await;
    let checked = check(&state, endpoint()).await;
    assert_eq!(checked["valid"], true, "{checked}");
    let verdict = &checked["verdict"];
    assert_eq!(verdict["ok"], true, "{verdict}");
    assert_eq!(verdict["waitsOn"], json!(["chg-00000007"]), "{verdict}");
    assert_eq!(verdict["findings"][0]["level"], "warning", "{verdict}");
    assert_eq!(
        verdict["findings"][0]["path"], "spec.contextSpaceRef",
        "{verdict}"
    );
    assert_eq!(
        verdict["findings"][0]["message"],
        "ContextSpace 'air' resolves once chg-00000007 is approved"
    );

    let proposed = common::send(
        &state,
        steward(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
        Some(endpoint()),
    )
    .await;
    assert_eq!(proposed.status, StatusCode::ACCEPTED, "{}", proposed.text);
    let requests = gitea.received_requests().await.expect("recorded");
    let opened = requests
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path() == format!("{REPO}/pulls"))
        .expect("a merge request was opened");
    let body: Value = serde_json::from_slice(&opened.body).expect("json");
    assert!(
        body["body"]
            .as_str()
            .unwrap_or_default()
            .contains("Waits-On: #7 (chg-00000007)"),
        "{body}"
    );
}

/// MF-13: a reference neither the project nor an open change holds is still a red verdict
/// naming the field.
#[tokio::test]
async fn a_reference_nothing_creates_is_refused_naming_the_field() {
    let (_gitea, state) = forge_creating_air().await;
    let mut other = endpoint();
    other["spec"]["contextSpaceRef"] = json!("water");
    let checked = check(&state, other).await;
    assert_eq!(checked["valid"], false, "{checked}");
    assert_eq!(checked["verdict"]["ok"], false);
    let message = checked["verdict"]["findings"][0]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(message.contains("spec.contextSpaceRef"), "{message}");
    assert!(message.contains("'water'"), "{message}");
    assert!(checked["verdict"].get("waitsOn").is_none());
}

/// PF-57, MF-48: the check passed while change #7 was open; once it is rejected, the proposal of
/// the same manifest is refused like any reference to nothing, and nothing reaches the forge.
#[tokio::test]
async fn the_strict_gate_refuses_a_proposal_whose_awaited_change_was_rejected() {
    let (gitea, state) = forge_creating_air().await;
    assert_eq!(check(&state, endpoint()).await["verdict"]["ok"], true);

    gitea.reset().await;
    common::mount_repository(&gitea, REPO).await;
    let proposed = common::send(
        &state,
        steward(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/endpoints"),
        Some(endpoint()),
    )
    .await;
    assert_eq!(
        proposed.status,
        StatusCode::BAD_REQUEST,
        "{}",
        proposed.text
    );
    assert!(
        proposed.text.contains("spec.contextSpaceRef"),
        "{}",
        proposed.text
    );
    let requests = gitea.received_requests().await.expect("recorded");
    assert!(
        !requests.iter().any(|r| r.method.as_str() == "POST"),
        "nothing was written"
    );
}

/// Change #3 proposes the space `air` and waits on #7, the model it names, which is in
/// `awaited_state`.
async fn waiting_change(awaited_state: &str, merged: bool) -> (MockServer, AppState) {
    let gitea = common::forge().await;
    let branch = branch_name(PROJECT, "ContextSpace", "air", Operation::Create);
    let body = "Proposed create of ContextSpace `air`.\n\nWaits-On: #7 (chg-00000007)";
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/3")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(pull(3, &branch, "open", false, body)),
        )
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls")))
        .and(query_param("state", "open"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([pull(3, &branch, "open", false, body)])),
        )
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/7")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull(
            7,
            "portal/create-datamodel-air-00000000",
            awaited_state,
            merged,
            "",
        )))
        .mount(&gitea)
        .await;
    let space = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "air", "namespace": PROJECT },
        "spec": { "dataModelRef": "air" }
    });
    let manifest = serde_norway::to_string(&space).expect("yaml");
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/contents/projects/{PROJECT}/spaces/air/space.yaml"
        )))
        .and(query_param("ref", branch.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "blob-1",
            "content": STANDARD.encode(manifest.as_bytes())
        })))
        .mount(&gitea)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/pulls/3/files")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "filename": format!("projects/{PROJECT}/spaces/air/space.yaml"),
            "status": "added"
        }])))
        .mount(&gitea)
        .await;
    let state = common::state_on(&gitea);
    // The mirror holds the model once #7 merged, and not while it is open or rejected.
    if merged {
        state
            .mirror
            .upsert(envelope("DataModel", "air", PROJECT, json!({})));
    }
    (gitea, state)
}

async fn approve(state: &AppState) -> common::Answer {
    let approver = Identity {
        client: None,
        subject: "sub-approver".into(),
        username: "approver".into(),
        email: Some("approver@hel.fi".into()),
        name: Some("Approver".into()),
        roles: vec!["portal-approver".into()],
        groups: Vec::new(),
        second_factor: false,
    };
    common::send(
        state,
        approver,
        "POST",
        &format!("/api/v1/projects/{PROJECT}/changes/chg-00000003/approve"),
        Some(json!({ "confirm": "air" })),
    )
    .await
}

async fn merged(gitea: &MockServer) -> bool {
    gitea
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .any(|r| r.url.path() == format!("{REPO}/pulls/3/merge"))
}

async fn detail(state: &AppState) -> Value {
    let answer = common::send(
        state,
        steward(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/changes/chg-00000003"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    serde_json::from_str(&answer.text).expect("a change")
}

/// MF-48: while #7 is open, #3 lists it as pending and its approval is refused naming #7.
#[tokio::test]
async fn a_change_that_waits_on_an_open_change_cannot_merge_before_it() {
    let (gitea, state) = waiting_change("open", false).await;
    assert_eq!(
        detail(&state).await["waitsOn"],
        json!([{ "name": "chg-00000007", "phase": "PendingApproval" }])
    );
    let answer = approve(&state).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer.text.contains("approve chg-00000007 first"),
        "{}",
        answer.text
    );
    assert!(!merged(&gitea).await);

    let listed = common::send(
        &state,
        steward(),
        "GET",
        &format!("/api/v1/projects/{PROJECT}/changes"),
        None,
    )
    .await;
    let listed: Value = serde_json::from_str(&listed.text).expect("a list");
    assert_eq!(listed["items"][0]["waitsOn"][0]["name"], "chg-00000007");
}

/// MF-48: #7 rejected flags #3, whose approval stays refused and says so.
#[tokio::test]
async fn a_change_whose_awaited_change_was_rejected_is_flagged_and_not_approved() {
    let (gitea, state) = waiting_change("closed", false).await;
    assert_eq!(detail(&state).await["waitsOn"][0]["phase"], "Rejected");
    let answer = approve(&state).await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("was rejected"), "{}", answer.text);
    assert!(!merged(&gitea).await);
}

/// MF-48: once #7 merged, #3 is approved like any other change.
#[tokio::test]
async fn a_change_whose_awaited_change_merged_is_approved() {
    let (gitea, state) = waiting_change("closed", true).await;
    assert_eq!(detail(&state).await["waitsOn"][0]["phase"], "Merged");
    let answer = approve(&state).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    assert!(merged(&gitea).await);
}
