//! T-7962: under JC_TENANT_NAMES=project-key a space whose project the registry has no id for
//! yet has no broker tenant: Costs keeps the space with that reason instead of a made-up 0 and
//! asks the broker nothing for it. A binary of its own, since the setting is the process's.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::MockServer;

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

#[tokio::test]
async fn a_space_without_a_project_key_keeps_its_reason_and_asks_the_broker_nothing() {
    std::env::set_var("JC_TENANT_NAMES", "project-key");
    let broker = MockServer::start().await;
    let token = std::env::temp_dir().join(format!("costs-tenant-{}", std::process::id()));
    std::fs::write(&token, "admin-token").expect("write the token");
    let mut config = Config::for_tests();
    config.broker_url = Some(broker.uri());
    config.broker_admin_token_file = Some(token.clone());
    config.cost_prices = Some(
        serde_json::from_value(json!({
            "currency": "EUR", "perUsd": 0.9,
            "nodes": [{"name": "cpx42 nbg1", "monthly": 30.0}],
            "fixed": [], "volumes": [],
        }))
        .expect("the table parses"),
    );
    let state = AppState::new(config, None);
    state.mirror.upsert(envelope(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Organization", "RoleBinding"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "org-admins",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "org-admin", "scope": { "organization": "bb" } }),
    ));
    state
        .mirror
        .upsert(envelope("ContextSpace", "air", "ovzdusie", json!({})));
    let answer = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/organization/costs?month=2026-09",
        None,
    )
    .await;
    let _ = std::fs::remove_file(&token);
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    let space = &body["projects"][0]["spaces"][0];
    assert_eq!(space["space"], "air", "{body}");
    assert!(space["bytes"].is_null(), "{space}");
    assert!(
        space["reason"]
            .as_str()
            .is_some_and(|r| r.contains("has no broker tenant yet")),
        "{space}"
    );
    assert!(broker
        .received_requests()
        .await
        .unwrap_or_default()
        .is_empty());
}
