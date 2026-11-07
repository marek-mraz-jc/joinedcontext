//! T-5876 (ADR-N-048 §2.13, API/01 §47): the explain route answers which roles of a broker a
//! ServiceAccount holds to whoever reads the project's brokers, 403 to a member who does not, 404
//! for a broker or account that is not there; and a ServiceAccount proposal grants a broker role
//! only when its proposer reads the broker and the broker declares the role.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{checked_send, envelope, person, send, state_on_forge};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

async fn state() -> AppState {
    let state = state_on_forge().await;
    let upsert = |kind: &str, name: &str, namespace: &str, spec: Value| {
        state.mirror.upsert(envelope(kind, name, namespace, spec));
    };
    for (role, kinds, verbs) in [
        (
            "steward",
            json!(["MqttBroker", "ServiceAccount"]),
            json!(["read", "propose"]),
        ),
        // Writes accounts and reads no broker.
        (
            "account-author",
            json!(["ServiceAccount"]),
            json!(["read", "propose"]),
        ),
    ] {
        upsert(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": kinds, "verbs": verbs }] }),
        );
    }
    for (user, role, project) in [
        ("stef", "steward", "helsinki"),
        ("aino", "account-author", "helsinki"),
        ("olli", "steward", "espoo"),
    ] {
        upsert(
            "RoleBinding",
            &format!("{user}-{role}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{user}@hel.fi") }], "role": role, "scope": { "project": project } }),
        );
    }
    upsert(
        "MqttBroker",
        "air",
        "helsinki",
        json!({ "roles": [
            { "name": "sensor", "publish": ["air/${clientid}/pm10"] },
            { "name": "camera", "serviceAccounts": ["old-cam"], "publish": ["cams/${clientid}/#"] },
        ] }),
    );
    for (name, roles) in [
        (
            "fleet",
            json!([{ "role": "mqtt/air/sensor", "scope": { "project": "helsinki" } }]),
        ),
        (
            "lapsed",
            json!([{
                "role": "mqtt/*/sensor", "scope": { "project": "helsinki" },
                "validity": { "notAfter": "2026-01-01T00:00:00Z" }
            }]),
        ),
        (
            "old-cam",
            json!([{ "role": "project-reader", "scope": { "project": "helsinki" } }]),
        ),
    ] {
        upsert("ServiceAccount", name, "helsinki", account(roles));
    }
    state
}

fn account(roles: Value) -> Value {
    json!({ "owner": { "user": "stef" }, "purpose": "air sensors", "credentials": [{ "kind": "oauth-client", "name": "main" }], "roles": roles })
}

async fn explain(state: &AppState, who: &str, broker: &str, query: &str) -> (StatusCode, Value) {
    let uri = format!("/api/v1/projects/helsinki/mqttbrokers/{broker}/access{query}");
    let answer = send(state, person(who), "GET", &uri, None).await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_reader_of_the_brokers_sees_the_bound_role_with_its_filters() {
    let (status, body) = explain(&state().await, "stef", "air", "?account=fleet").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["roles"],
        json!([{ "role": "sensor", "publish": ["air/${clientid}/pm10"], "subscribe": [], "legacy": false, "bound": true }])
    );
    assert_eq!(body["outOfWindow"], json!([]));
}

#[tokio::test]
async fn an_expired_grant_holds_nothing_and_a_listed_account_holds_its_legacy_role() {
    let state = state().await;
    let (status, lapsed) = explain(&state, "stef", "air", "?account=lapsed").await;
    assert_eq!(status, StatusCode::OK, "{lapsed}");
    assert_eq!(lapsed["roles"], json!([]));
    assert_eq!(lapsed["outOfWindow"][0]["grant"], "mqtt/*/sensor");
    let (status, old) = explain(&state, "stef", "air", "?account=old-cam").await;
    assert_eq!(status, StatusCode::OK, "{old}");
    assert_eq!(old["roles"][0]["role"], "camera");
    assert_eq!(old["roles"][0]["legacy"], true);
}

#[tokio::test]
async fn a_member_who_reads_no_broker_is_403_and_a_stranger_404() {
    let state = state().await;
    let (member, body) = explain(&state, "aino", "air", "?account=fleet").await;
    assert_eq!(member, StatusCode::FORBIDDEN, "{body}");
    let (stranger, body) = explain(&state, "olli", "air", "?account=fleet").await;
    assert_eq!(stranger, StatusCode::NOT_FOUND, "{body}");
}

#[tokio::test]
async fn an_unknown_broker_or_account_is_404_and_no_account_400() {
    let state = state().await;
    let (broker, _) = explain(&state, "stef", "water", "?account=fleet").await;
    let (account, _) = explain(&state, "stef", "air", "?account=nobody").await;
    let (none, _) = explain(&state, "stef", "air", "").await;
    let (bad, _) = explain(&state, "stef", "air", "?account=Not%20a%20name").await;
    assert_eq!(
        (broker, account, none, bad),
        (
            StatusCode::NOT_FOUND,
            StatusCode::NOT_FOUND,
            StatusCode::BAD_REQUEST,
            StatusCode::BAD_REQUEST
        )
    );
}

async fn propose_account(state: &AppState, who: &str, role: &str) -> (StatusCode, String) {
    let body = json!({
        "apiVersion": "joinedcontext.com/v1alpha1", "kind": "ServiceAccount",
        "metadata": { "name": "new-fleet" },
        "spec": account(json!([{ "role": role, "scope": { "project": "helsinki" } }])),
    });
    let answer = checked_send(
        state,
        person(who),
        "POST",
        "/api/v1/projects/helsinki/serviceaccounts",
        Some(body),
    )
    .await;
    (answer.status, answer.text)
}

#[tokio::test]
async fn a_proposer_who_reads_no_broker_cannot_grant_a_broker_role_403_naming_the_field() {
    let (status, text) = propose_account(&state().await, "aino", "mqtt/air/sensor").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(text.contains("spec.roles[0].role"), "{text}");
}

#[tokio::test]
async fn a_role_the_broker_does_not_declare_is_422_and_a_missing_broker_names_the_field() {
    let state = state().await;
    let (status, text) = propose_account(&state, "stef", "mqtt/air/vent").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{text}");
    assert!(text.contains("spec.roles[0].role"), "{text}");
    let (status, text) = propose_account(&state, "stef", "mqtt/water/sensor").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("spec.roles[0].role"), "{text}");
    assert!(text.contains("MqttBroker 'water'"), "{text}");
}

#[tokio::test]
async fn a_declared_role_of_a_readable_broker_is_proposed() {
    let state = state().await;
    for role in ["mqtt/air/sensor", "mqtt/*/camera"] {
        let (status, text) = propose_account(&state, "stef", role).await;
        assert!(status.is_success(), "{role}: {status} {text}");
    }
}
