//! T-2715 (PF-99, PF-101, PF-102; API/01 §28): Organization settings reads every entry of the
//! catalog with the operator's bound, the value in force and where it comes from, and each
//! readable project's quota with what it uses.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

fn administrator() -> Identity {
    Identity {
        roles: vec!["portal-approver".into()],
        ..person("admin")
    }
}

fn state() -> AppState {
    let mut config = Config::for_tests();
    config.organization_bounds = serde_json::from_value(json!({
        "spec.projects.quota.contextSpaces": { "max": 20 },
        "spec.limits.signIn.sessionIdleMinutes": { "max": 120 }
    }))
    .expect("bounds");
    let state = AppState::new(config, None);
    state.mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        json!({
            "domain": "hel.fi",
            "locales": ["en"],
            "defaultLocale": "en",
            "projects": { "quota": { "contextSpaces": 10 } },
            "limits": { "edge": { "requestsPerMinute": { "web": 300 } } }
        }),
    ));
    // helsinki carries its own quota and holds one space; espoo takes the organization's.
    state.mirror.upsert(envelope(
        "Project",
        "helsinki",
        ORG_NAMESPACE,
        json!({ "quotas": { "contextSpaces": 12 } }),
    ));
    state
        .mirror
        .upsert(envelope("Project", "espoo", ORG_NAMESPACE, json!({})));
    state
        .mirror
        .upsert(envelope("ContextSpace", "air", "helsinki", json!({})));
    state
}

async fn limits(identity: Identity) -> (StatusCode, Value) {
    let answer = send(
        &state(),
        identity,
        "GET",
        "/api/v1/organization/limits",
        None,
    )
    .await;
    let body = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
    (answer.status, body)
}

fn entry<'a>(body: &'a Value, path: &str) -> &'a Value {
    body["entries"]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["path"] == path))
        .unwrap_or_else(|| panic!("no entry {path}: {body}"))
}

#[tokio::test]
async fn every_entry_carries_its_bound_its_value_and_where_the_value_comes_from() {
    let (status, body) = limits(administrator()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entries = body["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), jc_core::kinds::org_settings::CATALOG.len());

    let spaces = entry(&body, "spec.projects.quota.contextSpaces");
    assert_eq!(
        (
            &spaces["section"],
            &spaces["min"],
            &spaces["max"],
            &spaces["value"],
            &spaces["origin"]
        ),
        (
            &json!("projects"),
            &json!(0),
            &json!(20),
            &json!(10),
            &json!("organization")
        )
    );
    let web = entry(&body, "spec.limits.edge.requestsPerMinute.web");
    assert_eq!(
        (&web["value"], &web["origin"], &web["section"]),
        (&json!(300), &json!("organization"), &json!("edge"))
    );

    // Not set: the default, and the operator's tighter security bound.
    let idle = entry(&body, "spec.limits.signIn.sessionIdleMinutes");
    assert_eq!(
        (
            &idle["value"],
            &idle["origin"],
            &idle["default"],
            &idle["max"],
            &idle["security"]
        ),
        (
            &Value::Null,
            &json!("default"),
            &json!(60),
            &json!(120),
            &json!(true)
        )
    );
}

#[tokio::test]
async fn each_project_shows_whose_quota_is_in_force_and_what_it_uses() {
    let (_, body) = limits(administrator()).await;
    let project = |name: &str| {
        body["projects"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["project"] == name))
            .cloned()
            .unwrap_or_else(|| panic!("no project {name}: {body}"))
    };
    let helsinki = project("helsinki");
    assert_eq!(helsinki["origin"], "project");
    assert_eq!(
        helsinki["quota"]["contextSpaces"],
        json!({ "limit": 12, "used": 1 })
    );
    // A limit enforced at run time has no manifest count.
    assert_eq!(helsinki["quota"]["agentRunsPerDay"]["used"], Value::Null);
    let espoo = project("espoo");
    assert_eq!(espoo["origin"], "organization");
    assert_eq!(
        espoo["quota"]["contextSpaces"],
        json!({ "limit": 10, "used": 0 })
    );
}

#[tokio::test]
async fn a_person_who_reads_no_project_gets_the_catalog_and_no_project() {
    let (status, body) = limits(person("stranger")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body["entries"].as_array().expect("entries").is_empty());
    assert_eq!(
        body["projects"],
        json!([]),
        "nothing of a project the caller cannot read"
    );
}
