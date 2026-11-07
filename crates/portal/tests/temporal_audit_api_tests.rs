//! The temporal audit of a space (T-4844): every pipeline the caller may read that writes the
//! space, each attribute its mapping writes, how the space records it and a verdict. A caller
//! without read on the space is told it is not there and the forge is never asked.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const AUDIT: &str = "/api/v1/projects/helsinki/spaces/transport/temporal-audit";
const FILE: &str = r#"pipeline:
  processors:
    - mapping: |
        root = {
          "id": $id,
          "type": "Vehicle",
          "speed": { "type": "Property", "value": this.spd, "unitCode": "MTS", "observedAt": this.tst },
          "mode": { "type": "Property", "value": this.mode }
        }
"#;

async fn world(file: ResponseTemplate) -> (AppState, MockServer) {
    let gitea = common::forge().await;
    Mock::given(method("GET"))
        .and(path_regex(format!(
            "^{REPO}/contents/projects/helsinki/.*pipelines/vehicles/bento.yaml$"
        )))
        .respond_with(file)
        .mount(&gitea)
        .await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline", "ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-space-reader",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "space-reader",
            "scope": { "project": "helsinki" },
        }),
    ));
    for space in ["transport", "events"] {
        state.mirror.upsert(envelope(
            "ContextSpace",
            space,
            "helsinki",
            json!({ "retention": [{ "entityType": "Vehicle", "history": "PT1H" }] }),
        ));
        state.mirror.upsert(envelope(
            "Endpoint",
            space,
            "helsinki",
            json!({ "contextSpaceRef": space }),
        ));
    }
    let pipeline = |endpoint: &str| json!({ "class": "auto", "targetEndpoint": format!("urn:ngsi-ld:Endpoint:hel.fi:helsinki:{endpoint}") });
    state.mirror.upsert(envelope(
        "Pipeline",
        "vehicles",
        "helsinki",
        pipeline("transport"),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "linked-events",
        "helsinki",
        pipeline("events"),
    ));
    (state, gitea)
}

fn file_answer() -> ResponseTemplate {
    use base64::Engine as _;
    ResponseTemplate::new(200).set_body_json(json!({
        "sha": "sha-bento-1",
        "content": base64::engine::general_purpose::STANDARD.encode(FILE),
    }))
}

fn rows(answer: &Value) -> Vec<(String, String, String, String)> {
    answer["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let s = |k: &str| r[k].as_str().unwrap_or_default().to_owned();
            (s("pipeline"), s("attribute"), s("record"), s("verdict"))
        })
        .collect()
}

#[tokio::test]
async fn a_reader_gets_a_row_per_attribute_of_the_pipelines_that_write_the_space() {
    let (state, _gitea) = world(file_answer()).await;
    let answer = send(&state, person("jana"), "GET", AUDIT, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    let pair = |p: &str, a: &str, r: &str, v: &str| {
        (p.to_owned(), a.to_owned(), r.to_owned(), v.to_owned())
    };
    assert_eq!(
        rows(&body),
        [
            pair("vehicles", "mode", "observed", "duplicates-rows"),
            pair("vehicles", "speed", "observed", "ok"),
        ]
    );
    assert_eq!(body["rows"][1]["observedAt"], json!(["mapped"]));
    assert_eq!(body["rows"][1]["unitCode"], json!(["MTS"]));
    assert_eq!(body["rows"][1]["history"], "PT1H");
    assert_eq!(body["verdicts"]["duplicates-rows"], 1);
    assert_eq!(body["unread"], json!([]));
    // Names and sources only: no value of the data, not even the mapping's text.
    assert!(!answer.text.contains("this.spd"), "{}", answer.text);
}

#[tokio::test]
async fn one_pipeline_is_audited_alone_and_one_that_writes_elsewhere_is_a_404() {
    let (state, _gitea) = world(file_answer()).await;
    let one = send(
        &state,
        person("jana"),
        "GET",
        &format!("{AUDIT}?pipeline=vehicles"),
        None,
    )
    .await;
    assert_eq!(one.status, StatusCode::OK, "{}", one.text);
    let other = send(
        &state,
        person("jana"),
        "GET",
        &format!("{AUDIT}?pipeline=linked-events"),
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
    assert!(other.text.contains("does not write"), "{}", other.text);
}

#[tokio::test]
async fn a_forge_that_fails_names_the_pipeline_whose_rows_are_missing() {
    let (state, _gitea) = world(ResponseTemplate::new(500)).await;
    let answer = send(&state, person("jana"), "GET", AUDIT, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("json");
    assert_eq!(body["unread"], json!(["vehicles"]));
    assert_eq!(body["rows"], json!([]));
}

#[tokio::test]
async fn a_caller_without_read_on_the_space_is_refused_before_the_forge() {
    let (state, gitea) = world(file_answer()).await;
    let refused = send(&state, person("mikko"), "GET", AUDIT, None).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    let other = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/praha/spaces/transport/temporal-audit",
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
    let asked = gitea.received_requests().await.unwrap_or_default();
    assert!(
        !asked.iter().any(|r| r.url.path().ends_with("bento.yaml")),
        "the forge was asked for a mapping"
    );
}
