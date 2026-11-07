//! T-4043 (API/01 §44, ADR-N-048 §2.8): the Portal reads what an MQTT broker measures from the
//! broker's own HTTP API, here a recorded RMQTT 0.24.0 answer; a broker that does not answer is
//! "not measured", and a caller who may not read the project's brokers learns nothing of one.

mod common;

use axum::http::StatusCode;
use jc_core::kinds::MqttBrokerSpec;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::api::mqtt_measured::measure;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const HISTORY: &str = include_str!("fixtures/rmqtt/metrics-history-sum.json");
const CLIENTS: &str = include_str!("fixtures/rmqtt/clients.json");

fn spec() -> Value {
    json!({
        "roles": [
            {"name": "camera", "serviceAccounts": ["cam-west", "cam-east"], "publish": ["cameras/${clientid}/#"]},
            {"name": "viewer", "serviceAccounts": ["dashboard"], "subscribe": ["dashboard/${clientid}/#"]},
        ],
    })
}

fn parsed() -> MqttBrokerSpec {
    serde_json::from_value(spec()).expect("a broker reads")
}

async fn rmqtt(history: ResponseTemplate, clients: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/metrics/history/sum"))
        .respond_with(history)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/clients"))
        .and(query_param("_limit", "1000"))
        .respond_with(clients)
        .mount(&server)
        .await;
    server
}

fn json_body(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(text, "application/json")
}

#[tokio::test]
async fn a_recorded_rmqtt_answer_reads_as_rate_publishers_and_accounts() {
    let server = rmqtt(json_body(HISTORY), json_body(CLIENTS)).await;
    let seen = serde_json::to_value(measure(&server.uri(), "helsinki", &parsed()).await).unwrap();
    assert_eq!(seen["measured"], true, "{seen}");
    // 311 and 301 published five seconds apart.
    assert_eq!(seen["messagesPerSecond"], 2.0);
    assert_eq!(seen["publishers"], 1);
    assert_eq!(seen["reason"], Value::Null);
    // T-6103: the page's chart, one rate per neighbouring pair of samples, oldest first.
    assert_eq!(
        seen["rates"],
        json!([
            {"at": "2026-10-12T08:47:00Z", "perSecond": 2.0},
            {"at": "2026-10-12T08:47:05Z", "perSecond": 2.0},
        ])
    );
    assert_eq!(seen["refusedPublishes"], 0);
    assert_eq!(
        seen["accounts"],
        json!([
            {"serviceAccount": "cam-east", "connected": 0, "since": null},
            {"serviceAccount": "cam-west", "connected": 1, "since": "2026-10-12T08:46:51Z"},
            {"serviceAccount": "dashboard", "connected": 0, "since": null},
        ])
    );
    // No client id, address or topic of the broker's answer leaves the Portal.
    let text = seen.to_string();
    for private in ["cam-west-1", "172.19.0", "dashboard-1"] {
        assert!(!text.contains(private), "{private} in {text}");
    }
}

#[tokio::test]
async fn a_broker_that_does_not_answer_is_not_measured_and_says_why() {
    // A port nobody listens on: bound, then let go.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let gone = format!("http://127.0.0.1:{port}");
    let seen = measure(&gone, "helsinki", &parsed()).await;
    assert!(!seen.measured);
    assert_eq!((seen.messages_per_second, seen.publishers), (None, None));
    assert!(seen.accounts.is_empty());
    assert!(seen.reason.is_some_and(|r| r.contains("did not answer")));
}

#[tokio::test]
async fn a_refusal_or_an_answer_not_of_the_api_is_not_measured() {
    let refused = rmqtt(json_body(HISTORY), ResponseTemplate::new(403)).await;
    let seen = measure(&refused.uri(), "helsinki", &parsed()).await;
    assert!(!seen.measured);
    assert!(
        seen.reason.as_deref().is_some_and(|r| r.contains("403")),
        "{seen:?}"
    );

    let page = ResponseTemplate::new(200).set_body_raw("<html>login</html>", "text/html");
    let other = rmqtt(page, json_body(CLIENTS)).await;
    let seen = measure(&other.uri(), "helsinki", &parsed()).await;
    assert!(!seen.measured);
    assert_eq!(seen.publishers, None);
}

fn state() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    state.mirror.upsert(envelope(
        "Role",
        "broker-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["MqttBroker"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-reader",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "broker-reader", "scope": { "project": "helsinki" } }),
    ));
    state
        .mirror
        .upsert(envelope("MqttBroker", "cameras", "helsinki", spec()));
    state
}

async fn get(state: &AppState, who: &str, name: &str) -> (StatusCode, Value) {
    let uri = format!("/api/v1/projects/helsinki/mqttbrokers/{name}/measured");
    let answer = send(state, person(who), "GET", &uri, None).await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn only_a_reader_of_the_brokers_learns_of_one() {
    let state = state();
    // No brokers namespace on this instance: the reader is told why nothing is measured.
    let (status, body) = get(&state, "jana", "cameras").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["measured"], false);
    assert!(body["reason"]
        .as_str()
        .is_some_and(|r| r.contains("JC_PORTAL_MQTT_NAMESPACE")));

    // Somebody who may not read the project's brokers gets what a missing broker gets.
    let (stranger, refused) = get(&state, "petr", "cameras").await;
    let (missing, absent) = get(&state, "jana", "nope").await;
    assert_eq!(
        (stranger, missing),
        (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND)
    );
    assert_eq!(refused["status"], absent["status"]);
}
