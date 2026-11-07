//! Workspaces over REST and the registry (API/01 §22, T-1236, T-1237): open, list, read,
//! compare, update from main, bring back and discard, each by the person the contract allows.

mod common;

use axum::http::StatusCode;
use chrono::Utc;
use common::{encode, envelope, forge, person, send, state_on, REPO};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::workspaces::{Opening, Scope};
use joinedcontext_portal::ops::{self, Caller, OpError, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "ovzdusie";
const AIR: &str = "projects/ovzdusie/spaces/air/space.yaml";
const WS: &str = "/api/v1/projects/ovzdusie/workspaces";

fn steward() -> Identity {
    Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("jana")
    }
}

/// Reads the project and proposes nothing.
fn viewer() -> Identity {
    Identity {
        client: None,
        groups: vec!["readers".into()],
        ..person("vera")
    }
}

fn space(locale: &str, ttl: u32) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: air\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n  defaultLocale: {locale}\n  ttlDays: {ttl}\n"
    )
}

async fn tree(server: &MockServer, git_ref: &str, entries: &[(&str, &str)]) {
    let tree: Vec<_> = entries
        .iter()
        .map(|(p, sha)| json!({ "path": p, "type": "blob", "sha": sha }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/{git_ref}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tree": tree, "truncated": false })),
        )
        .mount(server)
        .await;
}

/// The head commit of a branch, as `GET /branches/{branch}` answers it.
async fn branch(server: &MockServer, name: &str, head: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": head } })))
        .mount(server)
        .await;
}

async fn file(server: &MockServer, git_ref: &str, content: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{AIR}")))
        .and(query_param("ref", git_ref))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "path": AIR, "sha": "x", "encoding": "base64", "content": encode(content)
        })))
        .mount(server)
        .await;
}

/// The forge, the mirror with `air` at base, and the viewer's read-only binding.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/main")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": "base1" } })),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{REPO}/branches/workspace/air-v2")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let state = state_on(&server);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        PROJECT,
        json!({ "isSandbox": true, "defaultLocale": "en", "ttlDays": 10 }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "readers",
        ORG_NAMESPACE,
        json!({ "role": "reader", "subjects": [{ "group": "readers" }], "scope": { "project": PROJECT } }),
    ));
    (server, state)
}

/// The workspace `air-v2`, owned by the steward, opened at `base1`.
async fn opened(state: &AppState) {
    state
        .workspaces
        .create(Opening {
            name: "air-v2",
            title: Some("Air"),
            project: PROJECT,
            owner: "jana@hel.fi",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 24,
        })
        .await
        .expect("opened");
}

/// The workspace changed `air`'s ttlDays to 20; main is `main_air`.
async fn changed(server: &MockServer, main_air: &str, main_sha: &str) {
    // The copy's branch is read as real Gitea serves a slashed ref: the head commit from
    // `branches/{branch}`, then the tree by that sha (T-2266).
    branch(server, "workspace/air-v2", "ws1").await;
    tree(server, "ws1", &[(AIR, "s-ours")]).await;
    tree(server, "base1", &[(AIR, "s-base")]).await;
    tree(server, "main", &[(AIR, main_sha)]).await;
    file(server, "workspace/air-v2", &space("en", 12)).await;
    file(server, "base1", &space("en", 10)).await;
    file(server, "main", main_air).await;
}

async fn calls(server: &MockServer, verb: &str, suffix: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == verb && r.url.path().ends_with(suffix))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

#[tokio::test]
async fn every_workspace_operation_is_in_the_registry() {
    for name in [
        "jc_workspace_open",
        "jc_workspace_list",
        "jc_workspace_get",
        "jc_workspace_compare",
        "jc_workspace_update_from_main",
        "jc_workspace_propose",
        "jc_workspace_discard",
    ] {
        assert!(ops::find(name).is_some(), "{name}");
    }
}

#[tokio::test]
async fn opening_records_the_workspace_and_cuts_its_branch_from_main() {
    let (server, state) = world().await;
    let answer = send(
        &state,
        steward(),
        "POST",
        WS,
        Some(json!({ "name": "air-v2", "title": "Air cleanup" })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.text);
    let view = body(&answer.text);
    assert_eq!(view["branch"], "workspace/air-v2");
    assert_eq!(view["baseRevision"], "base1");
    assert_eq!(view["owner"], "jana@hel.fi");
    assert_eq!(view["scope"], json!({ "kind": "project" }));
    let branches = calls(&server, "POST", "/branches").await;
    assert_eq!(branches[0]["new_branch_name"], "workspace/air-v2");
    let listed = send(&state, viewer(), "GET", WS, None).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(body(&listed.text)["items"][0]["name"], "air-v2");
}

#[tokio::test]
async fn opening_refuses_a_bad_name_title_or_ttl_and_a_second_of_the_same_name() {
    let (_server, state) = world().await;
    for bad in [
        json!({ "name": "" }),
        json!({ "name": "čistenie" }),
        json!({ "name": "a-name-far-longer-than-twenty" }),
        json!({ "name": "ok", "title": "two\nlines" }),
        json!({ "name": "ok", "ttlDays": 15 }),
        json!({ "name": "ok", "ttlDays": 0 }),
        json!({ "name": "ok", "scope": { "kind": "resources", "items": [] } }),
    ] {
        let answer = send(&state, steward(), "POST", WS, Some(bad.clone())).await;
        assert_eq!(
            answer.status,
            StatusCode::BAD_REQUEST,
            "{bad}: {}",
            answer.text
        );
    }
    opened(&state).await;
    let again = send(
        &state,
        steward(),
        "POST",
        WS,
        Some(json!({ "name": "air-v2" })),
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.text);
}

#[tokio::test]
async fn a_secret_typed_into_the_opening_is_refused_and_never_echoed() {
    let (_server, state) = world().await;
    let answer = send(
        &state,
        steward(),
        "POST",
        WS,
        Some(json!({ "name": "ok", "password": "hunter2" })),
    )
    .await;
    assert!(answer.status.is_client_error(), "{}", answer.text);
    assert!(!answer.text.contains("hunter2"), "{}", answer.text);
    let op = ops::find("jc_workspace_open").unwrap();
    assert!(matches!(
        (op.validate)(&json!({ "name": "ok", "token": "hunter2" })),
        Err(OpError::InvalidInput { .. })
    ));
}

#[tokio::test]
async fn a_viewer_may_read_but_not_open_and_a_stranger_sees_nothing() {
    let (_server, state) = world().await;
    let answer = send(
        &state,
        viewer(),
        "POST",
        WS,
        Some(json!({ "name": "mine" })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(answer.text.contains("propose"), "{}", answer.text);
    let stranger = send(&state, person("nobody"), "GET", WS, None).await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND);
    let unknown = send(&state, viewer(), "GET", &format!("{WS}/nope"), None).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn compare_lists_the_changed_file_with_its_fields_and_no_conflict_while_main_stood_still() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 10), "s-base").await;
    let answer = send(
        &state,
        viewer(),
        "GET",
        &format!("{WS}/air-v2/compare"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let cmp = body(&answer.text);
    assert_eq!(cmp["files"][0]["path"], AIR);
    assert_eq!(cmp["files"][0]["operation"], "Update");
    assert_eq!(cmp["files"][0]["kind"], "ContextSpace");
    assert!(cmp["files"][0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == "spec.ttlDays" && f["to"] == 12));
    assert_eq!(cmp["conflicts"], json!([]));
    let read = send(&state, viewer(), "GET", &format!("{WS}/air-v2"), None).await;
    assert_eq!(body(&read.text)["changes"], 1);
}

#[tokio::test]
async fn compare_names_the_field_both_sides_changed_and_merges_the_rest() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 13), "s-main").await;
    let cmp = body(
        &send(
            &state,
            steward(),
            "GET",
            &format!("{WS}/air-v2/compare"),
            None,
        )
        .await
        .text,
    );
    assert_eq!(
        cmp["conflicts"],
        json!([{ "path": AIR, "fields": [{ "path": "spec.ttlDays", "ours": 12, "theirs": 13, "base": 10 }] }])
    );

    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("fi", 10), "s-main").await;
    let cmp = body(
        &send(
            &state,
            steward(),
            "GET",
            &format!("{WS}/air-v2/compare"),
            None,
        )
        .await
        .text,
    );
    assert_eq!(
        cmp["conflicts"],
        json!([{ "path": AIR, "fields": [] }]),
        "main changed another field: nothing to choose, but an update is owed"
    );
}

#[tokio::test]
async fn update_from_main_refuses_an_unanswered_conflict_and_merges_an_answered_one() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("fi", 13), "s-main").await;
    let uri = format!("{WS}/air-v2/update");
    let refused = send(&state, steward(), "POST", &uri, Some(json!({}))).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert!(refused.text.contains("spec.ttlDays"), "{}", refused.text);
    assert!(
        calls(&server, "POST", "/contents").await.is_empty(),
        "nothing written"
    );

    let resolved = send(
        &state,
        steward(),
        "POST",
        &uri,
        Some(json!({ "resolutions": [{ "path": AIR, "field": "spec.ttlDays", "keep": "ours" }] })),
    )
    .await;
    assert_eq!(resolved.status, StatusCode::OK, "{}", resolved.text);
    assert_eq!(body(&resolved.text)["merged"], json!([AIR]));
    let commit = &calls(&server, "POST", "/contents").await[0];
    assert_eq!(commit["branch"], "workspace/air-v2");
    let merged = String::from_utf8(
        base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            commit["files"][0]["content"].as_str().unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(merged.contains("ttlDays: 12"), "ours kept: {merged}");
    assert!(
        merged.contains("defaultLocale: fi"),
        "main's other change taken: {merged}"
    );
    let now = state.workspaces.get("air-v2").await.unwrap().unwrap();
    assert_eq!(
        now.base_revision, "base1",
        "main's head, as the forge answers it"
    );
}

#[tokio::test]
async fn only_the_owner_updates_brings_back_or_discards() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 10), "s-base").await;
    let other = Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("petra")
    };
    for (verb, uri) in [
        ("POST", format!("{WS}/air-v2/update")),
        ("POST", format!("{WS}/air-v2/propose")),
        ("DELETE", format!("{WS}/air-v2")),
    ] {
        let answer = send(&state, other.clone(), verb, &uri, Some(json!({}))).await;
        assert_eq!(
            answer.status,
            StatusCode::FORBIDDEN,
            "{uri}: {}",
            answer.text
        );
        assert!(answer.text.contains("jana@hel.fi"), "{}", answer.text);
    }
    assert!(calls(&server, "POST", "/pulls").await.is_empty());
    assert!(state.workspaces.get("air-v2").await.unwrap().is_some());
}

#[tokio::test]
async fn bringing_back_opens_one_change_in_the_lane_of_its_files() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 10), "s-base").await;
    let answer = send(
        &state,
        steward(),
        "POST",
        &format!("{WS}/air-v2/propose"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let change = body(&answer.text);
    assert_eq!(
        change["status"]["lane"], "green",
        "a sandbox space is the whole change"
    );
    let pulls = calls(&server, "POST", "/pulls").await;
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0]["head"], "workspace/air-v2");
    assert!(pulls[0]["body"].as_str().unwrap().contains("jana@hel.fi"));
}

#[tokio::test]
async fn bringing_back_is_refused_with_a_conflict_or_nothing_changed() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 13), "s-main").await;
    let answer = send(
        &state,
        steward(),
        "POST",
        &format!("{WS}/air-v2/propose"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);

    let (server, state) = world().await;
    opened(&state).await;
    branch(&server, "workspace/air-v2", "ws1").await;
    tree(&server, "ws1", &[(AIR, "s-base")]).await;
    tree(&server, "base1", &[(AIR, "s-base")]).await;
    tree(&server, "main", &[(AIR, "s-base")]).await;
    let answer = send(
        &state,
        steward(),
        "POST",
        &format!("{WS}/air-v2/propose"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(answer.text.contains("changes nothing"), "{}", answer.text);
    assert!(calls(&server, "POST", "/pulls").await.is_empty());
}

#[tokio::test]
async fn an_agent_or_an_mcp_client_never_brings_a_workspace_back() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 10), "s-base").await;
    let op = ops::find("jc_workspace_propose").unwrap();
    for via in [Via::Mcp, Via::Agent] {
        let caller = Caller::new(steward(), via);
        let err = ops::call(op, &caller, &state, PROJECT, json!({ "name": "air-v2" }))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("AG-82"), "{err:?}");
    }
    assert!(calls(&server, "POST", "/pulls").await.is_empty());
    // It may still compare, which is what it presents to the person (AG-82).
    let compare = ops::find("jc_workspace_compare").unwrap();
    let cmp = ops::call(
        compare,
        &Caller::new(steward(), Via::Mcp),
        &state,
        PROJECT,
        json!({ "name": "air-v2" }),
    )
    .await
    .expect("compared");
    assert_eq!(cmp["files"].as_array().unwrap().len(), 1);
}

/// AG-82, T-1259: the other way out of a workspace is a person's too. A discard deletes the
/// branch and every edit on it, so an agent that could call it could throw away work nobody
/// reviewed — and it is not even offered the tool, because `may_run` refuses it before the
/// registry lists it.
#[tokio::test]
async fn an_agent_or_an_mcp_client_never_throws_a_workspace_away() {
    let (server, state) = world().await;
    opened(&state).await;
    let op = ops::find("jc_workspace_discard").unwrap();
    for via in [Via::Mcp, Via::Agent] {
        let caller = Caller::new(steward(), via);
        let refused = caller.may_run(op).unwrap_err();
        assert!(
            format!("{refused:?}").contains("AG-82"),
            "the tool is offered to an agent: {refused:?}"
        );
        let err = ops::call(op, &caller, &state, PROJECT, json!({ "name": "air-v2" }))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("AG-82"), "{err:?}");
    }
    assert!(
        calls(&server, "DELETE", "/branches/workspace/air-v2")
            .await
            .is_empty(),
        "the branch was deleted anyway"
    );
    assert!(
        state.workspaces.get("air-v2").await.unwrap().is_some(),
        "the record went with it"
    );

    // A person still discards it, which is what makes the refusal a rule about who and not about
    // whether (CC-79).
    let answer = send(&state, steward(), "DELETE", &format!("{WS}/air-v2"), None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
}

/// AG-82: neither way out of a workspace is offered to an agent at all — the listing an agent
/// reads is what it believes it can do, and a tool it is told about and then refused teaches it
/// that the platform is unreliable rather than that the decision is a person's.
#[tokio::test]
async fn the_listing_an_agent_reads_offers_the_work_and_neither_way_out() {
    let (_server, state) = world().await;
    let offered: Vec<String> = ops::listing(&Caller::new(steward(), Via::Agent), &state, PROJECT)
        .into_iter()
        .map(|op| op.name)
        .collect();
    for refused in ["jc_workspace_propose", "jc_workspace_discard"] {
        assert!(
            !offered.contains(&refused.to_owned()),
            "{refused} is offered to an agent"
        );
    }
    for allowed in [
        "jc_workspace_open",
        "jc_workspace_compare",
        "jc_workspace_update_from_main",
        "jc_workspace_preview_start",
    ] {
        assert!(
            offered.contains(&allowed.to_owned()),
            "{allowed} is the work itself and is not offered"
        );
    }
}

#[tokio::test]
async fn discarding_removes_the_branch_and_the_record() {
    let (server, state) = world().await;
    opened(&state).await;
    let answer = send(&state, steward(), "DELETE", &format!("{WS}/air-v2"), None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
    assert_eq!(
        calls(&server, "DELETE", "/branches/workspace/air-v2")
            .await
            .len(),
        1
    );
    assert!(state.workspaces.get("air-v2").await.unwrap().is_none());
    let again = send(&state, steward(), "DELETE", &format!("{WS}/air-v2"), None).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_expired_workspace_is_neither_listed_nor_read() {
    let (_server, state) = world().await;
    state
        .workspaces
        .create(Opening {
            name: "old",
            title: None,
            project: PROJECT,
            owner: "jana@hel.fi",
            base_revision: "base1",
            scope: Scope::Project {},
            ttl_hours: 1,
        })
        .await
        .unwrap();
    let expired = state.workspaces.get("old").await.unwrap().unwrap();
    assert!(!expired.expired(Utc::now()));
    assert!(expired.expired(Utc::now() + chrono::Duration::hours(2)));
    let listed = body(&send(&state, steward(), "GET", WS, None).await.text);
    assert_eq!(listed["items"][0]["name"], "old", "live within its hour");
}

#[tokio::test]
async fn a_write_into_someone_elses_workspace_is_refused() {
    let (_server, state) = world().await;
    opened(&state).await;
    let other = Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("petra")
    };
    let manifest = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "ContextSpace",
        "metadata": { "name": "air", "namespace": PROJECT },
        "spec": { "isSandbox": true, "defaultLocale": "en", "ttlDays": 12 }
    });
    let uri = "/api/v1/projects/ovzdusie/spaces/air";
    send(
        &state,
        other.clone(),
        "PUT",
        &format!("{uri}?dryRun=All"),
        Some(manifest.clone()),
    )
    .await;
    let answer = send(
        &state,
        other,
        "PUT",
        &format!("{uri}?workspace=air-v2"),
        Some(manifest),
    )
    .await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(answer.text.contains("only its owner"), "{}", answer.text);
}

#[tokio::test]
async fn a_form_checks_and_writes_its_draft_into_the_owners_workspace() {
    // T-4868: every Portal form names the draft it holds beside the manifest; inside a copy the
    // API refused that with "a draft is proposed on its own", so no form could change a copy.
    let (server, state) = world().await;
    opened(&state).await;
    let mut proposal = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "ContextSpace",
        "metadata": { "name": "air", "namespace": PROJECT },
        "spec": { "isSandbox": true, "defaultLocale": "en", "ttlDays": 12 }
    });
    proposal["draft"] = json!({ "kind": "ContextSpace", "name": "air" });
    let uri = "/api/v1/projects/ovzdusie/spaces/air";
    let checked = send(
        &state,
        steward(),
        "PUT",
        &format!("{uri}?dryRun=All&workspace=air-v2"),
        Some(proposal.clone()),
    )
    .await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text);
    assert_eq!(
        body(&checked.text)["verdict"]["ok"],
        true,
        "{}",
        checked.text
    );
    let written = send(
        &state,
        steward(),
        "PUT",
        &format!("{uri}?workspace=air-v2"),
        Some(proposal),
    )
    .await;
    assert_eq!(written.status, StatusCode::OK, "{}", written.text);
    assert_eq!(
        body(&written.text)["branch"],
        "workspace/air-v2",
        "{}",
        written.text
    );
    assert!(
        body(&written.text)["metadata"].is_null(),
        "a write, not a Change of the project: {}",
        written.text
    );
    drop(server);
}

/// `validate_only=true` checks a form's draft inside a workspace as `dryRun=All` does, and
/// writes nothing to the copy's branch (T-5949, AIP-163).
#[tokio::test]
async fn validate_only_checks_a_draft_in_a_workspace_and_writes_nothing() {
    let (server, state) = world().await;
    opened(&state).await;
    let mut proposal = json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "ContextSpace",
        "metadata": { "name": "air", "namespace": PROJECT },
        "spec": { "isSandbox": true, "defaultLocale": "en", "ttlDays": 12 }
    });
    proposal["draft"] = json!({ "kind": "ContextSpace", "name": "air" });
    let uri = "/api/v1/projects/ovzdusie/spaces/air";
    let writes = |requests: Vec<wiremock::Request>| {
        requests
            .iter()
            .filter(|r| r.method.as_str() != "GET")
            .count()
    };
    let before = writes(server.received_requests().await.unwrap_or_default());
    let checked = send(
        &state,
        steward(),
        "PUT",
        &format!("{uri}?validate_only=true&workspace=air-v2"),
        Some(proposal.clone()),
    )
    .await;
    assert_eq!(checked.status, StatusCode::OK, "{}", checked.text);
    assert_eq!(
        body(&checked.text)["verdict"]["ok"],
        true,
        "{}",
        checked.text
    );
    assert!(body(&checked.text)["branch"].is_null(), "{}", checked.text);
    let after = writes(server.received_requests().await.unwrap_or_default());
    assert_eq!(after, before, "validate_only wrote to the forge");
}
