//! Access and data models are approved by a second person (PF-58, T-4833, T-4830).
//!
//! A change to a Role, RoleBinding, Group, ServiceAccount or DataModel is never approved by the person who
//! proposed it, even when they administer the kind; another administrator approves it. Proposing
//! through the Portal leaves such a change pending.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use joinedcontext_portal::api::changes::{approve_change_for, ApprovedBy};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::error::ApiError;
use joinedcontext_portal::forge::ShakoClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{self, ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROPOSER: &str = "jana.kovacova@banskabystrica.sk";
const SECOND: &str = "eva.hruba@banskabystrica.sk";
const ACCESS_KINDS: [&str; 5] = [
    "Role",
    "RoleBinding",
    "Group",
    "ServiceAccount",
    "DataModel",
];

fn person(email: &str) -> Identity {
    let user = email.split('@').next().unwrap_or_default().to_owned();
    Identity {
        client: None,
        subject: format!("sub-{user}"),
        username: user.clone(),
        email: Some(email.into()),
        name: Some(user),
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

/// The manifest each access kind is proposed with: the self-escalation case for a binding.
fn manifest(kind: &str) -> (String, String) {
    let spec = match kind {
        "Role" => json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["propose"] }] }),
        "RoleBinding" => json!({
            "subjects": [{ "user": PROPOSER }],
            "role": "admin-role",
            "scope": { "organization": "bb" }
        }),
        "Group" => json!({ "members": [{ "user": PROPOSER }] }),
        "DataModel" => json!({ "version": "1.1.0", "linkml": "models/air.linkml.yaml" }),
        _ => json!({
            "owner": { "user": PROPOSER },
            "purpose": "the air-quality adapter",
            "roles": [],
            "credentials": []
        }),
    };
    let name = format!("{}-x", kind.to_ascii_lowercase());
    let yaml = format!(
        "apiVersion: {API_VERSION}\nkind: {kind}\nmetadata:\n  name: {name}\n  namespace: {ORG_NAMESPACE}\nspec: {spec}\n"
    );
    let info = resource::by_kind(kind).expect("a served kind");
    let path = resource::repository_path(info, ORG_NAMESPACE, None, &name).expect("a path");
    (path, yaml)
}

/// A forge holding one proposal of `kind` by PROPOSER, and a mirror where PROPOSER and SECOND
/// both administer every kind across the organization.
async fn forge(kind: &str) -> (MockServer, AppState) {
    forge_by(kind, "jana.kovacova", PROPOSER).await
}

/// The change of `kind`, proposed by the account named `author` with the e-mail `email`.
async fn forge_by(kind: &str, author: &str, email: &str) -> (MockServer, AppState) {
    let server = MockServer::start().await;
    let repo = "/odata/Repositories(Owner='test-owner',Name='test-repo')";
    let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
    let (file, yaml) = manifest(kind);
    let name = format!("{}-x", kind.to_ascii_lowercase());
    Mock::given(method("GET"))
        .and(path(format!("{repo}/Shako.File(Path=@p,Ref=@r)")))
        .respond_with(ok(
            json!({ "Sha": "blob-1", "Content": STANDARD.encode(&yaml) }),
        ))
        .mount(&server)
        .await;
    let pull = json!({
        "Number": 1,
        "HtmlUrl": "https://shako.example.sk/pulls/1",
        "State": "open",
        "Title": format!("create {kind} {name}"),
        "Head": format!("portal/create-{}-{name}-11111111", kind.to_ascii_lowercase()),
        "HeadSha": "head-1",
        "Base": "main",
        "CreatedAt": "2026-10-20T09:14:22Z",
        "Author": "jana.kovacova",
        "AuthorName": author,
        "AuthorEmail": email,
        "Mergeable": true,
        "Merged": false
    });
    Mock::given(method("GET"))
        .and(path(format!("{repo}/PullRequests(1)")))
        .respond_with(ok(pull))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{repo}/PullRequests(1)/Files")))
        .respond_with(ok(
            json!({ "value": [{ "Filename": file, "Status": "added" }] }),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{repo}/PullRequests(1)/Shako.Merge")))
        .respond_with(ok(json!({})))
        .mount(&server)
        .await;

    let client = ShakoClient::new(
        server.uri().parse().expect("url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let state = AppState::new(Config::for_tests(), None).with_shako(Arc::new(client));
    let mut kinds: Vec<&str> = ACCESS_KINDS.to_vec();
    kinds.push("ContextSpace");
    state.mirror.upsert(org(
        "Role",
        "admin-role",
        json!({ "rules": [{ "kinds": kinds, "verbs": ["propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "admin-binding",
        json!({
            "subjects": [{ "user": PROPOSER }, { "user": SECOND }, { "user": "jana.kovacova" }],
            "role": "admin-role",
            "scope": { "organization": "bb" }
        }),
    ));
    (server, state)
}

async fn merged(server: &MockServer) -> bool {
    server
        .received_requests()
        .await
        .expect("requests")
        .iter()
        .any(|r| r.url.path().ends_with("/PullRequests(1)/Shako.Merge"))
}

/// The hole closed: an administrator of the kind approving their own access change is refused,
/// and nothing is merged.
#[tokio::test]
async fn the_proposer_of_an_access_change_cannot_approve_it() {
    for kind in ACCESS_KINDS {
        let (server, state) = forge(kind).await;
        let err = approve_change_for(
            &state,
            &person(PROPOSER),
            ORG_NAMESPACE,
            "chg-org-00000001",
            Some(&format!("{}-x", kind.to_ascii_lowercase())),
            None,
            ApprovedBy::Person,
        )
        .await
        .expect_err(kind);
        assert!(matches!(err, ApiError::SelfApproval(_)), "{kind}: {err:?}");
        assert!(!merged(&server).await, "{kind} merged by its proposer");
    }
}

/// T-6706: a proposer without an e-mail is the author of what the Portal wrote for them
/// (`{username}@{project}.local`), whatever display name they carry now: a name changed since
/// the proposal, or a namesake, never decides who proposed.
#[tokio::test]
async fn a_proposer_without_an_email_cannot_approve_their_change_under_another_name() {
    let (server, state) = forge_by("Role", "Jana Kováčová", "jana.kovacova@org.local").await;
    let renamed = Identity {
        email: None,
        name: Some("Jana Nováková".into()),
        ..person(PROPOSER)
    };
    let err = approve_change_for(
        &state,
        &renamed,
        ORG_NAMESPACE,
        "chg-org-00000001",
        Some("role-x"),
        None,
        ApprovedBy::Person,
    )
    .await
    .expect_err("the proposer approves their own access change");
    assert!(matches!(err, ApiError::SelfApproval(_)), "{err:?}");
    assert!(!merged(&server).await);
}

/// Another administrator approves the same change.
#[tokio::test]
async fn a_second_administrator_approves_an_access_change() {
    for kind in ACCESS_KINDS {
        let (server, state) = forge(kind).await;
        approve_change_for(
            &state,
            &person(SECOND),
            ORG_NAMESPACE,
            "chg-org-00000001",
            Some(&format!("{}-x", kind.to_ascii_lowercase())),
            None,
            ApprovedBy::Person,
        )
        .await
        .unwrap_or_else(|err| panic!("{kind}: {err:?}"));
        assert!(merged(&server).await, "{kind} not merged");
    }
}
