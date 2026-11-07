//! The operations on changes: the list a person reviews and the approval only a person may give
//! (CC-34, AG-11).

use super::bounds;
use super::resources;
use super::Operation;
use super::OperationAnnotations;
use super::{parse_input, OpError};
use crate::api::changes::{self, ChangeList};
use crate::change::Lane;
use crate::state::AppState;
use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeApproveInput {
    pub id: String,
    #[serde(default)]
    pub confirm: Option<String>,
}

/// Changes listed when the call names no limit, and the most it may name (T-6291).
const CHANGES: usize = 20;
const MAX_CHANGES: usize = 100;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ChangeListInput {
    #[serde(default)]
    pub limit: Option<usize>,
    /// The `next` of the previous page: the name of the last change it held.
    #[serde(default)]
    pub cursor: Option<String>,
}

fn change_list_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_CHANGES, "description": "Default 20" },
            "cursor": text("The last page's next", NAME)
        },
        "additionalProperties": false
    })
}

/// One page of `list` for an operation (T-6291): the changes after `cursor`, each as `shown`
/// makes it, at most `limit` and [`bounds::PAGE_BYTES`] of them, and `next`, the name of the
/// last one, while more follow. The REST list stays whole: the page a person reads renders every
/// open change.
pub fn page_of(
    list: ChangeList,
    input: &ChangeListInput,
    shown: impl Fn(Value) -> Value,
) -> Result<Value, OpError> {
    let ChangeList {
        mut items, outside, ..
    } = list;
    if let Some(cursor) = &input.cursor {
        let at = items
            .iter()
            .position(|change| &change.metadata.name == cursor)
            .ok_or_else(|| OpError::InvalidInput {
                path: "cursor".into(),
                message: format!(
                    "'{cursor}' is no longer an open change of this list; list again without cursor"
                ),
            })?;
        items.drain(..=at);
    }
    let limit = input.limit.unwrap_or(CHANGES).clamp(1, MAX_CHANGES);
    let mut more = items.len() > limit;
    items.truncate(limit);
    let mut items = items
        .into_iter()
        .map(|change| serde_json::to_value(change).map(&shown))
        .collect::<Result<Vec<_>, _>>()?;
    let kept = bounds::fitting(&items, bounds::PAGE_BYTES);
    more |= kept < items.len();
    items.truncate(kept);
    let next = more
        .then(|| items.last()?.pointer("/metadata/name").cloned())
        .flatten();
    let mut page = serde_json::to_value(ChangeList::new(Vec::new()))?;
    if let Some(map) = page.as_object_mut() {
        map.insert("items".into(), Value::Array(items));
        // The merge requests that are no Change are told once, on the first page.
        if input.cursor.is_none() && !outside.is_empty() {
            map.insert("outside".into(), serde_json::to_value(outside)?);
        }
        if let Some(next) = next {
            map.insert("next".into(), next);
        }
    }
    Ok(page)
}

fn change_approve_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "id": text("The Change to approve", ID),
            "confirm": text(
                "Red lane: the Change's name typed out to confirm",
                NAME
            )
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

/// The `Change` resource as an MCP client reads it (T-0838).
///
/// Written out rather than derived from the OpenAPI document: utoipa's schema points at
/// `#/components/schemas/…`, a pointer no MCP client resolves, so a tool that published it
/// described nothing (AG-60).
fn change_document_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "apiVersion": { "type": "string" },
            "kind": { "type": "string", "enum": ["Change"] },
            "metadata": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "`chg-` and eight hex characters; what `jc_change_approve` takes as `id`" },
                    "namespace": { "type": "string" }
                },
                "required": ["name", "namespace"]
            },
            "status": {
                "type": "object",
                "properties": {
                    "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
                    "phase": { "type": "string", "enum": ["PendingApproval", "Deploying", "Merged", "Applied", "Rejected"] },
                    "mergeRequest": { "type": "string" },
                    "plan": {
                        "type": "object",
                        "properties": {
                            "create": { "type": "integer" },
                            "update": { "type": "integer" },
                            "delete": { "type": "integer" }
                        }
                    }
                },
                "required": ["lane", "phase", "plan"]
            }
        },
        "required": ["apiVersion", "kind", "metadata", "status"]
    })
}

/// What every propose, the resource delete and a rejection answer: the id and the lane beside
/// the `Change` itself, which is what the caller gets, not the bare resource (T-0838).
pub(crate) fn change_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "changeId": { "type": "string", "description": "The change's name; `jc_change_approve` takes it as `id`" },
            "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
            "url": { "type": "string", "description": "The merge request to review, when the forge has one" },
            "change": change_document_schema(),
            "warning": { "type": "string", "description": "Present when the draft was proposed without a fresh green verdict" }
        },
        "required": ["changeId", "lane", "change"]
    })
}

/// What `jc_change_list` answers: the list envelope, not one proposal (T-0838).
fn change_list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "apiVersion": { "type": "string" },
            "kind": { "type": "string", "enum": ["ChangeList"] },
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "title": { "type": "string" },
                        "lane": { "type": "string", "enum": ["green", "yellow", "red"] },
                        "phase": { "type": "string" },
                        "mergeRequest": { "type": "string" },
                        "url": { "type": "string", "description": "The Portal page where a person decides it" },
                        "proposedBy": { "type": "string" }
                    }
                }
            },
            "next": { "type": "string", "description": "Present when more changes follow: send it as cursor" }
        },
        "required": ["apiVersion", "kind", "items"]
    })
}

/// The Portal page where a person decides change `id` (T-6293): the `url` every change tool
/// answers, while the forge's merge request stays `mergeRequest`.
pub(super) fn approval_url(state: &AppState, project: &str, id: &str) -> String {
    let base = state.config.public_base_url.as_str().trim_end_matches('/');
    format!("{base}/projects/{project}/approvals/{id}")
}

/// A `Change` document with its approval page as `url`.
pub(super) fn with_approval_url(state: &AppState, project: &str, mut change: Value) -> Value {
    let id = change
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let (Some(id), Some(doc)) = (id, change.as_object_mut()) {
        doc.insert("url".into(), json!(approval_url(state, project, &id)));
    }
    change
}

/// The `Change` the approval answers: the resource itself, with no wrapper around it.
fn approved_change_schema() -> Value {
    change_document_schema()
}

/// This module's operations in the registry (`super::init_registry`).
/// `jc_change_history`: the closed changes, as the Approvals history pages them (JC2-28).
pub const HISTORY: &str = "jc_change_history";

fn change_history_input_schema() -> Value {
    use bounds::*;
    json!({
        "type": "object",
        "properties": {
            "page": { "type": "integer", "minimum": 1, "description": "From 1; the previous answer's next" },
            "kind": text("Only changes to this kind, e.g. Endpoint", TERM),
            "name": text("Resource name contains, e.g. air", NAME),
            "author": text("Author name or e-mail contains, e.g. jana", TITLE),
            "since": text("Decided on or after, YYYY-MM-DD", 10),
            "until": text("Decided on or before, YYYY-MM-DD", 10)
        },
        "additionalProperties": false
    })
}

fn change_history_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "description": "Merged or rejected changes, most recently closed first, each naming who decided it",
                "items": { "type": "object" }
            },
            "next": { "type": "integer", "description": "Present when older changes follow: send it as page" }
        },
        "required": ["items"]
    })
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: HISTORY,
            title: "List Closed Changes",
            description: "One page of the merged and rejected changes, most recently closed first, each naming who decided it; `next` is the page that continues it",
            input: change_history_input_schema,
            output: change_history_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            // As jc_change_list: no manifest kind, and the function checks the project read.
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<changes::HistoryQuery>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let query: changes::HistoryQuery = parse_input(val)?;
                    let list =
                        changes::change_history_readable(state, &caller.identity, project, &query)
                            .await?;
                    Ok(serde_json::to_value(list)?)
                })
            },
        },
        Operation {
            name: "jc_change_list",
            title: "List Changes",
            description: "Lists open change proposals and merge requests for review",
            input: change_list_input_schema,
            output: change_list_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            // A Change is no manifest kind, so `access.kinds` cannot name it (MF-40) and a profile
            // could never be offered this read. Verbless, so the kind plays no part in `permitted`
            // either: the function checks that the caller may read the project (T-1475, AG-70).
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ChangeListInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: ChangeListInput = parse_input(val)?;
                    let list =
                        changes::list_changes_readable(state, &caller.identity, project).await?;
                    page_of(list, &input, |item| with_approval_url(state, project, item))
                })
            },
        },
        Operation {
            name: "jc_change_approve",
            title: "Approve Change",
            description: "Approves and merges a change proposal",
            input: change_approve_input_schema,
            output: approved_change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: false,
            },
            kind: "Change",
            verb: Some(Verb::Approve),
            lane: Lane::Red,
            validate: |val| parse_input::<ChangeApproveInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    resources::refuse_agent_decision(caller)?;
                    let input: ChangeApproveInput = parse_input(val)?;
                    let change = changes::approve_change_for(
                        state,
                        &caller.identity,
                        project,
                        &input.id,
                        input.confirm.as_deref(),
                        changes::ApprovedBy::Operation,
                    )
                    .await?;
                    Ok(serde_json::to_value(change)?)
                })
            },
        },
    ]
}
