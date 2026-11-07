//! Organization setup (T-2748, API/01 §25, PF-90, UI-82): one read says which setup steps a new
//! organization still lacks and what the installation has to provide, judged from what the
//! Portal holds, and only an Organization Administrator may read it.

mod common;

use joinedcontext_portal::config::ClientAuth;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, forge, person, send};
use joinedcontext_portal::config::SetupStatements;
use joinedcontext_portal::people::People;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const SETUP: &str = "/api/v1/organization/setup";

/// A realm whose admin API lists `people` people.
async fn realm(people: usize) -> MockServer {
    let kc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/realms/hel/protocol/openid-connect/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "access_token": "t" })))
        .mount(&kc)
        .await;
    let users: Vec<Value> = (0..people)
        .map(|i| json!({ "id": format!("id-{i}"), "username": format!("p{i}@hel.fi"), "email": format!("p{i}@hel.fi"), "enabled": true }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/admin/realms/hel/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(users))
        .mount(&kc)
        .await;
    kc
}

/// ada is the Organization Administrator; jana reads pipelines and nothing else.
fn state_with(kc: &MockServer, gitea: &MockServer, said: SetupStatements) -> AppState {
    let mut state = common::state_on(gitea);
    let mut config = (*state.config).clone();
    config.setup = said;
    state.config = Arc::new(config);
    state.people = People::new(
        &format!("{}/realms/hel", kc.uri()),
        "portal".into(),
        ClientAuth::Secret("secret".into()),
    )
    .map(Arc::new);
    let mirror = &state.mirror;
    mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Organization", "RoleBinding"], "verbs": ["propose", "approve", "delete"] }] }),
    ));
    mirror.upsert(envelope(
        "Role",
        "pipeline-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    ));
    for (name, user, role) in [
        ("admins", "ada@hel.fi", "org-admin"),
        ("readers", "jana@hel.fi", "pipeline-reader"),
    ] {
        mirror.upsert(envelope(
            "RoleBinding",
            name,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": user }], "role": role, "scope": { "organization": "hel" } }),
        ));
    }
    state
}

fn done(answer: &Value, list: &str) -> Vec<(String, bool)> {
    answer[list]
        .as_array()
        .expect("a list")
        .iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap_or_default().to_owned(),
                item["done"] == true,
            )
        })
        .collect()
}

#[tokio::test]
async fn an_empty_organization_lacks_every_step_and_says_nothing_it_was_not_told() {
    let (kc, gitea) = (realm(1).await, forge().await);
    let state = state_with(&kc, &gitea, SetupStatements::default());

    let answer = send(&state, person("ada"), "GET", SETUP, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(body["complete"], false);
    let steps: Vec<(String, bool)> = [
        "organization",
        "domain",
        "people",
        "project",
        "publishers",
        "policies",
    ]
    .iter()
    .map(|id| ((*id).to_owned(), false))
    .collect();
    assert_eq!(done(&body, "steps"), steps);
    assert!(
        done(&body, "operator").iter().all(|(_, done)| !done),
        "{body}"
    );
}

#[tokio::test]
async fn each_step_is_done_by_what_the_portal_holds() {
    let (kc, gitea) = (realm(2).await, forge().await);
    let said = SetupStatements {
        login_theme: Some("joinedcontext".into()),
        smtp: true,
        backups: true,
    };
    let state = state_with(&kc, &gitea, said);
    let mut organization = envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({ "domain": "hel.fi", "locales": ["en", "fi"], "projects": { "creation": "org-admin" } }),
    );
    organization.status = Some(serde_json::from_value(json!({
        "phase": "Live",
        "domainVerification": {
            "state": "verified", "method": "dns-txt", "challenge": "c",
            "record": "_joinedcontext.hel.fi TXT \"jc-verify=c\"", "checkedAt": "2026-09-25T08:00:00Z",
        },
    })).expect("a status"));
    state.mirror.upsert(organization);
    state.mirror.upsert(envelope(
        "ContextSpace",
        "helsinki",
        "helsinki",
        json!({ "dataModelRef": { "kind": "DataModel", "name": "air" } }),
    ));
    state.mirror.upsert(envelope(
        "CkanInstance",
        "open-data",
        ORG_NAMESPACE,
        json!({ "url": "https://data.hel.fi" }),
    ));

    let answer = send(&state, person("ada"), "GET", SETUP, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    assert!(done(&body, "steps").iter().all(|(_, done)| *done), "{body}");
    assert_eq!(
        done(&body, "operator"),
        vec![
            ("branding".into(), false),
            ("loginTheme".into(), true),
            ("smtp".into(), true),
            ("backups".into(), true)
        ],
        "the branding is the stock one"
    );
    assert_eq!(body["complete"], false);
}

#[tokio::test]
async fn a_space_without_a_model_is_not_a_first_project() {
    let (kc, gitea) = (realm(2).await, forge().await);
    let state = state_with(&kc, &gitea, SetupStatements::default());
    state
        .mirror
        .upsert(envelope("ContextSpace", "empty", "helsinki", json!({})));
    let answer = send(&state, person("ada"), "GET", SETUP, None).await;
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    assert!(
        done(&body, "steps").contains(&("project".into(), false)),
        "{body}"
    );
    assert!(
        done(&body, "steps").contains(&("people".into(), true)),
        "{body}"
    );
}

#[tokio::test]
async fn only_an_organization_administrator_reads_it() {
    let (kc, gitea) = (realm(1).await, forge().await);
    let state = state_with(&kc, &gitea, SetupStatements::default());
    let answer = send(&state, person("jana"), "GET", SETUP, None).await;
    assert_eq!(answer.status, StatusCode::FORBIDDEN, "{}", answer.text);
}
