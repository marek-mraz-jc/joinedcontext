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
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{self, ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, path_regex};
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
    let server = MockServer::start().await;
    let repo = "/api/v1/repos/test-owner/test-repo";
    let ok = |body: Value| ResponseTemplate::new(200).set_body_json(body);
    let (file, yaml) = manifest(kind);
    let name = format!("{}-x", kind.to_ascii_lowercase());
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
        "title": format!("create {kind} {name}"),
        "head": { "ref": format!("portal/create-{}-{name}-11111111", kind.to_ascii_lowercase()), "sha": "head-1" },
        "base": { "ref": "main" },
        "created_at": "2026-10-20T09:14:22Z",
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
        .respond_with(ok(json!([{ "filename": file, "status": "added" }])))
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
            "subjects": [{ "user": PROPOSER }, { "user": SECOND }],
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
        .any(|r| r.url.path().ends_with("/pulls/1/merge"))
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
            ApprovedBy::Person,
        )
        .await
        .expect_err(kind);
        assert!(matches!(err, ApiError::SelfApproval(_)), "{kind}: {err:?}");
        assert!(!merged(&server).await, "{kind} merged by its proposer");
    }
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
            ApprovedBy::Person,
        )
        .await
        .unwrap_or_else(|err| panic!("{kind}: {err:?}"));
        assert!(merged(&server).await, "{kind} not merged");
    }
}
