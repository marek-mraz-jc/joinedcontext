//! The editor shows the mapping a pipeline runs (T-4818): the `bento.yaml` beside its manifest,
//! read with read on the pipeline and by nobody else. A caller without it, or asking another
//! project, is told the pipeline is not there and the forge is never asked for the file.

mod common;

use axum::http::StatusCode;
use serde_json::json;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{envelope, person, send, REPO};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::state::AppState;

const BENTO: &str = "/api/v1/projects/helsinki/pipelines/stations/bento";
const FILE: &str = "pipeline:\n  processors:\n    - mapping: |\n        root.type = \"AirQualityObserved\"\n        root.token = \"${FEED_TOKEN}\"\n";

async fn world(file: Option<ResponseTemplate>) -> (AppState, MockServer) {
    let gitea = common::forge().await;
    if let Some(answer) = file {
        Mock::given(method("GET"))
            .and(path_regex(format!(
                "^{REPO}/contents/projects/helsinki/.*pipelines/stations/bento.yaml$"
            )))
            .respond_with(answer)
            .mount(&gitea)
            .await;
    }
    let state = common::state_on(&gitea);
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
    for project in ["helsinki", "praha"] {
        state.mirror.upsert(envelope(
            "Pipeline",
            "stations",
            project,
            json!({ "class": "auto", "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:bikes" }),
        ));
    }
    (state, gitea)
}

fn file_answer() -> ResponseTemplate {
    use base64::Engine as _;
    ResponseTemplate::new(200).set_body_json(json!({
        "sha": "sha-bento-1",
        "content": base64::engine::general_purpose::STANDARD.encode(FILE),
    }))
}

#[tokio::test]
async fn a_reader_gets_the_file_beside_the_manifest_as_written() {
    let (state, _gitea) = world(Some(file_answer())).await;
    let answer = send(&state, person("jana"), "GET", BENTO, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    // A credential is the `${NAME}` the runner fills (PL-16): served as the reference it is.
    assert_eq!(answer.text, FILE);
}

#[tokio::test]
async fn a_pipeline_without_the_file_is_a_404_that_says_so() {
    let (state, _gitea) = world(Some(ResponseTemplate::new(404))).await;
    let answer = send(&state, person("jana"), "GET", BENTO, None).await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);
    assert!(answer.text.contains("no bento.yaml"), "{}", answer.text);
}

#[tokio::test]
async fn a_forge_that_fails_is_not_an_empty_mapping() {
    let (state, _gitea) = world(Some(ResponseTemplate::new(500))).await;
    let answer = send(&state, person("jana"), "GET", BENTO, None).await;
    assert!(
        answer.status.is_server_error(),
        "{} {}",
        answer.status,
        answer.text
    );
}

#[tokio::test]
async fn another_projects_file_and_a_caller_without_read_are_refused_before_the_forge() {
    let (state, gitea) = world(Some(file_answer())).await;
    let other = send(
        &state,
        person("jana"),
        "GET",
        "/api/v1/projects/praha/pipelines/stations/bento",
        None,
    )
    .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
    let refused = send(&state, person("mikko"), "GET", BENTO, None).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.text);
    let asked = gitea.received_requests().await.unwrap_or_default();
    assert!(
        !asked.iter().any(|r| r.url.path().ends_with("bento.yaml")),
        "the forge was asked for the file"
    );
}
