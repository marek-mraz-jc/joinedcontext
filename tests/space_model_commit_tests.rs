//! A new Context Space comes with its one data model, in the same Change (DM-61, DM-62,
//! ADR-N-033, T-2699).
//!
//! Whoever creates the space (the form, the API, the assistant or MCP) goes through the one
//! propose engine, so that is where the model is added: an empty draft model beside the space
//! manifest, and `spec.dataModelRef` naming it. A space that names a model already, or one the
//! project already holds a model for, gets no second one.

mod common;

use axum::http::StatusCode;
use base64::Engine;
use serde_json::{json, Value};

use common::{checked_send as send, envelope, forge, person};
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::API_VERSION;
use joinedcontext_portal::state::AppState;

const SPACES: &str = "/api/v1/projects/helsinki/spaces";

fn state_with(gitea: &wiremock::MockServer) -> AppState {
    let state = common::state_on(gitea);
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

/// Every file the forge was asked to write, one request per file or several in one commit:
/// `(path, content)`.
async fn committed(gitea: &wiremock::MockServer) -> Vec<(String, String)> {
    let decode = |value: &Value| {
        value
            .as_str()
            .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    };
    let one = format!("{}/contents/", common::REPO);
    let many = format!("{}/contents", common::REPO);
    let mut files = Vec::new();
    for request in gitea.received_requests().await.unwrap_or_default() {
        if !matches!(request.method.as_str(), "POST" | "PUT") {
            continue;
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let path = request.url.path();
        if let Some(file) = path.strip_prefix(&one) {
            files.push((file.to_owned(), decode(&body["content"])));
        } else if path == many {
            for file in body["files"].as_array().into_iter().flatten() {
                files.push((
                    file["path"].as_str().unwrap_or_default().to_owned(),
                    decode(&file["content"]),
                ));
            }
        }
    }
    files
}

fn file<'a>(files: &'a [(String, String)], suffix: &str) -> Option<&'a str> {
    files
        .iter()
        .find(|(path, _)| path.ends_with(suffix))
        .map(|(_, content)| content.as_str())
}

#[tokio::test]
async fn a_new_space_change_carries_its_empty_model_and_names_it() {
    let gitea = forge().await;
    let state = state_with(&gitea);

    let accepted = send(
        &state,
        person("jana"),
        "POST",
        SPACES,
        Some(space("parking", json!({ "isSandbox": false }))),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.text);

    let files = committed(&gitea).await;
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
    let gitea = forge().await;
    let state = state_with(&gitea);
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
    let files = committed(&gitea).await;
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
