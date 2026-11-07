//! T-5829 (Antares T-5816): the Portal decision page of a broker's MCP write. The Portal reads
//! the question from the broker that holds it, shows it only to the subject whose call asked, and
//! records the decision on the broker in the name of the session's subject, never a name the
//! request carries.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const ID: &str = "0a1b2c3d4e5f60718293a4b5c6d7e8f9";
const PAGE: &str = "/api/v1/mcp-confirmations/0a1b2c3d4e5f60718293a4b5c6d7e8f9";
const ASKED: &str = "/q/mcp-confirmations/0a1b2c3d4e5f60718293a4b5c6d7e8f9";

fn token(tag: &str) -> std::path::PathBuf {
    let file = std::env::temp_dir().join(format!("mcp-confirm-{}-{tag}", std::process::id()));
    std::fs::write(&file, "admin-token\n").expect("write the token");
    file
}

/// A Portal reaching `main` at `main` and every declared broker at `template`.
fn state(main: &str, template: Option<String>, tag: &str) -> AppState {
    let mut config = Config::for_tests();
    config.broker_url = Some(main.to_owned());
    config.broker_url_template = template;
    config.broker_admin_token_file = Some(token(tag));
    AppState::new(config, None)
}

fn question(decision: Value) -> Value {
    json!({
        "id": ID,
        "view": "bb-air",
        "tool": "create_entity",
        "message": "Create the Entity urn:ngsi-ld:AirQualityObserved:1 in view bb-air.",
        "subject": {"x-jc-subject": ["sub-ana"], "x-jc-groups": ["city"]},
        "expiresInSeconds": 540,
        "decision": decision,
    })
}

async fn holding(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(ASKED))
        .and(header("authorization", "Bearer admin-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(question(Value::Null)))
        .mount(server)
        .await;
}

async fn puts(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .map(|r| serde_json::from_slice(&r.body).expect("json"))
        .collect()
}

#[tokio::test]
async fn the_asking_person_reads_the_question_and_their_accept_is_recorded_in_their_name() {
    let broker = MockServer::start().await;
    holding(&broker).await;
    Mock::given(method("PUT"))
        .and(path(ASKED))
        .and(header("authorization", "Bearer admin-token"))
        .and(body_json(
            json!({"decision": "accept", "subject": {"x-jc-subject": "sub-ana"}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(question(json!("accept"))))
        .expect(1)
        .mount(&broker)
        .await;
    let state = state(&broker.uri(), None, "accept");

    let read = send(&state, person("ana"), "GET", PAGE, None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    let shown: Value = serde_json::from_str(&read.text).expect("json");
    assert_eq!(shown["view"], "bb-air");
    assert_eq!(shown["tool"], "create_entity");
    assert_eq!(shown["decision"], Value::Null);
    assert!(
        shown.get("subject").is_none(),
        "the subject headers stay on the server"
    );

    let decided = send(
        &state,
        person("ana"),
        "POST",
        &format!("{PAGE}/decision"),
        Some(json!({"decision": "accept"})),
    )
    .await;
    assert_eq!(decided.status, StatusCode::OK, "{}", decided.text);
    assert_eq!(
        serde_json::from_str::<Value>(&decided.text).expect("json")["decision"],
        "accept"
    );
}

#[tokio::test]
async fn another_person_gets_the_same_404_as_an_unknown_id_and_decides_nothing() {
    let broker = MockServer::start().await;
    holding(&broker).await;
    let state = state(&broker.uri(), None, "other");

    let read = send(&state, person("bo"), "GET", PAGE, None).await;
    assert_eq!(read.status, StatusCode::NOT_FOUND, "{}", read.text);
    let decided = send(
        &state,
        person("bo"),
        "POST",
        &format!("{PAGE}/decision"),
        Some(json!({"decision": "accept"})),
    )
    .await;
    assert_eq!(decided.status, StatusCode::NOT_FOUND, "{}", decided.text);
    assert!(puts(&broker).await.is_empty(), "nothing reached the broker");
}

#[tokio::test]
async fn a_subject_in_the_body_is_refused_and_never_sent() {
    let broker = MockServer::start().await;
    holding(&broker).await;
    let state = state(&broker.uri(), None, "forged");

    let forged = send(
        &state,
        person("bo"),
        "POST",
        &format!("{PAGE}/decision"),
        Some(json!({"decision": "accept", "subject": {"x-jc-subject": "sub-ana"}})),
    )
    .await;
    assert_eq!(forged.status, StatusCode::BAD_REQUEST, "{}", forged.text);
    assert!(puts(&broker).await.is_empty());
}

#[tokio::test]
async fn the_question_is_found_on_a_declared_broker_and_a_bad_id_asks_none() {
    let main = MockServer::start().await;
    let canary = MockServer::start().await;
    holding(&canary).await;
    let template = canary.uri().replace("127.0.0.1", "{broker}");
    let state = state(&main.uri(), Some(template), "canary");
    let image = format!("ghcr.io/x/antares@sha256:{}", "a".repeat(64));
    state.mirror.upsert(envelope(
        "ContextBroker",
        "localhost",
        ORG_NAMESPACE,
        json!({ "image": image }),
    ));

    let read = send(&state, person("ana"), "GET", PAGE, None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);

    let bad = send(
        &state,
        person("ana"),
        "GET",
        "/api/v1/mcp-confirmations/..%2Fviews",
        None,
    )
    .await;
    assert_eq!(bad.status, StatusCode::NOT_FOUND, "{}", bad.text);
    assert_eq!(
        canary.received_requests().await.unwrap_or_default().len(),
        1,
        "only the well-formed id was asked"
    );
}

#[tokio::test]
async fn a_broker_that_does_not_answer_is_a_503_not_a_missing_question() {
    let state = state("http://127.0.0.1:9", None, "down");
    let read = send(&state, person("ana"), "GET", PAGE, None).await;
    assert_eq!(
        read.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        read.text
    );
}

#[tokio::test]
async fn an_oversized_broker_answer_is_a_503_never_read_whole() {
    let server = MockServer::start().await;
    let mut huge = question(Value::Null);
    huge["message"] = json!("x".repeat(300 * 1024));
    Mock::given(method("GET"))
        .and(path(ASKED))
        .respond_with(ResponseTemplate::new(200).set_body_json(huge))
        .mount(&server)
        .await;
    let state = state(&server.uri(), None, "huge");
    let read = send(&state, person("ana"), "GET", PAGE, None).await;
    assert_eq!(
        read.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        read.text
    );
}
