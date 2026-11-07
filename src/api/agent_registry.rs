//! `GET /api/v1/organization/agents`: every agent identity of the organization and its projects
//! in one answer (API/01 §48, AG-126…AG-129): assistant deployments, agent profiles, MCP servers,
//! service accounts and the runs not yet ended, each with whom it acts as, what it reaches and
//! what it spends. Only what the caller may read is listed or counted; no token, credential,
//! key, `secretRef` or prompt is part of it (AG-127).

use std::collections::BTreeSet;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::agents::run::{AgentRun, AgentRunStatus};
use crate::agents::store::RunFilter;
use crate::auth::session::{CurrentUser, Identity};
use crate::error::ApiError;
use crate::permissions::Effective;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::ListOptions;

/// The manifest kinds that are agents, beside the runs.
const KINDS: [&str; 4] = [
    "AssistantDeployment",
    "AgentProfile",
    "McpServer",
    "ServiceAccount",
];

/// The newest runs read per project. ponytail: an older run still in flight is left out; page
/// the store when a project keeps more than this many runs a day.
const RUNS_PER_PROJECT: i64 = 200;

/// Every agent the caller may read.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentRegistry {
    pub items: Vec<AgentItem>,
}

/// One agent identity (API/01 §48).
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentItem {
    /// `AssistantDeployment`, `AgentProfile`, `McpServer`, `ServiceAccount` or `AgentRun`.
    pub kind: String,
    pub name: String,
    /// The namespace: a project, or `org` for the organization's own manifests.
    pub project: String,
    pub title: Option<String>,
    pub owner: Owner,
    pub acts_as: ActsAs,
    pub spaces: Vec<String>,
    pub endpoints: Vec<String>,
    pub last_activity: Option<String>,
    pub spend: Spend,
    pub state: AgentState,
    /// The run's id, for `AgentRun` only.
    pub run_id: Option<String>,
    /// The agent profile a run executes, for `AgentRun` only.
    pub profile: Option<String>,
}

/// Who answers for an agent.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[schema(as = AgentOwner)]
pub struct Owner {
    /// `person` or `project`.
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
}

/// Whose rights an agent uses (AG-94, AG-115).
#[derive(Debug, Clone, Copy, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[schema(as = AgentActsAs)]
pub enum ActsAs {
    /// Its own service account.
    #[serde(rename = "self")]
    Own,
    /// The person or client that called it.
    Caller,
}

/// Tokens spent against a cap.
#[derive(Debug, Default, Serialize, ToSchema, PartialEq)]
#[schema(as = AgentSpend)]
pub struct Spend {
    pub used: Option<i64>,
    pub cap: Option<u64>,
    /// `day` or `run`.
    pub per: Option<String>,
}

/// Whether an agent answers now.
#[derive(Debug, Clone, Copy, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Active,
    Paused,
    Running,
    Waiting,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/agents",
    summary = "List Agents",
    description = "Every agent identity of the organization and its projects the caller may read: assistant deployments, agent profiles, MCP servers, service accounts and runs not yet ended (API/01 §48, AG-126).",
    tag = "agents",
    responses(
        (status = 200, description = "The agents", body = AgentRegistry),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "The caller reads no agent kind anywhere", body = crate::error::ProblemDetails),
        (status = 503, description = "The run store did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_agents(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<AgentRegistry>, ApiError> {
    let mut items = Vec::new();
    let mut reads_any = false;
    // `namespaces` leaves the organization's own out: its profiles, accounts and servers count too.
    let namespaces = std::iter::once(crate::permissions::ORG_NAMESPACE.to_owned())
        .chain(state.mirror.namespaces());
    for project in namespaces {
        let effective = crate::permissions::for_request(&state, &user.0.identity, &project);
        let (found, reads) =
            namespace_items(&state, &user.0.identity, &project, &effective).await?;
        reads_any |= reads;
        items.extend(found);
    }
    if !reads_any {
        return Err(ApiError::Denied(NO_AGENT_KIND.into()));
    }
    Ok(Json(AgentRegistry { items }))
}

const NO_AGENT_KIND: &str = "Your roles read no agent in any project: assistant deployments, \
     agent profiles, MCP servers, service accounts and agent runs need read on their kind. Ask a \
     steward of the project or an organization administrator to grant it.";

/// The agents of one namespace the caller may read, and whether they read any agent kind there.
async fn namespace_items(
    state: &AppState,
    identity: &Identity,
    project: &str,
    effective: &Effective,
) -> Result<(Vec<AgentItem>, bool), ApiError> {
    let runs = if effective.may_read_project() {
        visible_runs(state, identity, project).await?
    } else {
        Vec::new()
    };
    let mut items = Vec::new();
    let mut reads = false;
    for kind in KINDS {
        if !effective.may_read(kind) {
            continue;
        }
        reads = true;
        for env in state
            .mirror
            .list(project, kind, &ListOptions::default())
            .items
        {
            let value = serde_json::to_value(&env).unwrap_or(Value::Null);
            if effective.may_read_manifest(kind, &value) {
                items.push(manifest_item(state, &env, &runs));
            }
        }
    }
    items.extend(
        runs.iter()
            .filter(|run| in_flight(run))
            .map(|run| run_item(state, run)),
    );
    Ok((items, reads))
}

/// The runs of a project the caller would see in its run list: their own, or every person's
/// for an approver of the project (AG-93, as `GET …/agent-runs` lists them).
pub(crate) async fn visible_runs(
    state: &AppState,
    identity: &Identity,
    project: &str,
) -> Result<Vec<AgentRun>, ApiError> {
    let approver = crate::api::changes::may_approve_anything(state, identity, project).is_ok();
    let filter = RunFilter {
        created_by: (!approver).then(|| identity.username.clone()),
        origin: Some("person".into()),
        ..RunFilter::default()
    };
    state
        .agents
        .list_runs_filtered(project, &filter, RUNS_PER_PROJECT)
        .await
        .map_err(crate::api::agent_runs::unavailable)
}

fn in_flight(run: &AgentRun) -> bool {
    AgentRunStatus::parse(&run.status).is_some_and(|status| !status.is_terminal())
}

/// One manifest as a row; `runs` are the project's runs the caller sees, for activity and spend.
fn manifest_item(state: &AppState, env: &ResourceEnvelope, runs: &[AgentRun]) -> AgentItem {
    let project = env.metadata.namespace.clone().unwrap_or_default();
    let spec = &env.spec;
    let endpoints = endpoints_of(&env.kind, spec);
    let mut spaces: BTreeSet<String> = endpoints
        .iter()
        .filter_map(|name| endpoint_space(state, &project, name))
        .collect();
    spaces.extend(role_spaces(spec));
    let own_runs: Vec<&AgentRun> = match env.kind.as_str() {
        "AgentProfile" => runs
            .iter()
            .filter(|r| r.profile == env.metadata.name)
            .collect(),
        _ => Vec::new(),
    };
    AgentItem {
        kind: env.kind.clone(),
        name: env.metadata.name.clone(),
        title: crate::resource::title_text(&spec["title"]),
        owner: manifest_owner(spec, &project),
        acts_as: manifest_acts_as(&env.kind, spec),
        spaces: spaces.into_iter().collect(),
        endpoints,
        last_activity: own_runs.iter().map(|run| last_seen(run)).max(),
        spend: manifest_spend(&env.kind, spec, &own_runs),
        state: match (env.kind.as_str(), &spec["enabled"]) {
            ("AssistantDeployment", Value::Bool(false)) => AgentState::Paused,
            _ => AgentState::Active,
        },
        project,
        run_id: None,
        profile: None,
    }
}

fn run_item(state: &AppState, run: &AgentRun) -> AgentItem {
    let endpoints: Vec<String> = run
        .endpoints
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["name"].as_str().map(str::to_owned))
        .collect();
    let endpoints = if endpoints.is_empty() {
        vec![run.endpoint_name.clone()]
    } else {
        endpoints
    };
    let spaces: BTreeSet<String> = run
        .endpoints
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["space"].as_str().map(str::to_owned))
        .chain(
            endpoints
                .iter()
                .filter_map(|n| endpoint_space(state, &run.project, n)),
        )
        .collect();
    let cap = state
        .mirror
        .get(
            crate::permissions::ORG_NAMESPACE,
            "AgentProfile",
            &run.profile,
        )
        .and_then(|profile| profile.spec.pointer("/model/maxTokensPerRun")?.as_u64());
    AgentItem {
        kind: "AgentRun".into(),
        name: run.title.clone().unwrap_or_else(|| run.app_name.clone()),
        project: run.project.clone(),
        title: None,
        owner: Owner {
            kind: "person".into(),
            name: run.created_by.clone(),
        },
        acts_as: ActsAs::Caller,
        spaces: spaces.into_iter().collect(),
        endpoints,
        last_activity: Some(last_seen(run)),
        spend: Spend {
            used: Some(run.tokens_used),
            cap,
            per: Some("run".into()),
        },
        state: run_state(&run.status),
        run_id: Some(run.id.clone()),
        profile: Some(run.profile.clone()),
    }
}

fn run_state(status: &str) -> AgentState {
    match AgentRunStatus::parse(status) {
        Some(AgentRunStatus::Interviewing | AgentRunStatus::AwaitingApproval) => {
            AgentState::Waiting
        }
        _ => AgentState::Running,
    }
}

fn last_seen(run: &AgentRun) -> String {
    [
        Some(&run.created_at),
        run.started_at.as_ref(),
        run.finished_at.as_ref(),
    ]
    .into_iter()
    .flatten()
    .max()
    .cloned()
    .unwrap_or_default()
}

/// The Endpoints a manifest names: a deployment's connectors and MCP server, an MCP server's
/// `Endpoint` members.
fn endpoints_of(kind: &str, spec: &Value) -> Vec<String> {
    let names = |list: &str, key: &str, only: Option<&str>| -> Vec<String> {
        spec[list]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| only.is_none_or(|kind| item["kind"] == kind))
            .filter_map(|item| item[key].as_str().map(str::to_owned))
            .collect()
    };
    let mut found: BTreeSet<String> = match kind {
        "AssistantDeployment" => names("connectors", "endpoint", None).into_iter().collect(),
        "McpServer" => names("members", "name", Some("Endpoint"))
            .into_iter()
            .collect(),
        _ => BTreeSet::new(),
    };
    found.remove("");
    found.into_iter().collect()
}

fn endpoint_space(state: &AppState, project: &str, endpoint: &str) -> Option<String> {
    let env = state.mirror.get(project, "Endpoint", endpoint)?;
    crate::permissions::space_ref(&serde_json::to_value(env).ok()?)
}

/// The spaces a service account's roles are scoped to.
fn role_spaces(spec: &Value) -> Vec<String> {
    spec["roles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|role| {
            role.pointer("/scope/contextSpace")?
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

fn manifest_owner(spec: &Value, project: &str) -> Owner {
    match spec.pointer("/owner/user").and_then(Value::as_str) {
        Some(user) => Owner {
            kind: "person".into(),
            name: user.to_owned(),
        },
        None => Owner {
            kind: "project".into(),
            name: project.to_owned(),
        },
    }
}

/// A service account and an assistant on a visitor channel use their own identity; an internal
/// assistant, a profile's runs and an MCP server's clients use the caller's (AG-94, AG-115).
fn manifest_acts_as(kind: &str, spec: &Value) -> ActsAs {
    match kind {
        "ServiceAccount" => ActsAs::Own,
        "AssistantDeployment" if spec["channel"] != "internal" => ActsAs::Own,
        _ => ActsAs::Caller,
    }
}

fn manifest_spend(kind: &str, spec: &Value, runs: &[&AgentRun]) -> Spend {
    match kind {
        "AssistantDeployment" => Spend {
            used: None,
            cap: spec.pointer("/budget/tokensPerDay").and_then(Value::as_u64),
            per: Some("day".into()),
        },
        "AgentProfile" => {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let used = runs
                .iter()
                .filter(|run| run.created_at.starts_with(&today))
                .map(|run| run.tokens_used)
                .sum();
            Spend {
                used: Some(used),
                cap: None,
                per: Some("day".into()),
            }
        }
        _ => Spend::default(),
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/organization/agents", get(list_agents))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_visitor_assistant_acts_as_itself_and_an_internal_one_as_the_person() {
        let public = json!({"channel": "public"});
        let internal = json!({"channel": "internal"});
        assert_eq!(
            manifest_acts_as("AssistantDeployment", &public),
            ActsAs::Own
        );
        assert_eq!(
            manifest_acts_as("AssistantDeployment", &internal),
            ActsAs::Caller
        );
        assert_eq!(manifest_acts_as("ServiceAccount", &internal), ActsAs::Own);
        assert_eq!(manifest_acts_as("McpServer", &public), ActsAs::Caller);
        assert_eq!(manifest_acts_as("AgentProfile", &public), ActsAs::Caller);
    }

    #[test]
    fn endpoints_come_from_connectors_and_endpoint_members_only() {
        let deployment = json!({"connectors": [{"endpoint": "air"}, {"endpoint": "bikes"}, {"endpoint": "air"}]});
        assert_eq!(
            endpoints_of("AssistantDeployment", &deployment),
            ["air", "bikes"]
        );
        let server = json!({"members": [
            {"kind": "Endpoint", "name": "air"},
            {"kind": "KnowledgeSource", "name": "guide"}
        ]});
        assert_eq!(endpoints_of("McpServer", &server), ["air"]);
        assert!(endpoints_of("ServiceAccount", &server).is_empty());
    }

    #[test]
    fn the_owner_is_the_named_person_else_the_project() {
        let account = json!({"owner": {"user": "jana@hel.fi"}});
        assert_eq!(
            manifest_owner(&account, "helsinki"),
            Owner {
                kind: "person".into(),
                name: "jana@hel.fi".into()
            }
        );
        assert_eq!(
            manifest_owner(&json!({}), "helsinki"),
            Owner {
                kind: "project".into(),
                name: "helsinki".into()
            }
        );
    }

    #[test]
    fn a_run_waits_while_it_asks_or_awaits_approval() {
        assert_eq!(run_state("interviewing"), AgentState::Waiting);
        assert_eq!(run_state("awaiting_approval"), AgentState::Waiting);
        assert_eq!(run_state("building"), AgentState::Running);
    }

    #[test]
    fn a_deployment_spends_against_its_day_cap_and_a_service_account_carries_none() {
        let spec = json!({"budget": {"tokensPerDay": 200000}});
        assert_eq!(
            manifest_spend("AssistantDeployment", &spec, &[]),
            Spend {
                used: None,
                cap: Some(200000),
                per: Some("day".into())
            }
        );
        assert_eq!(
            manifest_spend("ServiceAccount", &spec, &[]),
            Spend::default()
        );
    }

    #[test]
    fn a_service_account_reaches_the_spaces_its_roles_name() {
        let spec = json!({"roles": [
            {"role": "reader", "scope": {"contextSpace": "air"}},
            {"role": "reader", "scope": {"project": "helsinki"}}
        ]});
        assert_eq!(role_spaces(&spec), ["air"]);
    }

    /// Project helsinki, where `reader` may read `kinds`, holding `extra` too.
    fn reader_world(
        reader: &str,
        kinds: serde_json::Value,
        extra: Vec<crate::resource::ResourceEnvelope>,
    ) -> AppState {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let envelope =
            |kind: &str, name: &str, namespace: &str, spec: serde_json::Value| ResourceEnvelope {
                api_version: API_VERSION.to_owned(),
                kind: kind.to_owned(),
                metadata: ObjectMeta::new(name, namespace),
                spec,
                status: None,
            };
        let state = AppState::new(crate::config::Config::for_tests(), None);
        let org = crate::permissions::ORG_NAMESPACE;
        state.mirror.upsert(envelope(
            "Role",
            "reader",
            org,
            serde_json::json!({ "rules": [{ "kinds": kinds, "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            "reader",
            org,
            serde_json::json!({ "subjects": [{ "user": reader }], "role": "reader", "scope": { "project": "helsinki" } }),
        ));
        for env in extra {
            state.mirror.upsert(env);
        }
        state
    }

    fn signed_in(email: &str) -> CurrentUser {
        CurrentUser(crate::auth::session::Session {
            identity: crate::auth::session::Identity {
                client: None,
                subject: format!("f:1:{email}"),
                username: email.to_owned(),
                email: Some(email.to_owned()),
                name: None,
                roles: Vec::new(),
                groups: Vec::new(),
                second_factor: false,
            },
            expires_at: 0,
            issued_at: 0,
            signed_in_at: None,
            id_token: String::new(),
            access_expires_at: 0,
            refresh_token: None,
        })
    }

    #[tokio::test]
    async fn the_registry_lists_what_the_caller_reads_and_refuses_one_who_reads_no_agent_kind() {
        use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
        let account = ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: "ServiceAccount".to_owned(),
            metadata: ObjectMeta::new("pipelines", "helsinki"),
            spec: json!({}),
            status: None,
        };
        let state = reader_world("reader@hel.fi", json!(["ServiceAccount"]), vec![account]);
        let listed = list_agents(signed_in("reader@hel.fi"), State(state.clone()))
            .await
            .expect("the registry");
        let names: Vec<_> = listed
            .0
            .items
            .iter()
            .map(|item| {
                (
                    item.kind.as_str(),
                    item.project.as_str(),
                    item.name.as_str(),
                )
            })
            .collect();
        assert_eq!(names, [("ServiceAccount", "helsinki", "pipelines")]);
        match list_agents(signed_in("stranger@hel.fi"), State(state)).await {
            Err(ApiError::Denied(reason)) => assert_eq!(reason, NO_AGENT_KIND),
            other => panic!("not refused: {:?}", other.map(|_| ())),
        }
    }
}
