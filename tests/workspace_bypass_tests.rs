//! T-1709 — "a copy or a preview as a way around review": the bring-back as an attack
//! (PF-82, PF-83, CC-81).
//!
//! **The attack.** A workspace is a copy of the configuration that nobody reviews while it is
//! being written, so the way around review is to put the risky manifest inside the copy and
//! bring the copy back as a change that looks small: a grant edited among a sandbox space's
//! fields, or a grant removed altogether. The defence is that the bring-back is not a merge but
//! a proposal: every file of the branch is classified again, the riskiest one decides the lane
//! the approval enforces (CC-63, CC-79), and every file is checked against the rights the person
//! has *now*, not the ones they had when they wrote it (PF-82).
//!
//! What the rest of the vector is played by, so these cases do not repeat it:
//! `workspace_propose_tests.rs` (the write inside a copy takes the same checks — viewer, quota,
//! secret, own rights), `workspace_security_tests.rs` (whose copy a person may read),
//! `edge_workspace_routes_tests.rs` (only the owner brings one back, another project's is not
//! there), `workspace_preview_tests.rs` and `workspace_reaper_tests.rs` (a preview serves only
//! its own project, an expired copy is served to nobody and loses its branch),
//! `internal_listener_tests.rs` (`/internal/previews` answers the gateway's own token alone),
//! `preview_tests.rs` in the platform (a preview never takes over a slug or a space of `main`).

mod common;

use axum::http::StatusCode;
use common::{encode, envelope, forge, person, send, state_on, REPO};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::ops::workspaces::{Opening, Scope};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT: &str = "ovzdusie";
const WS: &str = "/api/v1/projects/ovzdusie/workspaces";
const AIR: &str = "projects/ovzdusie/spaces/air/space.yaml";
const GRANT: &str = "projects/ovzdusie/spaces/air/policies/open.yaml";

/// The owner of the copy: bound to read and propose the one kind a sandbox space is, and to
/// nothing else. A grant is exactly what such a person may not write (PF-52).
fn author() -> Identity {
    Identity {
        client: None,
        groups: vec!["space-authors".into()],
        ..person("jana")
    }
}

/// Proposes everything: the bootstrap group, which is how the first binding is written.
fn steward() -> Identity {
    Identity {
        client: None,
        groups: vec!["portal-approver".into()],
        ..person("jana")
    }
}

fn space(ttl: u32) -> String {
    format!(
        "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: air\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n  defaultLocale: en\n  ttlDays: {ttl}\n"
    )
}

/// A Policy over the space: the grant this vector hides inside the copy. It reads to the public,
/// which is what makes it worth hiding.
fn grant() -> String {
    "apiVersion: joinedcontext.com/v1alpha1\nkind: Policy\nmetadata:\n  name: open\n  namespace: ovzdusie\nspec:\n  contextSpaceRef: { kind: ContextSpace, name: air }\n  assigner: did:web:banskabystrica.sk\n  assignee: { kind: role, id: public }\n  operations: [queryEntity, retrieveEntity]\n".to_owned()
}

async fn tree(server: &MockServer, git_ref: &str, entries: &[(&str, &str)]) {
    let tree: Vec<_> = entries
        .iter()
        .map(|(path, sha)| json!({ "path": path, "type": "blob", "sha": sha }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/git/trees/{git_ref}")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tree": tree, "truncated": false })),
        )
        .mount(server)
        .await;
}

async fn branch(server: &MockServer, name: &str, head: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/branches/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "commit": { "id": head } })))
        .mount(server)
        .await;
}

async fn file(server: &MockServer, git_ref: &str, at: &str, content: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{REPO}/contents/{at}")))
        .and(query_param("ref", git_ref))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "path": at, "sha": "x", "encoding": "base64", "content": encode(content)
        })))
        .mount(server)
        .await;
}

/// The forge, the mirror with `air` and the author's narrow binding.
async fn world() -> (MockServer, AppState) {
    let server = forge().await;
    branch(&server, "main", "base1").await;
    let state = state_on(&server);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        PROJECT,
        json!({ "isSandbox": true, "defaultLocale": "en", "ttlDays": 10 }),
    ));
    state.mirror.upsert(envelope(
        "Role",
        "space-author",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "space-authors",
        ORG_NAMESPACE,
        json!({ "role": "space-author", "subjects": [{ "group": "space-authors" }],
                "scope": { "project": PROJECT } }),
    ));
    (server, state)
}

/// The copy `air-v2` of the whole project, owned by `jana@hel.fi`, opened at `base1`.
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

/// The copy changed the sandbox space and wrote the grant beside it; main moved neither.
async fn a_grant_beside_a_space(server: &MockServer) {
    branch(server, "workspace/air-v2", "ws1").await;
    tree(server, "ws1", &[(AIR, "s-ours"), (GRANT, "g-ours")]).await;
    tree(server, "base1", &[(AIR, "s-base")]).await;
    tree(server, "main", &[(AIR, "s-base")]).await;
    file(server, "workspace/air-v2", AIR, &space(12)).await;
    file(server, "base1", AIR, &space(10)).await;
    file(server, "workspace/air-v2", GRANT, &grant()).await;
}

async fn bring_back(state: &AppState, who: Identity) -> common::Answer {
    send(
        state,
        who,
        "POST",
        &format!("{WS}/air-v2/propose"),
        Some(json!({})),
    )
    .await
}

async fn pulls(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/pulls"))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// PF-82, CC-63, CC-79: the grant decides the lane of the whole bring-back. The copy's other
/// file is a sandbox space, which is Green on its own and is what a change of one file would be
/// listed as; the Policy beside it is Red, so the Change is Red and the approval CC-19 asks for
/// is the one a grant gets. The case fails the moment the fold in `propose` takes the last
/// file's lane, or the headline's, instead of the riskiest.
#[tokio::test]
async fn a_grant_written_in_a_copy_makes_the_whole_bring_back_red() {
    let (server, state) = world().await;
    opened(&state).await;
    a_grant_beside_a_space(&server).await;

    let answer = bring_back(&state, steward()).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let change = body(&answer.text);
    assert_eq!(
        change["status"]["lane"], "red",
        "a grant among green files is still a grant: {}",
        answer.text
    );
    let plan = &change["status"]["plan"];
    assert_eq!(plan["create"], 1, "the grant is new: {plan}");
    assert_eq!(plan["update"], 1, "the space changed: {plan}");
    assert_eq!(pulls(&server).await.len(), 1, "one Change, not two");
}

/// PF-82: the rights are read at the bring-back, not only at the write. The author may propose a
/// ContextSpace and nothing else, so the copy holding a Policy is refused — which is what closes
/// the way around review for a person whose rights were narrowed after they wrote the file, or
/// who was given the copy by somebody who had them. Nothing is opened for an approver to see.
#[tokio::test]
async fn a_copy_whose_grant_is_above_the_owners_rights_opens_no_change() {
    let (server, state) = world().await;
    opened(&state).await;
    a_grant_beside_a_space(&server).await;

    let answer = bring_back(&state, author()).await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
    assert!(
        answer.text.contains("Policy"),
        "the refusal names the kind the person is missing: {}",
        answer.text
    );
    assert!(
        pulls(&server).await.is_empty(),
        "a refused bring-back opens no merge request"
    );
}

/// CC-19, PF-82: the other half of the same attack — the grant is not edited but deleted, so the
/// copy's own files are only the ones that stayed. A removal is Red whatever it removes, and the
/// manifest of a file the branch no longer holds is read from the copy's base revision, which is
/// the only place it still exists.
#[tokio::test]
async fn a_grant_deleted_in_a_copy_comes_back_as_a_removal_and_stays_red() {
    let (server, state) = world().await;
    opened(&state).await;
    branch(&server, "workspace/air-v2", "ws1").await;
    tree(&server, "ws1", &[(AIR, "s-ours")]).await;
    tree(&server, "base1", &[(AIR, "s-base"), (GRANT, "g-base")]).await;
    tree(&server, "main", &[(AIR, "s-base"), (GRANT, "g-base")]).await;
    file(&server, "workspace/air-v2", AIR, &space(12)).await;
    file(&server, "base1", AIR, &space(10)).await;
    file(&server, "base1", GRANT, &grant()).await;

    let answer = bring_back(&state, steward()).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
    let change = body(&answer.text);
    assert_eq!(
        change["status"]["lane"], "red",
        "a removal is Red (CC-19): {}",
        answer.text
    );
    assert_eq!(change["status"]["plan"]["delete"], 1);
}
