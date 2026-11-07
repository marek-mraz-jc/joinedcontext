//! T-3223: `GET /api/v1/projects/{project}/spaces/{space}/types/{type}/attributes`, the attributes
//! the pipeline editor's output node maps onto. A reader of the space gets the type's attributes
//! from the model the space pins; a type the model does not declare and a space with no model are
//! 404 with the reason; a person who may not read the space learns nothing (the authorization
//! matrix holds every role).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{envelope, person, send};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::pipeline_validation::ModelSchema;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";

fn world() -> AppState {
    let state = AppState::new(Config::for_tests(), None);
    for space in ["bikes", "empty"] {
        state
            .mirror
            .upsert(envelope("ContextSpace", space, PROJECT, json!({})));
    }
    state.mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Project"], "verbs": ["read"] }] }),
    ));
    for space in ["bikes", "empty"] {
        state.mirror.upsert(envelope(
            "RoleBinding",
            &format!("{space}-readers"),
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "jana@hel.fi" }], "role": "space-reader", "scope": { "contextSpace": space } }),
        ));
    }
    // Only `bikes` pins a model this Portal compiled.
    state.model_schemas.replace(std::collections::HashMap::from([(
        (PROJECT.to_owned(), "bikes".to_owned()),
        std::sync::Arc::new(ModelSchema::compile(
            "bikes",
            "1.2.0",
            &json!({ "definitions": { "BikeHireDockingStation": {
                "description": "One city bike station.",
                "required": ["id", "type", "availableBikeNumber"],
                "properties": {
                    "id": { "type": "string" }, "type": { "type": "string" },
                    "name": { "type": ["string", "null"], "x-ngsi-ld-kind": "Property" },
                    "availableBikeNumber": { "type": ["integer", "null"], "x-ngsi-ld-kind": "Property",
                        "x-unit": { "exactMappings": ["ucefact:C62"] } }
                }
            }}}),
            &["BikeHireDockingStation".to_owned()],
            false,
        )),
    )]));
    state
}

fn attributes(space: &str, entity_type: &str) -> String {
    format!("/api/v1/projects/{PROJECT}/spaces/{space}/types/{entity_type}/attributes")
}

#[tokio::test]
async fn a_reader_gets_the_types_attributes_required_first() {
    let state = world();
    let answer = send(
        &state,
        person("jana"),
        "GET",
        &attributes("bikes", "BikeHireDockingStation"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.text);
    let body: Value = serde_json::from_str(&answer.text).expect("JSON");
    assert_eq!(body["type"], "BikeHireDockingStation");
    assert_eq!(body["model"], "bikes");
    assert_eq!(body["version"], "1.2.0");
    assert_eq!(body["description"], "One city bike station.");
    let names: Vec<&str> = body["attributes"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    assert_eq!(names, ["availableBikeNumber", "name"]);
    assert_eq!(body["attributes"][0]["required"], true);
    assert_eq!(body["attributes"][0]["valueType"], "integer");
    assert_eq!(body["attributes"][0]["unit"]["code"], "C62");
}

#[tokio::test]
async fn an_undeclared_type_and_a_space_without_a_model_are_404_with_the_reason() {
    let state = world();
    let answer = send(
        &state,
        person("jana"),
        "GET",
        &attributes("bikes", "Tram"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);
    assert!(
        answer
            .text
            .contains("Tram is not a class of the space's data model bikes"),
        "{}",
        answer.text
    );

    let answer = send(
        &state,
        person("jana"),
        "GET",
        &attributes("empty", "BikeHireDockingStation"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);
    assert!(
        answer.text.contains("names no data model"),
        "{}",
        answer.text
    );
}

#[tokio::test]
async fn a_person_who_may_not_read_the_space_learns_nothing_of_its_model() {
    let state = world();
    let answer = send(
        &state,
        person("eva"),
        "GET",
        &attributes("bikes", "BikeHireDockingStation"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND, "{}", answer.text);
    assert!(
        !answer.text.contains("availableBikeNumber"),
        "{}",
        answer.text
    );
    assert!(!answer.text.contains("data model bikes"), "{}", answer.text);
}
