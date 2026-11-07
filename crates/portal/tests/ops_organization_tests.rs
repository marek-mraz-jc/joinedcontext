//! The organization's settings through the assistant's tools (T-2732; AG-87, PF-50, TS-26): the
//! page reads the `Organization` manifest and edits it with `PUT …/org/organizations/{name}`, and
//! the assistant reaches the same two things through `jc_resource_get` and `jc_resource_propose`,
//! which call the same read and the same propose, under the same permission check.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{checked_send, envelope, person, send};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

/// An administrator: `Config::for_tests` makes `portal-approver` the bootstrap group.
fn administrator() -> Identity {
    Identity {
        roles: vec!["portal-approver".into()],
        ..person("admin")
    }
}

/// The settings the page edits: the organization's languages and the one a new person starts in.
fn spec(default_locale: &str) -> Value {
    json!({ "domain": "hel.fi", "locales": ["en", "fi", "sv"], "defaultLocale": default_locale })
}

async fn state() -> AppState {
    let state = common::state_on_forge().await;
    state
        .mirror
        .upsert(envelope("Organization", "hel", ORG_NAMESPACE, spec("en")));
    state
}

fn settings(default_locale: &str) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Organization",
        "metadata": { "name": "hel", "namespace": ORG_NAMESPACE },
        "spec": spec(default_locale),
    })
}

fn op(name: &str) -> String {
    format!("/api/v1/projects/{ORG_NAMESPACE}/ops/{name}")
}

#[tokio::test]
async fn the_assistant_reads_the_organization_the_page_reads() {
    let state = state().await;
    let page = send(
        &state,
        administrator(),
        "GET",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/organizations/hel"),
        None,
    )
    .await;
    let tool = send(
        &state,
        administrator(),
        "POST",
        &op("jc_resource_get"),
        Some(json!({ "kind": "Organization", "name": "hel" })),
    )
    .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.text);
    assert_eq!(tool.status, StatusCode::OK, "{}", tool.text);
    let page: Value = serde_json::from_str(&page.text).expect("json");
    let tool: Value = serde_json::from_str(&tool.text).expect("json");
    assert_eq!(tool["spec"], page["spec"]);
    assert_eq!(tool["spec"]["defaultLocale"], "en");
}

#[tokio::test]
async fn the_assistant_proposes_a_settings_edit_as_the_page_does() {
    let state = state().await;
    let page = checked_send(
        &state,
        administrator(),
        "PUT",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/organizations/hel"),
        Some(settings("fi")),
    )
    .await;
    let tool = checked_send(
        &state,
        administrator(),
        "POST",
        &op("jc_resource_propose"),
        Some(json!({ "manifest": settings("fi") })),
    )
    .await;
    assert_eq!(page.status, StatusCode::ACCEPTED, "{}", page.text);
    assert_eq!(tool.status, page.status, "{}", tool.text);
    let tool: Value = serde_json::from_str(&tool.text).expect("json");
    assert_eq!(tool["change"]["kind"], "Change", "{tool}");
}

#[tokio::test]
async fn a_person_who_may_not_edit_the_organization_is_refused_by_the_tool_too() {
    let state = state().await;
    let viewer = person("viewer");
    let page = checked_send(
        &state,
        viewer.clone(),
        "PUT",
        &format!("/api/v1/projects/{ORG_NAMESPACE}/organizations/hel"),
        Some(settings("sv")),
    )
    .await;
    let tool = checked_send(
        &state,
        viewer,
        "POST",
        &op("jc_resource_propose"),
        Some(json!({ "manifest": settings("sv") })),
    )
    .await;
    assert_eq!(page.status, StatusCode::FORBIDDEN, "{}", page.text);
    assert_eq!(tool.status, StatusCode::FORBIDDEN, "{}", tool.text);
    assert!(
        tool.text.contains("Organization"),
        "the refusal names the kind: {}",
        tool.text
    );
}

#[tokio::test]
async fn an_organization_the_mirror_does_not_hold_is_named_as_missing() {
    let state = state().await;
    let tool = send(
        &state,
        administrator(),
        "POST",
        &op("jc_resource_get"),
        Some(json!({ "kind": "Organization", "name": "espoo" })),
    )
    .await;
    assert_eq!(tool.status, StatusCode::NOT_FOUND, "{}", tool.text);
    assert!(
        tool.text.contains("hel"),
        "the answer names the one there is: {}",
        tool.text
    );
}
