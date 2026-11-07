//! Reading and saving DataModel LinkML source and compiled schema artifacts (DM-01, DM-02, DM-22, DM-24, DM-56, DM-57).

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use jc_core::envelope::ORG_NAMESPACE;
use jc_core::kinds::data_model::{DataModelLifecycle, DataModelOrigin, ModelImport};
use jc_core::kinds::{DataModelSpec, GeneratedArtifacts, SemVer};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use utoipa::ToSchema;

use crate::api::mutate::{author_credentials, branch_name, create_or_reuse_branch};
use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::change::{Change, ChangePhase, ChangeStatus, Lane, Operation, PlanSummary};
use crate::error::{ApiError, ProblemDetails};
use crate::git::Author;
use crate::state::AppState;
use crate::store::ListOptions;
use crate::tools::model_tools::{Artifacts, MAX_REQUEST_BYTES};

/// Single detected difference between the published model and the candidate LinkML source.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelChange {
    pub severity: String,
    pub subject: String,
    pub reason: String,
}

/// Result returned for `PUT /source?dryRun=All`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceDryRunResult {
    pub severity: String,
    pub changes: Vec<ModelChange>,
    pub version: String,
    pub artifacts: Artifacts,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePutQuery {
    pub version: Option<String>,
    #[serde(default, rename = "dryRun")]
    pub dry_run: Option<String>,
    /// `validate_only=true` (AIP-163): the same as `dryRun=All` (T-5949).
    #[serde(default, rename = "validate_only")]
    pub validate_only: Option<bool>,
    /// The space a model the project does not hold yet is created in (DM-57).
    pub space: Option<String>,
}

/// The manifest a `PUT` creates for a name the project does not hold yet (DM-57). With a space
/// it is that space's model; without one it is a project model in its own folder (DM-75), and
/// a space takes it later through its `dataModelRef` (DM-61).
fn new_model_envelope(
    state: &AppState,
    project: &str,
    name: &str,
    space: Option<&str>,
) -> Result<crate::resource::ResourceEnvelope, ApiError> {
    if !crate::resource::is_dns1123(name) {
        return Err(ApiError::BadRequest(format!(
            "'{name}' is not a manifest name: lower-case letters, digits and '-', starting and ending with a letter or a digit"
        )));
    }
    let space = space.map(str::trim).filter(|s| !s.is_empty());
    if let Some(space) = space {
        if state.mirror.get(project, "ContextSpace", space).is_none() {
            return Err(ApiError::NotFound(format!(
                "ContextSpace '{space}' not found in project '{project}'"
            )));
        }
    }
    let mut spec = json!({
        "linkml": format!("./{name}.linkml.yaml"),
        "version": "0.1.0",
        "lifecycle": "draft",
        "classes": [],
    });
    if let Some(space) = space {
        spec["contextSpaceRef"] = json!(space);
    }
    serde_json::from_value(json!({
        "apiVersion": crate::resource::API_VERSION,
        "kind": "DataModel",
        "metadata": { "name": name, "namespace": project },
        "spec": spec,
    }))
    .map_err(|e| ApiError::Internal(format!("build DataModel manifest: {e}")))
}

/// Confines `spec.linkml`: rejects absolute paths, `..` segments, and paths outside `datamodels/`.
pub fn confine_linkml_path(linkml: &str) -> Result<String, ApiError> {
    if linkml.starts_with('/') || linkml.contains('\\') {
        return Err(ApiError::BadRequest(
            "linkml path must be relative, not absolute".into(),
        ));
    }
    let trimmed = linkml.strip_prefix("./").unwrap_or(linkml);
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.split('/').any(|seg| seg == ".." || seg == ".")
    {
        return Err(ApiError::BadRequest(
            "linkml path must not contain '..' segments or be empty".into(),
        ));
    }
    // The path becomes part of a forge URL, where `%2e%2e` is read as `..` and `?` or `#` end the
    // path: none of them names a file of a model (T-1484).
    if trimmed
        .chars()
        .any(|c| matches!(c, '%' | '?' | '#') || c.is_control())
    {
        return Err(ApiError::BadRequest(
            "linkml path must not contain '%', '?', '#' or a control character".into(),
        ));
    }
    if !trimmed.ends_with(".linkml.yaml") {
        return Err(ApiError::BadRequest(
            "linkml path must end with .linkml.yaml".into(),
        ));
    }
    Ok(trimmed.to_string())
}

/// The folder a DataModel's manifest, source and artifacts live in (DM-01, DM-75): a space's
/// model in the space's `datamodels/`, a model no space owns in a folder of its own, in the
/// project or, for `org`, in the organization repository.
pub(crate) fn model_folder(namespace: &str, spec: &Value, name: &str) -> String {
    let space = spec
        .get("contextSpaceRef")
        .and_then(crate::resource::reference_name)
        .unwrap_or_default();
    let manifest = jc_core::registry::by_kind("DataModel")
        .map(|info| info.repo_path(namespace, space, name))
        .unwrap_or_default();
    manifest
        .rsplit_once('/')
        .map(|(folder, _)| folder.to_owned())
        .unwrap_or_default()
}

/// Platform models one compile may import, transitively; Model Tools refuses more.
pub(crate) const MAX_IMPORTS: usize = 32;

/// The LinkML source of every platform model `document` imports, transitively, by its import
/// name (DM-76): an organization model (`org.{name}.v{major}`), or a model of `project`
/// (`project.{name}.v{major}`), at the pinned major and published. Another project's model has
/// no import name, and an organization model imports the organization's models only. What the
/// caller may not read is refused like what does not exist, so an import learns nothing of a
/// space its caller cannot see (R20).
///
/// ponytail: a pin resolves to the version the mirror holds, and a pin to a superseded major is
/// refused naming both; read the source at that version's commit when consumers must stay on an
/// older major while the organization publishes a new one.
pub(crate) async fn resolve_imports(
    state: &AppState,
    identity: &Identity,
    project: &str,
    document: &Value,
) -> Result<BTreeMap<String, String>, ApiError> {
    let effective = crate::permissions::for_request(state, identity, project);
    let member = crate::permissions::is_organization_member(state, identity);
    let mut resolved = BTreeMap::new();
    let mut todo = vec![(project.to_owned(), document.clone())];
    while let Some((importer, document)) = todo.pop() {
        let imports = ModelImport::all_in(&document).map_err(|err| invalid(err.to_string()))?;
        for import in imports {
            let key = import.to_string();
            if resolved.contains_key(&key) {
                continue;
            }
            if resolved.len() >= MAX_IMPORTS {
                return Err(invalid(format!(
                    "a model imports at most {MAX_IMPORTS} platform models, its imports' imports included"
                )));
            }
            let refused = |why: String| invalid(format!("import '{key}': {why} (DM-76)"));
            let home = match (import.organization, importer.as_str()) {
                (true, _) => ORG_NAMESPACE,
                (false, ORG_NAMESPACE) => {
                    return Err(refused(
                        "an organization model imports organization models only".into(),
                    ))
                }
                (false, own) => own,
            };
            let envelope = state
                .mirror
                .get(home, "DataModel", &import.name)
                .filter(|envelope| {
                    if home == ORG_NAMESPACE {
                        member
                    } else {
                        effective.may_read_in(
                            "DataModel",
                            envelope.spec.get("contextSpaceRef").and_then(Value::as_str),
                        )
                    }
                })
                .ok_or_else(|| {
                    refused(if import.organization {
                        format!("the organization has no model '{}'", import.name)
                    } else {
                        format!(
                            "project '{home}' has no model '{}'; a model imports its own project's models and the organization's",
                            import.name
                        )
                    })
                })?;
            let spec: DataModelSpec = serde_json::from_value(envelope.spec.clone())
                .map_err(|err| refused(format!("its manifest does not read: {err}")))?;
            if spec.version.major() != import.major {
                return Err(refused(format!(
                    "'{}' is at version {}, and the import pins major {}",
                    import.name, spec.version, import.major
                )));
            }
            if !matches!(
                spec.lifecycle,
                DataModelLifecycle::Published
                    | DataModelLifecycle::Deprecated
                    | DataModelLifecycle::Mirrored
            ) {
                return Err(refused(format!(
                    "'{}' is {}; only a published model is imported (DM-26)",
                    import.name, spec.lifecycle
                )));
            }
            let text = read_source(state, home, &import.name).await?;
            let parsed: Value = serde_norway::from_str(&text)
                .map_err(|err| refused(format!("its source does not read: {err}")))?;
            todo.push((home.to_owned(), parsed));
            resolved.insert(key, text);
        }
    }
    Ok(resolved)
}

fn invalid(detail: String) -> ApiError {
    ApiError::Invalid {
        errors: vec![detail.clone()],
        detail,
    }
}

/// Every class of the imported sources: a space's types are its own classes and the ones it
/// imports, and the gateway admits a write by `spec.classes` (DM-61, DM-76).
fn imported_classes(imports: &BTreeMap<String, String>) -> Vec<String> {
    imports
        .values()
        .filter_map(|text| serde_norway::from_str::<Value>(text).ok())
        .filter_map(|source| source.get("classes").and_then(Value::as_object).cloned())
        .flat_map(|classes| classes.into_iter().map(|(name, _)| name))
        .collect()
}

/// Every model that imports the organization model `name`, at any major, as `project/model`
/// (DM-76): what refuses its deletion. A source that cannot be read imports nothing here.
///
/// ponytail: reads every model's source from the local checkout on each organization-model
/// delete; keep an index of imports when there are thousands of models.
pub(crate) async fn importers_of(state: &AppState, name: &str) -> Vec<String> {
    let mut importers = Vec::new();
    let homes = std::iter::once(ORG_NAMESPACE.to_owned()).chain(state.mirror.namespaces());
    for namespace in homes {
        for envelope in state
            .mirror
            .list(&namespace, "DataModel", &ListOptions::default())
            .items
        {
            let model = &envelope.metadata.name;
            if namespace == ORG_NAMESPACE && model == name {
                continue;
            }
            let Ok(text) = read_source(state, &namespace, model).await else {
                continue;
            };
            let imports = serde_norway::from_str::<Value>(&text)
                .ok()
                .and_then(|source| ModelImport::all_in(&source).ok())
                .unwrap_or_default();
            if imports
                .iter()
                .any(|import| import.organization && import.name == name)
            {
                importers.push(format!("{namespace}/{model}"));
            }
        }
    }
    importers.sort();
    importers
}

/// Reads the LinkML source from the forge for a DataModel in a project.
pub async fn read_source(state: &AppState, project: &str, name: &str) -> Result<String, ApiError> {
    let envelope = state
        .mirror
        .get(project, "DataModel", name)
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "DataModel '{name}' not found in project '{project}'"
            ))
        })?;

    let linkml = envelope
        .spec
        .get("linkml")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("DataModel spec missing linkml".into()))?;

    let confined = confine_linkml_path(linkml)?;

    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;

    let default_branch = gitea.default_branch().await?;
    let repo_path = format!("{}/{confined}", model_folder(project, &envelope.spec, name));

    let file = gitea
        .get_file(&repo_path, &default_branch)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("source file '{repo_path}' not found in repository"))
        })?;

    // The forge client decodes the file once; a second decode would mangle a source that
    // happens to be valid base64 (T-1483).
    Ok(file.content)
}

/// The document with each class's inline `attributes` as slots of their own, `{Class}.{name}`:
/// an attribute is a slot the class alone holds, judged as one (F-POR1-015).
fn attributes_as_slots(doc: &Value) -> Value {
    let mut doc = doc.clone();
    let mut inline = serde_json::Map::new();
    if let Some(classes) = doc.get("classes").and_then(Value::as_object) {
        for (class, definition) in classes {
            for (name, slot) in definition
                .get("attributes")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                inline.insert(format!("{class}.{name}"), slot.clone());
            }
        }
    }
    if !inline.is_empty() {
        if !doc.get("slots").is_some_and(Value::is_object) {
            doc["slots"] = Value::Object(serde_json::Map::new());
        }
        if let Some(slots) = doc["slots"].as_object_mut() {
            slots.extend(inline);
        }
    }
    doc
}

/// A removed enum or permissible value breaks the records that hold it; an added one does not.
fn enum_changes(prev: &Value, next: &Value, changes: &mut Vec<ModelChange>) {
    let values = |doc: &Value, name: &str| -> Vec<String> {
        doc.pointer(&format!("/enums/{name}/permissible_values"))
            .and_then(Value::as_object)
            .map(|values| values.keys().cloned().collect())
            .unwrap_or_default()
    };
    let names = |doc: &Value| -> Vec<String> {
        doc.get("enums")
            .and_then(Value::as_object)
            .map(|enums| enums.keys().cloned().collect())
            .unwrap_or_default()
    };
    let next_names = names(next);
    for name in names(prev) {
        if !next_names.contains(&name) {
            changes.push(ModelChange {
                severity: "breaking".into(),
                subject: name.clone(),
                reason: format!("enum '{name}' was removed"),
            });
            continue;
        }
        let kept = values(next, &name);
        for value in values(prev, &name) {
            if !kept.contains(&value) {
                changes.push(ModelChange {
                    severity: "breaking".into(),
                    subject: format!("{name}.{value}"),
                    reason: format!("value '{value}' was removed from enum '{name}'"),
                });
            }
        }
    }
    for name in next_names {
        let had = values(prev, &name);
        for value in values(next, &name) {
            if !had.contains(&value) {
                changes.push(ModelChange {
                    severity: "additive".into(),
                    subject: format!("{name}.{value}"),
                    reason: format!("value '{value}' was added to enum '{name}'"),
                });
            }
        }
    }
}

/// The types `linkml:types` declares: a slot's range besides a class, an enum or a type.
const LINKML_TYPES: [&str; 19] = [
    "string",
    "integer",
    "boolean",
    "float",
    "double",
    "decimal",
    "time",
    "date",
    "datetime",
    "date_or_datetime",
    "uriorcurie",
    "curie",
    "uri",
    "ncname",
    "objectidentifier",
    "nodeidentifier",
    "jsonpointer",
    "jsonpath",
    "sparqlpath",
];

/// Every slot range of `doc` that names nothing the model or its imports define (S-15): one
/// `path: message` per range, `path` a JSON pointer into the source without its leading `/`,
/// e.g. `classes/BikeCounter/attributes/refStation/range`. A relationship to a class the model
/// lacks is refused here, before Model Tools is asked.
pub(crate) fn unknown_ranges(doc: &Value, imports: &BTreeMap<String, String>) -> Vec<String> {
    let imported: Vec<Value> = imports
        .values()
        .filter_map(|text| serde_norway::from_str(text).ok())
        .collect();
    let defines = |name: &str| {
        LINKML_TYPES.contains(&name)
            || std::iter::once(doc).chain(&imported).any(|source| {
                ["classes", "enums", "types"]
                    .iter()
                    .any(|section| source[*section].get(name).is_some())
            })
    };
    let mut ranges: Vec<(String, String)> = Vec::new();
    let mut collect = |at: String, slots: &Value| {
        for (slot, definition) in slots.as_object().into_iter().flatten() {
            if let Some(range) = definition.get("range").and_then(Value::as_str) {
                ranges.push((format!("{at}/{slot}/range"), range.to_owned()));
            }
        }
    };
    collect("slots".to_owned(), &doc["slots"]);
    for (class, definition) in doc["classes"].as_object().into_iter().flatten() {
        for member in ["attributes", "slot_usage"] {
            collect(format!("classes/{class}/{member}"), &definition[member]);
        }
    }
    ranges
        .into_iter()
        // A CURIE names a type outside the model's namespace; Model Tools judges it.
        .filter(|(_, range)| !range.contains(':') && !defines(range))
        .map(|(path, range)| {
            format!(
                "{path}: class '{range}' is not in this model or its imports; add class \
                 '{range}' or import the model that defines it"
            )
        })
        .collect()
}

/// The lane a model change goes in: a model that did not exist has nothing to break, and an
/// organization model changes for every project at once, so only an administrator approves it
/// (DM-75).
pub(crate) fn model_lane(project: &str, creating: bool, severity: &str) -> Lane {
    match severity {
        _ if project == ORG_NAMESPACE => Lane::Red,
        _ if creating => Lane::Green,
        "breaking" => Lane::Red,
        "additive" => Lane::Yellow,
        _ => Lane::Green,
    }
}

/// Classifies changes between previous and next LinkML YAML documents.
pub fn classify_linkml_changes(prev_val: &Value, next_val: &Value) -> Vec<ModelChange> {
    let mut changes = Vec::new();
    let empty_map = serde_json::Map::new();
    let (prev_val, next_val) = (
        &attributes_as_slots(prev_val),
        &attributes_as_slots(next_val),
    );
    enum_changes(prev_val, next_val, &mut changes);

    let prev_classes = prev_val
        .get("classes")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let next_classes = next_val
        .get("classes")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);

    for (name, prev_class) in prev_classes {
        if let Some(next_class) = next_classes.get(name) {
            let prev_class_slots: Vec<&str> = prev_class
                .get("slots")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let next_class_slots: Vec<&str> = next_class
                .get("slots")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for slot in prev_class_slots {
                if !next_class_slots.contains(&slot) {
                    changes.push(ModelChange {
                        severity: "breaking".into(),
                        subject: format!("{name}.{slot}"),
                        reason: format!("slot '{slot}' was removed from class '{name}'"),
                    });
                }
            }
        } else {
            changes.push(ModelChange {
                severity: "breaking".into(),
                subject: name.clone(),
                reason: format!("class '{name}' was removed"),
            });
        }
    }

    for (name, _) in next_classes {
        if !prev_classes.contains_key(name) {
            changes.push(ModelChange {
                severity: "additive".into(),
                subject: name.clone(),
                reason: format!("class '{name}' was added"),
            });
        }
    }

    let prev_slots = prev_val
        .get("slots")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let next_slots = next_val
        .get("slots")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);

    for (name, prev_slot) in prev_slots {
        if let Some(next_slot) = next_slots.get(name) {
            let prev_req = prev_slot
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let next_req = next_slot
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !prev_req && next_req {
                changes.push(ModelChange {
                    severity: "breaking".into(),
                    subject: name.clone(),
                    reason: format!("slot '{name}' became required"),
                });
            } else if prev_req && !next_req {
                changes.push(ModelChange {
                    severity: "additive".into(),
                    subject: name.clone(),
                    reason: format!("slot '{name}' is no longer required"),
                });
            }

            let prev_multi = prev_slot
                .get("multivalued")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let next_multi = next_slot
                .get("multivalued")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if prev_multi != next_multi {
                changes.push(ModelChange {
                    severity: "breaking".into(),
                    subject: name.clone(),
                    reason: format!("slot '{name}' multivalued changed"),
                });
            }

            let prev_range = prev_slot.get("range").and_then(Value::as_str);
            let next_range = next_slot.get("range").and_then(Value::as_str);
            if prev_range != next_range {
                let numeric = ["integer", "float", "double", "decimal"];
                let p = prev_range.unwrap_or("string");
                let n = next_range.unwrap_or("string");
                let widens = (p == "integer" && numeric.contains(&n) && n != "integer")
                    || (n == "string" && p != "string");
                if widens {
                    changes.push(ModelChange {
                        severity: "additive".into(),
                        subject: name.clone(),
                        reason: format!("range of slot '{name}' widened from {p} to {n}"),
                    });
                } else {
                    changes.push(ModelChange {
                        severity: "breaking".into(),
                        subject: name.clone(),
                        reason: format!("range of slot '{name}' changed from {p} to {n}"),
                    });
                }
            }
        } else {
            changes.push(ModelChange {
                severity: "breaking".into(),
                subject: name.clone(),
                reason: format!("slot '{name}' was removed"),
            });
        }
    }

    for (name, next_slot) in next_slots {
        if !prev_slots.contains_key(name) {
            let next_req = next_slot
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if next_req {
                changes.push(ModelChange {
                    severity: "breaking".into(),
                    subject: name.clone(),
                    reason: format!("new required slot '{name}' was added"),
                });
            } else {
                changes.push(ModelChange {
                    severity: "additive".into(),
                    subject: name.clone(),
                    reason: format!("new optional slot '{name}' was added"),
                });
            }
        }
    }

    changes
}

pub fn overall_severity(changes: &[ModelChange]) -> &'static str {
    if changes.iter().any(|c| c.severity == "breaking") {
        "breaking"
    } else if changes.iter().any(|c| c.severity == "additive") {
        "additive"
    } else {
        "none"
    }
}

pub fn bump_version(current: &SemVer, severity: &str) -> Result<SemVer, ApiError> {
    let (major, minor, patch) = (current.major(), current.minor(), current.patch());
    let exhausted =
        || ApiError::BadRequest(format!("version {current} has no next {severity} version"));
    let bumped = match severity {
        "breaking" => format!("{}.0.0", major.checked_add(1).ok_or_else(exhausted)?),
        "additive" => format!(
            "{}.{}.0",
            major,
            minor.checked_add(1).ok_or_else(exhausted)?
        ),
        _ => format!(
            "{}.{}.{}",
            major,
            minor,
            patch.checked_add(1).ok_or_else(exhausted)?
        ),
    };
    SemVer::new(&bumped).map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// A breaking change of a published model needs a new major version (DM-22); a model not yet
/// published breaks nothing, so its first version may hold required slots.
fn refuse_unversioned_break(
    published: bool,
    changes: &[ModelChange],
    published_version: &SemVer,
    target_version: &SemVer,
) -> Result<(), ApiError> {
    if !published
        || overall_severity(changes) != "breaking"
        || target_version.major() > published_version.major()
    {
        return Ok(());
    }
    let suggested = bump_version(published_version, "breaking")?;
    Err(ApiError::Invalid {
        detail: format!(
            "a breaking change cannot be saved under version {target_version}; publish it as {suggested}"
        ),
        errors: changes
            .iter()
            .filter(|c| c.severity == "breaking")
            .map(|c| c.reason.clone())
            .collect(),
    })
}

/// The four generated artifacts of the model `name` at `major` as its manifest names them, and
/// each as `(path in the model's folder, content)` (DM-02).
pub(crate) fn artifact_files(
    name: &str,
    major: impl std::fmt::Display,
    artifacts: &Artifacts,
) -> Result<(GeneratedArtifacts, Vec<(String, String)>), ApiError> {
    let pretty = |value: &Option<Value>, what: &str| {
        serde_json::to_string_pretty(value.as_ref().unwrap_or(&json!({})))
            .map_err(|e| ApiError::Internal(format!("serialize {what}: {e}")))
    };
    let files = vec![
        (
            format!("json-schema/{name}.v{major}.json"),
            pretty(&artifacts.json_schema, "json schema")?,
        ),
        (
            format!("context/{name}.v{major}.jsonld"),
            pretty(&artifacts.context, "context")?,
        ),
        (
            format!("docs/{name}.md"),
            artifacts.docs.clone().unwrap_or_default(),
        ),
        (
            format!("examples/{name}.example.jsonld"),
            pretty(&artifacts.example, "example")?,
        ),
    ];
    let at = |i: usize| Some(format!("./{}", files[i].0));
    let spec = GeneratedArtifacts {
        json_schema: at(0),
        context: at(1),
        docs: at(2),
        example: at(3),
    };
    Ok((spec, files))
}

/// Sets `spec.version` and `spec.artifacts` of the model `name` the way a save writes them and
/// answers the artifact files, each as `(path in the model's folder, content)` (DM-02). The
/// editor's save and `jc_space_complete`'s proposal both write a model through it (JC1-06).
pub(crate) fn place_artifacts(
    spec: &mut Value,
    name: &str,
    version: &SemVer,
    artifacts: &Artifacts,
) -> Result<Vec<(String, String)>, ApiError> {
    let (artifacts_spec, files) = artifact_files(name, version.major(), artifacts)?;
    spec["version"] = Value::String(version.to_string());
    spec["artifacts"] = serde_json::to_value(&artifacts_spec)
        .map_err(|e| ApiError::Internal(format!("serialize artifacts: {e}")))?;
    Ok(files)
}

/// The artifacts Model Tools renders from one LinkML source (DM-02); the export reuses it for a
/// model whose repository holds no JSON Schema (MF-41).
pub(crate) async fn compile_artifacts(
    state: &AppState,
    source: &str,
    imports: &BTreeMap<String, String>,
) -> Result<Artifacts, ApiError> {
    let base = state
        .config
        .model_tools_url
        .as_deref()
        .ok_or_else(|| ApiError::Unavailable("no model tools service is configured".into()))?;
    let url = format!("{}/generate", base.trim_end_matches('/'));

    // One client, its pool and TLS set up once: a model share compiles once per landed model.
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default()
    });
    let client = &*CLIENT;

    // The imported sources are the Portal's to hand over, never the caller's (DM-76).
    let mut body = json!({ "source": source });
    if !imports.is_empty() {
        body["imports"] = json!(imports);
    }
    let response = client.post(&url).json(&body).send().await.map_err(|err| {
        tracing::warn!(route = "generate", error = %err, "model tools unreachable");
        ApiError::Unavailable("the model tools service did not answer".into())
    })?;

    if !response.status().is_success() {
        return Err(ApiError::Unavailable(
            "the model tools service did not answer".into(),
        ));
    }

    response.json::<Artifacts>().await.map_err(|err| {
        tracing::warn!(route = "generate", error = %err, "model tools answered unreadably");
        ApiError::Unavailable("the model tools service did not answer".into())
    })
}

/// What saving `source` as the model `name` would do (DM-22, DM-24): the changes against the
/// published source, the version it takes (`version`, or the published one bumped by the
/// severity) and what Model Tools compiles, with the source parsed. A breaking change under the
/// published major and a source Model Tools refuses are errors. The route and the assistant's
/// `change_resource` check a source the same way (AG-77).
pub(crate) async fn check_source(
    state: &AppState,
    identity: &Identity,
    project: &str,
    name: &str,
    spec: &Value,
    source: &str,
    version: Option<&str>,
) -> Result<(SourceDryRunResult, Value, BTreeMap<String, String>), ApiError> {
    let next_val: Value = serde_norway::from_str(source)
        .map_err(|e| ApiError::BadRequest(format!("invalid yaml body: {e}")))?;
    // The source is committed as typed, so a pasted credential would land in Git (MF-24).
    if let Some(key) = crate::api::mutate::find_literal_secret(&next_val) {
        return Err(ApiError::BadRequest(format!(
            "literal secret in field '{key}' is forbidden; use secretRef instead (MF-24)"
        )));
    }

    // A model the mirror lacks has no published version a change could break (T-6268); one it
    // holds is compared with its published source, so a forge that cannot read it now refuses
    // the check instead of reading every slot as new (JC1-09).
    let published = state.mirror.get(project, "DataModel", name).is_some();
    let published_source = match read_source(state, project, name).await {
        Ok(source) => Some(source),
        Err(ApiError::NotFound(_)) => None,
        Err(err) if published => return Err(err),
        Err(_) => None,
    };
    let prev_val: Value = published_source
        .as_deref()
        .and_then(|s| serde_norway::from_str(s).ok())
        .unwrap_or(Value::Null);

    let changes = classify_linkml_changes(&prev_val, &next_val);
    let severity = overall_severity(&changes);

    let current_version_str = spec
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("0.1.0");
    let published_version = SemVer::new(current_version_str)
        .map_err(|e| ApiError::BadRequest(format!("invalid current version: {e}")))?;

    let target_version = match version {
        Some(v) => {
            SemVer::new(v).map_err(|e| ApiError::BadRequest(format!("invalid version: {e}")))?
        }
        None => bump_version(&published_version, severity)?,
    };

    refuse_unversioned_break(published, &changes, &published_version, &target_version)?;

    let imports = resolve_imports(state, identity, project, &next_val).await?;
    let unknown = unknown_ranges(&next_val, &imports);
    if !unknown.is_empty() {
        return Err(ApiError::Invalid {
            detail: "a slot's range names a class the model does not define".into(),
            errors: unknown,
        });
    }
    let artifacts = compile_artifacts(state, source, &imports).await?;
    if !artifacts.errors.is_empty() {
        return Err(ApiError::Invalid {
            detail: artifacts.errors.join("; "),
            errors: artifacts.errors.clone(),
        });
    }

    Ok((
        SourceDryRunResult {
            severity: severity.to_string(),
            changes,
            version: target_version.to_string(),
            artifacts,
        },
        next_val,
        imports,
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/datamodels/{name}/source",
    summary = "Read Model Source",
    description = "One DataModel's LinkML, as the repository holds it.",
    tag = "datamodels",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "DataModel name"),
    ),
    responses(
        (status = 200, description = "LinkML source in YAML format", content_type = "text/yaml", body = String),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 503, description = "Service unavailable", body = ProblemDetails),
    )
)]
pub async fn get_source(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    // PF-59, R20 (T-1367): the source is a read of the model, so a caller who may not read the
    // project's models gets the answer of a model that is not there, and the forge is not asked.
    // The operations registry and the MCP resource ask the same question before `read_source`.
    // An organization model is every member's to read, since a schema carries no data (DM-75).
    let readable = if project == ORG_NAMESPACE {
        crate::permissions::is_organization_member(&state, &user.0.identity)
    } else {
        crate::permissions::for_request(&state, &user.0.identity, &project).may_read("DataModel")
    };
    if !readable {
        return Err(ApiError::NotFound(format!(
            "DataModel '{name}' not found in project '{project}'"
        )));
    }
    let content = read_source(&state, &project, &name).await?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/yaml; charset=utf-8")],
        content,
    )
        .into_response())
}

#[utoipa::path(
    put,
    path = "/api/v1/projects/{project}/datamodels/{name}/source",
    summary = "Write Model Source",
    description = "Writes one DataModel's LinkML and its generated artifacts as a change a person approves.",
    tag = "datamodels",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "DataModel name"),
        ("version" = Option<String>, Query, description = "Target semver"),
        ("dryRun" = Option<String>, Query, description = "Set to 'All' for dry run"),
        ("space" = Option<String>, Query, description = "The space a new model is created in; without it a new model is a project model (DM-75)"),
    ),
    request_body(
        content = String,
        description = "LinkML source in YAML format",
        content_type = "text/yaml",
        example = json!("id: https://hel.fi/models/air\nname: air\nprefixes:\n  linkml: https://w3id.org/linkml/\nimports: [linkml:types]\nclasses:\n  AirQualityObserved:\n    attributes:\n      pm10: { range: float }\n"),
    ),
    responses(
        (status = 202, description = "Change proposal accepted", body = Change),
        (status = 200, description = "Dry run result", body = SourceDryRunResult),
        (status = 400, description = "Bad request", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails),
        (status = 403, description = "Forbidden", body = ProblemDetails),
        (status = 404, description = "Not found", body = ProblemDetails),
        (status = 503, description = "Service unavailable", body = ProblemDetails),
    )
)]
pub async fn put_source(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    Query(query): Query<SourcePutQuery>,
    body: Bytes,
) -> Result<Response, ApiError> {
    if body.len() > MAX_REQUEST_BYTES {
        return Err(ApiError::BadRequest(format!(
            "the source is larger than the {MAX_REQUEST_BYTES} byte limit"
        )));
    }

    let is_dry = query
        .dry_run
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("all") || s.eq_ignore_ascii_case("true"))
        .unwrap_or(query.validate_only == Some(true));

    let existing = state.mirror.get(&project, "DataModel", &name);
    let creating = existing.is_none();
    let envelope = match existing {
        Some(envelope) => envelope,
        None => new_model_envelope(&state, &project, &name, query.space.as_deref())?,
    };

    // The whole manifest: a space-bound grant reads `spec.contextSpaceRef`, a constraint may read
    // `metadata` (F-POR1-002).
    let target = serde_json::to_value(&envelope).map_err(|e| ApiError::Internal(e.to_string()))?;
    crate::permissions::for_request(&state, &user.0.identity, &project).check(
        "DataModel",
        jc_core::kinds::Verb::Propose,
        Some(&target),
    )?;

    let source_str = std::str::from_utf8(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid utf-8 body: {e}")))?
        .to_string();

    // A model that is being created starts where DM-22 starts, not one additive bump above it.
    let requested_version =
        query
            .version
            .as_deref()
            .or(if creating { Some("0.1.0") } else { None });

    let (checked, next_val, imports) = check_source(
        &state,
        &user.0.identity,
        &project,
        &name,
        &envelope.spec,
        &source_str,
        requested_version,
    )
    .await?;

    if is_dry {
        return Ok((StatusCode::OK, Json(checked)).into_response());
    }
    let lane = model_lane(&project, creating, &checked.severity);
    let (verb, noun) = if creating {
        ("create", "creation")
    } else {
        ("update", "update")
    };
    let change = propose_model(
        &state,
        &user.0.identity,
        ModelProposal {
            namespace: &project,
            name: &name,
            envelope,
            source: &source_str,
            checked,
            next_val,
            imports,
            creating,
            lane,
            title: format!("{verb} DataModel {name}"),
            body: format!(
                "Proposed {noun} of DataModel `{name}` source, manifest and generated artifacts in project `{project}` via joinedcontext Portal."
            ),
        },
    )
    .await?;

    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// The body of a share: the model's name typed back by an organization administrator who
/// approves their own share at once (PF-58, CC-19). Without it the Change waits for one.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareBody {
    #[serde(default)]
    pub confirm: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/datamodels/{name}/share",
    summary = "Share Model with the Organization",
    description = "Proposes a red-lane Change of the organization repository that copies the published model's source byte for byte, with spec.origin (DM-77). Only an organization-scope approver of DataModel approves it.",
    tag = "datamodels",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "DataModel name"),
    ),
    request_body(
        content = Option<ShareBody>,
        description = "Empty, or the model's name typed by an organization administrator sharing their own model",
        content_type = "application/json",
        example = json!({ "confirm": "air-quality" })
    ),
    responses(
        (status = 202, description = "The Change, waiting for an organization approver or merged", body = Change),
        (status = 400, description = "The model imports a model of the project, or the body does not read", body = ProblemDetails),
        (status = 403, description = "No propose on DataModel", body = ProblemDetails),
        (status = 404, description = "No such model the caller may read", body = ProblemDetails),
        (status = 409, description = "Not published, the name is held by an organization model of another origin, or the organization's copy already has this source", body = ProblemDetails),
        (status = 503, description = "Git forge unavailable", body = ProblemDetails),
    )
)]
pub async fn share_model(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let share: ShareBody = if body.is_empty() {
        ShareBody::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::BadRequest(format!("invalid json body: {e}")))?
    };
    let identity = &user.0.identity;
    if project == ORG_NAMESPACE {
        return Err(ApiError::BadRequest(format!(
            "'{name}' is an organization model already; a project's model is what is shared (DM-77)"
        )));
    }
    let effective = crate::permissions::for_request(&state, identity, &project);
    let model = state
        .mirror
        .get(&project, "DataModel", &name)
        .filter(|model| {
            effective.may_read_in(
                "DataModel",
                model.spec.get("contextSpaceRef").and_then(Value::as_str),
            )
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!("project '{project}' has no data model '{name}'"))
        })?;
    let target = serde_json::to_value(&model).map_err(|e| ApiError::Internal(e.to_string()))?;
    effective.check("DataModel", jc_core::kinds::Verb::Propose, Some(&target))?;
    let spec: DataModelSpec = serde_json::from_value(model.spec.clone())
        .map_err(|e| ApiError::Internal(format!("DataModel {name} does not read: {e}")))?;
    if spec.lifecycle != DataModelLifecycle::Published {
        return Err(ApiError::Conflict(format!(
            "'{name}' is {}; only a published model is shared with the organization (DM-77)",
            spec.lifecycle
        )));
    }
    let source = read_source(&state, &project, &name).await?;
    let from = |origin: &DataModelOrigin| {
        origin.project == project && origin.space == spec.context_space_ref && origin.name == name
    };

    // The organization's copy of this model, when an earlier share made one: the next version of
    // it (DM-22). A model of that name from anywhere else keeps its name (DM-77).
    let existing = state.mirror.get(ORG_NAMESPACE, "DataModel", &name);
    let (envelope, version) = match existing {
        None => {
            let mut envelope = model.clone();
            envelope.metadata.namespace = Some(ORG_NAMESPACE.to_owned());
            if let Some(spec) = envelope.spec.as_object_mut() {
                spec.remove("contextSpaceRef");
                spec.remove("origin");
            }
            (envelope, Some(spec.version.to_string()))
        }
        Some(organization) => {
            let origin = organization
                .spec
                .get("origin")
                .and_then(|origin| serde_json::from_value::<DataModelOrigin>(origin.clone()).ok());
            if !origin.as_ref().is_some_and(from) {
                return Err(ApiError::Conflict(format!(
                    "the organization already has a data model '{name}'{}; rename this model to share it (DM-77)",
                    origin.map_or_else(String::new, |o| format!(
                        ", shared from project '{}' model '{}'",
                        o.project, o.name
                    ))
                )));
            }
            if read_source(&state, ORG_NAMESPACE, &name)
                .await
                .ok()
                .as_deref()
                == Some(&source)
            {
                return Err(ApiError::Conflict(format!(
                    "the organization's '{name}' already holds this source, at version {}",
                    organization
                        .spec
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )));
            }
            (organization, None)
        }
    };
    let creating = version.is_some();

    // Checked as an organization model: its imports resolve among the organization's models only,
    // so one importing a model of this project is refused before anything is written (DM-76).
    let (checked, next_val, imports) = check_source(
        &state,
        identity,
        ORG_NAMESPACE,
        &name,
        &envelope.spec,
        &source,
        version.as_deref(),
    )
    .await?;

    let gitea = state
        .forge_for(&project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let commit = gitea.branch_head(&gitea.default_branch().await?).await?;
    let origin = DataModelOrigin {
        project: project.clone(),
        space: spec.context_space_ref.clone(),
        name: name.clone(),
        version: spec.version.clone(),
        commit,
    };
    let mut envelope = envelope;
    envelope.spec["origin"] =
        serde_json::to_value(&origin).map_err(|e| ApiError::Internal(e.to_string()))?;
    envelope.spec["lifecycle"] = json!("published");

    let from_space = origin
        .space
        .as_deref()
        .map(|space| format!(", space `{space}`"))
        .unwrap_or_default();
    let change = propose_model(
        &state,
        identity,
        ModelProposal {
            namespace: ORG_NAMESPACE,
            name: &name,
            envelope,
            source: &source,
            checked,
            next_val,
            imports,
            creating,
            lane: Lane::Red,
            title: format!("share DataModel {name} with the organization"),
            body: format!(
                "Shares DataModel `{name}` {} of project `{project}`{from_space} with the organization (DM-77): the source copied byte for byte at commit {}. An organization administrator approves it.",
                origin.version, origin.commit
            ),
        },
    )
    .await?;
    // Only a person at the Portal approves their own share, and only as an organization
    // administrator with the name typed; a bearer caller's waits like anyone's (PF-58).
    let change = if identity.client.is_none() {
        crate::api::changes::approve_as_proposed(
            &state,
            identity,
            ORG_NAMESPACE,
            "DataModel",
            change,
            share.confirm.as_deref(),
        )
        .await
    } else {
        change
    };
    Ok((StatusCode::ACCEPTED, Json(change)).into_response())
}

/// One model Change: the manifest, its source as typed and the artifacts compiled from it, in
/// the model's folder of the repository its namespace lives in (DM-01, DM-75).
pub(crate) struct ModelProposal<'a> {
    pub namespace: &'a str,
    pub name: &'a str,
    /// The manifest as it stands, or as it will be created; version, classes and artifacts are
    /// set here from `checked`.
    pub envelope: crate::resource::ResourceEnvelope,
    pub source: &'a str,
    pub checked: SourceDryRunResult,
    pub next_val: Value,
    pub imports: BTreeMap<String, String>,
    pub creating: bool,
    pub lane: Lane,
    pub title: String,
    pub body: String,
}

/// Writes a checked model to a branch of its repository and opens the Change for it.
pub(crate) async fn propose_model(
    state: &AppState,
    identity: &Identity,
    proposal: ModelProposal<'_>,
) -> Result<Change, ApiError> {
    let (project, name, creating, lane) = (
        proposal.namespace,
        proposal.name,
        proposal.creating,
        proposal.lane,
    );
    let (title, body) = (proposal.title.as_str(), proposal.body.as_str());
    let files = model_files(&proposal)?;

    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;

    let default_branch = gitea.default_branch().await?;
    let operation = if creating {
        Operation::Create
    } else {
        Operation::Update
    };
    let branch = branch_name(project, "datamodel", name, operation);
    let held = crate::api::mutate::turn(gitea, &branch).await;
    let branch = create_or_reuse_branch(gitea, &held, &default_branch).await?;
    commit_model(gitea, identity, project, title, &branch, files).await?;

    crate::telemetry::proposed(lane, "DataModel");

    let pr = gitea
        .create_pull_request(&branch, &default_branch, title, body)
        .await?;

    let change_meta = crate::api::changes::change_meta(state, gitea, pr.number, project);
    let change_status = ChangeStatus::new(
        lane,
        ChangePhase::PendingApproval,
        if creating {
            PlanSummary {
                create: 6,
                update: 0,
                delete: 0,
            }
        } else {
            PlanSummary {
                create: 0,
                update: 6,
                delete: 0,
            }
        },
    )
    .in_repository(&pr.repository)
    .proposed_through(&pr.body)
    .with_merge_request(pr.url);
    Ok(Change::new(change_meta, change_status))
}

/// Writes a checked model onto `branch`, the shared branch of a plan of several changes opened
/// as one Change (T-6282, JC2-10): a change already open on the model still refuses it, as its
/// own proposal would be (CC-34).
pub(crate) async fn propose_model_onto(
    state: &AppState,
    identity: &Identity,
    proposal: ModelProposal<'_>,
    branch: &str,
) -> Result<Lane, ApiError> {
    let (project, name, lane) = (proposal.namespace, proposal.name, proposal.lane);
    let title = proposal.title.clone();
    let files = model_files(&proposal)?;
    let gitea = state
        .forge_for(project)
        .ok_or_else(|| ApiError::Unavailable("git forge is not configured".into()))?;
    let gitea: &crate::git::GiteaClient = &gitea;
    let default_branch = gitea.default_branch().await?;
    let operation = if proposal.creating {
        Operation::Create
    } else {
        Operation::Update
    };
    let own = branch_name(project, "datamodel", name, operation);
    if let Some(pending) = crate::api::mutate::open_change_on(state, gitea, &own, project).await? {
        return Err(ApiError::Conflict(format!(
            "a change for DataModel '{name}' is already open: {}; approve or reject it first",
            pending.name
        )));
    }
    match gitea.create_branch(branch, &default_branch).await {
        Ok(()) | Err(crate::git::GitError::Conflict(_)) => {}
        Err(err) => return Err(err.into()),
    }
    commit_model(gitea, identity, project, &title, branch, files).await?;
    crate::telemetry::proposed(lane, "DataModel");
    Ok(lane)
}

/// The files of one model Change, by repository path: the manifest with its version, classes
/// and artifacts set from the check, its source as typed and the artifacts compiled from it.
fn model_files(proposal: &ModelProposal<'_>) -> Result<Vec<(String, String)>, ApiError> {
    let name = proposal.name;
    let project = proposal.namespace;
    let folder = model_folder(project, &proposal.envelope.spec, name);
    let linkml = proposal
        .envelope
        .spec
        .get("linkml")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::BadRequest("DataModel spec missing linkml".into()))?;
    let confined_linkml = confine_linkml_path(linkml)?;
    let (source_str, next_val, imports) = (proposal.source, &proposal.next_val, &proposal.imports);
    let SourceDryRunResult {
        version, artifacts, ..
    } = &proposal.checked;
    let target_version =
        SemVer::new(version).map_err(|e| ApiError::BadRequest(format!("invalid version: {e}")))?;

    let empty_map = serde_json::Map::new();
    let next_classes = next_val
        .get("classes")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);
    let mut classes: Vec<String> = next_classes.keys().cloned().collect();
    classes.extend(imported_classes(imports));
    classes.sort();
    classes.dedup();

    let mut new_spec = proposal.envelope.spec.clone();
    let artifact_writes = place_artifacts(&mut new_spec, name, &target_version, artifacts)?;
    new_spec["classes"] = serde_json::to_value(&classes).unwrap_or(Value::Array(Vec::new()));

    let typed_spec: DataModelSpec = serde_json::from_value(new_spec.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid DataModel spec: {e}")))?;
    typed_spec
        .validate()
        .map_err(|e| ApiError::BadRequest(format!("DataModel validation error: {e}")))?;

    let mut updated_envelope = proposal.envelope.clone();
    updated_envelope.spec = new_spec;
    updated_envelope.strip_status();

    let manifest_yaml = serde_norway::to_string(&updated_envelope)
        .map_err(|e| ApiError::Internal(format!("serialize manifest to yaml: {e}")))?;

    let manifest_path = format!("{folder}/{name}.yaml");
    let source_path = format!("{folder}/{confined_linkml}");
    let mut writes = vec![
        (manifest_path, manifest_yaml),
        (source_path, source_str.to_owned()),
    ];
    writes.extend(
        artifact_writes
            .into_iter()
            .map(|(file, content)| (format!("{folder}/{file}"), content)),
    );
    Ok(writes)
}

/// Commits a model's files onto `branch` in one commit: a forge failure halfway leaves no branch
/// holding a manifest without its source or artifacts (F-POR1-023).
async fn commit_model(
    gitea: &crate::git::GiteaClient,
    identity: &Identity,
    project: &str,
    title: &str,
    branch: &str,
    writes: Vec<(String, String)>,
) -> Result<(), ApiError> {
    let (author_name, author_email) = author_credentials(identity, project);
    let commit_msg = format!("{title} with its source and artifacts");
    let mut files = Vec::with_capacity(writes.len());
    for (file_p, content) in writes {
        let existing_sha = gitea
            .get_file(&file_p, branch)
            .await
            .ok()
            .flatten()
            .map(|f| f.sha);
        files.push((file_p, content, existing_sha));
    }
    gitea
        .write_files(
            branch,
            &commit_msg,
            Author {
                name: &author_name,
                email: &author_email,
            },
            &files,
        )
        .await?;
    Ok(())
}

/// The longest `search` the organization's model list takes (API/01).
const MAX_SEARCH_CHARS: usize = 100;
/// How many catalogue entries one search answers: the index holds about a thousand.
const MAX_CATALOGUE_ENTRIES: usize = 50;

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct OrganizationModelsQuery {
    /// Matched case-insensitively on name, project, space and classes, and on a catalogue
    /// entry's name, id and description.
    #[serde(default)]
    pub search: Option<String>,
}

/// Where a model lives (DM-75, DM-79).
#[derive(Debug, Clone, Copy, Serialize, ToSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum ModelLevel {
    /// In the organization repository, usable by every project of the organization.
    Organization,
    /// In one project, a space's model or one no space owns.
    Project,
}

/// One `DataModel` of the organization as the pickers list it (DM-63, DM-79).
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationModel {
    pub name: String,
    pub level: ModelLevel,
    /// The project it belongs to; `org` for an organization model.
    pub project: String,
    /// The space whose model it is; absent for a model no space owns (DM-75).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    pub version: String,
    pub lifecycle: String,
    pub classes: Vec<String>,
    /// The project model an organization model was shared from (DM-77); absent on every other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<SharedFrom>,
}

/// Where an organization model was shared from (DM-77): what lets that project offer to use the
/// organization's copy instead of its own (DM-78).
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SharedFrom {
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    pub name: String,
    /// The version that was shared.
    pub version: String,
}

/// One Smart Data Models catalogue entry as the pickers list it (DM-12, DM-63).
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueEntry {
    /// `dataModel.Environment/AirQualityObserved`.
    pub id: String,
    pub name: String,
    /// `dataModel.Environment`.
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationModels {
    pub api_version: String,
    pub kind: String,
    pub items: Vec<OrganizationModel>,
    pub smart_data_models: Vec<CatalogueEntry>,
    /// Why no catalogue entries could be listed, when Model Tools did not answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalogue_unavailable: Option<String>,
}

fn matches(needle: &str, haystack: &[&str]) -> bool {
    needle.is_empty() || haystack.iter().any(|h| h.to_lowercase().contains(needle))
}

/// A mirrored `DataModel` manifest as a picker item; `None` for a retired version (DM-26) or a
/// manifest that does not deserialize, which no picker can offer anyway.
fn organization_model(
    project: &str,
    env: &crate::resource::ResourceEnvelope,
) -> Option<OrganizationModel> {
    let spec: DataModelSpec = serde_json::from_value(env.spec.clone()).ok()?;
    let lifecycle = serde_json::to_value(spec.lifecycle)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))?;
    if lifecycle == "retired" {
        return None;
    }
    Some(OrganizationModel {
        name: env.metadata.name.clone(),
        level: if project == ORG_NAMESPACE {
            ModelLevel::Organization
        } else {
            ModelLevel::Project
        },
        project: project.to_owned(),
        space: spec.context_space_ref,
        version: spec.version.to_string(),
        lifecycle,
        classes: spec.classes,
        origin: spec.origin.map(|origin| SharedFrom {
            project: origin.project,
            space: origin.space,
            name: origin.name,
            version: origin.version.to_string(),
        }),
    })
}

/// `GET /api/v1/organization/datamodels` (DM-63, ADR-N-033): every model of every project the
/// caller may read, project by project and space by space (PF-60), and the catalogue entries a
/// search matches. What the caller may not read is not in it, and nobody is refused (R20).
#[utoipa::path(
    get,
    path = "/api/v1/organization/datamodels",
    summary = "List Data Models Everywhere",
    description = "Every DataModel the caller may read across projects, and the Smart Data Models entries a search of two characters or more matches: what the model and type pickers list.",
    tag = "resources",
    params(("search" = Option<String>, Query, description = "Case-insensitive substring of a name, project, space or class; at most 100 characters")),
    responses(
        (status = 200, description = "The models the caller may read and the matching catalogue entries", body = OrganizationModels),
        (status = 400, description = "A search longer than 100 characters", body = ProblemDetails),
        (status = 401, description = "Unauthorized", body = ProblemDetails)
    )
)]
pub async fn list_organization_datamodels(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<OrganizationModelsQuery>,
) -> Result<Json<OrganizationModels>, ApiError> {
    let search = query.search.unwrap_or_default();
    let search = search.trim();
    if search.chars().count() > MAX_SEARCH_CHARS {
        return Err(ApiError::BadRequest(format!(
            "search is at most {MAX_SEARCH_CHARS} characters"
        )));
    }
    let needle = search.to_lowercase();

    let member = crate::permissions::is_organization_member(&state, &user.0.identity);
    let mut items = Vec::new();
    // `namespaces` names the projects; the organization's own models are in `org` (DM-75).
    let homes = std::iter::once(ORG_NAMESPACE.to_owned()).chain(state.mirror.namespaces());
    for project in homes {
        let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
        // An organization model is every member's to read; a project's by its grants (DM-75).
        let organization = project == ORG_NAMESPACE;
        if !(if organization {
            member
        } else {
            effective.may_read("DataModel")
        }) {
            continue;
        }
        for env in state
            .mirror
            .list(&project, "DataModel", &ListOptions::default())
            .items
        {
            let Some(model) = organization_model(&project, &env) else {
                continue;
            };
            if !organization && !effective.may_read_in("DataModel", model.space.as_deref()) {
                continue;
            }
            let mut haystack = vec![
                model.name.as_str(),
                model.project.as_str(),
                model.space.as_deref().unwrap_or_default(),
            ];
            haystack.extend(model.classes.iter().map(String::as_str));
            if matches(&needle, &haystack) {
                items.push(model);
            }
        }
    }
    items.sort_by(|a, b| {
        (a.level, &a.project, &a.space, &a.name).cmp(&(b.level, &b.project, &b.space, &b.name))
    });

    let mut smart_data_models = Vec::new();
    let mut catalogue_unavailable = None;
    if needle.chars().count() >= 2 {
        match crate::tools::model_tools::fetch_catalogue(&state, false, None).await {
            Ok(catalogue) => {
                smart_data_models = catalogue
                    .subjects
                    .into_iter()
                    .flat_map(|subject| {
                        let name = subject.name;
                        subject.models.into_iter().map(move |model| CatalogueEntry {
                            id: model.id,
                            name: model.name,
                            subject: name.clone(),
                            description: model.description,
                        })
                    })
                    .filter(|entry| {
                        matches(
                            &needle,
                            &[
                                entry.name.as_str(),
                                entry.id.as_str(),
                                entry.description.as_deref().unwrap_or_default(),
                            ],
                        )
                    })
                    .take(MAX_CATALOGUE_ENTRIES)
                    .collect();
            }
            Err(ApiError::Unavailable(reason)) => catalogue_unavailable = Some(reason),
            Err(err) => return Err(err),
        }
    }

    Ok(Json(OrganizationModels {
        api_version: crate::resource::API_VERSION.to_string(),
        kind: "List".to_string(),
        items,
        smart_data_models,
        catalogue_unavailable,
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/organization/datamodels",
            get(list_organization_datamodels),
        )
        .route(
            "/projects/{project}/datamodels/{name}/source",
            get(get_source).put(put_source),
        )
        .route(
            "/projects/{project}/datamodels/{name}/share",
            axum::routing::post(share_model),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JC1-09, DM-24: a published model whose source the forge cannot read now is not checked
    /// as a model with no published source, which reads every slot as new and lets a removal
    /// pass under the same major: the check is refused with the forge's reason.
    #[tokio::test]
    async fn a_published_model_whose_source_cannot_be_read_is_not_checked_as_new() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        state.mirror.upsert(crate::resource::ResourceEnvelope {
            api_version: crate::resource::API_VERSION.to_owned(),
            kind: "DataModel".to_owned(),
            metadata: crate::resource::ObjectMeta::new("air", "hel"),
            spec: json!({ "linkml": "model.linkml.yaml", "version": "1.0.0" }),
            status: None,
        });
        let identity = Identity {
            client: None,
            subject: "s".into(),
            username: "jana".into(),
            email: None,
            name: None,
            roles: vec![],
            groups: vec![],
            second_factor: false,
        };
        let refused = check_source(
            &state,
            &identity,
            "hel",
            "air",
            &json!({ "linkml": "model.linkml.yaml", "version": "1.0.0" }),
            "id: air\nclasses:\n  Station: {}\n",
            None,
        )
        .await
        .expect_err("no forge to read the published source from");
        assert!(refused.to_string().contains("git forge"), "{refused}");
    }

    /// DM-56, T-1484: a model's source stays inside its space's `datamodels/` folder.
    // DM-01, DM-75: a space's model sits in the space's folder, one no space owns in its own.
    #[test]
    fn a_models_folder_follows_its_level() {
        let space = json!({ "contextSpaceRef": "air" });
        assert_eq!(
            model_folder("hel", &space, "aq"),
            "projects/hel/spaces/air/datamodels"
        );
        assert_eq!(
            model_folder("hel", &json!({}), "aq"),
            "projects/hel/datamodels/aq"
        );
        assert_eq!(
            model_folder(ORG_NAMESPACE, &json!({}), "aq"),
            "datamodels/aq"
        );
    }

    // DM-61, DM-76: the gateway admits a write by `spec.classes`, so a space's classes include
    // the ones it imports; an imported source that does not read adds none.
    #[test]
    fn imported_classes_are_every_class_of_every_imported_source() {
        let imports = BTreeMap::from([
            (
                "org.a.v1".to_owned(),
                "classes:\n  Station: {}\n  Dock: {}\n".to_owned(),
            ),
            (
                "project.b.v2".to_owned(),
                "classes:\n  Kiosk: {}\n".to_owned(),
            ),
            ("org.c.v1".to_owned(), ": not yaml [".to_owned()),
        ]);
        let mut classes = imported_classes(&imports);
        classes.sort();
        assert_eq!(classes, ["Dock", "Kiosk", "Station"]);
    }

    /// S-15: a range names a class, enum or type of the model, its imports or `linkml:types`;
    /// anything else is refused at its path, naming the class.
    #[test]
    fn a_range_naming_no_defined_class_is_refused_at_its_path() {
        let doc: Value = serde_norway::from_str(concat!(
            "enums:\n  Direction: {}\n",
            "types:\n  Percent: { typeof: float }\n",
            "slots:\n  owner: { range: Owner }\n  seen: { range: datetime }\n",
            "classes:\n  BikeCounter:\n    attributes:\n",
            "      refStation: { range: Station }\n      refDepot: { range: Depot }\n",
            "      direction: { range: Direction }\n      share: { range: Percent }\n",
            "      kind: { range: 'schema:Thing' }\n",
            "    slot_usage:\n      owner: { range: Kiosk }\n",
        ))
        .expect("yaml");
        let imports = BTreeMap::from([(
            "org.a.v1".to_owned(),
            "classes:\n  Station: {}\n  Kiosk: {}\n".to_owned(),
        )]);
        assert_eq!(
            unknown_ranges(&doc, &imports),
            [
                "slots/owner/range: class 'Owner' is not in this model or its imports; add \
                 class 'Owner' or import the model that defines it",
                "classes/BikeCounter/attributes/refDepot/range: class 'Depot' is not in this \
                 model or its imports; add class 'Depot' or import the model that defines it",
            ]
        );
    }

    #[test]
    fn a_model_lane_follows_its_severity() {
        assert_eq!(model_lane("p", false, "breaking"), Lane::Red);
        assert_eq!(model_lane("p", false, "additive"), Lane::Yellow);
        assert_eq!(model_lane("p", false, "patch"), Lane::Green);
        assert_eq!(model_lane("p", true, "breaking"), Lane::Green);
        assert_eq!(model_lane(ORG_NAMESPACE, true, "patch"), Lane::Red);
    }

    #[test]
    fn confine_linkml_path_keeps_a_source_inside_its_folder() {
        assert_eq!(
            confine_linkml_path("./air-quality.linkml.yaml").expect("relative"),
            "air-quality.linkml.yaml"
        );
        assert_eq!(
            confine_linkml_path("v2/meranie-ovzdušia.linkml.yaml").expect("a folder, Unicode"),
            "v2/meranie-ovzdušia.linkml.yaml"
        );
        let long = format!("{}.linkml.yaml", "a".repeat(4096));
        assert_eq!(confine_linkml_path(&long).expect("long"), long);
        for refused in [
            "",
            "./",
            "/etc/air.linkml.yaml",
            ".//air.linkml.yaml",
            "../air.linkml.yaml",
            "./a/../b.linkml.yaml",
            "a/./b.linkml.yaml",
            "a\\..\\b.linkml.yaml",
            "%2e%2e/%2e%2e/other/secret.linkml.yaml",
            "air.linkml.yaml?ref=other",
            "x#.linkml.yaml",
            "air\n.linkml.yaml",
            "air.yaml",
            "air.linkml.yaml.bak",
        ] {
            assert!(
                confine_linkml_path(refused).is_err(),
                "{refused:?} was accepted"
            );
        }
    }

    fn classes(value: Value) -> Value {
        json!({ "classes": value })
    }

    fn severities(prev: &Value, next: &Value) -> Vec<(String, String)> {
        classify_linkml_changes(prev, next)
            .into_iter()
            .map(|change| (change.severity, change.subject))
            .collect()
    }

    /// F-POR1-015: an inline class attribute and an enum value are part of the contract; removing
    /// either is breaking, adding an optional one is not.
    #[test]
    fn inline_attributes_and_enum_values_are_classified() {
        let prev = json!({
            "classes": { "Air": { "attributes": { "pm10": { "range": "integer" } } } },
            "enums": { "Level": { "permissible_values": { "low": {}, "high": {} } } },
        });
        let next = json!({
            "classes": { "Air": { "attributes": { "no2": { "range": "integer" } } } },
            "enums": { "Level": { "permissible_values": { "low": {}, "mid": {} } } },
        });
        let changes = classify_linkml_changes(&prev, &next);
        let of = |subject: &str| {
            changes
                .iter()
                .find(|c| c.subject == subject)
                .map(|c| c.severity.as_str())
        };
        assert_eq!(of("Air.pm10"), Some("breaking"), "{changes:?}");
        assert_eq!(of("Air.no2"), Some("additive"), "{changes:?}");
        assert_eq!(of("Level.high"), Some("breaking"), "{changes:?}");
        assert_eq!(of("Level.mid"), Some("additive"), "{changes:?}");
        assert_eq!(overall_severity(&changes), "breaking");
    }

    /// DM-23, T-1484: removing is breaking, adding is additive, nothing changed is nothing.
    #[test]
    fn classify_linkml_changes_names_each_class_and_slot_change() {
        let before = json!({
            "classes": { "Station": { "slots": ["name", "bikes"] } },
            "slots": { "name": { "range": "string", "required": true }, "bikes": { "range": "integer" } }
        });
        assert!(severities(&before, &before).is_empty(), "nothing changed");
        assert!(
            severities(&json!({}), &json!({})).is_empty(),
            "two empty schemas"
        );

        let added = json!({ "classes": { "Station": { "slots": ["name", "bikes"] }, "Dock": {} },
                            "slots": before["slots"] });
        assert_eq!(
            severities(&before, &added),
            [("additive".into(), "Dock".into())]
        );
        assert_eq!(
            severities(&added, &before),
            [("breaking".into(), "Dock".into())],
            "a class removed"
        );
        assert_eq!(
            severities(&classes(json!({})), &classes(json!({ "Dock": {} }))),
            [("additive".into(), "Dock".into())]
        );
        assert_eq!(
            severities(&json!({}), &before)
                .iter()
                .filter(|(severity, _)| severity == "breaking")
                .count(),
            1,
            "from an empty schema, only the new required slot breaks"
        );

        let slot = |range: &str| {
            json!({ "classes": before["classes"],
                    "slots": { "name": before["slots"]["name"], "bikes": { "range": range } } })
        };
        assert_eq!(
            severities(&before, &slot("float")),
            [("additive".into(), "bikes".into())],
            "integer widened to float"
        );
        assert_eq!(
            severities(&slot("float"), &slot("integer")),
            [("breaking".into(), "bikes".into())],
            "float narrowed to integer"
        );
        assert_eq!(
            severities(&slot("datetime"), &slot("boolean")),
            [("breaking".into(), "bikes".into())],
            "a slot's type changed"
        );
        let unslotted =
            json!({ "classes": { "Station": { "slots": ["name"] } }, "slots": before["slots"] });
        assert_eq!(
            severities(&before, &unslotted),
            [("breaking".into(), "Station.bikes".into())]
        );
    }

    fn version(raw: &str) -> SemVer {
        SemVer::new(raw).expect("a version")
    }

    /// DM-22, T-6268: a create compares against nothing, so its required slots count as breaking;
    /// the gate lets them through at 0.1.0 and still refuses the same change of a published model.
    #[test]
    fn only_a_published_model_refuses_a_break_under_its_major() {
        let next: Value = serde_norway::from_str(
            "classes:\n  Park:\n    attributes:\n      name: { range: string, required: true }\n",
        )
        .expect("yaml");
        let changes = classify_linkml_changes(&Value::Null, &next);
        assert_eq!(overall_severity(&changes), "breaking");
        let first = version("0.1.0");
        assert!(refuse_unversioned_break(false, &changes, &first, &first).is_ok());
        let Err(ApiError::Invalid { detail, errors }) =
            refuse_unversioned_break(true, &changes, &first, &first)
        else {
            panic!("a published model's break under its major is refused");
        };
        assert!(detail.contains("publish it as 1.0.0"), "{detail}");
        assert!(!errors.is_empty());
        assert!(refuse_unversioned_break(true, &changes, &first, &version("1.0.0")).is_ok());
    }

    /// DM-22, DM-23, T-1484: breaking bumps the major, additive the minor, anything else the patch; a
    /// part already at its largest has no next version and says so.
    #[test]
    fn bump_version_moves_the_part_the_severity_names() {
        let current = version("1.4.2");
        let bumped = |severity: &str| {
            bump_version(&current, severity)
                .expect(severity)
                .to_string()
        };
        assert_eq!(bumped("breaking"), "2.0.0");
        assert_eq!(bumped("additive"), "1.5.0");
        assert_eq!(bumped("none"), "1.4.3");
        assert_eq!(bumped("an unknown severity"), "1.4.3");
        assert_eq!(bumped(""), "1.4.3");

        let max = u32::MAX;
        for (raw, severity) in [
            (format!("{max}.0.0"), "breaking"),
            (format!("0.{max}.0"), "additive"),
            (format!("0.0.{max}"), "none"),
        ] {
            let error = bump_version(&version(&raw), severity).expect_err(&raw);
            assert!(error.to_string().contains(&raw), "{error}");
        }
    }
}
