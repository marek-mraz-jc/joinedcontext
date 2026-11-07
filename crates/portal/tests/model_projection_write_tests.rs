//! A `ModelProjection` is checked against its model on the write path, not only by
//! `jcctl validate` (T-2558, T-2376, MP-01, MF-37): a projection naming a class or slot the
//! referenced model version does not have is refused before a Change exists, every stale name
//! at once, so an approval never lands a projection the Endpoint then serves nothing for.

mod common;

use axum::http::StatusCode;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const PROJECT: &str = "ovzdusie";

const LINKML: &str = r#"id: https://example.org/models/air-quality
name: air-quality
classes:
  AirQualityObserved:
    slots:
      - dateObserved
      - pm10
slots:
  dateObserved:
    range: string
  pm10:
    range: integer
"#;

fn admin() -> Identity {
    let mut identity = common::person("admin");
    identity.groups = vec!["portal-approver".to_owned()];
    identity
}

/// The Portal against a forge that holds the model's LinkML, and a mirror holding the space and
/// the model at `version`.
async fn portal(version: &str) -> (AppState, wiremock::MockServer) {
    let gitea = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{}/contents/projects/{PROJECT}/spaces/mobility/datamodels/air-quality.linkml.yaml",
            common::REPO
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": common::encode(LINKML)
        })))
        .mount(&gitea)
        .await;
    let state = common::state_on(&gitea);
    state.mirror.upsert(common::envelope(
        "ContextSpace",
        "mobility",
        PROJECT,
        json!({ "isSandbox": true }),
    ));
    state.mirror.upsert(common::envelope(
        "DataModel",
        "air-quality",
        PROJECT,
        json!({
            "contextSpaceRef": "mobility",
            "linkml": "./air-quality.linkml.yaml",
            "version": version,
            "lifecycle": "published",
            "classes": ["AirQualityObserved"]
        }),
    ));
    (state, gitea)
}

fn projection(model: &str, version: &str, classes: Value) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "ModelProjection",
        "metadata": { "name": "air-public", "namespace": PROJECT },
        "spec": {
            "contextSpaceRef": "mobility",
            "dataModelRef": { "kind": "DataModel", "name": model, "version": version },
            "classes": classes
        }
    })
}

async fn propose(state: &AppState, manifest: Value) -> common::Answer {
    common::checked_send(
        state,
        admin(),
        "POST",
        &format!("/api/v1/projects/{PROJECT}/projections"),
        Some(manifest),
    )
    .await
}

#[tokio::test]
async fn a_projection_naming_what_the_model_lacks_is_refused_with_every_name() {
    let (state, _gitea) = portal("1.0.0").await;
    let answer = propose(
        &state,
        projection(
            "air-quality",
            "1",
            json!([
                { "name": "AirQualityObserved", "slots": ["pm10", "no2"] },
                { "name": "WaterQualityObserved", "slots": [] },
                { "name": "NoiseLevelObserved", "slots": [] }
            ]),
        ),
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    for stale in [
        "AirQualityObserved.no2",
        "WaterQualityObserved",
        "NoiseLevelObserved",
    ] {
        assert!(
            answer.text.contains(stale),
            "{stale} not named: {}",
            answer.text
        );
    }
    assert!(!answer.text.contains("dateObserved"), "{}", answer.text);
}

#[tokio::test]
async fn an_absent_model_and_a_model_at_another_major_each_say_why() {
    let (state, _gitea) = portal("2.1.0").await;
    let classes = json!([{ "name": "AirQualityObserved", "slots": ["pm10"] }]);

    let absent = propose(&state, projection("water-quality", "1", classes.clone())).await;
    assert_eq!(absent.status, StatusCode::BAD_REQUEST, "{}", absent.text);
    assert!(absent.text.contains("water-quality"), "{}", absent.text);

    let other_major = propose(&state, projection("air-quality", "1", classes)).await;
    assert_eq!(
        other_major.status,
        StatusCode::BAD_REQUEST,
        "{}",
        other_major.text
    );
    assert!(
        other_major.text.contains("version 1") && other_major.text.contains("2.1.0"),
        "{}",
        other_major.text
    );
}

/// A model of another project is not one this project can project: it is absent here (R20).
#[tokio::test]
async fn a_model_of_another_project_is_absent() {
    let (state, _gitea) = portal("1.0.0").await;
    state.mirror.upsert(common::envelope(
        "DataModel",
        "noise",
        "another-city",
        json!({ "contextSpaceRef": "mobility", "linkml": "./noise.linkml.yaml", "version": "1.0.0" }),
    ));
    let answer = propose(
        &state,
        projection(
            "noise",
            "1",
            json!([{ "name": "NoiseLevelObserved", "slots": [] }]),
        ),
    )
    .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST, "{}", answer.text);
    assert!(!answer.text.contains("another-city"), "{}", answer.text);
}

#[tokio::test]
async fn a_correct_projection_still_opens_a_change() {
    let (state, _gitea) = portal("1.0.0").await;
    let answer = propose(
        &state,
        projection(
            "air-quality",
            "1",
            json!([{ "name": "AirQualityObserved", "slots": ["pm10", "dateObserved", "id"] }]),
        ),
    )
    .await;
    assert!(
        answer.status.is_success(),
        "{} {}",
        answer.status,
        answer.text
    );
    assert!(answer.text.contains("Change"), "{}", answer.text);
}
