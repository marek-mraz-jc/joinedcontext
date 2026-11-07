//! T-4880 (API/01 §47, ADR-N-048 §2.10, PL-87): a capture broker's kept messages reach a person
//! who may approve MqttBrokers in the project and nobody else; a reader of the manifest, a
//! steward of another project and a broker that does not exist all answer the same 404. A
//! delete removes the capture at once and is recorded without a payload.

mod common;

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::activity::ActivityFilter;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::mqtt_capture::Message;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

fn broker(mode: &str) -> Value {
    json!({
        "mode": mode,
        "roles": [{"name": "camera", "serviceAccounts": ["cam-west"], "publish": ["cams/${clientid}/frame"]}],
    })
}

async fn state() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    for (role, verbs) in [
        ("broker-reader", json!(["read"])),
        ("broker-steward", json!(["read", "approve"])),
    ] {
        state.mirror.upsert(envelope(
            "Role",
            role,
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["MqttBroker"], "verbs": verbs }] }),
        ));
    }
    for (binding, who, role, project) in [
        ("jana-reader", "jana", "broker-reader", "helsinki"),
        ("eva-steward", "eva", "broker-steward", "helsinki"),
        ("olli-steward", "olli", "broker-steward", "espoo"),
    ] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            binding,
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": format!("{who}@hel.fi") }], "role": role, "scope": { "project": project } }),
        ));
    }
    let mut capture = broker("capture");
    capture["capture"] = json!({"messagesPerTopic": 5, "window": "PT1H"});
    state
        .mirror
        .upsert(envelope("MqttBroker", "cameras", "helsinki", capture));
    state.mirror.upsert(envelope(
        "MqttBroker",
        "plain",
        "helsinki",
        broker("ingest"),
    ));
    let now = Utc::now();
    let message = |topic: &str, client: &str, ago: i64, payload: &[u8]| Message {
        topic: topic.to_owned(),
        client_id: Some(client.to_owned()),
        received_at: now - Duration::seconds(ago),
        expires_at: now - Duration::seconds(ago) + Duration::hours(1),
        payload: payload.to_vec(),
    };
    state
        .mqtt_capture
        .keep(
            "helsinki",
            "cameras",
            5,
            vec![
                message("cams/cam-west/frame", "cam-west", 30, br#"{"seq": 1}"#),
                message("cams/cam-east/frame", "cam-east", 20, &[0xff, 0x00]),
                message("cams/cam-west/frame", "cam-west", 10, br#"{"seq": 2}"#),
            ],
            now,
        )
        .await
        .expect("kept");
    state
}

async fn call(state: &AppState, who: &str, method: &str, query: &str) -> (StatusCode, Value) {
    let (name, query) = query.split_once('?').unwrap_or((query, ""));
    let uri = format!("/api/v1/projects/helsinki/mqttbrokers/{name}/captured?{query}");
    let answer = send(state, person(who), method, &uri, None).await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_steward_reads_the_newest_messages_first_with_text_or_base64() {
    let state = state().await;
    let (status, body) = call(&state, "eva", "GET", "cameras").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (
            &body["broker"],
            &body["mode"],
            &body["window"],
            &body["messagesPerTopic"]
        ),
        (
            &json!("cameras"),
            &json!("capture"),
            &json!("PT1H"),
            &json!(5)
        )
    );
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["payload"], r#"{"seq": 2}"#);
    assert_eq!(messages[0]["encoding"], "utf-8");
    assert_eq!(messages[0]["clientId"], "cam-west");
    assert_eq!(messages[1]["payload"], "/wA=");
    assert_eq!(messages[1]["encoding"], "base64");
    assert_eq!(messages[1]["bytes"], 2);
    assert_eq!(body["truncated"], false);

    let (_, one) = call(&state, "eva", "GET", "cameras?limit=1").await;
    assert_eq!(one["messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(one["truncated"], true);
    let (_, east) = call(&state, "eva", "GET", "cameras?topic=cams/cam-east/frame").await;
    assert_eq!(east["messages"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn a_broker_outside_capture_answers_its_mode_and_nothing_kept() {
    let state = state().await;
    let (status, body) = call(&state, "eva", "GET", "plain").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "ingest");
    assert_eq!(body["messages"], json!([]));
    assert!(body.get("window").is_none(), "{body}");
}

#[tokio::test]
async fn a_reader_another_projects_steward_and_an_unknown_broker_get_the_same_404() {
    let state = state().await;
    let (_, unknown) = call(&state, "eva", "GET", "nope").await;
    for (who, method) in [
        ("jana", "GET"),
        ("jana", "DELETE"),
        ("olli", "GET"),
        ("olli", "DELETE"),
        ("nobody", "GET"),
    ] {
        let (status, body) = call(&state, who, method, "cameras").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} {method}: {body}");
        assert_eq!(
            body["detail"]
                .as_str()
                .map(|d| d.replace("cameras", "nope")),
            unknown["detail"].as_str().map(str::to_owned),
            "{who} {method}: the answer tells a broker that exists from one that does not"
        );
    }
    // Nothing was deleted by the refused callers.
    let (_, body) = call(&state, "eva", "GET", "cameras").await;
    assert_eq!(body["messages"].as_array().map(Vec::len), Some(3));
}

#[tokio::test]
async fn a_wildcard_topic_or_a_limit_out_of_range_is_refused_naming_it() {
    let state = state().await;
    for query in ["cameras?limit=0", "cameras?limit=1001"] {
        let (status, body) = call(&state, "eva", "GET", query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
        assert!(
            body["detail"].as_str().is_some_and(|d| d.contains("limit")),
            "{body}"
        );
    }
    for query in ["cameras?topic=cams/%2B/frame", "cameras?topic=cams/%23"] {
        let (status, body) = call(&state, "eva", "GET", query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
        assert!(
            body["detail"].as_str().is_some_and(|d| d.contains("topic")),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_delete_removes_the_capture_and_records_it_without_payloads() {
    let state = state().await;
    let (status, _) = call(&state, "eva", "DELETE", "cameras").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = call(&state, "eva", "GET", "cameras").await;
    assert_eq!(body["messages"], json!([]));

    let page = state
        .activity
        .list("helsinki", &ActivityFilter::default())
        .await
        .expect("the activity reads");
    let recorded = serde_json::to_string(&page.items).expect("serializes");
    assert!(recorded.contains("mqtt.capture.deleted"), "{recorded}");
    assert!(recorded.contains("3 messages"), "{recorded}");
    for payload in ["seq", "/wA="] {
        assert!(!recorded.contains(payload), "{payload} in {recorded}");
    }
}
