//! `GET`/`POST /api/v1/projects/{project}/mqttbrokers/{name}/devices` against a mocked forge
//! (T-5532, PF-47, ADR-N-048 §5).
//!
//! Adding a device proposes one Change of two files, the ServiceAccount with the device's public
//! keys and the broker role that lists it, and it waits for a second person. A private key member,
//! a role or a topic the broker does not hold is refused with a 422 naming the field, before
//! anything reaches the forge; a person who may only read is refused; the form's role list holds
//! only the broker's own roles, and none for a caller the `POST` would refuse.

mod common;

use common::{envelope, forge, person, send, state_on, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::MockServer;

const PROJECT: &str = "helsinki";
const OTHER: &str = "espoo";

/// A P-256 public key as WebCrypto exports it: x and y of 32 bytes, base64url.
fn public_key() -> Value {
    json!({ "kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256", "use": "sig",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0" })
}

fn device(jwks: Value) -> Value {
    json!({ "name": "sensor-001", "role": "camera", "topic": "cameras/${clientid}/#", "jwks": jwks })
}

/// The project with broker `edge` in `helsinki` and another `edge` in `espoo` with its own roles;
/// `steward` proposes ServiceAccounts and MqttBrokers here, `viewer` reads.
async fn state() -> (AppState, MockServer) {
    let gitea = forge().await;
    let state = state_on(&gitea);
    let org = ORG_NAMESPACE;
    let read = json!({ "kinds": ["MqttBroker", "ServiceAccount"], "verbs": ["read"] });
    for manifest in [
        json!({ "apiVersion": API_VERSION, "kind": "Organization", "metadata": { "name": "hel", "namespace": org }, "spec": { "displayName": { "en": "Helsinki" }, "domain": "hel.fi" } }),
        json!({ "apiVersion": API_VERSION, "kind": "Project", "metadata": { "name": PROJECT, "namespace": org }, "spec": { "organizationRef": "hel", "quotas": { "mqtt": { "enabled": true, "brokers": 2, "connections": 2000 } } } }),
        json!({ "apiVersion": API_VERSION, "kind": "Project", "metadata": { "name": OTHER, "namespace": org }, "spec": { "organizationRef": "hel" } }),
    ] {
        state
            .mirror
            .upsert(serde_json::from_value(manifest).expect("manifest"));
    }
    state
        .mirror
        .upsert(envelope("Role", "reader", org, json!({ "rules": [read] })));
    state.mirror.upsert(envelope(
        "Role",
        "device-steward",
        org,
        json!({ "rules": [read, { "kinds": ["MqttBroker", "ServiceAccount"], "verbs": ["propose"] }] }),
    ));
    for (who, role, project) in [
        ("viewer", "reader", PROJECT),
        ("steward", "device-steward", PROJECT),
        ("steward", "device-steward", OTHER),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{who}-{project}"),
            org,
            json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": { "project": project } }),
        ));
    }
    state
        .mirror
        .upsert(envelope("ServiceAccount", "cam", PROJECT, json!({})));
    state.mirror.upsert(envelope(
        "MqttBroker",
        "edge",
        PROJECT,
        json!({ "roles": [
            { "name": "camera", "serviceAccounts": ["cam"], "publish": ["cameras/${clientid}/#"] },
            { "name": "gate", "serviceAccounts": ["cam"], "publish": ["gates/${clientid}/#"] },
        ] }),
    ));
    state.mirror.upsert(envelope(
        "MqttBroker",
        "edge",
        OTHER,
        json!({ "roles": [{ "name": "espoo-only", "serviceAccounts": ["cam"], "publish": ["espoo/#"] }] }),
    ));
    (state, gitea)
}

fn devices(project: &str) -> String {
    format!("/api/v1/projects/{project}/mqttbrokers/edge/devices")
}

/// The files the forge was asked to write, as `(path, decoded YAML)`.
async fn written(gitea: &MockServer) -> Vec<(String, Value)> {
    common::committed_files(gitea, REPO)
        .await
        .into_iter()
        .map(|(path, text)| (path, serde_norway::from_str(&text).expect("yaml")))
        .collect()
}

async fn requests_to(gitea: &MockServer, suffix: &str) -> usize {
    gitea
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path().starts_with(REPO) && r.url.path().ends_with(suffix))
        .count()
}

#[tokio::test]
async fn a_device_is_one_change_of_its_public_key_and_the_role_that_lists_it() {
    let (state, gitea) = state().await;
    let answer = send(
        &state,
        person("steward"),
        "POST",
        &devices(PROJECT),
        Some(device(json!({ "keys": [public_key()] }))),
    )
    .await;
    assert_eq!(answer.status, 202, "{}", answer.text);
    let change: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(
        change["status"]["phase"], "PendingApproval",
        "a second person approves: {change}"
    );

    let files = written(&gitea).await;
    assert_eq!(files.len(), 2, "{files:?}");
    let of = |kind: &str| {
        files
            .iter()
            .find(|(_, f)| f["kind"] == kind)
            .map(|(_, f)| f.clone())
            .expect(kind)
    };
    let account = of("ServiceAccount");
    assert_eq!(account["metadata"]["name"], "sensor-001");
    assert_eq!(account["spec"]["owner"]["user"], "steward");
    assert_eq!(account["spec"]["credentials"][0]["kind"], "oauth-client");
    assert_eq!(
        account["spec"]["credentials"][0]["jwks"]["keys"][0]["kid"],
        "k1"
    );
    let broker = of("MqttBroker");
    assert_eq!(
        broker["spec"]["roles"][0]["serviceAccounts"],
        json!(["cam", "sensor-001"])
    );
    assert_eq!(
        broker["spec"]["roles"][1]["serviceAccounts"],
        json!(["cam"]),
        "only the role picked"
    );
    assert_eq!(requests_to(&gitea, "/pulls").await, 1, "one merge request");
    assert_eq!(
        requests_to(&gitea, "/merge").await,
        0,
        "never merged by its proposer"
    );
}

#[tokio::test]
async fn a_private_key_member_is_refused_naming_it_and_nothing_reaches_the_forge() {
    let (state, gitea) = state().await;
    let mut key = public_key();
    key["d"] = json!("jpsQnnGQmL-YBIffH1136cspYG6-0iY7X1fCE9-E9LI");
    let answer = send(
        &state,
        person("steward"),
        "POST",
        &devices(PROJECT),
        Some(device(json!({ "keys": [key] }))),
    )
    .await;
    assert_eq!(answer.status, 422, "{}", answer.text);
    assert!(answer.text.contains("jwks.keys[0].d"), "{}", answer.text);
    assert!(
        !answer.text.contains("jpsQnnGQ"),
        "the private value is never echoed: {}",
        answer.text
    );
    assert!(written(&gitea).await.is_empty());
}

#[tokio::test]
async fn a_symmetric_or_malformed_key_set_is_refused_naming_jwks() {
    let (state, _gitea) = state().await;
    for jwks in [
        json!({ "keys": [{ "kty": "oct", "kid": "s", "k": "c2VjcmV0" }] }),
        json!([public_key()]),
        json!({ "keys": [{ "kty": "EC", "crv": "P-384", "kid": "k1", "x": "a", "y": "b" }] }),
    ] {
        let answer = send(
            &state,
            person("steward"),
            "POST",
            &devices(PROJECT),
            Some(device(jwks.clone())),
        )
        .await;
        assert_eq!(answer.status, 422, "{jwks}: {}", answer.text);
        assert!(answer.text.contains("jwks"), "{}", answer.text);
    }
}

#[tokio::test]
async fn a_topic_or_role_outside_the_broker_is_refused_naming_the_field() {
    let (state, gitea) = state().await;
    let keys = json!({ "keys": [public_key()] });
    let mut outside = device(keys.clone());
    outside["topic"] = json!("#");
    let answer = send(
        &state,
        person("steward"),
        "POST",
        &devices(PROJECT),
        Some(outside),
    )
    .await;
    assert_eq!(answer.status, 422, "{}", answer.text);
    assert!(answer.names_field("topic", ""), "{}", answer.text);

    let mut foreign = device(keys.clone());
    foreign["role"] = json!("espoo-only");
    let answer = send(
        &state,
        person("steward"),
        "POST",
        &devices(PROJECT),
        Some(foreign),
    )
    .await;
    assert_eq!(
        answer.status, 422,
        "a role of another project's broker: {}",
        answer.text
    );
    assert!(answer.names_field("role", ""), "{}", answer.text);

    let mut name = device(keys);
    name["name"] = json!("Sensor_1");
    let answer = send(
        &state,
        person("steward"),
        "POST",
        &devices(PROJECT),
        Some(name),
    )
    .await;
    assert_eq!(answer.status, 422, "{}", answer.text);
    assert!(answer.names_field("name", ""), "{}", answer.text);
    assert!(written(&gitea).await.is_empty());
}

#[tokio::test]
async fn an_existing_account_name_is_a_conflict() {
    let (state, _gitea) = state().await;
    let mut taken = device(json!({ "keys": [public_key()] }));
    taken["name"] = json!("cam");
    let answer = send(
        &state,
        person("steward"),
        "POST",
        &devices(PROJECT),
        Some(taken),
    )
    .await;
    assert_eq!(answer.status, 409, "{}", answer.text);
}

#[tokio::test]
async fn a_person_who_may_only_read_is_refused_and_offered_no_role() {
    let (state, gitea) = state().await;
    let answer = send(
        &state,
        person("viewer"),
        "POST",
        &devices(PROJECT),
        Some(device(json!({ "keys": [public_key()] }))),
    )
    .await;
    assert_eq!(answer.status, 403, "{}", answer.text);
    assert!(written(&gitea).await.is_empty());

    let options = send(&state, person("viewer"), "GET", &devices(PROJECT), None).await;
    assert_eq!(options.status, 200, "{}", options.text);
    let options: Value = serde_json::from_str(&options.text).expect("json");
    assert_eq!(options["mayAdd"], false);
    assert_eq!(options["roles"], json!([]));
}

#[tokio::test]
async fn the_role_list_holds_this_brokers_roles_and_no_other_projects() {
    let (state, _gitea) = state().await;
    let options = send(&state, person("steward"), "GET", &devices(PROJECT), None).await;
    assert_eq!(options.status, 200, "{}", options.text);
    let options: Value = serde_json::from_str(&options.text).expect("json");
    assert_eq!(options["mayAdd"], true);
    assert_eq!(
        options["roles"],
        json!([{ "name": "camera", "publish": ["cameras/${clientid}/#"] }, { "name": "gate", "publish": ["gates/${clientid}/#"] }])
    );

    let stranger = send(&state, person("nobody"), "GET", &devices(PROJECT), None).await;
    assert_eq!(
        stranger.status, 404,
        "a caller who may not read the brokers: {}",
        stranger.text
    );
}
