//! A connector its owner set autonomous has its changes approved as that person (T-6552).
//!
//! The Portal approves through the approval page's own path: a DataModel merges at once, the
//! merge commit and the activity feed name the connector, and a change granting access waits for
//! a person's click.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::api::changes::{approve_change_for, ApprovedBy};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{self, ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROPOSER: &str = "jana.kovacova@banskabystrica.sk";
const PROJECT: &str = "doprava";
const CONNECTOR: &str = "doprava-in-claude";
const KINDS: [&str; 4] = ["DataModel", "Policy", "RoleBinding", "ContextSpace"];

/// Jana as a connector's token carries her: a client beside the person.
fn through_connector() -> Identity {
    Identity {
        client: Some("claude-code".into()),
        subject: "sub-jana".into(),
        username: "jana.kovacova".into(),
        email: Some(PROPOSER.into()),
        name: Some("jana.kovacova".into()),
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
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

fn manifest(kind: &str) -> (String, String) {
    let spec = match kind {
        "DataModel" => json!({ "version": "1.0.0", "linkml": "models/air.linkml.yaml" }),
        "Policy" => json!({
            "assignee": { "kind": "user", "id": "eva.hruba@banskabystrica.sk" },
            "actions": ["read"]
        }),
        _ => {
            json!({
                "subjects": [{ "user": "eva.hruba@banskabystrica.sk" }],
                "role": "admin-role",
                "scope": { "project": PROJECT }
            })
        }
    };
    let name = format!("{}-x", kind.to_ascii_lowercase());
    let yaml = format!(
        "apiVersion: {API_VERSION}\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: {PROJECT}\nspec: {spec}\n"
    );
    let info = resource::by_kind(kind).expect("a served kind");
    let space = (kind == "Policy").then_some("air");
    let path = resource::repository_path(info, PROJECT, space, &name).expect("a path");
    (path, yaml)
}

/// A forge holding Jana's proposal of `kind` in PROJECT, and a mirror where she administers every
/// kind across the organization.
async fn forge(kind: &str) -> (MockServer, AppState) {
    forge_with(kind, "added").await
}

/// [`forge`] with the proposal's one file in the forge's `status` (`added`, `deleted`).
async fn forge_with(kind: &str, status: &str) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    let repo = "/api/v1/repos/test-owner/test-repo";
    let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
    let (file, yaml) = manifest(kind);
    Mock::given(method("GET"))
        .and(path_regex(format!("^{repo}/contents/")))
        .respond_with(ok(
            json!({ "sha": "blob-1", "content": STANDARD.encode(&yaml) }),
        ))
        .mount(&server)
        .await;
    let pull = json!({
        "number": 1,
        "html_url": "https://gitea.example.sk/pulls/1",
        "state": "open",
        "title": format!("create {kind}"),
        "head": { "ref": format!("portal/create-{}-x-11111111", kind.to_ascii_lowercase()), "sha": "head-1" },
        "base": { "ref": "main" },
        "created_at": "2026-11-03T05:14:22Z",
        "user": { "login": "jana.kovacova", "full_name": "jana.kovacova", "email": PROPOSER },
        "mergeable": true,
        "merged": false
    });
    Mock::given(method("GET"))
        .and(path(format!("{repo}/pulls/1")))
        .respond_with(ok(pull))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/pulls/1/files")))
        .respond_with(ok(json!([{ "filename": file, "status": status }])))
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
    state.mirror.upsert(org(
        "Role",
        "admin-role",
        json!({ "rules": [{ "kinds": KINDS, "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "admin-binding",
        json!({
            "subjects": [{ "user": PROPOSER }],
            "role": "admin-role",
            "scope": { "organization": "bb" }
        }),
    ));
    (server, state)
}

async fn merge_message(server: &MockServer) -> Option<String> {
    server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .find(|r| r.url.path().ends_with("/pulls/1/merge"))
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
}

async fn approve(state: &AppState, by: ApprovedBy) -> Result<(), ApiError> {
    approve_change_for(
        state,
        &through_connector(),
        PROJECT,
        "chg-00000001",
        None,
        by,
    )
    .await
    .map(|_| ())
}

/// An autonomous connector's DataModel merges as its owner, the commit naming the connector.
#[tokio::test]
async fn an_autonomous_connector_merges_its_owners_change() {
    let (server, state) = forge("DataModel").await;
    approve(&state, ApprovedBy::Connector(CONNECTOR.into()))
        .await
        .expect("approved");
    let message = merge_message(&server).await.expect("merged");
    assert!(
        message.contains(&format!("{PROPOSER}, via connector {CONNECTOR}")),
        "{message}"
    );
}

/// The audit row on the activity feed names the person and the connector.
#[tokio::test]
async fn the_activity_row_names_the_connector() {
    let (_server, state) = forge("DataModel").await;
    approve(&state, ApprovedBy::Connector(CONNECTOR.into()))
        .await
        .expect("approved");
    let filter = joinedcontext_portal::activity::ActivityFilter {
        limit: 10,
        ..Default::default()
    };
    let feed = state
        .activity
        .list(PROJECT, &filter)
        .await
        .expect("activity");
    let row = feed
        .items
        .iter()
        .find(|e| e.kind == "change.merged")
        .expect("a merged row");
    assert_eq!(row.details["viaConnector"], CONNECTOR);
    assert!(row
        .summary
        .contains(&format!("{PROPOSER} via connector {CONNECTOR}")));
}

/// A change granting access waits for a person's click, autonomous or not.
#[tokio::test]
async fn an_access_change_waits_for_a_click() {
    for kind in ["Policy", "RoleBinding"] {
        let (server, state) = forge(kind).await;
        let err = approve(&state, ApprovedBy::Connector(CONNECTOR.into()))
            .await
            .expect_err(kind);
        assert!(matches!(err, ApiError::Denied(_)), "{kind}: {err:?}");
        assert!(merge_message(&server).await.is_none(), "{kind} merged");
    }
}

/// A deletion waits for a person's click too, even by an owner who may delete the kind: it is red
/// lane, and the autonomous path never types the name its confirmation asks (PF-13, CC-19).
#[tokio::test]
async fn a_deletion_waits_for_a_click() {
    let (server, state) = forge_with("DataModel", "deleted").await;
    let err = approve(&state, ApprovedBy::Connector(CONNECTOR.into()))
        .await
        .expect_err("held");
    assert!(
        matches!(&err, ApiError::BadRequest(text) if text.contains("requires confirm")),
        "{err:?}"
    );
    assert!(
        merge_message(&server).await.is_none(),
        "the deletion merged"
    );
}

/// Without the autonomous switch a connector's token decides nothing (AG-11).
#[tokio::test]
async fn a_connector_token_alone_approves_nothing() {
    let (server, state) = forge("DataModel").await;
    let err = approve(&state, ApprovedBy::Operation)
        .await
        .expect_err("refused");
    assert!(matches!(err, ApiError::Denied(_)), "{err:?}");
    assert!(merge_message(&server).await.is_none());
}

fn project_grant(state: &AppState, verbs: Value) {
    let envelope = |kind: &str, name: &str, spec: Value| ResourceEnvelope {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    };
    state.mirror.upsert(envelope(
        "Role",
        "petra-role",
        json!({ "rules": [{ "kinds": KINDS, "verbs": verbs }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "petra-binding",
        json!({ "subjects": [{ "user": "petra@hel.fi" }], "role": "petra-role",
                "scope": { "project": PROJECT } }),
    ));
}

fn petra() -> Identity {
    Identity {
        client: None,
        subject: "sub-petra".into(),
        username: "petra@hel.fi".into(),
        email: Some("petra@hel.fi".into()),
        name: None,
        roles: Vec::new(),
        groups: Vec::new(),
        second_factor: false,
    }
}

fn ask(writes: bool) -> joinedcontext_portal::connectors::NewConnector {
    serde_json::from_value(json!({
        "name": "auto", "segments": ["context"], "writes": writes, "autonomous": true,
        "scope": [{ "project": PROJECT, "access": "write" }],
        "expiresAt": "2999-01-01T00:00:00Z",
    }))
    .expect("request")
}

/// Only a person who may approve in every project the connector writes switches autonomous on;
/// a reader gets the 403 naming the field, and autonomous without writes is a 422.
#[test]
fn a_reader_cannot_switch_autonomous_on() {
    use joinedcontext_portal::api::connectors::may_be_autonomous;
    let state = AppState::new(Config::for_tests(), None);
    project_grant(&state, json!(["read", "propose"]));
    let refused = may_be_autonomous(&state, &petra(), &ask(true));
    assert!(
        matches!(&refused, Err(ApiError::Denied(d)) if d.starts_with("autonomous:")),
        "{refused:?}"
    );
    let no_writes = may_be_autonomous(&state, &petra(), &ask(false));
    assert!(
        matches!(&no_writes, Err(ApiError::Unprocessable { errors, .. }) if errors == &["autonomous"]),
        "{no_writes:?}"
    );
    project_grant(&state, json!(["read", "propose", "approve"]));
    let ok = may_be_autonomous(&state, &petra(), &ask(true));
    assert!(ok.is_ok(), "{ok:?}");
}
