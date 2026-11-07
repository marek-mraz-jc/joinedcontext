//! Every kind read, changed and removed through the registry (AG-77, ADR-N-021): the functions
//! behind the resource routes, so an MCP client, the assistant and the Portal's forms reach the
//! same Verdict and the same Change.

use jc_core::kinds::Verb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::bounds::{text, ID, NAME, TERM, TEXT};
use super::{
    change_schema, check_for_kind, manifest_input_schema, parse_input, propose_with_optional_draft,
    Caller, ManifestInput, OpError, Operation, OperationAnnotations, Via,
};
use crate::api::assistant::ref_name;
use crate::api::changes;
use crate::api::delete::{self, DeleteOutcome};
use crate::api::mutate::ProposeOutcome;
use crate::change::Lane;
use crate::error::ApiError;
use crate::resource::{self, KindInfo, ResourceEnvelope};
use crate::state::AppState;
use crate::store::ListOptions;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceListInput {
    pub kind: String,
    #[serde(default)]
    pub space: Option<String>,
    /// The REST list's `filter` (API/13 section 2), run after the read check.
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGetInput {
    pub kind: String,
    pub name: String,
    /// An Endpoint's writers: who may write to it, from the gateway's PDP (T-6027).
    #[serde(default)]
    pub writers: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDeleteInput {
    pub kind: String,
    pub name: String,
    /// The resource's name typed back (CC-39).
    pub confirm: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRejectInput {
    pub id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// The operations whose function is the REST route's own and checks the caller as the route
/// does: reading a resource needs a signed-in person, proposing and deleting the verb on the
/// kind, a run answers only to whoever started it, and a service account's keys only to its
/// owner or an approver (AG-77, PF-50, PF-34).
///
/// The order is the registry's own, so [`super::listing`] answers these in this order for a
/// caller with no bindings at all.
pub const CHECKED_BY_THE_ROUTE: &[&str] = &[
    "jc_change_list",
    "jc_resource_list",
    "jc_resource_get",
    "jc_resource_propose",
    "jc_resource_delete",
    "jc_change_get",
    "jc_run_list",
    "jc_run_get",
    "jc_project_get",
    "jc_endpoint_list_all",
    "jc_project_create",
    "jc_project_delete",
    "jc_project_export",
    "jc_project_import",
    // People live in the realm: the route checks the verb on Person at organization scope and
    // every right the person holds (PF-91, PF-93).
    "jc_person_list",
    "jc_person_get",
    "jc_person_create",
    "jc_person_edit",
    "jc_person_disable",
    "jc_person_enable",
    "jc_person_sign_out",
];

/// The catalogue row of a kind an input names; an unknown kind is answered with the kinds there are.
pub(crate) fn kind_named(kind: &str) -> Result<&'static KindInfo, OpError> {
    resource::by_kind(kind).ok_or_else(|| OpError::InvalidInput {
        path: "/kind".into(),
        message: format!(
            "'{kind}' is not a kind of this Portal; the kinds are {}",
            resource::kinds()
                .map(|info| info.kind)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// The context space a resource belongs to: its space label, else the space its spec references.
fn space_of(envelope: &ResourceEnvelope) -> Option<String> {
    envelope
        .metadata
        .labels
        .get("joinedcontext.com/space")
        .cloned()
        .or_else(|| ref_name(&envelope.spec["contextSpaceRef"]))
}

/// One row of a listing: enough to pick a resource and open it, not the whole manifest.
fn row(envelope: &ResourceEnvelope) -> Value {
    let status = envelope.status.as_ref();
    let mut row = json!({
        "kind": envelope.kind,
        "name": envelope.metadata.name,
        "space": space_of(envelope),
        "title": envelope.metadata.title,
        "phase": status.map(|status| status.phase),
        "sourceUrl": status.and_then(|status| status.source_url.clone()),
    });
    // The phase stays the gateway's (T-3640), so a Live Endpoint the broker holds no view of
    // says so beside it, or it reads as serving (T-6435).
    if let Some(unserved) = status.and_then(|status| {
        status.conditions.iter().find(|held| {
            held.r#type == crate::reconciler::views::CONDITION && held.status == "False"
        })
    }) {
        row["notServing"] = json!(unserved.message);
    }
    row
}

fn list_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": text("Manifest kind, e.g. Pipeline", TERM),
            "space": text("Only the resources of this context space", NAME),
            "filter": {
                "type": "string",
                "maxLength": crate::resource::filter::MAX_LEN,
                "description": "AIP-160 over manifest fields, e.g. labels.team = \"air\" AND spec.schedule:*"
            }
        },
        "required": ["kind"],
        "additionalProperties": false
    })
}

fn list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": { "type": "array", "items": { "type": "object" } }
        }
    })
}

fn get_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": text("Manifest kind, e.g. Pipeline", TERM),
            "name": text("The resource's metadata.name", NAME),
            "writers": {
                "type": "boolean",
                "description": "Endpoint only: also who may write to it, for readers of the space's Policies"
            }
        },
        "required": ["kind", "name"],
        "additionalProperties": false
    })
}

fn get_output_schema() -> Value {
    json!({
        "type": "object",
        "description": "The manifest with its status, and `portal`: the address of its Portal page"
    })
}

fn delete_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": text("Manifest kind, e.g. Pipeline", TERM),
            "name": text("The resource's metadata.name", NAME),
            "confirm": text("The resource's name typed back", NAME)
        },
        "required": ["kind", "name", "confirm"],
        "additionalProperties": false
    })
}

fn reject_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": text("The Change to reject", ID),
            "reason": text("Why, for the author to read", TEXT)
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

/// The namespace one named manifest of this kind is in: the nearest of [`crate::resource::homes`]
/// that holds it, and the nearest one when it is nowhere (so the refusal names a real place).
///
/// An organization-scoped kind has one home, `org`; a project kind has the project of the call;
/// a kind that lives in either — `Role` — is looked for in the project first and then in the
/// organization, so `jc_resource_get {kind: "Role"}` finds both (PF-59, PF-68, T-0840, T-0872).
pub(crate) fn home_holding(
    state: &AppState,
    info: &'static crate::resource::KindInfo,
    project: &str,
    name: &str,
) -> String {
    let homes = crate::resource::homes(info, project);
    homes
        .iter()
        .find(|home| state.mirror.get(home, info.kind, name).is_some())
        .cloned()
        .unwrap_or_else(|| homes[0].clone())
}

/// What the caller may read is what they are answered (PF-59): a kind no binding of theirs
/// reads is not there at all, the same answer as a kind that does not exist (R20). The REST
/// list and get have said this since T-0906; this door says it too (T-0840).
fn readable(
    caller: &Caller,
    state: &AppState,
    project: &str,
    info: &'static crate::resource::KindInfo,
) -> Result<(), OpError> {
    let effective = crate::permissions::for_request(state, &caller.identity, project);
    if effective.may_read(info.kind) {
        return Ok(());
    }
    let readable: Vec<&str> = resource::kinds()
        .map(|other| other.kind)
        .filter(|kind| effective.may_read(kind))
        .collect();
    Err(OpError::Api(ApiError::NotFound(unreadable(
        info.kind, project, &readable,
    ))))
}

/// The refusal of a kind the caller may not read: it names the kinds they may read there, which
/// are their own grants (`/permissions/me` shows them too) and public kind names, so a client
/// asks for one of those instead of guessing kind after kind (T-6042).
fn unreadable(kind: &str, project: &str, readable: &[&str]) -> String {
    match readable {
        [] => format!("kind '{kind}' not found in project '{project}'; you may read no kind there"),
        _ => format!(
            "kind '{kind}' not found in project '{project}'; the kinds you may read there are {}",
            readable.join(", ")
        ),
    }
}

/// The kinds the caller may read in `project`, each judged in its own home as [`readable`]
/// judges a call: the set a refusal names and `/permissions/me` shows (T-6059).
pub(crate) fn readable_kinds(
    caller: &Caller,
    state: &AppState,
    project: &str,
) -> Vec<&'static str> {
    let in_project = crate::permissions::for_request(state, &caller.identity, project);
    let in_org =
        crate::permissions::for_request(state, &caller.identity, crate::permissions::ORG_NAMESPACE);
    resource::kinds()
        .filter(|info| {
            let effective = if resource::home(info, project) == project {
                &in_project
            } else {
                &in_org
            };
            effective.may_read(info.kind)
        })
        .map(|info| info.kind)
        .collect()
}

/// `tools/list` of an MCP door with the `kind` of `jc_resource_list` and `jc_resource_get` an
/// enum of the kinds this caller may read there, `within` the connector's segments when the door
/// is one: a model picks a kind it can read instead of guessing kinds that do not exist (T-6059).
/// A tool with no kind left to name is not offered. The call still checks the kind.
pub fn with_readable_kinds(
    tools: Vec<Value>,
    caller: &Caller,
    state: &AppState,
    project: &str,
    within: Option<&[&str]>,
) -> Vec<Value> {
    let kinds: Vec<&str> = readable_kinds(caller, state, project)
        .into_iter()
        .filter(|kind| within.is_none_or(|within| within.contains(kind)))
        .collect();
    tools
        .into_iter()
        .filter_map(|mut tool| {
            if !matches!(
                tool["name"].as_str(),
                Some("jc_resource_list" | "jc_resource_get")
            ) {
                return Some(tool);
            }
            if kinds.is_empty() {
                return None;
            }
            tool["inputSchema"]["properties"]["kind"]["enum"] = json!(kinds);
            Some(tool)
        })
        .collect()
}

/// The homes of this kind the caller may read it in, each with the grants they hold there: a
/// Role lives in the project and the organization, and a grant in one reads nothing in the
/// other, as [`readable`] judges `jc_resource_get` in the home holding the name (PF-59, T-6273).
fn readable_homes<'a>(
    caller: &Caller,
    state: &AppState,
    homes: &'a [String],
    info: &'static KindInfo,
) -> Vec<(&'a str, crate::permissions::Effective)> {
    homes
        .iter()
        .map(|home| {
            let effective = crate::permissions::for_request(state, &caller.identity, home);
            (home.as_str(), effective)
        })
        .filter(|(_, effective)| effective.may_read(info.kind))
        .collect()
}

/// Every manifest of this kind in `homes`, judged by the grants held in its own home; a grant
/// bound to one context space reads that space's manifests alone (T-5698).
fn readable_in(
    state: &AppState,
    homes: &[(&str, crate::permissions::Effective)],
    info: &'static KindInfo,
) -> Vec<ResourceEnvelope> {
    homes
        .iter()
        .flat_map(|(home, effective)| {
            state
                .mirror
                .list(home, info.kind, &ListOptions::default())
                .items
                .into_iter()
                .filter(move |envelope| effective.may_read_envelope(info.kind, envelope))
        })
        .collect()
}

async fn list(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ResourceListInput,
) -> Result<Value, OpError> {
    let info = kind_named(&input.kind)?;
    let homes = crate::resource::homes(info, project);
    let readable_homes = readable_homes(caller, state, &homes, info);
    if readable_homes.is_empty() {
        readable(caller, state, &homes[0], info)?;
    }
    let filter = match input.filter.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() => Some(
            crate::resource::filter::Filter::parse(
                text,
                jc_core::registry::schema_of(info.kind).as_ref(),
            )
            .map_err(|e| OpError::Api(ApiError::BadRequest(e.to_string())))?,
        ),
        _ => None,
    };
    let items: Vec<Value> = readable_in(state, &readable_homes, info)
        .into_iter()
        .filter(|envelope| {
            input
                .space
                .as_deref()
                .is_none_or(|space| space_of(envelope).as_deref() == Some(space))
        })
        .filter(|envelope| {
            filter.as_ref().is_none_or(|filter| {
                serde_json::to_value(envelope).is_ok_and(|doc| filter.matches(&doc))
            })
        })
        .map(|envelope| {
            let mut row = row(&envelope);
            if info.kind == "Endpoint" {
                let reach = super::endpoint_reach::reach(state, project, &envelope);
                row["types"] = json!(reach.types);
                row["url"] = json!(reach.url);
            }
            row
        })
        .collect();
    Ok(json!({ "items": items }))
}

/// A read object as `jc_resource_get` answers it: its manifest and where a person opens it
/// (T-6486, S-41). The address names only the kind, project and name the caller just read.
fn held(
    state: &AppState,
    project: &str,
    info: &crate::resource::KindInfo,
    envelope: &crate::resource::ResourceEnvelope,
) -> Result<Value, OpError> {
    let route = crate::agents::change::object_route(
        project,
        info.kind,
        info.plural,
        &envelope.metadata.name,
    );
    let mut answer = serde_json::to_value(envelope)?;
    answer["portal"] = json!(crate::agents::change::address(&state.config, &route));
    Ok(answer)
}

async fn get(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ResourceGetInput,
) -> Result<Value, OpError> {
    let info = kind_named(&input.kind)?;
    if input.writers && info.kind != "Endpoint" {
        return Err(OpError::InvalidInput {
            path: "/writers".into(),
            message: format!("writers is an Endpoint's; a {} has none", info.kind),
        });
    }
    let homes = crate::resource::homes(info, project);
    let project = &home_holding(state, info, project, &input.name);
    readable(caller, state, project, info)?;
    // What a grant bound to one context space may not read is not there, and is no name the
    // caller is told about either (PF-59, R20, T-5698).
    let effective = crate::permissions::for_request(state, &caller.identity, project);
    match state
        .mirror
        .get(project, info.kind, &input.name)
        .filter(|envelope| effective.may_read_envelope(info.kind, envelope))
    {
        Some(envelope) if info.kind == "Endpoint" => {
            let access =
                super::endpoint_reach::reach(state, project, &envelope).detail(state, &envelope);
            let mut answer = held(state, project, info, &envelope)?;
            answer["access"] = access;
            if input.writers {
                answer["writers"] =
                    super::endpoint_reach::writers(caller, state, project, &input.name).await?;
            }
            Ok(answer)
        }
        Some(envelope) => held(state, project, info, &envelope),
        None => {
            // The names it could have been are every name of every place this kind lives in,
            // so a caller who asked in the project is told about the organization's too.
            let mut names: Vec<String> =
                readable_in(state, &readable_homes(caller, state, &homes, info), info)
                    .into_iter()
                    .map(|envelope| envelope.metadata.name)
                    .collect();
            names.sort();
            Err(OpError::Api(ApiError::NotFound(if names.is_empty() {
                format!(
                    "{} '{}' not found in project '{project}', which has none",
                    info.kind, input.name
                )
            } else {
                format!(
                    "{} '{}' not found in project '{project}'; it has {}",
                    info.kind,
                    input.name,
                    names.join(", ")
                )
            })))
        }
    }
}

async fn propose(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ManifestInput,
) -> Result<Value, OpError> {
    let kind = match (&input.manifest, &input.draft) {
        (Some(manifest), _) => manifest
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        (None, Some(draft)) => draft.kind.clone(),
        (None, None) => {
            return Err(OpError::InvalidInput {
                path: "/manifest".into(),
                message: "either manifest or draft is required".into(),
            })
        }
    };
    let info = kind_named(&kind)?;
    propose_with_optional_draft(
        caller,
        state,
        project,
        info.plural,
        check_for_kind(&kind),
        input,
    )
    .await
}

async fn remove(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ResourceDeleteInput,
) -> Result<Value, OpError> {
    let info = kind_named(&input.kind)?;
    if input.confirm != input.name {
        return Err(OpError::Api(ApiError::BadRequest(format!(
            "confirm must be the resource's name, '{}' (CC-39)",
            input.name
        ))));
    }
    match delete::delete_with_identity(
        &caller.identity,
        state,
        &home_holding(state, info, project, &input.name),
        info.plural,
        &input.name,
        false,
        None,
    )
    .await?
    {
        // A person at the Portal who administers the kind removes it at once, the name typed
        // back (PF-58); an agent run, a bearer caller and MCP wait for an approver (AG-11).
        DeleteOutcome::Proposed(change) if caller.via == super::Via::Session => {
            let change = crate::api::changes::approve_as_proposed(
                state,
                &caller.identity,
                &home_holding(state, info, project, &input.name),
                info.kind,
                change,
                Some(&input.confirm),
            )
            .await;
            Ok(ProposeOutcome::Change(change).into_value())
        }
        DeleteOutcome::Proposed(change) => Ok(ProposeOutcome::Change(change).into_value()),
        DeleteOutcome::Workspace(commit) => Ok(ProposeOutcome::Workspace(commit).into_value()),
        DeleteOutcome::DryRun(result) => Ok(ProposeOutcome::DryRun(result).into_value()),
        DeleteOutcome::Referenced { here, elsewhere } => Err(OpError::Conflict(json!({
            "error": "referenced",
            "detail": DeleteOutcome::conflict_message(&here, elsewhere),
            "by": here,
            "elsewhere": elsewhere,
        }))),
    }
}

async fn reject(
    caller: &Caller,
    state: &AppState,
    project: &str,
    input: ChangeRejectInput,
) -> Result<Value, OpError> {
    let change = changes::reject_change_for(
        state,
        &caller.identity,
        project,
        &input.id,
        input.reason.as_deref(),
    )
    .await?;
    Ok(ProposeOutcome::Change(change).into_value())
}

/// Deciding a change is a person's, and an MCP client is refused with an agent run (AG-11,
/// owner decision T-1005): the client is a tool a model drives even when it carries the
/// person's token, so leaving `Via::Mcp` open would let an agent approve by choosing another
/// transport. A bearer token is refused too (AG-133): a ServiceAccount's or a CLI's token on
/// `POST …/ops/jc_change_approve` is the REST route's refusal by another door. Approving and
/// rejecting stay in a person's Portal session. The run operations keep
/// `super::runs::refuse_agent`: they are not a change's decision.
pub fn refuse_agent_decision(caller: &Caller) -> Result<(), OpError> {
    if matches!(caller.via, Via::Agent | Via::Mcp | Via::Bearer) {
        return Err(OpError::Api(ApiError::Denied(
            "an agent never approves or rejects a change, and neither does a bearer token; a \
             person does, in the Portal (AG-11, AG-133)"
                .into(),
        )));
    }
    Ok(())
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: "jc_resource_list",
            title: "List Resources",
            description: "Lists the resources of one kind in the project, optionally of one context space",
            input: list_input_schema,
            output: list_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ResourceListInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { list(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_resource_get",
            title: "Get Resource",
            description: "Reads one resource's manifest and status by kind and name",
            input: get_input_schema,
            output: get_output_schema,
            annotations: OperationAnnotations {
                read_only_hint: true,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<ResourceGetInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { get(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_resource_propose",
            title: "Propose Resource",
            description: "Proposes creating or changing a resource of any kind from its manifest or a draft; the change waits for a person's approval",
            input: manifest_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Yellow,
            validate: |val| parse_input::<ManifestInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { propose(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_resource_delete",
            title: "Delete Resource",
            description: "Proposes removing a resource by kind and name, its name typed back; refused while other resources reference it",
            input: delete_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: true,
                idempotent_hint: false,
            },
            kind: "*",
            verb: None,
            lane: Lane::Red,
            validate: |val| parse_input::<ResourceDeleteInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move { remove(caller, state, project, parse_input(val)?).await })
            },
        },
        Operation {
            name: "jc_change_reject",
            title: "Reject Change",
            description: "Rejects a change proposal with a reason and closes its merge request",
            input: reject_input_schema,
            output: change_schema,
            annotations: OperationAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: true,
            },
            kind: "Change",
            verb: Some(Verb::Approve),
            lane: Lane::Yellow,
            validate: |val| parse_input::<ChangeRejectInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    refuse_agent_decision(caller)?;
                    reject(caller, state, project, parse_input(val)?).await
                })
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{row, unreadable};

    /// T-6435: a Live Endpoint the broker holds no view of says why beside its phase; one the
    /// broker serves, or one whose other condition is false, carries no such member.
    #[test]
    fn a_listed_endpoint_without_a_view_does_not_read_as_plain_live() {
        use crate::reconciler::{streams::make_condition, views::CONDITION};
        let mut endpoint: crate::resource::ResourceEnvelope = serde_json::from_value(
            serde_json::json!({ "apiVersion": "joinedcontext.com/v1alpha1", "kind": "Endpoint",
                                "metadata": { "name": "air" }, "spec": {} }),
        )
        .expect("envelope");
        let with = |condition, status, message| {
            serde_json::from_value(serde_json::json!({
                "phase": "Live",
                "conditions": [make_condition(condition, status, "Why", message)]
            }))
            .expect("status")
        };
        endpoint.status = Some(with(CONDITION, "False", "no Policy grants anything"));
        let listed = row(&endpoint);
        assert_eq!(listed["phase"], "Live");
        assert_eq!(listed["notServing"], "no Policy grants anything");

        for (condition, status) in [(CONDITION, "True"), ("HubLinked", "False")] {
            endpoint.status = Some(with(condition, status, "said"));
            assert!(row(&endpoint).get("notServing").is_none());
        }
    }

    #[test]
    fn an_unreadable_kind_names_the_kinds_the_caller_may_read() {
        assert_eq!(
            unreadable("Blueprint", "org", &["ContextSpace", "DataModel"]),
            "kind 'Blueprint' not found in project 'org'; the kinds you may read there are \
             ContextSpace, DataModel"
        );
        assert_eq!(
            unreadable("Role", "org", &[]),
            "kind 'Role' not found in project 'org'; you may read no kind there"
        );
    }
}
