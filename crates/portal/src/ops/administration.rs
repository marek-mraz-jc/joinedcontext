//! Administration as operations of the registry (owner order 2026-11-08, MCP-ADMIN.md): what a
//! platform, an organization or a project administrator reads on the Administration pages and
//! the decisions on access requests that only propose a Change, reached through MCP and REST.
//!
//! Each operation calls the route's own function, so the check is the route's: the organization
//! readings are for administrators of the organization (PF-03), the access requests for the
//! owners of the space (PL-91). What a route keeps for a person in a Portal session (AG-133,
//! PF-58) has no operation here: every person's connectors, an invitation, a password, a second
//! factor, a declined request, a revoked service account grant, a service account secret.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::admin::proposed;
use super::bounds::{text, ID, NAME, TERM};
use super::runs::as_user;
use super::{annotations, parse_input, OpError, Operation};
use crate::api::access_requests::{approve_request, list_incoming, revoke_request, Decision};
use crate::change::Lane;
use crate::error::ApiError;

pub const ORGANIZATION: &str = "jc_organization_read";
pub const REQUESTS: &str = "jc_access_request_list";
pub const DECIDE: &str = "jc_access_request_decide";
pub const ACCOUNT: &str = "jc_serviceaccount_access";

/// The operations of this module: an agent run never reaches them, since administration is a
/// person's own (AG-11, AG-70), and [`super::Caller::may_run`] refuses it there.
pub const NAMES: &[&str] = &[ORGANIZATION, REQUESTS, DECIDE, ACCOUNT];

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Aspect {
    Projects,
    Setup,
    Health,
    HealthChecks,
    Limits,
    Brokers,
    Agents,
    AgentActivity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct OrganizationInput {
    aspect: Aspect,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoInput {}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Decided {
    Approve,
    Revoke,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideInput {
    id: String,
    decision: Decided,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountInput {
    name: String,
}

fn organization_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "aspect": {
                "type": "string",
                "enum": ["projects", "setup", "health", "healthChecks", "limits", "brokers", "agents", "agentActivity"],
                "description": "projects: the projects you may read; setup: first-start checklist; health: validation results; healthChecks: every project's checks; limits: quotas and use; brokers: brokers and sizes; agents: agent profiles and runs; agentActivity: what agent runs did"
            },
            "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "agentActivity: page size, default 20" },
            "cursor": text("agentActivity: nextCursor of the previous page", TERM)
        },
        "required": ["aspect"],
        "additionalProperties": false
    })
}

fn decide_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": text("The access request's id", ID),
            "decision": {
                "type": "string",
                "enum": ["approve", "revoke"],
                "description": "approve: propose the grant; revoke: propose taking an approved grant back"
            },
            "reason": text("revoke: why, read by the requester", 500)
        },
        "required": ["id", "decision"],
        "additionalProperties": false
    })
}

fn account_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "name": text("ServiceAccount name, e.g. cam-west", NAME) },
        "required": ["name"],
        "additionalProperties": false
    })
}

fn reading_schema() -> Value {
    json!({ "type": "object", "description": "The reading, as the Administration page shows it" })
}

fn requests_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "items": { "type": "array", "items": { "type": "object" } } },
        "required": ["items"]
    })
}

fn pages_only_the_activity(input: &OrganizationInput) -> Result<(), OpError> {
    if input.aspect != Aspect::AgentActivity && (input.limit.is_some() || input.cursor.is_some()) {
        return Err(OpError::InvalidInput {
            path: "/limit".into(),
            message: "limit and cursor page only the agentActivity aspect".into(),
        });
    }
    Ok(())
}

fn organization_validate(val: &Value) -> Result<(), OpError> {
    pages_only_the_activity(&parse_input(val.clone())?)
}

/// The reading `aspect` names, from the route that answers it on the Administration page.
async fn read(
    caller: &super::Caller,
    state: &crate::state::AppState,
    input: OrganizationInput,
) -> Result<Value, OpError> {
    pages_only_the_activity(&input)?;
    let user = || as_user(caller);
    let state = || State(state.clone());
    let answer = match input.aspect {
        Aspect::Projects => {
            json!(
                crate::api::projects::list_projects(user(), state())
                    .await?
                    .0
            )
        }
        Aspect::Setup => json!(crate::api::setup::get_setup(user(), state()).await?.0),
        Aspect::Health => json!(crate::api::validation::get_health(user(), state()).await?.0),
        Aspect::HealthChecks => json!(
            crate::api::health_checks::list_all(user(), state())
                .await?
                .0
        ),
        Aspect::Limits => {
            json!(
                crate::api::organization_limits::get_limits(user(), state())
                    .await?
                    .0
            )
        }
        Aspect::Brokers => json!(crate::api::brokers::list_brokers(user(), state()).await?.0),
        Aspect::Agents => json!(
            crate::api::agent_registry::list_agents(user(), state())
                .await?
                .0
        ),
        Aspect::AgentActivity => {
            let query = crate::api::agent_activity::ActivityQuery {
                limit: input.limit,
                cursor: input.cursor,
                ..Default::default()
            };
            json!(
                crate::api::agent_activity::list_agent_activity(user(), state(), Query(query))
                    .await?
                    .0
            )
        }
    };
    Ok(answer)
}

fn decide_validate(val: &Value) -> Result<(), OpError> {
    let input: DecideInput = parse_input(val.clone())?;
    match (&input.decision, input.reason.as_deref().map(str::trim)) {
        (Decided::Revoke, None | Some("")) => Err(OpError::InvalidInput {
            path: "/reason".into(),
            message: "a revoke says why: the requester reads the reason".into(),
        }),
        (Decided::Approve, Some(_)) => Err(OpError::InvalidInput {
            path: "/reason".into(),
            message: "an approval takes no reason; it proposes the grant".into(),
        }),
        _ => Ok(()),
    }
}

async fn decide(
    caller: &super::Caller,
    state: &crate::state::AppState,
    project: &str,
    input: DecideInput,
) -> Result<Value, OpError> {
    let path = Path((project.to_owned(), input.id));
    let response = match input.decision {
        Decided::Approve => approve_request(as_user(caller), State(state.clone()), path).await?,
        Decided::Revoke => {
            let reason = input.reason.unwrap_or_default();
            revoke_request(
                as_user(caller),
                State(state.clone()),
                path,
                Ok(Json(Decision { reason })),
            )
            .await?
        }
    };
    proposed(response).await
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            name: ORGANIZATION,
            title: "Read The Organization",
            description: "One reading of the Administration pages: projects, setup, validation health, project checks, limits, brokers, agents or agent activity",
            input: organization_schema,
            output: reading_schema,
            annotations: annotations(true, false, true),
            // Each aspect's route decides: an administrator of the organization for all but
            // projects and agents, which answer what the caller may read (PF-03, PF-59).
            kind: "*",
            verb: None,
            lane: Lane::Green,
            validate: organization_validate,
            run: |caller, state, _project, val| {
                Box::pin(async move { read(caller, state, parse_input(val)?).await })
            },
        },
        Operation {
            name: REQUESTS,
            title: "List Access Requests",
            description: "The access requests to the project's Endpoints that you may decide, newest first",
            input: super::empty_input_schema,
            output: requests_schema,
            annotations: annotations(true, false, true),
            kind: "ContextSpace",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<NoInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, _val| {
                Box::pin(async move {
                    let Json(found) = list_incoming(
                        as_user(caller),
                        State(state.clone()),
                        Path(project.to_owned()),
                    )
                    .await?;
                    Ok(found)
                })
            },
        },
        Operation {
            name: DECIDE,
            title: "Decide An Access Request",
            description: "Proposes the grant of a pending access request, or taking an approved one back; a person approves the Change. Declining is a person's, in the Portal",
            input: decide_schema,
            output: super::change_schema,
            annotations: annotations(false, false, false),
            // The route checks approve on the request's space and that the caller did not file it
            // (PL-91); the Change it opens is approved by a person (AG-133).
            kind: "*",
            verb: None,
            lane: Lane::Yellow,
            validate: decide_validate,
            run: |caller, state, project, val| {
                Box::pin(async move {
                    decide_validate(&val)?;
                    decide(caller, state, project, parse_input(val)?).await
                })
            },
        },
        Operation {
            name: ACCOUNT,
            title: "Read A Service Account's Access",
            description: "What a ServiceAccount may read and write, through which Policy and binding",
            input: account_schema,
            output: reading_schema,
            annotations: annotations(true, false, true),
            kind: "ServiceAccount",
            verb: None,
            lane: Lane::Green,
            validate: |val| parse_input::<AccountInput>(val.clone()).map(|_| ()),
            run: |caller, state, project, val| {
                Box::pin(async move {
                    let input: AccountInput = parse_input(val)?;
                    let Json(access) = crate::api::agent_access::account_access(
                        as_user(caller),
                        State(state.clone()),
                        Path((project.to_owned(), input.name)),
                    )
                    .await?;
                    Ok(json!(access))
                })
            },
        },
    ]
}

/// Refuses an agent run every administration operation (AG-11, AG-70): its profile only ever
/// narrows a person, and administering is never delegated to a run.
pub fn refuse_agent(caller: &super::Caller, name: &str) -> Result<(), OpError> {
    if caller.via == super::Via::Agent && NAMES.contains(&name) {
        return Err(OpError::Api(ApiError::Denied(
            "administration is a person's, at their own MCP client or in the Portal; an agent run \
             never administers (AG-11, AG-70)"
                .into(),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revoke_says_why_and_an_approval_takes_no_reason() {
        assert!(decide_validate(&json!({ "id": "r1", "decision": "approve" })).is_ok());
        assert!(
            decide_validate(&json!({ "id": "r1", "decision": "revoke", "reason": "ended" }))
                .is_ok()
        );
        for refused in [
            json!({ "id": "r1", "decision": "revoke" }),
            json!({ "id": "r1", "decision": "revoke", "reason": "  " }),
            json!({ "id": "r1", "decision": "approve", "reason": "x" }),
        ] {
            match decide_validate(&refused) {
                Err(OpError::InvalidInput { path, .. }) => assert_eq!(path, "/reason"),
                other => panic!("{refused}: {other:?}"),
            }
        }
        assert!(decide_validate(&json!({ "id": "r1", "decision": "decline" })).is_err());
    }

    #[tokio::test]
    async fn limit_and_cursor_page_only_the_agent_activity() {
        let state = crate::state::AppState::new(crate::config::Config::for_tests(), None);
        let caller = super::super::Caller::new(
            crate::auth::session::Identity {
                client: None,
                subject: "f:1:ana".into(),
                username: "ana".into(),
                email: None,
                name: None,
                roles: vec![],
                groups: vec![],
                second_factor: false,
            },
            super::super::Via::Session,
        );
        let input = OrganizationInput {
            aspect: Aspect::Setup,
            limit: Some(5),
            cursor: None,
        };
        match read(&caller, &state, input).await {
            Err(OpError::InvalidInput { path, .. }) => assert_eq!(path, "/limit"),
            other => panic!("{other:?}"),
        }
    }
}
