//! T-5299: `GET /api/v1/projects/{project}/spaces/{space}/surface-fit`, the gaps the Endpoint form
//! greys a surface out by (EP-125). A model with a measurement serves `sta`; one without, a draft
//! and a space with no model each give the sentence propose refuses with; a person who may not
//! read the space learns nothing (the authorization matrix holds every role, 401 included).

mod common;

use axum::http::StatusCode;
use joinedcontext_portal::auth::session::Identity;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const PROJECT: &str = "ovzdusie";

const AIR: &str = "id: https://example.org/models/air\nname: air\nclasses:\n  AirQualityObserved:\n    slots: [pm10]\nslots:\n  pm10:\n    range: float\n    unit: {ucum_code: ug/m3}\n";
const ARTICLES: &str = "id: https://example.org/models/news\nname: news\nclasses:\n  Article:\n    slots: [words]\nslots:\n  words:\n    range: integer\n";

fn admin() -> Identity {
    let mut identity = common::person("admin");
    identity.groups = vec!["portal-approver".to_owned()];
    identity
}

/// The Portal whose space `air` holds `linkml` in `lifecycle`, and a space `bare` without a model.
/// Deprecated backs a surface like published, without the four artifacts (DM-02, DM-26).
async fn portal(linkml: &str, lifecycle: &str) -> (AppState, wiremock::MockServer) {
    let gitea = common::forge().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{}/contents/projects/{PROJECT}/spaces/air/datamodels/model.linkml.yaml",
            common::REPO
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "sha": "sha-linkml-1",
            "content": common::encode(linkml)
        })))
        .mount(&gitea)
        .await;
    let state = common::state_on(&gitea);
    for space in ["air", "bare"] {
        state
            .mirror
            .upsert(common::envelope("ContextSpace", space, PROJECT, json!({})));
    }
    state.mirror.upsert(common::envelope(
        "DataModel",
        "model",
        PROJECT,
        json!({ "contextSpaceRef": "air", "linkml": "./model.linkml.yaml", "version": "1.0.0", "lifecycle": lifecycle }),
    ));
    (state, gitea)
}

async fn fit(state: &AppState, who: Identity, space: &str) -> (StatusCode, Value) {
    let uri = format!("/api/v1/projects/{PROJECT}/spaces/{space}/surface-fit");
    let answer = common::send(state, who, "GET", &uri, None).await;
    (
        answer.status,
        serde_json::from_str(&answer.text).unwrap_or(Value::Null),
    )
}

fn gap<'a>(body: &'a Value, representation: &str) -> &'a Value {
    body["surfaces"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["representation"] == representation))
        .map_or(&Value::Null, |row| &row["gap"])
}

#[tokio::test]
async fn a_model_with_a_measurement_serves_sta() {
    let (state, _gitea) = portal(AIR, "deprecated").await;
    let (status, body) = fit(&state, admin(), "air").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dataModel"], "model");
    assert_eq!(gap(&body, "sta"), &Value::Null);
    assert_eq!(gap(&body, "ical"), &Value::Null);
}

#[tokio::test]
async fn a_model_without_a_unit_cannot_serve_sta() {
    let (state, _gitea) = portal(ARTICLES, "deprecated").await;
    let (status, body) = fit(&state, admin(), "air").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        gap(&body, "sta"),
        "`sta` needs a type with a numeric attribute that declares its unit"
    );
}

#[tokio::test]
async fn a_draft_model_backs_no_surface_and_says_so() {
    let (state, _gitea) = portal(AIR, "draft").await;
    let (status, body) = fit(&state, admin(), "air").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for representation in ["sta", "ical"] {
        assert_eq!(
            gap(&body, representation),
            &json!(format!("`{representation}` needs a published data model in space air; its model is draft (EP-125, DM-26)"))
        );
    }
}

#[tokio::test]
async fn a_space_without_a_model_names_what_sta_lacks() {
    let (state, _gitea) = portal(AIR, "deprecated").await;
    let (status, body) = fit(&state, admin(), "bare").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dataModel"], Value::Null);
    assert_eq!(
        gap(&body, "sta"),
        "`sta` needs a type with a numeric attribute that declares its unit"
    );
}

#[tokio::test]
async fn a_person_who_may_not_read_the_space_and_an_unknown_space_are_404() {
    let (state, _gitea) = portal(AIR, "deprecated").await;
    let (status, _) = fit(&state, common::person("stranger"), "air").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = fit(&state, admin(), "nowhere").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn endpoint(representations: Value) -> Value {
    json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "Endpoint",
        "metadata": { "name": "ovzdusie-verejne", "namespace": PROJECT },
        "spec": {
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
            "slug": "si6epqkx364lprho5uaigutk274r5grb",
            "audience": "project-list",
            "allowedProjects": [PROJECT],
            "enabledRepresentations": representations
        }
    })
}

/// The server checks again what the form greyed out: `sta` on a model without a unit is a 422
/// with the same sentence; on a model with one it opens a Change.
#[tokio::test]
async fn propose_refuses_sta_the_model_cannot_serve_with_the_same_sentence() {
    let (state, _gitea) = portal(ARTICLES, "deprecated").await;
    let uri = format!("/api/v1/projects/{PROJECT}/endpoints");
    let answer = common::checked_send(
        &state,
        admin(),
        "POST",
        &uri,
        Some(endpoint(json!(["ngsi-ld", "sta"]))),
    )
    .await;
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.text
    );
    assert!(
        answer.text.contains(
            "Endpoint ovzdusie/ovzdusie-verejne, space air: `sta` needs a type with a numeric attribute that declares its unit (EP-125)"
        ),
        "{}",
        answer.text
    );

    let (state, _gitea) = portal(AIR, "deprecated").await;
    let answer = common::checked_send(
        &state,
        admin(),
        "POST",
        &uri,
        Some(endpoint(json!(["ngsi-ld", "sta"]))),
    )
    .await;
    assert!(answer.status.is_success(), "{}", answer.text);
}
