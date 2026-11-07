//! T-2716 (PF-97, ADR-N-035): an Organization change that crosses a bound of the operator's file
//! is refused at the door, before a Change exists, naming the entry, the value, the bound and who
//! moves the bound. Inside the bound it is proposed as before.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::MockServer;

use common::{checked_send, envelope, forge, person};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

fn administrator() -> Identity {
    Identity {
        roles: vec!["portal-approver".into()],
        ..person("admin")
    }
}

fn spec(spaces: u32) -> Value {
    json!({
        "domain": "hel.fi",
        "locales": ["en"],
        "defaultLocale": "en",
        "projects": { "quota": { "contextSpaces": spaces } }
    })
}

/// The Portal with the operator's `bounds`, and the forge it writes to, kept alive beside it.
async fn state(bounds: Value) -> (AppState, MockServer) {
    let gitea = forge().await;
    let client = GiteaClient::new(
        gitea.uri().parse().expect("mock url"),
        "test-owner",
        "test-repo",
        "token-xyz",
    )
    .expect("client");
    let mut config = Config::for_tests();
    config.organization_bounds = serde_json::from_value(bounds).expect("bounds");
    let state = AppState::new(config, None).with_gitea(Arc::new(client));
    state
        .mirror
        .upsert(envelope("Organization", "hel", ORG_NAMESPACE, spec(10)));
    (state, gitea)
}

async fn propose(state: &AppState, spaces: u32) -> common::Answer {
    checked_send(
        state,
        administrator(),
        "PUT",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/organizations/hel"),
        Some(json!({
            "apiVersion": API_VERSION,
            "kind": "Organization",
            "metadata": { "name": "hel", "namespace": ORG_NAMESPACE },
            "spec": spec(spaces),
        })),
    )
    .await
}

#[tokio::test]
async fn a_quota_over_the_operators_bound_is_refused_before_a_change_exists() {
    let (state, _forge) =
        state(json!({ "spec.projects.quota.contextSpaces": { "max": 20 } })).await;
    let answer = propose(&state, 50).await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    for part in [
        "spec.projects.quota.contextSpaces",
        "50",
        "0 … 20",
        "portal.organizationBounds",
    ] {
        assert!(answer.text.contains(part), "{part}: {}", answer.text);
    }
}

#[tokio::test]
async fn a_quota_inside_the_bound_is_proposed() {
    let (state, _forge) =
        state(json!({ "spec.projects.quota.contextSpaces": { "max": 20 } })).await;
    let answer = propose(&state, 20).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
}

#[tokio::test]
async fn without_an_operator_bound_a_quota_has_no_ceiling() {
    let (state, _forge) = state(json!({})).await;
    let answer = propose(&state, 500).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.text);
}
