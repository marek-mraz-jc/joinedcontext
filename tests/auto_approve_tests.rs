//! An administrator's own change is approved as it is proposed (T-2651, PF-58, AG-11, PF-70).
//!
//! A person proposing in the Portal who holds `approve` and `delete` on every kind of the change
//! has it merged in the same call, with the button's commit message; a red-lane change needs the
//! name typed back first. A steward, a binding that misses one kind, and an agent run acting for
//! the same administrator all leave the change waiting for another approver.

use std::sync::Arc;

mod common;
use common::CheckFirst;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::change::{Change, ChangePhase};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::ops::{self, Caller, Via};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CSRF: &str = "test-csrf-token-2651";
const EMAIL: &str = "jana.kovacova@banskabystrica.sk";
const SPACE_PATH: &str = "projects/ovzdusie/spaces/mobility/space.yaml";
const SPACE_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: ContextSpace\nmetadata:\n  name: mobility\n  namespace: ovzdusie\nspec:\n  isSandbox: true\n";
const PIPELINE_PATH: &str = "projects/ovzdusie/pipelines/aq/pipeline.yaml";
const PIPELINE_YAML: &str = "apiVersion: joinedcontext.com/v1alpha1\nkind: Pipeline\nmetadata:\n  name: aq\n  namespace: ovzdusie\nspec:\n  class: resident\n  targetEndpoint: urn:ngsi-ld:Endpoint:bb.sk:ovzdusie:air\n";

fn identity() -> Identity {
    Identity {
        client: None,
        subject: "sub-jana.kovacova".into(),
        username: "jana.kovacova".into(),
        email: Some(EMAIL.into()),
        name: Some("Jana Kováčová".into()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn cookies(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: identity(),
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &s).expect("store");
    let response = (jar, StatusCode::OK).into_response();
    let session: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| {
            v.to_str()
                .expect("cookie")
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    format!("{}; {CSRF_COOKIE}={CSRF}", session.join("; "))
}

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

/// A forge that takes a proposal as pull request 1, authored by the Portal on the proposer's
/// behalf, lists `files` in it, and merges it; and a mirror that binds the proposer to `rules`
/// across the organization and holds the space `mobility`.
async fn forge(rules: Value, files: &[(&str, &str)]) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    let repo = "/api/v1/repos/test-owner/test-repo";
    let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
    Mock::given(method("GET"))
        .and(path(repo))
        .respond_with(ok(json!({ "default_branch": "main" })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/branches")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/contents/{PIPELINE_PATH}")))
        .respond_with(ok(
            json!({ "sha": "blob-p", "content": STANDARD.encode(PIPELINE_YAML) }),
        ))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{repo}/contents/")))
        .respond_with(ok(
            json!({ "sha": "blob-1", "content": STANDARD.encode(SPACE_YAML) }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(format!("^{repo}/git/trees/")))
        .respond_with(ok(json!({
            "sha": "tree-1",
            "truncated": false,
            "tree": [{ "path": SPACE_PATH, "type": "blob", "sha": "blob-1", "mode": "100644" }]
        })))
        .mount(&server)
        .await;
    for verb in ["PUT", "POST", "DELETE"] {
        Mock::given(method(verb))
            .and(path_regex(format!("^{repo}/contents(/.*)?$")))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(json!({ "commit": { "sha": "c1" } })),
            )
            .mount(&server)
            .await;
    }
    let pull = json!({
        "number": 1,
        "html_url": "https://gitea.example.sk/pulls/1",
        "state": "open",
        "title": "create ContextSpace mobility",
        "head": { "ref": "portal/create-contextspace-mobility-11111111", "sha": "head-1" },
        "base": { "ref": "main" },
        "created_at": "2026-09-22T09:14:22Z",
        "user": { "login": "jana.kovacova", "full_name": "Jana Kováčová", "email": EMAIL },
        "mergeable": true,
        "merged": false
    });
    Mock::given(method("POST"))
        .and(path(format!("{repo}/pulls")))
        .respond_with(ResponseTemplate::new(201).set_body_json(pull.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/pulls/1")))
        .respond_with(ok(pull))
        .mount(&server)
        .await;
    let listed: Vec<Value> = files
        .iter()
        .map(|(filename, status)| json!({ "filename": filename, "status": status }))
        .collect();
    Mock::given(method("GET"))
        .and(path(format!("{repo}/pulls/1/files")))
        .respond_with(ok(json!(listed)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/pulls/1/merge")))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;

    let client = GiteaClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(Config::for_tests(), None).with_gitea(Arc::new(client));
    state
        .mirror
        .upsert(org("Role", "admin-role", json!({ "rules": rules })));
    state.mirror.upsert(org(
        "RoleBinding",
        "admin-binding",
        json!({ "subjects": [{ "user": EMAIL }], "role": "admin-role", "scope": { "organization": "bb" } }),
    ));
    let mut space = org("ContextSpace", "mobility", json!({ "isSandbox": true }));
    space.metadata.namespace = Some("ovzdusie".into());
    state.mirror.upsert(space);
    (server, state)
}

fn admin_of(kinds: &[&str]) -> Value {
    json!([{ "kinds": kinds, "verbs": ["propose", "approve", "delete"] }])
}

/// The merge commit's message, when the change was merged.
async fn merge_message(server: &MockServer) -> Option<String> {
    server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .find(|r| r.url.path().ends_with("/pulls/1/merge"))
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
}

async fn change_of(response: axum::response::Response) -> Change {
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).expect("a Change")
}

/// Proposes the sandbox space `mobility` through the REST door, its check first (PF-57).
async fn propose_space(state: AppState) -> Change {
    let config = state.config.clone();
    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility-2", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let response = server::app(state)
        .oneshot_checked(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(manifest.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    change_of(response).await
}

/// Proposes removing the space `mobility`, a red-lane change, with the name typed back or not.
async fn remove_space(state: AppState, confirm: Option<&str>) -> Change {
    let config = state.config.clone();
    let uri = match confirm {
        Some(name) => format!("/api/v1/projects/ovzdusie/spaces/mobility?confirm={name}"),
        None => "/api/v1/projects/ovzdusie/spaces/mobility".to_owned(),
    };
    let response = server::app(state)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(uri)
                .header(header::COOKIE, cookies(&config))
                .header(CSRF_HEADER, CSRF)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    change_of(response).await
}

/// PF-58: an administrator of the kind proposes a green-lane change and it is approved and
/// merged in the same call, the commit naming them as its author and administrator.
#[tokio::test]
async fn an_administrators_own_change_is_approved_as_it_is_proposed() {
    let (server, state) = forge(admin_of(&["ContextSpace"]), &[(SPACE_PATH, "added")]).await;

    let change = propose_space(state).await;

    assert_eq!(change.status.phase, ChangePhase::Deploying);
    let message = merge_message(&server).await.expect("merged at propose");
    assert!(
        message.contains(&format!("Approved in the Portal by {EMAIL}, its author, as an administrator of ContextSpace (PF-58)")),
        "{message}"
    );
    // PL-95: the proposer beside the approver, for the reconciler's re-check.
    assert!(
        message.contains(&format!("Proposed in the Portal by {EMAIL}")),
        "{message}"
    );
}

/// PF-58, CC-39: a red-lane change of an administrator lands with the name typed back at propose.
#[tokio::test]
async fn an_administrators_red_lane_removal_lands_with_the_name_typed_back() {
    let (server, state) = forge(admin_of(&["ContextSpace"]), &[(SPACE_PATH, "deleted")]).await;

    let change = remove_space(state, Some("mobility")).await;

    assert_eq!(change.status.phase, ChangePhase::Deploying);
    assert!(merge_message(&server).await.is_some());
}

/// CC-39: without the typed name nothing red is merged; the change waits at its approval page,
/// where the name is typed.
#[tokio::test]
async fn a_red_lane_removal_without_the_typed_name_waits_and_merges_nothing() {
    let (server, state) = forge(admin_of(&["ContextSpace"]), &[(SPACE_PATH, "deleted")]).await;

    let change = remove_space(state, None).await;

    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert!(merge_message(&server).await.is_none());
}

/// PF-70: a steward approves but may not delete, so their own change waits for another approver.
#[tokio::test]
async fn a_stewards_own_change_waits_for_another_approver() {
    let rules = json!([{ "kinds": ["ContextSpace"], "verbs": ["propose", "approve"] }]);
    let (server, state) = forge(rules, &[(SPACE_PATH, "added")]).await;

    let change = propose_space(state).await;

    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert!(merge_message(&server).await.is_none());
}

/// PF-58: administering the headline kind is not enough when the change carries a file of a kind
/// the proposer may approve but not delete.
#[tokio::test]
async fn an_administrator_of_some_of_the_changes_kinds_waits_for_another_approver() {
    let rules = json!([
        { "kinds": ["ContextSpace"], "verbs": ["propose", "approve", "delete"] },
        { "kinds": ["Pipeline"], "verbs": ["propose", "approve"] }
    ]);
    let (server, state) = forge(rules, &[(SPACE_PATH, "added"), (PIPELINE_PATH, "added")]).await;

    let change = propose_space(state).await;

    assert_eq!(change.status.phase, ChangePhase::PendingApproval);
    assert!(merge_message(&server).await.is_none());
}

/// Proposes the space through `jc_resource_propose` as the administrator, by `via`, its check
/// recorded first by the same caller (PF-57).
async fn propose_by(state: &AppState, via: Via) -> Value {
    let caller = Caller {
        bearer: None,
        identity: identity(),
        via,
        access: None,
    };
    let manifest = json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": "mobility-3", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    let check = ops::find("jc_manifest_dry_run").expect("dry run");
    ops::call(
        check,
        &caller,
        state,
        "ovzdusie",
        json!({ "manifest": manifest }),
    )
    .await
    .expect("the check");
    let propose = ops::find("jc_resource_propose").expect("propose");
    ops::call(
        propose,
        &caller,
        state,
        "ovzdusie",
        json!({ "manifest": manifest }),
    )
    .await
    .expect("the proposal")
}

/// AG-11, AG-82: an agent run acting for the same administrator never approves their change; it
/// waits for a person. The acceptance test of the boundary.
#[tokio::test]
async fn an_agent_run_never_borrows_its_owners_administrator_standing() {
    let (server, state) = forge(admin_of(&["ContextSpace"]), &[(SPACE_PATH, "added")]).await;

    let out = propose_by(&state, Via::Agent).await;

    assert_eq!(out["change"]["status"]["phase"], "PendingApproval", "{out}");
    assert!(merge_message(&server).await.is_none());
}

/// AG-11: MCP is refused the same way, whoever holds the token.
#[tokio::test]
async fn an_mcp_client_never_approves_on_propose() {
    let (server, state) = forge(admin_of(&["ContextSpace"]), &[(SPACE_PATH, "added")]).await;

    let out = propose_by(&state, Via::Mcp).await;

    assert_eq!(out["change"]["status"]["phase"], "PendingApproval", "{out}");
    assert!(merge_message(&server).await.is_none());
}

/// PF-58: the form proposes through the same operation with the person's session, and that is
/// approved as it is proposed.
#[tokio::test]
async fn the_forms_session_proposal_through_the_operation_is_approved() {
    let (server, state) = forge(admin_of(&["ContextSpace"]), &[(SPACE_PATH, "added")]).await;

    let out = propose_by(&state, Via::Session).await;

    assert_eq!(out["change"]["status"]["phase"], "Deploying", "{out}");
    assert!(merge_message(&server).await.is_some());
}
