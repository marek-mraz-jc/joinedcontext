//! Driving a SyncSource through the registry (AG-59, CC-48, T-0840).
//!
//! A SyncSource pulls a foreign catalogue into the project and proposes what it found. Reading
//! its state, running it now, pausing it and detaching it had routes and no operations, so the
//! one loop nobody watches was the one an MCP client could not ask about. Each operation calls
//! the route's own function; the grants and the answers are the route's.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, NAME};
use super::runs::as_user;
use super::{parse_input, OpError, Operation, OperationAnnotations};
use crate::change::Lane;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NameInput {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PauseInput {
    pub name: String,
    /// `true` switches the loop off, `false` switches it back on.
    pub paused: bool,
}

fn name_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "name": text("The SyncSource's name", NAME) },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn pause_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": text("The SyncSource's name", NAME),
            "paused": { "type": "boolean", "description": "true pauses the loop, false resumes it" }
        },
        "required": ["name", "paused"],
        "additionalProperties": false
    })
}

fn status_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": { "type": "string" },
            "name": { "type": "string" },
            "phase": { "type": "string", "enum": ["Synced", "OutOfSync", "PendingApproval", "Error", "Paused"] },
            "observedRevision": { "type": "string" },
            "lastRunAt": { "type": "integer" },
            "mergeRequest": { "type": "string" },
            "lastError": { "type": "string" }
        },
        "required": ["project", "name", "phase"]
    })
}

fn report_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "What one run of the loop found: the changes it opened, and what it could not do",
        "properties": {
            "proposed": { "type": "array", "items": { "type": "object" } },
            "unchanged": { "type": "integer" },
            "flags": { "type": "array", "items": { "type": "string" } },
            "status": status_output_schema()
        },
        "required": ["proposed", "unchanged", "flags", "status"]
    })
}

fn detached_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The change that removes the SyncSource, or nothing when there was none to remove",
        "properties": {
            "changeId": { "type": "string" },
            "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
            "url": { "type": "string" },
            "change": { "type": "object" }
        }
    })
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_syncsource_status",
            title: "Read Sync State",
            description: "Where one SyncSource stands: its phase, the revision it saw and why the last run stopped",
            input: name_input_schema,
            output: status_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "SyncSource",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NameInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NameInput = parse_input(val)?;
                    // A kind no binding of this caller reads is not there, never refused by
                    // name (PF-59, R20).
                    if !crate::permissions::for_request(state, &caller.identity, project)
                        .may_read("SyncSource")
                    {
                        return Err(OpError::Api(crate::error::ApiError::NotFound(format!(
                            "kind 'SyncSource' not found in project '{project}'"
                        ))));
                    }
                    let Json(status) = crate::api::sync_sources::status(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.name)),
                    )
                    .await?;
                    Ok(serde_json::to_value(status)?)
                })
            },
        },
        Operation {
            name: "jc_syncsource_sync",
            title: "Sync Now",
            description: "Runs one SyncSource at once; what it finds becomes changes a person approves",
            input: name_input_schema,
            output: report_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "SyncSource",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<NameInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NameInput = parse_input(val)?;
                    let Json(report) = crate::api::sync_sources::sync_now(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.name)),
                    )
                    .await?;
                    Ok(serde_json::to_value(report)?)
                })
            },
        },
        Operation {
            name: "jc_syncsource_pause",
            title: "Pause Or Resume Syncing",
            description: "Switches one SyncSource's loop off, or back on; nothing already proposed is touched",
            input: pause_input_schema,
            output: status_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "SyncSource",
            verb: Some(jc_core::kinds::Verb::Propose),
            lane: Lane::Yellow,
            validate: |val| parse_input::<PauseInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: PauseInput = parse_input(val)?;
                    let Json(status) = crate::api::sync_sources::pause(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.name)),
                        Json(crate::api::sync_sources::PauseRequest {
                            paused: input.paused,
                        }),
                    )
                    .await?;
                    Ok(serde_json::to_value(status)?)
                })
            },
        },
        Operation {
            name: "jc_syncsource_detach",
            title: "Detach A Sync Source",
            description: "Stops the loop and proposes removing the SyncSource; what it imported stays",
            input: name_input_schema,
            output: detached_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: true,
            },
            kind: "SyncSource",
            verb: Some(jc_core::kinds::Verb::Delete),
            lane: Lane::Red,
            validate: |val| parse_input::<NameInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: NameInput = parse_input(val)?;
                    // The route's own body is the merge request and the status; the registry
                    // answers a proposal as its Change (T-1531: parsing the route's body as one
                    // answered 500 after the merge request was already open).
                    let (pull, _) = crate::api::sync_sources::detach_for(
                        state,
                        &as_user(caller),
                        project,
                        &input.name,
                    )
                    .await?;
                    // The route read the merge request from the project's forge (CC-87).
                    let forge = state.forge_for(project).ok_or_else(|| {
                        crate::error::ApiError::Unavailable("git forge is not configured".into())
                    })?;
                    let change = crate::change::Change::new(
                        crate::api::changes::change_meta(state, &forge, pull.number, project),
                        crate::change::ChangeStatus::new(
                            crate::change::Lane::Red,
                            crate::change::ChangePhase::PendingApproval,
                            crate::change::PlanSummary::new(0, 0, 1),
                        )
                        .in_repository(&pull.repository)
        .proposed_through(&pull.body).with_merge_request(pull.url),
                    );
                    Ok(crate::api::mutate::ProposeOutcome::Change(change).into_value())
                })
            },
        },
    ]
}
