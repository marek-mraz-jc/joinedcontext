//! Workspaces over REST and the registry (API/01 §22, T-1236, T-1237): open, list, read,
//! compare, update from main, bring back and discard, each by the person the contract allows.

mod common;

use axum::http::StatusCode;
use common::{encode, envelope, forge, person, send, state_on, REPO};
use jc_time::TimestampTz;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::workspaces::{Opening, Scope};
use joinedcontext_portal::ops::{self, Caller, OpError, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{body_string_contains, method, path, query_param};
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
        .map(|(p, sha)| json!({ "Path": p, "Type": "blob", "Sha": sha }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .and(query_param("@r", format!("'{git_ref}'")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "Tree": tree, "Truncated": false })),
        )
        .mount(server)
        .await;
}

async fn file(server: &MockServer, git_ref: &str, content: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", format!("'{AIR}'")))
        .and(query_param("@r", format!("'{}'", git_ref)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "x",
            "Content": encode(content)
        })))
        .mount(server)
        .await;
}

/// The forge, the mirror with `air` at base, and the viewer's read-only binding.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Branches('main')")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "HeadSha": "base1" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.DeleteBranch")))
        .and(body_string_contains(r#""Branch":"workspace/air-v2""#))
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
    // The copy's slashed branch is read directly: shako takes the ref as a parameter value, so
    // no branch lookup comes first (T-2266).
    tree(server, "workspace/air-v2", &[(AIR, "s-ours")]).await;
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
    let branches = calls(&server, "POST", "/Shako.CreateBranch").await;
    assert_eq!(branches[0]["NewBranchName"], "workspace/air-v2");
    let listed = send(&state, viewer(), "GET", WS, None).await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(body(&listed.text)["items"][0]["name"], "air-v2");
}

/// A branch an earlier workspace of the name left is deleted and cut again from main; a forge
/// that cuts no branch leaves no record behind.
#[tokio::test]
async fn opening_recuts_a_left_branch_and_records_nothing_when_no_branch_is_cut() {
    let (server, state) = world().await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.CreateBranch")))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({ "message": "exists" })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let answer = send(
        &state,
        steward(),
        "POST",
        WS,
        Some(json!({ "name": "air-v2" })),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.text);
    assert_eq!(calls(&server, "POST", "/Shako.DeleteBranch").await.len(), 1);
    assert_eq!(calls(&server, "POST", "/Shako.CreateBranch").await.len(), 2);

    let (server, state) = world().await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.CreateBranch")))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "message": "down" })))
        .with_priority(1)
        .mount(&server)
        .await;
    let answer = send(
        &state,
        steward(),
        "POST",
        WS,
        Some(json!({ "name": "air-v2" })),
    )
    .await;
    assert!(answer.status.is_server_error(), "{}", answer.text);
    let listed = send(&state, steward(), "GET", WS, None).await;
    assert_eq!(body(&listed.text)["items"], json!([]), "{}", listed.text);
}

/// A left branch the forge will not delete opens no workspace; a forge failing the workspace's
/// tree read is said on a compare, never read as an empty workspace.
#[tokio::test]
async fn a_forge_failing_the_branch_or_tree_is_said_and_records_nothing() {
    let (server, state) = world().await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.CreateBranch")))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({ "message": "exists" })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{REPO}/Shako.DeleteBranch")))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "message": "down" })))
        .with_priority(1)
        .mount(&server)
        .await;
    let answer = send(
        &state,
        steward(),
        "POST",
        WS,
        Some(json!({ "name": "air-v2" })),
    )
    .await;
    assert!(answer.status.is_server_error(), "{}", answer.text);
    let listed = send(&state, steward(), "GET", WS, None).await;
    assert_eq!(body(&listed.text)["items"], json!([]), "{}", listed.text);

    let (server, state) = world().await;
    opened(&state).await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .and(query_param("@r", "'workspace/air-v2'"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({ "message": "down" })))
        .mount(&server)
        .await;
    let compared = send(
        &state,
        steward(),
        "GET",
        &format!("{WS}/air-v2/compare"),
        None,
    )
    .await;
    assert!(compared.status.is_server_error(), "{}", compared.text);
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

/// The workspaces page names each copy's scope and the bar its count of changed files (T-7116):
/// the OData set answers both under `$select`, as the REST doors do, and leaves them out of a
/// plain row.
#[tokio::test]
async fn the_workspaces_set_answers_scope_and_changes_as_the_rest_doors() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 10), "s-base").await;
    // The service answers at the installation's Organization, which holds the project.
    let org = json!({ "installation": true });
    state
        .mirror
        .upsert(envelope("Organization", "hel", ORG_NAMESPACE, org));
    let owner = json!({ "organizationRef": "hel" });
    state
        .mirror
        .upsert(envelope("Project", PROJECT, ORG_NAMESPACE, owner));
    let rest = body(&send(&state, viewer(), "GET", WS, None).await.text);
    let set = format!("/api/v1/projects('{PROJECT}')/workspaces");
    let selected = send(
        &state,
        viewer(),
        "GET",
        &format!("{set}?$select=name,scope"),
        None,
    )
    .await;
    assert_eq!(selected.status, StatusCode::OK, "{}", selected.text);
    let rows = body(&selected.text);
    assert_eq!(rows["value"][0]["scope"], rest["items"][0]["scope"]);
    assert_eq!(rows["value"][0]["scope"], json!({ "kind": "project" }));
    let one = format!("{set}('air-v2')?$select=name,scope,changes");
    let keyed = send(&state, viewer(), "GET", &one, None).await;
    assert_eq!(keyed.status, StatusCode::OK, "{}", keyed.text);
    let read = body(
        &send(&state, viewer(), "GET", &format!("{WS}/air-v2"), None)
            .await
            .text,
    );
    assert_eq!(body(&keyed.text)["scope"], json!({ "kind": "project" }));
    assert_eq!(body(&keyed.text)["changes"], read["changes"]);
    assert_eq!(read["changes"], 1);
    let plain = body(&send(&state, viewer(), "GET", &set, None).await.text);
    assert_eq!(plain["value"][0]["name"], "air-v2");
    assert!(plain["value"][0].get("scope").is_none(), "{plain}");
    assert!(plain["value"][0].get("changes").is_none(), "{plain}");
    // The detail does not move the ETag a write's If-Match sends.
    assert_eq!(
        body(&keyed.text)["@odata.etag"],
        plain["value"][0]["@odata.etag"]
    );
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

/// T-7817: a file the workspace removed while main changed it is one whole-file choice, and a
/// changed file that is no manifest nor a native kind is not listed.
#[tokio::test]
async fn compare_names_a_removal_against_mains_change_as_the_whole_file() {
    let (server, state) = world().await;
    opened(&state).await;
    tree(&server, "workspace/air-v2", &[("notes.txt", "n-ours")]).await;
    tree(
        &server,
        "base1",
        &[(AIR, "s-base"), ("notes.txt", "n-base")],
    )
    .await;
    tree(&server, "main", &[(AIR, "s-main"), ("notes.txt", "n-base")]).await;
    file(&server, "base1", &space("en", 10)).await;
    file(&server, "main", &space("fi", 10)).await;
    let compare = ops::find("jc_workspace_compare").unwrap();
    let cmp = ops::call(
        compare,
        &Caller::new(steward(), Via::Session),
        &state,
        PROJECT,
        json!({ "name": "air-v2" }),
    )
    .await
    .expect("compared");
    let files: Vec<&str> = cmp["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect();
    assert_eq!(files, [AIR], "{cmp}");
    assert_eq!(cmp["files"][0]["operation"], "Delete", "{cmp}");
    let conflict = &cmp["conflicts"][0];
    assert_eq!(conflict["path"], AIR, "{cmp}");
    assert_eq!(conflict["fields"][0]["ours"], "removed");
    assert_eq!(conflict["fields"][0]["theirs"], "changed");
}

/// The mirror image: the workspace changed a file main removed; the conflict is the whole file,
/// changed on ours and removed on theirs.
#[tokio::test]
async fn compare_names_mains_removal_against_the_workspaces_change_as_the_whole_file() {
    let (server, state) = world().await;
    opened(&state).await;
    tree(&server, "workspace/air-v2", &[(AIR, "s-ours")]).await;
    tree(&server, "base1", &[(AIR, "s-base")]).await;
    tree(&server, "main", &[]).await;
    file(&server, "workspace/air-v2", &space("en", 12)).await;
    file(&server, "base1", &space("en", 10)).await;
    let compare = ops::find("jc_workspace_compare").unwrap();
    let cmp = ops::call(
        compare,
        &Caller::new(steward(), Via::Session),
        &state,
        PROJECT,
        json!({ "name": "air-v2" }),
    )
    .await
    .expect("compared");
    let conflict = &cmp["conflicts"][0];
    assert_eq!(conflict["path"], AIR, "{cmp}");
    assert_eq!(conflict["fields"][0]["ours"], "changed", "{cmp}");
    assert_eq!(conflict["fields"][0]["theirs"], "removed", "{cmp}");
}

/// A workspace already brought back as an open change is not brought back a second time.
#[tokio::test]
async fn bringing_back_twice_names_the_change_already_open() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 10), "s-base").await;
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/PullRequests")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [{
            "Number": 5, "HtmlUrl": "https://shako.example/pulls/5", "State": "open",
            "Title": "workspace air-v2: Air", "Head": "workspace/air-v2", "Base": "main",
            "CreatedAt": "2026-09-18T10:00:00Z", "Author": "jana",
            "Mergeable": true, "Merged": false }] })))
        .with_priority(1)
        .mount(&server)
        .await;
    let answer = send(
        &state,
        steward(),
        "POST",
        &format!("{WS}/air-v2/propose"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::CONFLICT, "{}", answer.text);
    assert!(
        answer.text.contains("already brought back as chg-00000005"),
        "{}",
        answer.text
    );
    assert!(calls(&server, "POST", "/Shako.OpenPullRequest")
        .await
        .is_empty());
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
        calls(&server, "POST", "/Shako.CommitFiles")
            .await
            .is_empty(),
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
    let commit = &calls(&server, "POST", "/Shako.CommitFiles").await[0];
    assert_eq!(commit["Branch"], "workspace/air-v2");
    let merged = String::from_utf8(
        base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            commit["Files"][0]["content"].as_str().unwrap(),
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

/// A file at `path` of `git_ref` holding `content`.
async fn file_at(server: &MockServer, file_path: &str, git_ref: &str, content: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
        .and(query_param("@p", format!("'{file_path}'")))
        .and(query_param("@r", format!("'{git_ref}'")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Sha": "x",
            "Content": encode(content)
        })))
        .mount(server)
        .await;
}

/// CC-80 beyond one manifest: a file only main changed is taken, one main removed is removed,
/// a file that is no manifest and that both sides changed (or one removed) is a whole-file
/// choice, and a credential's field in a conflict is never shown, on either side.
#[tokio::test]
async fn update_from_main_takes_mains_files_asks_for_whole_files_and_masks_a_credential() {
    let (server, state) = world().await;
    opened(&state).await;
    const README: &str = "projects/ovzdusie/README.md";
    const NOTES: &str = "projects/ovzdusie/notes.txt";
    const GONE: &str = "projects/ovzdusie/gone.txt";
    const OLD: &str = "projects/ovzdusie/old.txt";
    tree(
        &server,
        "workspace/air-v2",
        &[
            (AIR, "a-ours"),
            (README, "r-ours"),
            (NOTES, "n1"),
            (GONE, "g1"),
        ],
    )
    .await;
    tree(
        &server,
        "base1",
        &[
            (AIR, "a-base"),
            (README, "r-base"),
            (NOTES, "n1"),
            (GONE, "g1"),
            (OLD, "o1"),
        ],
    )
    .await;
    tree(
        &server,
        "main",
        &[
            (AIR, "a-main"),
            (README, "r-main"),
            (NOTES, "n2"),
            (OLD, "o2"),
        ],
    )
    .await;
    let with_password = |password: &str| format!("{}  password: {password}\n", space("en", 10));
    file(&server, "workspace/air-v2", &with_password("ours-value")).await;
    file(&server, "base1", &with_password("base-value")).await;
    file(&server, "main", &with_password("main-value")).await;
    file_at(&server, README, "main", "# main's words\n").await;
    file_at(&server, NOTES, "main", "main's notes\n").await;
    file_at(&server, OLD, "main", "main kept this\n").await;

    let uri = format!("{WS}/air-v2/update");
    let refused = send(&state, steward(), "POST", &uri, Some(json!({}))).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    let conflicts = body(&refused.text)["conflicts"].clone();
    let at = |file_path: &str| {
        conflicts
            .as_array()
            .and_then(|all| all.iter().find(|c| c["path"] == file_path))
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(at(README)["fields"][0]["ours"], "changed", "{conflicts}");
    assert_eq!(at(README)["fields"][0]["theirs"], "changed", "{conflicts}");
    assert_eq!(at(OLD)["fields"][0]["ours"], "removed", "{conflicts}");
    assert_eq!(at(OLD)["fields"][0]["theirs"], "changed", "{conflicts}");
    let password = at(AIR)["fields"]
        .as_array()
        .and_then(|fields| fields.iter().find(|f| f["path"] == "spec.password"))
        .cloned()
        .unwrap_or(Value::Null);
    for side in ["ours", "theirs", "base"] {
        assert_eq!(password[side], "[REDACTED]", "{side}: {conflicts}");
    }
    for value in ["ours-value", "base-value", "main-value"] {
        assert!(
            !refused.text.contains(value),
            "{value} leaked: {}",
            refused.text
        );
    }

    let answers = json!({ "resolutions": [
        { "path": README, "field": "", "keep": "theirs" },
        { "path": OLD, "field": "", "keep": "ours" },
        { "path": AIR, "field": "spec.password", "keep": "ours" },
        { "path": AIR, "field": "spec.ttlDays", "keep": "ours" },
    ] });
    // Keeping a literal credential is the merged manifest's check to refuse, at its file.
    let kept_secret = send(&state, steward(), "POST", &uri, Some(answers.clone())).await;
    assert_eq!(
        kept_secret.status,
        StatusCode::BAD_REQUEST,
        "{}",
        kept_secret.text
    );
    assert!(
        kept_secret.text.contains(&format!("{AIR}: literal secret")),
        "{}",
        kept_secret.text
    );
    assert!(
        calls(&server, "POST", "/Shako.CommitFiles")
            .await
            .is_empty(),
        "nothing written"
    );

    // The workspace drops the credential; main changed the ttl the workspace changed too.
    for (git_ref, ttl) in [("workspace/air-v2", 12), ("base1", 10), ("main", 13)] {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{AIR}'")))
            .and(query_param("@r", format!("'{git_ref}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "x",
                "Content": encode(&space("en", ttl))
            })))
            .with_priority(1)
            .mount(&server)
            .await;
    }
    let resolved = send(&state, steward(), "POST", &uri, Some(answers)).await;
    assert_eq!(resolved.status, StatusCode::OK, "{}", resolved.text);
    let answer = body(&resolved.text);
    assert_eq!(answer["taken"], json!([GONE, NOTES]), "{answer}");
    assert!(
        answer["merged"]
            .as_array()
            .is_some_and(|merged| merged.contains(&json!(README))),
        "{answer}"
    );
    let commit = calls(&server, "POST", "/Shako.CommitFiles").await.remove(0);
    let text = commit.to_string();
    assert!(
        text.contains(NOTES) && text.contains(README) && text.contains(GONE),
        "{text}"
    );
    assert!(!text.contains(OLD), "ours kept: {text}");
}

/// CC-80 (T-7921): a YAML file both sides changed that merges into no manifest, or into a kind
/// this Portal does not serve, is refused at its path; a file main removed that the workspace
/// changed asks for a whole-file choice, naming main's side removed.
#[tokio::test]
async fn update_from_main_refuses_yaml_that_is_no_manifest_and_asks_for_a_removed_file() {
    let (server, state) = world().await;
    opened(&state).await;
    const README: &str = "projects/ovzdusie/README.md";
    const NOTES: &str = "projects/ovzdusie/notes.yaml";
    let uri = format!("{WS}/air-v2/update");
    tree(&server, "workspace/air-v2", &[(NOTES, "n-ours")]).await;
    tree(&server, "base1", &[(NOTES, "n-base")]).await;
    tree(&server, "main", &[(NOTES, "n-main")]).await;
    for (head, words) in [
        ("workspace/air-v2", "a: 2\nb: 1\n"),
        ("base1", "a: 1\nb: 1\n"),
        ("main", "a: 1\nb: 2\n"),
    ] {
        file_at(&server, NOTES, head, words).await;
    }
    let refused = send(&state, steward(), "POST", &uri, Some(json!({}))).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert!(
        refused.text.contains(&format!("{NOTES}: not a manifest")),
        "{}",
        refused.text
    );
    // The same file as a manifest of a kind the Portal does not serve.
    let bogus = |a: u8, b: u8| {
        format!("apiVersion: joinedcontext.com/v1alpha1\nkind: Bogus\nmetadata:\n  name: notes\n  namespace: ovzdusie\nspec:\n  a: {a}\n  b: {b}\n")
    };
    for (head, words) in [
        ("workspace/air-v2", bogus(2, 1)),
        ("base1", bogus(1, 1)),
        ("main", bogus(1, 2)),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("{REPO}/Shako.File(Path=@p,Ref=@r)")))
            .and(query_param("@p", format!("'{NOTES}'")))
            .and(query_param("@r", format!("'{head}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Sha": "x",
                "Content": encode(&words)
            })))
            .with_priority(1)
            .mount(&server)
            .await;
    }
    let refused = send(&state, steward(), "POST", &uri, Some(json!({}))).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert!(
        refused.text.contains("kind 'Bogus' is not served"),
        "{}",
        refused.text
    );
    // The workspace changed its README; main removed it.
    for (head, sha) in [("workspace/air-v2", "r-ours"), ("base1", "r-base")] {
        Mock::given(method("GET"))
            .and(path(format!(
                "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
            )))
            .and(query_param("@r", format!("'{head}'")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Tree": [{ "Path": README, "Type": "blob", "Sha": sha }],
                "Truncated": false
            })))
            .with_priority(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!(
            "{REPO}/Shako.Tree(Ref=@r,Recursive=true,Skip=@s,Top=@t)"
        )))
        .and(query_param("@r", "'main'"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "Tree": [], "Truncated": false })),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    let asked = send(&state, steward(), "POST", &uri, Some(json!({}))).await;
    assert_eq!(asked.status, StatusCode::CONFLICT, "{}", asked.text);
    let conflicts = body(&asked.text)["conflicts"].clone();
    assert_eq!(conflicts[0]["path"], README, "{conflicts}");
    assert_eq!(conflicts[0]["fields"][0]["ours"], "changed", "{conflicts}");
    assert_eq!(
        conflicts[0]["fields"][0]["theirs"], "removed",
        "{conflicts}"
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
    assert!(calls(&server, "POST", "/Shako.OpenPullRequest")
        .await
        .is_empty());
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
        change["status"]["lane"], "yellow",
        "an edit of a space that exists is reviewed, though it declares itself a sandbox"
    );
    let pulls = calls(&server, "POST", "/Shako.OpenPullRequest").await;
    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0]["Head"], "workspace/air-v2");
    assert!(pulls[0]["Body"].as_str().unwrap().contains("jana@hel.fi"));
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
    tree(&server, "workspace/air-v2", &[(AIR, "s-base")]).await;
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
    assert!(calls(&server, "POST", "/Shako.OpenPullRequest")
        .await
        .is_empty());
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
    assert!(calls(&server, "POST", "/Shako.OpenPullRequest")
        .await
        .is_empty());
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
        calls(&server, "POST", "/Shako.DeleteBranch")
            .await
            .iter()
            .all(|deleted| deleted["Branch"] != "workspace/air-v2"),
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

/// The registry's update, bring-back and discard run the same functions as the REST doors for a
/// person in a session (T-7817).
#[tokio::test]
async fn a_person_updates_brings_back_and_discards_through_the_registry() {
    let (server, state) = world().await;
    opened(&state).await;
    changed(&server, &space("en", 10), "s-base").await;
    let person = Caller::new(steward(), Via::Session);
    let name = json!({ "name": "air-v2" });
    let update = ops::find("jc_workspace_update_from_main").unwrap();
    let report = ops::call(
        update,
        &person,
        &state,
        PROJECT,
        json!({ "name": "air-v2", "resolutions": [] }),
    )
    .await
    .expect("updated");
    assert_eq!(report["merged"], json!([]), "{report}");
    let propose = ops::find("jc_workspace_propose").unwrap();
    let change = ops::call(propose, &person, &state, PROJECT, name.clone())
        .await
        .expect("proposed");
    assert_eq!(change["lane"], "yellow", "{change}");
    assert_eq!(
        calls(&server, "POST", "/Shako.OpenPullRequest").await.len(),
        1
    );
    let discard = ops::find("jc_workspace_discard").unwrap();
    let gone = ops::call(discard, &person, &state, PROJECT, name)
        .await
        .expect("discarded");
    assert_eq!(gone, json!({ "discarded": "air-v2" }));
    assert!(state.workspaces.get("air-v2").await.unwrap().is_none());
}

#[tokio::test]
async fn discarding_removes_the_branch_and_the_record() {
    let (server, state) = world().await;
    opened(&state).await;
    let answer = send(&state, steward(), "DELETE", &format!("{WS}/air-v2"), None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.text);
    assert_eq!(
        calls(&server, "POST", "/Shako.DeleteBranch")
            .await
            .iter()
            .filter(|deleted| deleted["Branch"] == "workspace/air-v2")
            .count(),
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
    assert!(!expired.expired(TimestampTz::now()));
    assert!(expired.expired(TimestampTz::now() + jiff::SignedDuration::from_hours(2)));
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

/// API/01 §22 (T-7657): a removal inside the owner's workspace is one commit on its branch and
/// no Change of the project; in someone else's workspace it is refused and nothing is written.
#[tokio::test]
async fn a_removal_inside_a_workspace_is_a_commit_on_its_branch_and_only_the_owners() {
    let (server, state) = world().await;
    opened(&state).await;
    for git_ref in ["base1", "workspace/air-v2"] {
        tree(&server, git_ref, &[(AIR, "x")]).await;
    }
    file(&server, "workspace/air-v2", &space("en", 10)).await;
    let uri = "/api/v1/projects/ovzdusie/spaces/air?workspace=air-v2&confirm=air";
    let other = Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("petra")
    };
    let refused = send(&state, other, "DELETE", uri, None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text);
    assert!(refused.text.contains("only its owner"), "{}", refused.text);
    assert!(calls(&server, "POST", "/Shako.CommitFiles")
        .await
        .is_empty());

    let removed = send(&state, steward(), "DELETE", uri, None).await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.text);
    let answer = body(&removed.text);
    assert_eq!(answer["branch"], "workspace/air-v2", "{answer}");
    assert_eq!(answer["path"], AIR, "{answer}");
    assert!(answer["metadata"].is_null(), "no Change: {answer}");
    let commits = calls(&server, "POST", "/Shako.CommitFiles").await;
    assert_eq!(commits.len(), 1, "{commits:?}");
    assert_eq!(commits[0]["Branch"], "workspace/air-v2", "{}", commits[0]);
    assert!(calls(&server, "POST", "/PullRequests").await.is_empty());
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
