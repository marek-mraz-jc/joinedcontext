//! The last messages through each step of a pipeline are read with read on the pipeline and by
//! nobody else (T-3974): a caller without it, or of another project, is told the pipeline is not
//! there, and a sample carries nothing its target Endpoint hides and no secret in the clear. Before
//! the mapping a record has the source's own field names, so with anything hidden only the mapped
//! `output` is served.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_samples::KEPT;
use joinedcontext_portal::state::AppState;

const SAMPLES: &str = "/api/v1/projects/helsinki/pipelines/stations/samples";

async fn world() -> AppState {
    world_hiding(json!(["contactPoint"])).await
}

async fn world_hiding(hidden: serde_json::Value) -> AppState {
    let state = common::state_on_forge().await;
    state.mirror.upsert(envelope(
        "Role",
        "pipeline-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["Pipeline"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-pipeline-reader",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "pipeline-reader",
            "scope": { "project": "helsinki" },
        }),
    ));
    state.mirror.upsert(envelope(
        "Endpoint",
        "bikes",
        "helsinki",
        json!({ "projection": { "hiddenAttributes": hidden } }),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "stations",
        "helsinki",
        json!({ "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes" }),
    ));
    // Linked Events' organizer contact is what the mapping turns into `contactPoint`.
    state.pipeline_samples.push(
        "helsinki",
        "stations",
        "input",
        json!({ "organizer": { "email": "jana@hel.fi" } }),
    );
    for n in 0..(KEPT + 5) {
        state.pipeline_samples.push(
            "helsinki",
            "stations",
            "output",
            json!({ "n": n, "contactPoint": "jana@hel.fi", "password": "hunter2hunter2" }),
        );
    }
    // The same pipeline name in another project keeps its own samples.
    state
        .pipeline_samples
        .push("praha", "stations", "input", json!({ "n": -1 }));
    state
}

async fn samples(state: &AppState) -> Value {
    let answer = send(state, person("jana"), "GET", SAMPLES, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    serde_json::from_str(&answer.text).expect("json")
}

#[tokio::test]
async fn a_reader_sees_the_newest_mapped_samples_without_what_the_target_hides() {
    let state = world().await;
    let body = samples(&state).await;
    assert_eq!(body["withheld"], true);
    let output = body["nodes"]["output"].as_array().expect("output");
    assert_eq!(output.len(), KEPT, "only the newest are kept");
    assert_eq!(output[0]["record"]["n"], KEPT + 4, "newest first");
    for sample in output {
        assert!(sample["record"].get("contactPoint").is_none(), "{sample}");
        assert_ne!(sample["record"]["password"], "hunter2hunter2", "{sample}");
    }
    // The raw input names the hidden contact by the source's field: it is not served at all.
    assert!(body["nodes"].get("input").is_none(), "{body}");
    assert!(!body.to_string().contains("jana@hel.fi"), "{body}");
}

#[tokio::test]
async fn every_node_is_served_when_the_target_hides_nothing() {
    let state = world_hiding(json!([])).await;
    let body = samples(&state).await;
    assert_eq!(body["withheld"], false);
    assert_eq!(
        body["nodes"]["input"][0]["record"]["organizer"]["email"],
        "jana@hel.fi"
    );
}

#[tokio::test]
async fn a_target_that_cannot_be_read_withholds_as_one_that_hides() {
    let state = world_hiding(json!([])).await;
    state.mirror.upsert(envelope(
        "Pipeline",
        "stations",
        "helsinki",
        json!({ "class": "resident", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:gone" }),
    ));
    let body = samples(&state).await;
    assert_eq!(body["withheld"], true);
    assert!(body["nodes"].get("input").is_none(), "{body}");
}

#[tokio::test]
async fn a_caller_without_read_on_the_pipeline_is_told_it_is_not_there() {
    let state = world().await;
    let refused = send(&state, person("mikko"), "GET", SAMPLES, None).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    let other = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/praha/pipelines/stations/samples",
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
}
