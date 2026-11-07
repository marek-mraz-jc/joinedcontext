//! The Smart Data Models catalogue and the model compiler through the registry (T-6271, JC2-34).
//!
//! The catalogue index, the import of one catalogue model and the compile of a LinkML source had
//! browser routes and no operations, so a connector asked to import `OnStreetParking` wrote the
//! LinkML from memory: the invented model DM-10 and DM-12 exist to avoid. Each operation calls the
//! route's own function, so Model Tools stays the only one that fetches, and only from the Smart
//! Data Models organisation (DM-10, DM-18). Nothing here writes: the imported source is the
//! `source` a `jc_change_plan` DataModel create takes.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, TERM};
use super::runs::as_user;
use super::{annotations, parse_input, OpError, Operation};
use crate::change::Lane;
use crate::tools::model_tools::{
    self, subjects_of, CatalogueModel, GenerateRequest, ImportSdmRequest, MAX_REQUEST_BYTES,
};
use crate::tools::sdm_mirror::is_catalogue_id;

pub const SEARCH: &str = "jc_model_sdm_search";
pub const IMPORT: &str = "jc_model_sdm_import";
pub const GENERATE: &str = "jc_model_generate";

/// The shortest search text, as the model pickers of the DataModel page ask it.
const MIN_Q: usize = 2;
const MAX_Q: usize = 100;
/// Models one search answers: a picker's page, a few hundred bytes each.
const MAX_HITS: usize = 25;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SearchInput {
    pub q: String,
    /// One catalogue subject whose models' attribute names are filled first, `dataModel.Parking`.
    #[serde(default)]
    pub subject: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ImportInput {
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GenerateInput {
    pub source: String,
}

fn search_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "q": { "type": "string", "minLength": MIN_Q, "maxLength": MAX_Q, "description": "A model, subject or attribute name, e.g. parking" },
            "subject": text("e.g. dataModel.Parking: search its attribute names too", TERM)
        },
        "required": ["q"],
        "additionalProperties": false
    })
}

fn import_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "model": text("subject/model as search lists it, e.g. dataModel.Parking/OnStreetParking", 2 * TERM)
        },
        "required": ["model"],
        "additionalProperties": false
    })
}

fn generate_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "source": { "type": "string", "maxLength": MAX_REQUEST_BYTES, "description": "LinkML YAML" }
        },
        "required": ["source"],
        "additionalProperties": false
    })
}

fn search_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "models": {
                "type": "array",
                "description": "Matching catalogue models, each with id (the import's model), name and description",
                "items": { "type": "object" }
            },
            "more": { "type": "boolean", "description": "More models matched than listed: narrow q" },
            "stale": { "type": "boolean", "description": "The index is the older cached one" }
        },
        "required": ["models", "more", "stale"]
    })
}

fn import_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "model": { "type": "string" },
            "source": { "type": ["string", "null"], "description": "The LinkML: jc_change_plan's DataModel source" },
            "classes": {
                "type": "array",
                "description": "Each class with its slots",
                "items": { "type": "object" }
            },
            "errors": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["model", "source", "classes", "errors"]
    })
}

fn generate_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The artifacts (jsonSchema, context, shacl, docs, example and the rest), or errors when the source does not compile",
        "properties": {
            "errors": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["errors"]
    })
}

fn invalid(path: &str, message: impl Into<String>) -> OpError {
    OpError::InvalidInput {
        path: path.into(),
        message: message.into(),
    }
}

fn validate_search(val: &Value) -> Result<(), OpError> {
    let input: SearchInput = parse_input(val.clone())?;
    let q = input.q.trim().chars().count();
    if !(MIN_Q..=MAX_Q).contains(&q) {
        return Err(invalid(
            "/q",
            format!("q must be {MIN_Q} to {MAX_Q} characters, e.g. parking"),
        ));
    }
    match input.subject.as_deref().map(|s| subjects_of(Some(s))) {
        Some(Ok(subjects)) if subjects.len() != 1 => Err(invalid(
            "/subject",
            "subject names one catalogue subject, e.g. dataModel.Parking",
        )),
        Some(Err(refused)) => Err(invalid("/subject", refused.to_string())),
        _ => Ok(()),
    }
}

fn validate_import(val: &Value) -> Result<(), OpError> {
    let input: ImportInput = parse_input(val.clone())?;
    if !is_catalogue_id(&input.model) {
        return Err(invalid(
            "/model",
            "model is a catalogue identifier such as dataModel.Parking/OnStreetParking, never a URL; jc_model_sdm_search lists them",
        ));
    }
    Ok(())
}

fn validate_generate(val: &Value) -> Result<(), OpError> {
    let input: GenerateInput = parse_input(val.clone())?;
    if input.source.trim().is_empty() {
        return Err(invalid("/source", "source is the LinkML YAML to compile"));
    }
    Ok(())
}

/// Whether a catalogue model matches `q`: its id, name, description or one attribute name.
fn matches(model: &CatalogueModel, q: &str) -> bool {
    let has = |text: &str| text.to_lowercase().contains(q);
    has(&model.id)
        || has(&model.name)
        || model.description.as_deref().is_some_and(has)
        || model.attributes.iter().any(|a| has(a))
}

async fn search(state: &crate::state::AppState, input: SearchInput) -> Result<Value, OpError> {
    let catalogue = model_tools::fetch_catalogue(state, false, input.subject.as_deref()).await?;
    let q = input.q.trim().to_lowercase();
    let mut hits = catalogue
        .subjects
        .iter()
        .flat_map(|subject| &subject.models)
        .filter(|model| matches(model, &q));
    let models: Vec<Value> = hits
        .by_ref()
        .take(MAX_HITS)
        .map(|m| json!({ "id": m.id, "name": m.name, "description": m.description }))
        .collect();
    Ok(json!({ "models": models, "more": hits.next().is_some(), "stale": catalogue.stale }))
}

/// The classes of an imported source with their slots: what a plan names, read off the LinkML so
/// a caller need not parse YAML to know them. A class lists its `slots` and its `attributes`.
fn classes_of(source: &str) -> Vec<Value> {
    let Ok(parsed) = serde_norway::from_str::<Value>(source) else {
        return Vec::new();
    };
    let Some(classes) = parsed.get("classes").and_then(Value::as_object) else {
        return Vec::new();
    };
    classes
        .iter()
        .map(|(name, class)| {
            let mut slots: Vec<&str> = class
                .get("slots")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if let Some(attributes) = class.get("attributes").and_then(Value::as_object) {
                slots.extend(attributes.keys().map(String::as_str));
            }
            json!({ "name": name, "slots": slots })
        })
        .collect()
}

async fn import(
    caller: &super::Caller,
    state: &crate::state::AppState,
    input: ImportInput,
) -> Result<Value, OpError> {
    // A well-formed identifier the catalogue does not hold is the caller's to fix, named by the
    // field: Model Tools would only answer that it could not fetch it.
    let catalogue = model_tools::fetch_catalogue(state, false, None).await?;
    let listed = catalogue
        .subjects
        .iter()
        .flat_map(|subject| &subject.models)
        .any(|model| model.id == input.model);
    if !listed {
        return Err(invalid(
            "/model",
            format!(
                "{} is not in the Smart Data Models catalogue; jc_model_sdm_search lists its models",
                input.model
            ),
        ));
    }
    let Json(artifacts) = model_tools::import_sdm(
        as_user(caller),
        State(state.clone()),
        Json(ImportSdmRequest {
            model: input.model.clone(),
        }),
    )
    .await?;
    let classes = artifacts
        .linkml
        .as_deref()
        .map(classes_of)
        .unwrap_or_default();
    Ok(json!({
        "model": input.model,
        "source": artifacts.linkml,
        "classes": classes,
        "errors": artifacts.errors,
    }))
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: SEARCH,
            title: "Search Smart Data Models",
            description: "Models of the Smart Data Models catalogue whose name, description or attributes match q; import one with jc_model_sdm_import",
            input: search_input_schema,
            output: search_output_schema,
            annotations: annotations(true, false, true),
            kind: "DataModel",
            verb: None,
            lane: Lane::Green,
            validate: validate_search,
            run: |_caller, state, _project, val| {
                Box::pin(async move { search(state, parse_input(val)?).await })
            },
        },
        Operation {
            name: IMPORT,
            title: "Import A Smart Data Model",
            description: "Compiles one Smart Data Models catalogue model into LinkML and its artifacts. Writes nothing; proposing it as a DataModel is a separate change",
            input: import_input_schema,
            output: import_output_schema,
            annotations: annotations(true, false, true),
            kind: "DataModel",
            verb: None,
            lane: Lane::Green,
            validate: validate_import,
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    validate_import(&val)?;
                    import(caller, state, parse_input(val)?).await
                })
            },
        },
        Operation {
            name: GENERATE,
            title: "Generate Model Artifacts",
            description: "Compiles LinkML source into its artifacts (JSON Schema, JSON-LD context, SHACL and the rest) through the model tools service, or answers the messages of a source that does not compile. Writes nothing",
            input: generate_input_schema,
            output: generate_output_schema,
            annotations: annotations(true, false, true),
            kind: "DataModel",
            verb: None,
            lane: Lane::Green,
            validate: validate_generate,
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    validate_generate(&val)?;
                    let input: GenerateInput = parse_input(val)?;
                    let Json(artifacts) = model_tools::generate(
                        as_user(caller),
                        State(state.clone()),
                        Json(GenerateRequest {
                            source: input.source,
                        }),
                    )
                    .await?;
                    Ok(serde_json::to_value(artifacts)?)
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused_at(result: Result<(), OpError>) -> String {
        match result {
            Err(OpError::InvalidInput { path, .. }) => path,
            other => panic!("expected an input refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_search_takes_a_short_q_and_one_subject() {
        assert!(validate_search(&json!({ "q": "parking" })).is_ok());
        assert!(validate_search(&json!({ "q": "pa", "subject": "dataModel.Parking" })).is_ok());
        assert_eq!(refused_at(validate_search(&json!({ "q": "p" }))), "/q");
        assert_eq!(
            refused_at(validate_search(&json!({ "q": "x".repeat(101) }))),
            "/q"
        );
        for subject in ["https://evil.example", "dataModel.A,dataModel.B", "Parking"] {
            assert_eq!(
                refused_at(validate_search(&json!({ "q": "park", "subject": subject }))),
                "/subject",
                "{subject}"
            );
        }
        assert!(validate_search(&json!({ "q": "park", "url": "http://a" })).is_err());
    }

    #[test]
    fn an_import_names_a_catalogue_model_never_a_url() {
        assert!(validate_import(&json!({ "model": "dataModel.Parking/OnStreetParking" })).is_ok());
        for model in [
            "https://raw.githubusercontent.com/smart-data-models/x/schema.json",
            "OnStreetParking",
            "../etc/passwd",
        ] {
            assert_eq!(
                refused_at(validate_import(&json!({ "model": model }))),
                "/model"
            );
        }
    }

    #[test]
    fn a_search_matches_names_descriptions_and_attributes() {
        let model = CatalogueModel {
            id: "dataModel.Parking/OnStreetParking".into(),
            name: "OnStreetParking".into(),
            description: Some("A street segment where vehicles park".into()),
            attributes: vec!["availableSpotNumber".into()],
            commit: None,
        };
        for q in ["onstreet", "datamodel.parking", "vehicles", "availablespot"] {
            assert!(matches(&model, q), "{q}");
        }
        assert!(!matches(&model, "airquality"));
    }

    #[test]
    fn an_imported_source_lists_its_classes_and_their_slots() {
        let source = "classes:\n  OnStreetParking:\n    slots: [id, type]\n    attributes:\n      availableSpotNumber: { range: integer }\n";
        assert_eq!(
            classes_of(source),
            vec![
                json!({ "name": "OnStreetParking", "slots": ["id", "type", "availableSpotNumber"] })
            ]
        );
        assert!(classes_of("not: [yaml").is_empty());
    }
}
