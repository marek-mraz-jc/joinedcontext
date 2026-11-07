//! Moving a project and its credentials through the registry (AG-59, CC-48, T-0840).
//!
//! Exporting a project, importing a bundle and writing a data model's LinkML were reachable
//! through the Portal's routes alone. Each operation here calls the route's own function, so the
//! grants, the refusals and the answers are the route's; nothing here decides anything of its own.

use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, NAME, QUERY, TERM, TITLE};
use super::runs::as_user;
use super::{
    change_schema, dry_run_output_schema, parse_input, OpError, Operation, OperationAnnotations,
};
use crate::change::Lane;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExportInput {
    /// `yaml` or `json`; the `zip` a browser downloads is a file, not an answer to a call.
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub kinds: Option<String>,
    #[serde(default)]
    pub names: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ImportInput {
    /// The bundle itself: the document an export answered.
    pub bundle: String,
    #[serde(default)]
    pub org_domain: Option<String>,
    #[serde(default)]
    pub conflict_policy: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CreateProjectInput {
    /// The slug: the `{project}` segment of every path of it (PF-67).
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeleteProjectInput {
    /// The project to delete, named in full: this change is not undone by a second one.
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourcePutInput {
    pub name: String,
    /// The LinkML document, as the editor writes it.
    pub source: String,
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

async fn delete_project(
    caller: &super::Caller,
    state: &AppState,
    project: &str,
    val: Value,
) -> Result<Value, OpError> {
    let input: DeleteProjectInput = parse_input(val)?;
    // The ceiling (an agent profile, a connector scope) was judged on the project the call is
    // addressed to: the name confirms that one and no other (JC1-01).
    if input.name != project {
        return Err(OpError::InvalidInput {
            path: "/name".into(),
            message: format!(
                "this call is addressed to project {project}: name that project to delete it, \
                 or address the call to {}",
                input.name
            ),
        });
    }
    let change =
        crate::api::projects::delete_project_for(state, &caller.identity, &input.name).await?;
    Ok(crate::api::mutate::ProposeOutcome::Change(change).into_value())
}

fn export_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "format": { "type": "string", "enum": ["yaml", "json"], "description": "The bundle's format; yaml when absent" },
            "kinds": text("Comma-separated plurals, e.g. endpoints,pipelines", QUERY),
            "names": text("Comma-separated metadata.name values, e.g. doprava", QUERY),
            "revision": text("A commit of the configuration repository", TERM)
        },
        "additionalProperties": false
    })
}

fn export_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "format": { "type": "string" },
            "document": { "type": "string", "description": "The bundle, as the download holds it" }
        },
        "required": ["format", "document"]
    })
}

fn create_project_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": text("The project slug, a DNS-1123 label", 63),
            "displayName": text("What people call it; the slug when absent", TITLE),
            "description": text("One line about what the project is for", TITLE)
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn delete_project_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": text("The project to delete, typed out: this is not undone", 63)
        },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn import_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "bundle": text(
                "The bundle document an export answered",
                crate::api::import::MAX_UPLOAD_BYTES as u64
            ),
            "orgDomain": text("Organisation imported ids are rewritten to; this one when absent", NAME),
            "conflictPolicy": {
                "type": "string",
                "enum": ["fail", "skip", "replace", "rename"],
                "description": "For a resource the project has; absent refuses the import"
            },
            "dryRun": { "type": "boolean", "description": "Answer the plan and write nothing" }
        },
        "required": ["bundle"],
        "additionalProperties": false
    })
}

fn import_output_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "description": "The plan alone, when `dryRun` is true",
                "properties": {
                    "created": { "type": "array", "items": { "type": "object" } },
                    "updated": { "type": "array", "items": { "type": "object" } },
                    "conflicts": { "type": "array", "items": { "type": "object" } }
                }
            },
            change_schema()
        ]
    })
}

fn source_put_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": text("The DataModel's name", NAME),
            "source": text(
                "The LinkML document",
                crate::tools::model_tools::MAX_REQUEST_BYTES as u64
            ),
            "space": text("The space a new model is created in", NAME),
            "version": text("The version to write; the computed next when absent", TERM),
            "dryRun": { "type": "boolean", "description": "Answer the check and write nothing" }
        },
        "required": ["name", "source"],
        "additionalProperties": false
    })
}

fn source_put_output_schema() -> Value {
    json!({ "oneOf": [dry_run_output_schema(), change_schema()] })
}

/// A route that answers a `Change` as its response, read back as an operation answers it: the id
/// and the lane beside the document, like every other propose (T-0838).
pub(super) async fn proposed(response: axum::response::Response) -> Result<Value, OpError> {
    let accepted = response.status() == axum::http::StatusCode::ACCEPTED;
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .map_err(|err| OpError::Api(ApiError::Internal(err.to_string())))?;
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if !accepted {
        // A dry run answers the check, not a change.
        return Ok(body);
    }
    let change: crate::change::Change = serde_json::from_value(body)
        .map_err(|err| OpError::Api(ApiError::Internal(err.to_string())))?;
    Ok(crate::api::mutate::ProposeOutcome::Change(change).into_value())
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_project_create",
            title: "Open A Project",
            description: "Opens a project, with the opener's steward binding in the same change; the organization's own setting says who may",
            input: create_project_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            // Who may open a project is `spec.projects.creation` of the Organization, not a rule
            // over a kind, so the route is what decides and the registry does not pre-judge it
            // (PF-65, AG-59).
            kind: "*",
            verb: None,
            lane: Lane::Yellow,
            validate: |val| parse_input::<CreateProjectInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let input: CreateProjectInput = parse_input(val)?;
                    let (_, axum::Json(change)) = crate::api::projects::open_project(
                        as_user(caller),
                        State(state.clone()),
                        axum::Json(crate::api::projects::OpenProject {
                            name: input.name,
                            display_name: input.display_name,
                            description: input.description,
                        }),
                    )
                    .await?;
                    Ok(crate::api::mutate::ProposeOutcome::Change(change).into_value())
                })
            },
        },
        Operation {
            name: "jc_project_delete",
            title: "Delete A Project",
            description: "Proposes the one red-lane change that removes a project and every space, endpoint, app, service account, role and binding written for it",
            input: delete_project_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: false,
            },
            // The route checks `delete` on `Project` where the caller is bound, which is the
            // project itself and not the one the call was addressed to, so the registry leaves
            // the decision to it (PF-77, AG-59).
            kind: "*",
            verb: None,
            lane: Lane::Red,
            validate: |val| parse_input::<DeleteProjectInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| Box::pin(delete_project(caller, state, project, val)),
        },
        Operation {
            name: "jc_project_export",
            title: "Export Project",
            description: "The project's manifests as one bundle, narrowed to the kinds and names asked for; the whole project is for organization administrators",
            input: export_input_schema,
            output: export_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ExportInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ExportInput = parse_input(val)?;
                    let format = input.format.unwrap_or_else(|| "yaml".to_owned());
                    if !matches!(format.as_str(), "yaml" | "json") {
                        return Err(OpError::InvalidInput {
                            path: "/format".into(),
                            message: "an export through an operation is yaml or json; the zip is \
                                      a download of the Portal"
                                .into(),
                        });
                    }
                    let response = crate::api::export::export(
                        as_user(caller),
                        State(state.clone()),
                        Path(project.to_owned()),
                        Query(crate::api::export::ExportQuery {
                            format: Some(format.clone()),
                            revision: input.revision,
                            kinds: input.kinds,
                            names: input.names,
                        }),
                    )
                    .await?;
                    let bytes = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024)
                        .await
                        .map_err(|err| OpError::Api(ApiError::Internal(err.to_string())))?;
                    Ok(json!({
                        "format": format,
                        "document": String::from_utf8_lossy(&bytes),
                    }))
                })
            },
        },
        Operation {
            name: "jc_project_import",
            title: "Import A Bundle",
            description: "Imports a bundle into the project as one change a person approves, or answers the plan alone",
            input: import_input_schema,
            output: import_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Yellow,
            validate: |val| parse_input::<ImportInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ImportInput = parse_input(val)?;
                    let conflict_policy = match input.conflict_policy.as_deref() {
                        None => crate::api::import::ConflictPolicy::default(),
                        Some("fail") => crate::api::import::ConflictPolicy::Fail,
                        Some("skip") => crate::api::import::ConflictPolicy::Skip,
                        Some("replace") => crate::api::import::ConflictPolicy::Replace,
                        Some("rename") => crate::api::import::ConflictPolicy::Rename,
                        Some(other) => {
                            return Err(OpError::InvalidInput {
                                path: "/conflictPolicy".into(),
                                message: format!(
                                    "conflictPolicy '{other}' is not fail, skip, replace or \
                                     rename (MF-23)"
                                ),
                            })
                        }
                    };
                    let options = crate::api::import::ImportOptions {
                        target_namespace: None,
                        org_domain: input.org_domain,
                        conflict_policy,
                        dry_run: input.dry_run,
                        url: None,
                        manifests: None,
                        space_mapping: Vec::new(),
                    };
                    let (_, body) = crate::api::import::import_bundle(
                        state,
                        &caller.identity,
                        project,
                        input.bundle.as_bytes(),
                        options,
                    )
                    .await?;
                    Ok(body)
                })
            },
        },
        Operation {
            name: "jc_model_source_put",
            title: "Write Model Source",
            description: "Writes one DataModel's LinkML and its generated artifacts as a change a person approves",
            input: source_put_input_schema,
            output: source_put_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "DataModel",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<SourcePutInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: SourcePutInput = parse_input(val)?;
                    let response = crate::api::datamodels::put_source(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.name)),
                        Query(crate::api::datamodels::SourcePutQuery {
                            version: input.version,
                            dry_run: input.dry_run.then(|| "all".to_owned()),
                            validate_only: None,
                            space: input.space,
                        }),
                        input.source.into_bytes().into(),
                    )
                    .await?;
                    proposed(response).await
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JC1-01: the delete op names the project it is addressed to, or nothing is deleted.
    #[tokio::test]
    async fn a_delete_naming_another_project_is_refused() {
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let caller = super::super::Caller::new(
            crate::auth::session::Identity {
                client: None,
                subject: "f:1:ana".into(),
                username: "ana".into(),
                email: None,
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: true,
            },
            super::super::Via::Mcp,
        );
        let refused = delete_project(&caller, &state, "ovzdusie", json!({ "name": "doprava" }))
            .await
            .expect_err("another project's name");
        assert!(
            matches!(&refused, OpError::InvalidInput { path, .. } if path == "/name"),
            "{refused:?}"
        );
    }
}
