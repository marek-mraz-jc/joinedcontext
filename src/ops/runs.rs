//! The agent runs, through the registry (AG-59, CC-48, T-0840).
//!
//! Starting an application, cancelling a run and publishing what it built had a route and no
//! operation, so an MCP client could ask the Portal for anything except the one thing the
//! Portal is for. Each operation calls the route's own handler, so the checks, the quotas and
//! the answers are the route's: nothing here decides anything of its own.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, texts, ID, NAME, RECORD_MEMBERS, TERM, TEXT};
use super::{parse_input, Caller, OpError, Operation, OperationAnnotations};
use crate::auth::session::{CurrentUser, Session};
use crate::change::Lane;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunIdInput {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AnswerInput {
    pub id: String,
    pub question_id: String,
    /// The answers themselves, shaped by the question the run asked (AG-45).
    pub answers: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MessageInput {
    pub id: String,
    pub text: String,
    /// On a conversation: the endpoints the assistant may query from this message on (AG-75).
    #[serde(default)]
    pub endpoint_names: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FlowInput {
    /// The organisation's Blueprint to run.
    pub blueprint: String,
    /// The version the form was generated from; a mismatch is a conflict, not an expansion
    /// against a schema nobody saw (CC-26).
    pub version: String,
    pub parameters: Value,
}

fn run_id_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "id": text("The run's id", ID) },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn create_input_schema() -> Value {
    json!({
        "type": "object",
        "description": "What an unattended run is given (AG-69, AP-44); the same body the route takes",
        "properties": {
            "appName": text("The App manifest's name", NAME),
            "endpointName": text("The one Endpoint read; endpointNames for several", NAME),
            "endpointNames": {
                "type": "array",
                "description": "The Endpoints read, the first the primary",
                "items": { "type": "string", "maxLength": NAME },
                "maxItems": 5
            },
            "profile": text("The AgentProfile; app-builder when absent", NAME),
            "appClass": {
                "type": "string",
                "description": "How the App is built and served",
                "enum": ["ui", "ui-rust", "wasm"]
            },
            "visibility": {
                "type": "string",
                "description": "Who may reach the published App; public is refused",
                "enum": ["private", "project", "organization"]
            },
            "prompt": text("What the application should do, in the person's own words", TEXT),
            "kind": {
                "type": "string",
                "description": "What kind of run to execute",
                "enum": ["application", "dashboard", "analysis"]
            },
            "unattended": {
                "type": "boolean",
                "description": "true: the run asks the person nothing"
            },
            "dataNeeds": {
                "type": "array",
                "description": "As the App's spec.dataNeeds: contextSpaceRef, types, attrs, operations, q, scopeQ, geoQ; checked against the endpoint",
                "items": { "type": "object", "maxProperties": RECORD_MEMBERS },
                "minItems": 1
            }
        },
        "required": ["appName", "appClass", "prompt", "dataNeeds"],
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

/// A run as a tool answers it (T-6487): its page in the Portal as `url` and the preview's full
/// address, which the record keeps relative to the Portal. An MCP client has no Portal to resolve
/// it against.
pub(super) fn with_pages(state: &crate::state::AppState, mut run: Value) -> Value {
    let base = state.config.public_base_url.as_str().trim_end_matches('/');
    let (Some(project), Some(id)) = (run["project"].as_str(), run["id"].as_str()) else {
        return run;
    };
    let url = format!("{base}/projects/{project}/apps?run={id}");
    let preview = run["previewUrl"]
        .as_str()
        .filter(|path| path.starts_with('/'))
        .map(|path| format!("{base}{path}"));
    if let Some(doc) = run.as_object_mut() {
        doc.insert("url".into(), json!(url));
        if let Some(preview) = preview {
            doc.insert("previewUrl".into(), json!(preview));
        }
    }
    run
}

/// The caller as the route's handlers read them.
///
/// The operations run behind the same permission checks the routes make, and those read a
/// session; this is that identity in the shape they take. Nothing is widened: the tokens and
/// the ticket stay where they were, and an agent is refused above by [`refuse_agent`].
pub(super) fn as_user(caller: &Caller) -> CurrentUser {
    let now = crate::auth::session::now_unix();
    CurrentUser(Session {
        identity: caller.identity.clone(),
        expires_at: now + 60,
        issued_at: now,
        signed_in_at: None,
        id_token: String::new(),
        access_expires_at: now + 60,
        refresh_token: None,
    })
}

/// Why an agent run never takes this operation, by name, or `None` when it may (AG-11).
///
/// A run acts for the person who started it, so driving another run and answering the question a
/// run asked that person are the person's. The reason is looked up by name because one sentence
/// for all of them made a refused answer read as a refused run, which tells the reader nothing
/// they can act on.
///
/// [`super::Caller::may_run`] asks this before an operation is listed or run, so the refusal
/// arrives where `GET …/ops` can see it: the listing names exactly what the caller would be let
/// through (API/01 §21), and a refusal that lived only in the body had the registry offer a run an
/// operation it would always be denied.
pub(super) fn agent_refusal(operation: &str) -> Option<&'static str> {
    Some(match operation {
        "jc_run_create" | "jc_run_cancel" | "jc_run_publish" => {
            "an agent run does not start, cancel or publish another run; a person does (AG-11)"
        }
        // A run asking a question and a run answering it would be a run deciding for the person it
        // acts for (AG-11, AG-45).
        "jc_run_answer" => {
            "an agent run or an MCP client does not answer the question a run asked the \
             person it acts for; a person does, in the Portal (AG-11, AG-45)"
        }
        _ => return None,
    })
}

/// [`agent_refusal`] applied to a caller: the second lock, in the body of the operation itself, so
/// the rule holds even for a body reached by some other path than [`super::call`].
pub(super) fn refuse_agent(caller: &Caller, operation: &str) -> Result<(), OpError> {
    match agent_refusal(operation) {
        Some(why) if caller.via == super::Via::Agent => {
            Err(OpError::Api(crate::error::ApiError::Denied(why.into())))
        }
        // The answer is the person's decision: an MCP client is a tool a model drives even with
        // the person's token (T-1005), so it never answers for them (T-5699).
        Some(why) if operation == "jc_run_answer" && caller.via == super::Via::Mcp => {
            Err(OpError::Api(crate::error::ApiError::Denied(why.into())))
        }
        _ => Ok(()),
    }
}

fn answer_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": text("The run's id", ID),
            "questionId": text("The question the run asked", ID),
            "answers": {
                "type": "object",
                "description": "The answers, shaped by that question (AG-45)",
                "maxProperties": RECORD_MEMBERS
            }
        },
        "required": ["id", "questionId", "answers"],
        "additionalProperties": false
    })
}

fn message_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": text("The run's id", ID),
            "text": text("The message, in the person's own words", TEXT),
            "endpointNames": texts(
                "On a conversation: the endpoints the assistant may query from now on",
                NAME
            )
        },
        "required": ["id", "text"],
        "additionalProperties": false
    })
}

fn accepted_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "accepted": { "type": "boolean" } },
        "required": ["accepted"]
    })
}

fn flow_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "blueprint": text("The organisation Blueprint to run", NAME),
            "version": text("The version the parameters were filled against", TERM),
            "parameters": {
                "type": "object",
                "description": "What the blueprint's parameterSchema asks for",
                "maxProperties": RECORD_MEMBERS
            }
        },
        "required": ["blueprint", "version", "parameters"],
        "additionalProperties": false
    })
}

/// The SDK's services catalog (T-3585, ADR-N-045): what an agent building an App may call, the
/// `app.yaml` lines each service needs and its quotas. Generated in the SDK's tests from API.md.
pub const SERVICES_CATALOG: &str = include_str!("../../sdk/services.json");

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_app_services",
            title: "App Services Catalog",
            description: "The SDK's services: calls, app.yaml lines, quotas",
            input: || json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            output: || json!({ "type": "object", "required": ["services", "kit"] }),
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            // The SDK's own reference, public in the package: no project's data.
            kind: "*",
            verb: None,
            lane: Lane::Green,
            // The input schema allows no member, so there is nothing more to check.
            validate: |_| Ok(()),
            run: |_caller, _state, _project, _val| {
                Box::pin(async move { Ok(serde_json::from_str(SERVICES_CATALOG)?) })
            },
        },
        Operation {
            name: "jc_run_create",
            title: "Start Unattended Work",
            description: "Starts an application, dashboard or analysis run, which waits for a person's approval at the end",
            input: create_input_schema,
            output: run_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "App",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| {
                if val.get("appName").and_then(Value::as_str).is_none() {
                    return Err(OpError::InvalidInput {
                        path: "/appName".into(),
                        message: "missing field `appName`".into(),
                    });
                }
                Ok(())
            },
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller, "jc_run_create")?;
                    let request = super::parse_input(val)?;
                    let (_, Json(mut created)) = crate::api::agent_runs::create_run(
                        as_user(caller),
                        // Through the registry a run is a person's: no header reaches it (AG-93).
                        crate::agents::run::RunOrigin::Person,
                        State(state.clone()),
                        Path(project.to_owned()),
                        // No token reaches the registry, so a write need cannot be checked
                        // against the caller's grants and is refused (AP-132).
                        axum::http::HeaderMap::new(),
                        Json(request),
                    )
                    .await?;
                    // A ticket is for who drives the workspace, never a model's transcript.
                    if caller.via == super::Via::Mcp {
                        created.ticket = None;
                    }
                    Ok(with_pages(state, serde_json::to_value(created)?))
                })
            },
        },
        Operation {
            name: "jc_run_cancel",
            title: "Cancel Run",
            description: "Stops one of this caller's runs; what it had not finished is not published",
            input: run_id_input_schema,
            output: run_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: true,
            },
            kind: "App",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<RunIdInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller, "jc_run_cancel")?;
                    let input: RunIdInput = parse_input(val)?;
                    let Json(run) = crate::api::agent_runs::cancel_run(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.id)),
                    )
                    .await?;
                    Ok(serde_json::to_value(run)?)
                })
            },
        },
        Operation {
            name: "jc_run_publish",
            title: "Publish What A Run Built",
            description: "Proposes the application a finished run built; a person approves the change",
            input: run_id_input_schema,
            output: super::change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "App",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<RunIdInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller, "jc_run_publish")?;
                    let input: RunIdInput = parse_input(val)?;
                    // The route answers the `Change` as a response; the operation answers the
                    // document inside it, as every other propose does.
                    let response = crate::api::agent_runs::publish_run(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.id)),
                    )
                    .await?;
                    super::admin::proposed(response).await
                })
            },
        },
        Operation {
            name: "jc_run_answer",
            title: "Answer A Run's Question",
            description: "Answers one question a run asked, so it carries on; a person answers, never another run",
            input: answer_input_schema,
            output: accepted_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "App",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<AnswerInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent(caller, "jc_run_answer")?;
                    let input: AnswerInput = parse_input(val)?;
                    crate::api::agent_runs::answer_question(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.id)),
                        Default::default(),
                        Json(crate::api::agent_runs::AnswerRequest {
                            question_id: input.question_id,
                            answers: input.answers,
                        }),
                    )
                    .await?;
                    Ok(json!({ "accepted": true }))
                })
            },
        },
        Operation {
            name: "jc_run_message",
            title: "Send A Run A Message",
            description: "Sends text to one of this caller's running conversations or applications",
            input: message_input_schema,
            output: accepted_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "App",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<MessageInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: MessageInput = parse_input(val)?;
                    crate::api::agent_runs::post_message(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.id)),
                        Default::default(),
                        Json(crate::api::agent_runs::MessageRequest {
                            text: input.text,
                            endpoint_names: input.endpoint_names,
                            page_context: None,
                            access: None,
                            scope: None,
                        }),
                    )
                    .await?;
                    Ok(json!({ "accepted": true }))
                })
            },
        },
        Operation {
            name: "jc_flow_start",
            title: "Run A Blueprint",
            description: "Expands one of the organisation's blueprints with the parameters given, as one change: a green flow merges at once, anything stricter waits for a person's approval",
            input: flow_input_schema,
            output: super::change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Yellow,
            validate: |val| parse_input::<FlowInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: FlowInput = parse_input(val)?;
                    let response = crate::api::blueprints::start_flow(
                        as_user(caller),
                        State(state.clone()),
                        Path(project.to_owned()),
                        Json(crate::api::blueprints::FlowRequest {
                            blueprint: input.blueprint,
                            version: input.version,
                            parameters: input.parameters,
                        }),
                    )
                    .await?;
                    super::admin::proposed(response).await
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JC2-46: jc_run_create offers no kind its route refuses (a conversation starts at
    /// /assistant/conversations).
    #[test]
    fn jc_run_create_offers_no_conversation_kind() {
        let op = operations()
            .into_iter()
            .find(|op| op.name == "jc_run_create")
            .expect("op");
        let kinds = (op.input)()["properties"]["kind"]["enum"].clone();
        assert!(!kinds.to_string().contains("conversation"), "{kinds}");
    }

    /// T-6487: a run's answer names its Portal page and a preview an MCP client can open.
    #[tokio::test]
    async fn a_run_names_its_page_and_a_whole_preview_address() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let base = state
            .config
            .public_base_url
            .as_str()
            .trim_end_matches('/')
            .to_owned();
        let run = with_pages(
            &state,
            json!({
                "id": "r1",
                "project": "doprava",
                "previewUrl": "/api/v1/projects/doprava/agent-runs/r1/preview?v=2",
            }),
        );
        assert_eq!(run["url"], format!("{base}/projects/doprava/apps?run=r1"));
        assert_eq!(
            run["previewUrl"],
            format!("{base}/api/v1/projects/doprava/agent-runs/r1/preview?v=2")
        );
        // Before the first version there is no preview to name.
        let queued = with_pages(&state, json!({ "id": "r1", "project": "doprava" }));
        assert!(queued.get("previewUrl").is_none());
    }

    /// JC2-23: jc_run_publish answers what its outputSchema declares, the Change wrapped as
    /// every other propose answers it, not the route's bare Change.
    #[tokio::test]
    async fn a_published_run_answers_its_output_schema() {
        let change = crate::change::Change::new(
            crate::change::ChangeMeta::new("chg-0123abcd", "demo"),
            crate::change::ChangeStatus::new(
                Lane::Yellow,
                crate::change::ChangePhase::PendingApproval,
                crate::change::PlanSummary::new(1, 0, 0),
            )
            .with_merge_request("https://forge.example/demo/pulls/1".to_owned()),
        );
        let response = axum::response::IntoResponse::into_response((
            axum::http::StatusCode::ACCEPTED,
            Json(change),
        ));
        let answer = super::super::admin::proposed(response)
            .await
            .expect("answer");
        let op = operations()
            .into_iter()
            .find(|op| op.name == "jc_run_publish")
            .expect("op");
        let schema = (op.output)();
        let validator = jsonschema::validator_for(&schema).expect("schema");
        let problems: Vec<String> = validator
            .iter_errors(&answer)
            .map(|e| e.to_string())
            .collect();
        assert!(problems.is_empty(), "{problems:?} in {answer}");
    }
}
