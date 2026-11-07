//! The reads the Portal serves that had no operation (AG-59, CC-48, T-0840).
//!
//! A route the registry does not know is a defect: what a browser can read through the Portal,
//! an MCP client and the assistant read through the operation behind the same function. These
//! are the read-only ones — a pipeline's counters, what happened in the project, the federation
//! of a project, and a data model's LinkML — each calling exactly what its route calls.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, ID, NAME, QUERY, TERM};
use super::runs::as_user;
use super::{parse_input, Caller, EmptyInput, OpError, Operation, OperationAnnotations};
use crate::change::Lane;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NamedInput {
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActivityInput {
    #[serde(default)]
    pub space: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IdInput {
    pub id: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunListInput {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Only the caller's own runs.
    #[serde(default)]
    pub mine: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RevisionsInput {
    #[serde(default)]
    pub limit: Option<usize>,
}

fn named_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "name": text("The resource's name", NAME) },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn metrics_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "What the runner has counted for this pipeline since it started (PL-46)",
        "properties": {
            "pipeline": { "type": "string" },
            "state": { "type": "string" },
            "received": { "type": "integer" },
            "sent": { "type": "integer" },
            "errors": { "type": "integer" },
            "messagesPerSecond": { "type": "number" },
            "latencyP99Ms": { "type": "number" },
            "lastError": { "type": "string" }
        }
    })
}

fn activity_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "space": text("Events of this context space", NAME),
            "kind": text("One or more event kinds, comma-separated", QUERY),
            "source": text("Events of this component, e.g. reconciler", TERM),
            "severity": { "type": "string", "enum": ["info", "warning", "error"], "description": "This severity and everything above it" },
            "since": { "type": "string", "format": "date-time", "description": "RFC 3339 instant, the oldest event to return", "maxLength": 64 },
            "object": text("Events of one object, as {plural}/{name}", NAME),
            "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "At most this many events" },
            "cursor": text("The previous page's cursor", ID)
        },
        "additionalProperties": false
    })
}

fn activity_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "time": { "type": "string", "format": "date-time" },
                        "project": { "type": "string" },
                        "space": { "type": "string" },
                        "kind": { "type": "string" },
                        "source": { "type": "string" },
                        "summary": { "type": "string" },
                        "severity": { "type": "string" },
                        "details": { "type": "object" }
                    }
                }
            },
            "next": { "type": "string", "description": "The cursor of the next page, when there is one" }
        },
        "required": ["items"]
    })
}

fn graph_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The project's spaces, endpoints, pipelines, apps, catalogues and registrations, and what links them (EP-58)",
        "properties": {
            "nodes": { "type": "array", "items": { "type": "object" } },
            "edges": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["nodes", "edges"]
    })
}

fn source_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "source": { "type": "string", "description": "The model's LinkML, as the repository holds it" }
        },
        "required": ["name", "source"]
    })
}

fn id_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "id": text("The id of the one to read", ID) },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn change_proposal_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "One proposed change: what it does, who wrote it and where it stands",
        "properties": {
            "apiVersion": { "type": "string" },
            "kind": { "type": "string", "enum": ["Change"] },
            "metadata": { "type": "object" },
            "status": { "type": "object" },
            "url": { "type": "string", "description": "The Portal page where a person decides it" },
            "summary": { "type": "object" },
            "author": { "type": "object" },
            "createdAt": { "type": "string", "format": "date-time" },
            "planFields": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["apiVersion", "kind", "metadata", "status"]
    })
}

fn run_list_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "description": "At most this many runs, newest first" },
            "app": text("Runs building this App, by name", NAME),
            "kind": {
                "type": "string",
                "description": "Only runs of this kind",
                "enum": ["application", "dashboard", "analysis", "conversation"]
            },
            "status": {
                "type": "string",
                "description": "Only runs in this state",
                "enum": [
                    "queued", "starting", "interviewing", "building", "testing", "previewing",
                    "awaiting_approval", "published", "failed", "cancelled", "expired"
                ]
            },
            "mine": { "type": "boolean", "description": "Only the runs this caller started" }
        },
        "additionalProperties": false
    })
}

fn run_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "project": { "type": "string" },
            "appName": { "type": "string" },
            "kind": { "type": "string" },
            "status": { "type": "string" },
            "createdBy": { "type": "string" },
            "createdAt": { "type": "string", "format": "date-time" },
            "url": { "type": "string", "description": "The Portal page following this run" },
            "previewUrl": { "type": "string", "description": "The preview once a version is built" }
        },
        "required": ["id", "status"]
    })
}

fn run_list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "items": { "type": "array", "items": run_output_schema() } },
        "required": ["items"]
    })
}

fn ckan_status_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "Every catalogue this project publishes to, and what each endpoint's publication is doing",
        "properties": {
            "instances": { "type": "array", "items": { "type": "object" } },
            "publications": { "type": "array", "items": { "type": "object" } }
        },
        "required": ["instances", "publications"]
    })
}

fn revisions_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "At most this many revisions, newest first" } },
        "additionalProperties": false
    })
}

fn project_usage_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The project and what it holds of each quota (PF-75)",
        "properties": {
            "metadata": { "type": "object" },
            "spec": { "type": "object" },
            "status": {
                "type": "object",
                "properties": {
                    "usage": {
                        "type": "object",
                        "description": "Per dimension: `used`, and `limit` when a quota sets one",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "used": { "type": "integer" },
                                "limit": { "type": "integer" }
                            }
                        }
                    }
                }
            }
        },
        "required": ["status"]
    })
}

fn endpoints_everywhere_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "Every Endpoint the caller may read, in every project; each item carries its project in metadata.namespace",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "metadata": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" },
                                "namespace": { "type": "string", "description": "The project the Endpoint lives in" }
                            }
                        },
                        "spec": { "type": "object" }
                    }
                }
            }
        },
        "required": ["items"]
    })
}

fn revisions_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The project's history: who changed what, and the sha an export reads from",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "sha": { "type": "string" },
                        "message": { "type": "string" },
                        "author": { "type": "string" }
                    },
                    "required": ["sha"]
                }
            }
        },
        "required": ["items"]
    })
}

/// A read needs the caller's `read` on the kind, the way every other door reads (PF-59). A kind
/// no binding of theirs reads is not there, never refused by name (R20).
fn readable(caller: &Caller, state: &AppState, project: &str, kind: &str) -> Result<(), OpError> {
    if crate::permissions::for_request(state, &caller.identity, project).may_read(kind) {
        return Ok(());
    }
    Err(OpError::Api(crate::error::ApiError::NotFound(format!(
        "kind '{kind}' not found in project '{project}'"
    ))))
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_pipeline_metrics",
            title: "Read Pipeline Counters",
            description: "What the runner has counted for one pipeline: state, messages, errors and latency",
            input: named_input_schema,
            output: metrics_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Pipeline",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NamedInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NamedInput = parse_input(val)?;
                    readable(caller, state, project, "Pipeline")?;
                    let metrics =
                        crate::api::pipelines::metrics_for(state, project, &input.name).await?;
                    Ok(serde_json::to_value(metrics)?)
                })
            },
        },
        Operation {
            name: "jc_activity_list",
            title: "List Activity",
            description: "What happened in the project: the same events, filters and paging the activity page reads",
            input: activity_input_schema,
            output: activity_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ActivityInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ActivityInput = parse_input(val)?;
                    let query = crate::api::activity::ActivityQuery {
                        space: input.space,
                        kind: input.kind,
                        source: input.source,
                        severity: input.severity,
                        since: input.since,
                        object: input.object,
                        connector: None,
                        limit: input.limit,
                        cursor: input.cursor,
                    };
                    let mut filter = query.into_filter()?;
                    // A space's events reach only the readers bound to it, as the REST list
                    // shows them (T-4876, T-5698).
                    filter.reach =
                        crate::permissions::for_request(state, &caller.identity, project)
                            .spaces_reached();
                    let page = state
                        .activity
                        .list(project, &filter)
                        .await
                        .map_err(|err| crate::error::ApiError::Unavailable(err.to_string()))?
                        // The REST page keeps its rows whole; a model reads at most PAGE_BYTES
                        // and pages on (T-6291).
                        .within(super::bounds::PAGE_BYTES);
                    Ok(json!({ "items": page.items, "next": page.next }))
                })
            },
        },
        Operation {
            name: "jc_federation_graph",
            title: "Read Federation",
            description: "The project's spaces, endpoints, pipelines and apps, and what links them",
            input: super::empty_input_schema,
            output: graph_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<EmptyInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let _: EmptyInput = parse_input(val)?;
                    let effective =
                        crate::permissions::for_request(state, &caller.identity, project);
                    Ok(serde_json::to_value(crate::api::federation::graph_of(
                        state, project, &effective,
                    ))?)
                })
            },
        },
        Operation {
            name: "jc_model_source_get",
            title: "Read Model Source",
            description: "One DataModel's LinkML, as the repository holds it",
            input: named_input_schema,
            output: source_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "DataModel",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NamedInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NamedInput = parse_input(val)?;
                    readable(caller, state, project, "DataModel")?;
                    // A model of a space the caller is not bound to is not there (T-5698).
                    let effective =
                        crate::permissions::for_request(state, &caller.identity, project);
                    let manifest = state
                        .mirror
                        .get(project, "DataModel", &input.name)
                        .filter(|envelope| effective.may_read_envelope("DataModel", envelope))
                        .ok_or_else(|| {
                            crate::error::ApiError::NotFound(format!(
                                "DataModel '{}' not found in project '{project}'",
                                input.name
                            ))
                        })?;
                    // A manifest that carries its LinkML is read from the manifest, as every
                    // editor reads it; one that names a file is read from the repository.
                    let inline = Some(manifest)
                        .and_then(|envelope| {
                            envelope
                                .spec
                                .get("linkml")
                                .and_then(Value::as_str)
                                .filter(|text| text.contains('\n'))
                                .map(str::to_owned)
                        });
                    let source = match inline {
                        Some(text) => text,
                        None => {
                            crate::api::datamodels::read_source(state, project, &input.name).await?
                        }
                    };
                    Ok(json!({ "name": input.name, "source": source }))
                })
            },
        },
        Operation {
            name: "jc_change_get",
            title: "Read One Change",
            description: "One proposed change: what it does, who wrote it, its lane and where it stands",
            input: id_input_schema,
            output: change_proposal_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            // As `jc_change_list`: a Change is no manifest kind a profile may grant, and a verbless
            // read is judged by whether the caller may read the project (T-1475, MF-40).
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<IdInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: IdInput = parse_input(val)?;
                    let axum::Json(proposal) = crate::api::changes::get_change(
                        as_user(caller),
                        axum::extract::State(state.clone()),
                        axum::extract::Path((project.to_owned(), input.id)),
                    )
                    .await?;
                    let proposal = serde_json::to_value(proposal)?;
                    Ok(super::changes::with_approval_url(state, project, proposal))
                })
            },
        },
        Operation {
            name: "jc_run_list",
            title: "List Runs",
            description: "The project's agent runs, newest first, narrowed by app, kind or status",
            input: run_list_input_schema,
            output: run_list_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "App",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<RunListInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: RunListInput = parse_input(val)?;
                    let axum::Json(list) = crate::api::agent_runs::list_runs(
                        as_user(caller),
                        crate::agents::run::RunOrigin::Person,
                        axum::extract::State(state.clone()),
                        axum::extract::Path(project.to_owned()),
                        axum::extract::Query(crate::api::agent_runs::ListRunsQuery {
                            limit: input.limit,
                            app: input.app,
                            kind: input.kind,
                            status: input.status,
                            mine: input.mine,
                            origin: None,
                        }),
                    )
                    .await?;
                    Ok(serde_json::to_value(list)?)
                })
            },
        },
        Operation {
            name: "jc_run_get",
            title: "Read One Run",
            description: "One of this caller's runs: what it is building, what it asks and where it stands",
            input: id_input_schema,
            output: run_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "App",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<IdInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: IdInput = parse_input(val)?;
                    let axum::Json(run) = crate::api::agent_runs::get_run(
                        as_user(caller),
                        axum::extract::State(state.clone()),
                        axum::extract::Path((project.to_owned(), input.id)),
                    )
                    .await?;
                    Ok(super::runs::with_pages(state, serde_json::to_value(run)?))
                })
            },
        },
        Operation {
            name: "jc_ckan_status",
            title: "Read Catalogue Publication",
            description: "The catalogues this project publishes to, and what each endpoint's publication is doing",
            input: super::empty_input_schema,
            output: ckan_status_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "CkanInstance",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<EmptyInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let _: EmptyInput = parse_input(val)?;
                    readable(caller, state, project, "CkanInstance")?;
                    let axum::Json(status) = crate::api::ckan::get_status(
                        axum::extract::State(state.clone()),
                        as_user(caller),
                        axum::extract::Path(project.to_owned()),
                    )
                    .await?;
                    Ok(serde_json::to_value(status)?)
                })
            },
        },
        Operation {
            name: "jc_project_get",
            title: "Read A Project",
            description: "The project and what it holds of each quota: context spaces, resident pipelines, public endpoints and apps",
            input: super::empty_input_schema,
            output: project_usage_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<EmptyInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let _: EmptyInput = parse_input(val)?;
                    let axum::Json(detail) = crate::api::projects::get_project(
                        as_user(caller),
                        axum::extract::State(state.clone()),
                        axum::extract::Path(project.to_owned()),
                    )
                    .await?;
                    Ok(serde_json::to_value(detail)?)
                })
            },
        },
        Operation {
            name: "jc_endpoint_list_all",
            title: "List Endpoints Everywhere",
            description: "Every Endpoint of every project with its project, for an organization administrator (PF-61, PF-03)",
            input: super::empty_input_schema,
            output: endpoints_everywhere_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            // The route itself refuses anyone who does not administer the organization with
            // `404` (PF-61, T-2877), so the operation asks for no verb of its own.
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<EmptyInput>(val.clone()).map(|_| ()),
            run: |caller, state, _project, val| {
                Box::pin(async move {
                    let _: EmptyInput = parse_input(val)?;
                    let axum::Json(list) = crate::api::resources::list_endpoints_everywhere(
                        as_user(caller),
                        axum::extract::State(state.clone()),
                        Ok(crate::api::list_query::ListQuery::default()),
                    )
                    .await?;
                    Ok(serde_json::to_value(list)?)
                })
            },
        },
        Operation {
            name: "jc_project_revisions",
            title: "List Project Revisions",
            description: "The project's history: the commits an export can be read from",
            input: revisions_input_schema,
            output: revisions_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<RevisionsInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: RevisionsInput = parse_input(val)?;
                    let axum::Json(list) = crate::api::export::revisions(
                        as_user(caller),
                        axum::extract::State(state.clone()),
                        axum::extract::Path(project.to_owned()),
                        axum::extract::Query(crate::api::export::RevisionsQuery {
                            limit: input.limit,
                        }),
                    )
                    .await?;
                    Ok(serde_json::to_value(list)?)
                })
            },
        },
    ]
}
