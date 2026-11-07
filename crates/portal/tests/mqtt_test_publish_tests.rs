//! T-5534 (API/01 §44a): the dry run of one device message on an MQTT broker, and the watch of
//! the gateway's refusals. Nothing is published or written, only a reader of the project's
//! brokers learns of one, and each member at fault is named.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send};
use joinedcontext_portal::api::mqtt_test_publish::refusals;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

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
    state.mirror.upsert(envelope(
        "MqttBroker",
        "cameras",
        "helsinki",
        json!({
            "roles": [
                {"name": "camera", "serviceAccounts": ["cam-west"], "publish": ["cameras/${clientid}/#"]},
                {"name": "viewer", "serviceAccounts": ["dashboard"], "subscribe": ["dashboard/${clientid}/#"]},
            ],
            "limits": {"maxMessageBytes": 32}
        }),
    ));
    state
}

async fn test(state: &AppState, who: &str, body: Value) -> (StatusCode, Value) {
    let uri = "/api/v1/projects/helsinki/mqttbrokers/cameras/test-publish";
    let answer = send(state, person(who), "POST", uri, Some(body)).await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

fn outcomes(body: &Value) -> Vec<(String, String)> {
    body["steps"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .map(|step| {
                    (
                        step["step"].as_str().unwrap_or_default().to_owned(),
                        step["outcome"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_valid_message_nobody_takes_dry_runs_without_writing() {
    let state = state();
    let before = state.mirror.len();
    let (status, body) = test(
        &state,
        "jana",
        json!({"account": "cam-west", "topic": "cameras/helsinki-cam-west/frame", "payload": "{\"t\": 21.5}"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["published"], false);
    assert_eq!(body["clientId"], "helsinki-cam-west");
    let pair = |step: &str, outcome: &str| (step.to_owned(), outcome.to_owned());
    assert_eq!(
        outcomes(&body),
        [
            pair("acl", "ok"),
            pair("size", "ok"),
            pair("rate", "ok"),
            pair("binding", "refused"),
            pair("mapping", "unchecked"),
            pair("endpoint", "unchecked"),
        ]
    );
    assert_eq!(state.mirror.len(), before, "the dry run wrote nothing");
}

#[tokio::test]
async fn another_clients_topic_and_an_oversized_message_are_refused_steps() {
    let state = state();
    let (status, body) = test(
        &state,
        "jana",
        json!({"account": "cam-west", "topic": "cameras/helsinki-cam-east/frame", "payload": "x".repeat(33)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let steps = outcomes(&body);
    assert_eq!(steps[0], ("acl".to_owned(), "refused".to_owned()));
    assert_eq!(steps[1], ("size".to_owned(), "refused".to_owned()));
    let text = body.to_string();
    assert!(text.contains("cameras/helsinki-cam-west/#"), "{text}");
    assert!(text.contains("maxMessageBytes"), "{text}");
}

/// DataSource `frames` reads `cameras/+/frame` of the broker, Pipeline `frames-in` takes it.
fn bind_frames(state: &AppState) {
    state.mirror.upsert(envelope(
        "DataSource",
        "frames",
        "helsinki",
        json!({"type": "mqtt", "mqtt": {"brokerRef": {"kind": "MqttBroker", "name": "cameras"}, "topics": ["cameras/+/frame"]}}),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "frames-in",
        "helsinki",
        json!({
            "class": "resident",
            "source": {"dataSourceRef": {"kind": "DataSource", "name": "frames"}},
            "compute": {"kind": "bloblang", "bloblang": "root = this"},
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:cams:frames"
        }),
    ));
}

/// Jana may also read the project's DataSources and, when `pipelines`, its Pipelines.
fn may_read_flow(state: &AppState, pipelines: bool) {
    let kinds = if pipelines {
        json!(["DataSource", "Pipeline"])
    } else {
        json!(["DataSource"])
    };
    state.mirror.upsert(envelope(
        "Role",
        "flow-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-flow",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "flow-reader", "scope": { "project": "helsinki" } }),
    ));
}

async fn frame(state: &AppState) -> Value {
    let (status, body) = test(
        state,
        "jana",
        json!({"account": "cam-west", "topic": "cameras/helsinki-cam-west/frame", "payload": "{}"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn a_broker_reader_learns_no_source_or_pipeline_name_they_may_not_read() {
    let state = state();
    bind_frames(&state);
    let body = frame(&state).await;
    let text = body.to_string();
    assert!(
        !text.contains("frames") && !text.contains("frames-in"),
        "{text}"
    );
    assert_eq!(body["steps"][3]["outcome"], "ok", "{body}");
    assert!(body["steps"][3]["reason"]
        .as_str()
        .is_some_and(|r| r.contains("a DataSource you may not read")));
    assert_eq!(body["steps"][4]["outcome"], "unchecked", "{body}");

    // DataSource read without Pipeline read names the source and hides the pipeline.
    may_read_flow(&state, false);
    let body = frame(&state).await;
    let text = body.to_string();
    assert!(!text.contains("frames-in"), "{text}");
    assert!(body["steps"][3]["reason"]
        .as_str()
        .is_some_and(|r| r.contains("'frames'") && r.contains("a Pipeline you may not read")));
    assert_eq!(body["steps"][4]["outcome"], "unchecked", "{body}");
}

#[tokio::test]
async fn a_bound_topic_names_its_source_and_pipeline() {
    let state = state();
    bind_frames(&state);
    may_read_flow(&state, true);
    let body = frame(&state).await;
    let binding = &body["steps"][3];
    assert_eq!(binding["outcome"], "ok", "{body}");
    let reason = binding["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("'frames'") && reason.contains("'frames-in'"),
        "{reason}"
    );
    // Jana may not propose Pipelines: the mapping is not run for her, and says so.
    assert_eq!(body["steps"][4]["outcome"], "unchecked", "{body}");
    assert!(body["steps"][4]["reason"]
        .as_str()
        .is_some_and(|r| r.contains("propose")));
}

#[tokio::test]
async fn each_member_at_fault_is_named_in_a_422() {
    let state = state();
    for (body, member) in [
        (
            json!({"account": "cam-east", "topic": "cameras/x", "payload": ""}),
            "account",
        ),
        (
            json!({"account": "cam-west", "topic": "cameras/#", "payload": ""}),
            "topic",
        ),
        (
            json!({"account": "cam-west", "topic": "cameras/x", "payload": "x".repeat(1024 * 1024 + 1)}),
            "payload",
        ),
    ] {
        let (status, answer) = test(&state, "jana", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{answer}");
        let named = answer["errors"]
            .as_array()
            .is_some_and(|errors| errors.iter().any(|e| e["path"] == member));
        assert!(named, "{member}: {answer}");
    }
}

#[tokio::test]
async fn only_a_reader_of_the_brokers_tests_or_watches_one() {
    let state = state();
    let body = json!({"account": "cam-west", "topic": "cameras/x", "payload": ""});
    let (stranger, _) = test(&state, "petr", body).await;
    assert_eq!(stranger, StatusCode::NOT_FOUND);
    let watch = |who: &'static str, query: &'static str| {
        let state = state.clone();
        async move {
            let uri = format!("/api/v1/projects/helsinki/mqttbrokers/cameras/watch{query}");
            let answer = send(&state, person(who), "GET", &uri, None).await;
            (
                answer.status,
                serde_json::from_str::<Value>(&answer.text).unwrap_or(Value::Null),
            )
        }
    };
    assert_eq!(watch("petr", "").await.0, StatusCode::NOT_FOUND);
    let (status, unknown) = watch("jana", "?account=cam-east").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unknown}");
    // No gateway named on this instance: nothing is read, and the answer says why.
    let (status, seen) = watch("jana", "?account=cam-west&since=2026-10-24T10:00:00Z").await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["refusalsRead"], false);
    assert_eq!(seen["refusals"], json!([]));
    assert!(seen["reason"]
        .as_str()
        .is_some_and(|r| r.contains("JC_PORTAL_GATEWAY_URL")));
}

#[tokio::test]
async fn the_gateways_record_reads_as_refusals_and_a_refusal_to_read_says_so() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/internal/mqtt/refusals/mqtt-helsinki-cameras"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"refusals": [
                {"at": "2026-10-24T10:02:00Z", "reason": "expired", "account": "cam-west"}
            ]})),
        )
        .mount(&server)
        .await;
    let kept = refusals(&server.uri(), "mqtt-helsinki-cameras")
        .await
        .expect("read");
    assert_eq!(kept.len(), 1);
    assert_eq!(
        (kept[0].reason.as_str(), kept[0].account.as_deref()),
        ("expired", Some("cam-west"))
    );
    let refused = refusals(&server.uri(), "mqtt-helsinki-other")
        .await
        .expect_err("404");
    assert!(refused.contains("404"), "{refused}");
}
