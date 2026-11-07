//! T-5058 (DM-108, DM-132): the Retention tab's "Rows of history" select is a hint, the server is
//! the check. A space proposal carries `record` per type; the Portal accepts the modes the broker
//! runs (`observed`, `every`, `onChange` with a heartbeat, `never`) and refuses anything else with
//! an answer that names the field.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use joinedcontext_portal::auth::session::Identity;

fn approver() -> Identity {
    let mut identity = common::person("admin");
    identity.groups = vec!["portal-approver".to_owned()];
    identity
}

async fn dry_run(retention: Value) -> (StatusCode, String) {
    let state = common::state_on_forge().await;
    let answer = common::send(
        &state,
        approver(),
        "POST",
        "/api/v1/projects/ovzdusie/spaces?dryRun=All",
        Some(json!({
            "apiVersion": "joinedcontext.com/v1alpha1",
            "kind": "ContextSpace",
            "metadata": { "name": "bikes", "namespace": "ovzdusie" },
            "spec": { "isSandbox": true, "retention": retention },
        })),
    )
    .await;
    (answer.status, answer.text)
}

fn valid(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|body| body["valid"] == json!(true))
}

#[tokio::test]
async fn each_mode_the_broker_runs_is_accepted() {
    let (status, text) = dry_run(json!([
        { "entityType": "Station", "history": "P30D", "record": "observed" },
        { "entityType": "Counter", "history": "P30D", "record": "every" },
        {
            "entityType": "BikeHireDockingStation",
            "history": "P30D",
            "record": "onChange",
            "heartbeat": "PT1H",
            "attributes": { "dateModified": { "record": "never" } }
        }
    ]))
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(valid(&text), "{text}");
}

#[tokio::test]
async fn a_mode_the_broker_does_not_run_is_refused_naming_record() {
    let (status, text) = dry_run(json!([
        { "entityType": "Station", "history": "P30D", "record": "sometimes" }
    ]))
    .await;
    assert!(!valid(&text), "{status}: {text}");
    assert!(
        text.contains("record"),
        "the refusal names the field: {text}"
    );
}

#[tokio::test]
async fn on_change_without_a_heartbeat_is_refused_naming_the_heartbeat() {
    let (status, text) = dry_run(json!([
        { "entityType": "Station", "history": "P30D", "record": "onChange" }
    ]))
    .await;
    assert!(!valid(&text), "{status}: {text}");
    assert!(
        text.contains("heartbeat"),
        "the refusal names the field: {text}"
    );
}
