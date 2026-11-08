//! A new Context Space comes with its one data model, in the same Change (DM-61, DM-62,
//! ADR-N-033, T-2699).
//!
//! Whoever creates the space (the form, the API, the assistant or MCP) goes through the one
//! propose engine, so that is where the model is added: an empty draft model beside the space
//! manifest, and `spec.dataModelRef` naming it. A space that names a model already, or one the
//! project already holds a model for, gets no second one.

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

use common::{checked_send as send, envelope, forge, person};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const SPACES: &str = "/api/v1/projects/helsinki/spaces";

fn state_with(shako: &wiremock::MockServer) -> AppState {
    let state = common::state_on(shako);
    state.mirror.upsert(envelope(
        "Role",
        "space-editor",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace"], "verbs": ["propose", "read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "jana-space-editor",
        ORG_NAMESPACE,
        json!({
            "subjects": [{ "user": "jana@hel.fi" }],
            "role": "space-editor",
            "scope": { "project": "helsinki" },
        }),
    ));
    state
}

fn space(name: &str, spec: Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "ContextSpace",
        "metadata": { "name": name, "namespace": "helsinki" },
        "spec": spec,
    })
}

/// Every file the forge was asked to write, in the commits of `Shako.CommitFiles`:
/// `(path, content)`.
async fn committed(shako: &wiremock::MockServer) -> Vec<(String, String)> {
    common::committed_files(shako, common::REPO).await
}

fn file<'a>(files: &'a [(String, String)], suffix: &str) -> Option<&'a str> {
    files
        .iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map(|(_, content)| content.as_str())
}

#[tokio::test]
async fn a_new_space_change_carries_its_empty_model_and_names_it() {
    let shako = forge().await;
    let state = state_with(&shako);

    let accepted = send(
        &state,
        person("jana"),
        "POST",
        SPACES,
        Some(space("parking", json!({ "isSandbox": false }))),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);

    let files = committed(&shako).await;
    let manifest = file(&files, "spaces/parking/space.yaml")
        .unwrap_or_else(|| panic!("the space manifest is committed: {files:?}"));
    let manifest: Value = serde_norway::from_str(manifest).expect("yaml");
    assert_eq!(
        manifest["spec"]["dataModelRef"],
        json!({ "kind": "DataModel", "name": "parking" })
    );

    let model = file(&files, "spaces/parking/datamodels/parking.yaml")
        .unwrap_or_else(|| panic!("the model manifest rides in the same commit: {files:?}"));
    let model: Value = serde_norway::from_str(model).expect("yaml");
    assert_eq!(model["kind"], "DataModel");
    assert_eq!(model["spec"]["contextSpaceRef"], "parking");
    assert_eq!(model["spec"]["linkml"], "./parking.linkml.yaml");
    assert_eq!(model["spec"]["lifecycle"], "draft");
    assert_eq!(model["spec"]["classes"], json!([]));
    jc_core::kinds::DataModelSpec::validate(
        &serde_json::from_value(model["spec"].clone()).expect("a DataModel spec"),
    )
    .expect("the model manifest is one jc-core accepts");

    let source = file(&files, "spaces/parking/datamodels/parking.linkml.yaml")
        .unwrap_or_else(|| panic!("the LinkML source rides in the same commit: {files:?}"));
    let source: Value = serde_norway::from_str(source).expect("the source is YAML");
    assert_eq!(source["name"], "parking");
    assert_eq!(source["default_prefix"], "helsinki");
    assert!(source["prefixes"]["helsinki"].is_string(), "{source}");
    assert_eq!(source["imports"], json!(["linkml:types", "ngsi-ld-core"]));
}

#[tokio::test]
async fn a_space_that_already_has_a_model_gets_no_second() {
    let shako = forge().await;
    let state = state_with(&shako);
    state.mirror.upsert(envelope(
        "DataModel",
        "garages",
        "helsinki",
        json!({
            "contextSpaceRef": "garages",
            "linkml": "./garages.linkml.yaml",
            "version": "1.0.0",
            "lifecycle": "draft",
        }),
    ));

    let accepted = send(
        &state,
        person("jana"),
        "POST",
        SPACES,
        Some(space("garages", json!({ "isSandbox": false }))),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);
    let files = committed(&shako).await;
    assert!(
        files
            .iter()
            .any(|(path, _)| path.ends_with("spaces/garages/space.yaml")),
        "the space is committed: {files:?}"
    );
    assert!(
        !files.iter().any(|(path, _)| path.contains("datamodels/")),
        "the project holds this space's model: {files:?}"
    );
}
