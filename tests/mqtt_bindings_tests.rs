//! T-5533 (API/01 §47): a broker's topic bindings reach whoever may read MqttBrokers in the
//! project, each hop as that person may read it; a stranger, a broker that does not exist and a
//! caller without a session learn nothing.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

fn state() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    for (role, kinds) in [
        ("brokers-only", json!(["MqttBroker"])),
        (
            "everything",
            json!(["MqttBroker", "DataSource", "Pipeline", "Endpoint"]),
        ),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
        ));
    }
    for (binding, who, role, project) in [
        ("jana", "jana", "brokers-only", "helsinki"),
        ("eva", "eva", "everything", "helsinki"),
        ("olli", "olli", "everything", "espoo"),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": { "project": project } }),
        ));
    }
    state.mirror.upsert(envelope(
        "MqttBroker",
        "sensors",
        "helsinki",
        json!({ "roles": [] }),
    ));
    state.mirror.upsert(envelope(
        "DataSource",
        "sensors-in",
        "helsinki",
        json!({ "type": "mqtt", "mqtt": { "brokerRef": { "kind": "MqttBroker", "name": "sensors" }, "topics": ["devices/+/data"] } }),
    ));
    state
}

async fn call(state: &AppState, who: &str, name: &str) -> (StatusCode, Value) {
    let uri = format!("/api/v1/projects/helsinki/mqttbrokers/{name}/bindings");
    let answer = send(state, person(who), "GET", &uri, None).await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_reader_of_every_hop_sees_the_topic_and_its_source() {
    let (status, body) = call(&state(), "eva", "sensors").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "broker": "sensors", "bindings": [{
            "topicFilter": "devices/+/data",
            "dataSource": { "kind": "DataSource", "name": "sensors-in" },
        }] })
    );
}

#[tokio::test]
async fn a_reader_of_brokers_alone_sees_the_source_hidden_without_its_name() {
    let (status, body) = call(&state(), "jana", "sensors").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["bindings"],
        json!([{ "dataSource": { "kind": "DataSource", "hidden": true } }])
    );
    assert!(!body.to_string().contains("sensors-in") && !body.to_string().contains("devices/"));
}

#[tokio::test]
async fn a_stranger_and_a_missing_broker_answer_the_same_404() {
    let state = state();
    let (stranger, _) = call(&state, "olli", "sensors").await;
    let (missing, _) = call(&state, "eva", "nobody").await;
    assert_eq!(
        (stranger, missing),
        (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND)
    );
}

#[tokio::test]
async fn no_session_is_401() {
    let response = server::app(state())
        .oneshot(
            Request::builder()
                .uri("/api/v1/projects/helsinki/mqttbrokers/sensors/bindings")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
