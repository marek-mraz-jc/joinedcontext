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

/// A broker answering `status` with `body` to the question read and to the decision.
async fn answering(status: u16, body: &str) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(path(ASKED))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn a_broker_without_the_question_is_a_404_and_a_refusing_or_garbled_one_a_503() {
    let missing = answering(404, "").await;
    let read = send(
        &state(&missing.uri(), None, "missing"),
        person("ana"),
        "GET",
        PAGE,
        None,
    )
    .await;
    assert_eq!(read.status, StatusCode::NOT_FOUND, "{}", read.text);
    assert!(read.text.contains("no open question"), "{}", read.text);

    for (status, body, tag) in [(500, "", "refused"), (200, "not json", "garbled")] {
        let broker = answering(status, body).await;
        let read = send(
            &state(&broker.uri(), None, tag),
            person("ana"),
            "GET",
            PAGE,
            None,
        )
        .await;
        assert_eq!(
            read.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{tag}: {}",
            read.text
        );
        assert!(read.text.contains("try again in a minute"), "{}", read.text);
    }
}

#[tokio::test]
async fn an_answer_that_breaks_off_mid_body_is_a_503() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"id\":")
            .await;
    });
    let state = state(&format!("http://{address}"), None, "broken");
    let read = send(&state, person("ana"), "GET", PAGE, None).await;
    assert_eq!(
        read.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        read.text
    );
}

#[tokio::test]
async fn no_broker_address_is_a_404_and_no_admin_token_a_503_before_any_call() {
    let mut config = Config::for_tests();
    config.broker_url = None;
    config.broker_url_template = None;
    let none = AppState::new(config, None);
    let read = send(&none, person("ana"), "GET", PAGE, None).await;
    assert_eq!(read.status, StatusCode::NOT_FOUND, "{}", read.text);

    let broker = MockServer::start().await;
    holding(&broker).await;
    let mut config = Config::for_tests();
    config.broker_url = Some(broker.uri());
    config.broker_admin_token_file = Some(std::env::temp_dir().join("mcp-confirm-no-such-token"));
    let tokenless = AppState::new(config, None);
    let read = send(&tokenless, person("ana"), "GET", PAGE, None).await;
    assert_eq!(
        read.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        read.text
    );
    assert!(
        broker
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "no call goes out without the admin token"
    );
}

#[tokio::test]
async fn a_decision_the_broker_no_longer_holds_is_a_404_and_a_refused_one_a_503() {
    for (status, expected, tag) in [
        (404, StatusCode::NOT_FOUND, "spent"),
        (500, StatusCode::SERVICE_UNAVAILABLE, "put-refused"),
    ] {
        let broker = MockServer::start().await;
        holding(&broker).await;
        Mock::given(method("PUT"))
            .and(path(ASKED))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&broker)
            .await;
        let decided = send(
            &state(&broker.uri(), None, tag),
            person("ana"),
            "POST",
            &format!("{PAGE}/decision"),
            Some(json!({"decision": "decline"})),
        )
        .await;
        assert_eq!(decided.status, expected, "{tag}: {}", decided.text);
    }
}

/// T-5699: a Portal MCP call's own question, held in the Portal. The person whose client asked
/// reads and decides it in the project it was asked in; anyone else, another project and a spent
/// id are refused (a client's token: the lib tests of api::mcp_elicitations).
#[tokio::test]
async fn the_portal_question_is_read_and_decided_by_its_asker_in_its_project_only() {
    let state = state("http://127.0.0.1:9", None, "portal-q");
    let id = state.mcp_elicitations.ask(
        "sub-ana",
        "jc_resource_delete",
        "city",
        "digest-1",
        "Delete Endpoint air in project city.",
        "Endpoint/air",
    );
    let page = format!("/api/v1/projects/city/mcp-elicitations/{id}");

    let read = send(&state, person("ana"), "GET", &page, None).await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text);
    let shown: Value = serde_json::from_str(&read.text).expect("json");
    assert_eq!(shown["id"], json!(id));
    assert!(shown["decision"].is_null(), "{shown}");

    for (who, uri) in [
        (person("bo"), page.clone()),
        (
            person("ana"),
            format!("/api/v1/projects/other/mcp-elicitations/{id}"),
        ),
        (
            person("ana"),
            "/api/v1/projects/city/mcp-elicitations/eli-0000000000000000".to_owned(),
        ),
    ] {
        let refused = send(&state, who.clone(), "GET", &uri, None).await;
        assert_eq!(
            refused.status,
            StatusCode::NOT_FOUND,
            "{uri}: {}",
            refused.text
        );
        assert!(
            refused.text.contains("no open question"),
            "{}",
            refused.text
        );
        let decided = send(
            &state,
            who,
            "POST",
            &format!("{uri}/decision"),
            Some(json!({"decision": "accept"})),
        )
        .await;
        assert_eq!(
            decided.status,
            StatusCode::NOT_FOUND,
            "{uri}: {}",
            decided.text
        );
    }

    let garbled = send(
        &state,
        person("ana"),
        "POST",
        &format!("{page}/decision"),
        Some(json!({"decision": "maybe"})),
    )
    .await;
    assert_eq!(garbled.status, StatusCode::BAD_REQUEST, "{}", garbled.text);

    let decided = send(
        &state,
        person("ana"),
        "POST",
        &format!("{page}/decision"),
        Some(json!({"decision": "decline"})),
    )
    .await;
    assert_eq!(decided.status, StatusCode::OK, "{}", decided.text);
    assert_eq!(
        serde_json::from_str::<Value>(&decided.text).expect("json")["decision"],
        "decline"
    );
    assert_eq!(
        state
            .mcp_elicitations
            .question("sub-ana", "city", &id)
            .map(|q| serde_json::to_value(q).expect("json")["decision"].clone()),
        Some(json!("decline")),
        "the decision is held for the client's second call"
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
