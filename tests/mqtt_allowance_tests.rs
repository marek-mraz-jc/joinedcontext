//! T-4859 (PF-117…PF-119, ADR-N-048 §2.9, API/01 §47): every door refuses a broker past the
//! project's MQTT allowance, naming the limit; only an org admin writes a project's own
//! allowance; whoever reads the project reads the allowance and its usage.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{checked_send, envelope, person, send, state_on_forge};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

fn broker(connections: u32) -> Value {
    json!({
        "roles": [{"name": "camera", "serviceAccounts": ["cam"], "publish": ["cams/${clientid}/#"]}],
        "limits": {"maxConnections": connections, "messagesPerSecond": 1}
    })
}

fn project(mqtt: Option<Value>) -> Value {
    let mut spec = json!({ "organizationRef": "hel" });
    if let Some(mqtt) = mqtt {
        spec["quotas"] = json!({ "mqtt": mqtt });
    }
    spec
}

fn organization(mqtt: Value) -> Value {
    json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en", "limits": { "mqtt": mqtt } })
}

async fn state() -> AppState {
    let state = state_on_forge().await;
    let upsert = |kind: &str, name: &str, namespace: &str, spec: Value| {
        state.mirror.upsert(envelope(kind, name, namespace, spec));
    };
    upsert(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        organization(json!({})),
    );
    upsert("Project", "helsinki", ORG_NAMESPACE, project(None));
    upsert("Project", "espoo", ORG_NAMESPACE, project(None));
    upsert(
        "Role",
        "org-admin",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["RoleBinding", "Project", "MqttBroker", "Organization"], "verbs": ["read", "propose", "approve", "delete"] }] }),
    );
    // A steward of every project, short of administering the organization.
    upsert(
        "Role",
        "steward",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Project", "MqttBroker", "ServiceAccount"], "verbs": ["read", "propose", "approve"] }] }),
    );
    // Writes brokers and reads no ServiceAccount (T-5590).
    upsert(
        "Role",
        "broker-author",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["MqttBroker"], "verbs": ["read", "propose"] }] }),
    );
    upsert(
        "Role",
        "viewer",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["MqttBroker"], "verbs": ["read"] }] }),
    );
    for (user, role, scope) in [
        ("ada", "org-admin", json!({ "organization": "hel" })),
        ("stef", "steward", json!({ "organization": "hel" })),
        ("vic", "viewer", json!({ "project": "helsinki" })),
        ("bea", "broker-author", json!({ "project": "helsinki" })),
    ] {
        upsert(
            "RoleBinding",
            &format!("{user}-{role}"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{user}@hel.fi") }], "role": role, "scope": scope }),
        );
    }
    upsert("ServiceAccount", "cam", "helsinki", json!({ "roles": [] }));
    state
}

async fn propose_broker(state: &AppState, name: &str, spec: Value) -> (StatusCode, String) {
    propose_broker_as(state, "stef", name, spec).await
}

async fn propose_broker_as(
    state: &AppState,
    who: &str,
    name: &str,
    spec: Value,
) -> (StatusCode, String) {
    let body = json!({
        "apiVersion": "joinedcontext.com/v1alpha1", "kind": "MqttBroker",
        "metadata": { "name": name }, "spec": spec
    });
    // Checked first the way every door is (PF-57); a refusal is the same either way.
    let answer = checked_send(
        state,
        person(who),
        "POST",
        "/api/v1/projects/helsinki/mqttbrokers",
        Some(body),
    )
    .await;
    (answer.status, answer.text)
}

async fn put_project(state: &AppState, who: &str, mqtt: Value) -> (StatusCode, String) {
    let body = json!({
        "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Project",
        "metadata": { "name": "helsinki", "namespace": ORG_NAMESPACE },
        "spec": project(Some(mqtt))
    });
    let answer = checked_send(
        state,
        person(who),
        "PUT",
        "/api/v1/projects/org/projects/helsinki",
        Some(body),
    )
    .await;
    (answer.status, answer.text)
}

#[tokio::test]
async fn a_project_nobody_enabled_refuses_every_broker_and_says_who_turns_it_on() {
    let state = state().await;
    let (status, text) = propose_broker(&state, "cams", broker(10)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(text.contains("MQTT is off for project helsinki"), "{text}");
    assert!(text.contains("Organization settings"), "{text}");
}

#[tokio::test]
async fn each_limit_refuses_the_broker_past_it_and_names_the_limit() {
    let state = state().await;
    state.mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        organization(json!({ "enabled": true, "brokers": 2, "connections": 1500 })),
    ));
    let (status, text) = propose_broker(&state, "one", broker(1000)).await;
    assert!(status.is_success(), "inside the allowance: {status} {text}");

    state
        .mirror
        .upsert(envelope("MqttBroker", "one", "helsinki", broker(1000)));
    let (status, text) = propose_broker(&state, "two", broker(600)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(text.contains("1000 in use, 500 left"), "{text}");

    let mut bytes = broker(10);
    bytes["limits"]["maxMessageBytes"] = json!(70000);
    let (status, text) = propose_broker(&state, "two", bytes).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(text.contains("maxMessageBytes"), "{text}");

    let mut relay = broker(10);
    relay["mode"] = json!("relay");
    let (status, text) = propose_broker(&state, "two", relay).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(text.contains("spec.mode relay"), "{text}");

    state
        .mirror
        .upsert(envelope("MqttBroker", "two", "helsinki", broker(10)));
    let (status, text) = propose_broker(&state, "three", broker(10)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(
        text.contains("the project may run 2 MQTT brokers (2 in use)"),
        "{text}"
    );
}

#[tokio::test]
async fn only_an_org_admin_writes_a_projects_own_allowance() {
    let state = state().await;
    let mqtt = json!({ "enabled": true, "brokers": 2 });
    let (status, text) = put_project(&state, "stef", mqtt.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(text.contains("spec.quotas.mqtt"), "{text}");

    let (status, text) = put_project(&state, "ada", mqtt).await;
    assert!(status.is_success(), "the org admin: {status} {text}");

    // Even an org admin stays inside the operator's bound.
    let (status, text) =
        put_project(&state, "ada", json!({ "enabled": true, "brokers": 21 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("spec.quotas.mqtt.brokers"), "{text}");
}

#[tokio::test]
async fn whoever_reads_the_project_reads_its_allowance_and_usage() {
    let state = state().await;
    state.mirror.upsert(envelope(
        "Project",
        "helsinki",
        ORG_NAMESPACE,
        project(Some(
            json!({ "enabled": true, "brokers": 2, "connections": 2000,
            "addresses": ["platform", "dedicatedPort"], "ports": [18831] }),
        )),
    ));
    let mut ported = broker(1000);
    ported["port"] = json!(18831);
    state
        .mirror
        .upsert(envelope("MqttBroker", "cameras", "helsinki", ported));

    let answer = send(
        &state,
        person("vic"),
        "GET",
        "/api/v1/projects/helsinki/mqtt/allowance",
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["origin"], "project");
    assert_eq!(body["brokers"], json!({ "limit": 2, "used": 1, "left": 1 }));
    assert_eq!(
        body["connections"],
        json!({ "limit": 2000, "used": 1000, "left": 1000 })
    );
    assert_eq!(body["modes"], json!(["ingest"]));
    assert_eq!(
        body["portsInUse"],
        json!([{ "port": 18831, "broker": "cameras" }])
    );

    let stranger = send(
        &state,
        person("petr"),
        "GET",
        "/api/v1/projects/helsinki/mqtt/allowance",
        None,
    )
    .await;
    assert_eq!(stranger.status, StatusCode::NOT_FOUND, "{}", stranger.text);

    let espoo = send(
        &state,
        person("ada"),
        "GET",
        "/api/v1/projects/espoo/mqtt/allowance",
        None,
    )
    .await;
    let espoo: Value = serde_json::from_str(&espoo.text).expect("json");
    assert_eq!(
        (espoo["enabled"].clone(), espoo["origin"].clone()),
        (json!(false), json!("organization"))
    );
    assert_eq!(espoo["brokers"]["limit"], 0);
}

/// T-5590: a role's account is one of the project's ServiceAccounts the proposer reads. A name
/// typed past the form is refused naming its element, and an account the proposer cannot read is
/// refused in the same words as one that is not there, so the answer tells them nothing of it.
#[tokio::test]
async fn a_role_naming_an_account_that_is_not_there_or_not_readable_is_refused_by_its_element() {
    let state = state().await;
    state.mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        organization(json!({ "enabled": true, "brokers": 5, "connections": 1500 })),
    ));
    let mut typed = broker(10);
    typed["roles"][0]["serviceAccounts"] = json!(["cam", "vent-1"]);
    let (status, text) = propose_broker(&state, "vents", typed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("spec.roles[0].serviceAccounts[1]"), "{text}");
    assert!(
        text.contains("ServiceAccount 'vent-1', which does not exist"),
        "{text}"
    );

    let (status, unseen) = propose_broker_as(&state, "bea", "cams", broker(10)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{unseen}");
    assert!(
        unseen.contains("spec.roles[0].serviceAccounts[0]"),
        "{unseen}"
    );
    assert!(
        unseen.contains("ServiceAccount 'cam', which does not exist"),
        "{unseen}"
    );

    let (status, text) = propose_broker(&state, "cams", broker(10)).await;
    assert!(
        status.is_success(),
        "an account the steward reads: {status} {text}"
    );
}

/// PF-129, T-5870: a broker's allowedSources is judged at the door by jc-core: a range with host
/// bits set is refused naming the field and the network it meant, a valid list is proposed.
#[tokio::test]
async fn allowed_sources_are_refused_at_the_door_naming_the_field_when_a_range_is_bad() {
    let state = state().await;
    state.mirror.upsert(envelope(
        "Organization",
        "hel",
        ORG_NAMESPACE,
        organization(json!({ "enabled": true, "brokers": 5, "connections": 1500 })),
    ));
    let mut bad = broker(10);
    bad["allowedSources"] = json!(["203.0.113.1/24"]);
    let (status, text) = propose_broker(&state, "fenced", bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
    assert!(text.contains("spec.allowedSources"), "{text}");
    assert!(text.contains("the network is 203.0.113.0/24"), "{text}");
    for list in [json!(["0.0.0.0/0"]), json!(["sensors.city.example/24"])] {
        let mut bad = broker(10);
        bad["allowedSources"] = list;
        let (status, text) = propose_broker(&state, "fenced", bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains("spec.allowedSources"), "{text}");
    }
    let mut good = broker(10);
    good["allowedSources"] = json!(["203.0.113.0/24", "2001:db8::/48"]);
    let (status, text) = propose_broker(&state, "fenced", good).await;
    assert!(status.is_success(), "{status} {text}");
}
