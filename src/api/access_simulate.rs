//! Trying a Policy on a person (API/01 §39, EP-103, T-3311).
//!
//! An organization administrator picks who, which Endpoint, which action and optionally which
//! type, and reads what the gateway would decide and which Policy decided it. The Portal decides
//! nothing itself: it resolves the chosen subject (a person's groups and realm roles through the
//! realm's admin API, as the People pages read them, PF-108) and asks the gateway's
//! `access/simulate` with its own service-account token, so the answer is the gateway's own
//! evaluator, not a second one that could drift from it.

use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::state::AppState;

/// Who is tried: one person, a member of a group or of a realm role with no person of their own,
/// one of the project's ServiceAccounts, or the public.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Who {
    /// A person, by their Keycloak user id.
    Person { id: String },
    /// A member of the realm group `name`.
    Group { name: String },
    /// A holder of the realm role `name`.
    Role { name: String },
    /// The project's ServiceAccount `name`.
    ServiceAccount { name: String },
    /// The anonymous caller.
    Public {},
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SimulateRequest {
    pub subject: Who,
    /// A CIM 009 operation name, such as `retrieveEntity`.
    pub action: String,
    /// The entity type the question is about; absent asks about any type.
    #[serde(default, rename = "type")]
    pub entity_type: Option<String>,
}

/// The subject as the gateway decided it.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    pub groups: Vec<String>,
    pub roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account: Option<String>,
}

impl Resolved {
    /// Whom a record names: the person, the ServiceAccount, a group or role member, or the public.
    pub(crate) fn label(&self) -> String {
        self.user
            .clone()
            .or_else(|| self.service_account.clone())
            .or_else(|| self.groups.first().map(|g| format!("a member of {g}")))
            .or_else(|| self.roles.first().map(|r| format!("a holder of {r}")))
            .unwrap_or_else(|| "the public".to_owned())
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Simulated {
    pub decision: bool,
    /// `policy_grant_matched`, `prohibited`, `no_grant` or `not_admitted`.
    pub reason: String,
    /// The manifest name of the Policy that decided, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigner: Option<String>,
    pub subject: Resolved,
}

/// The longest group, role or ServiceAccount name the route passes on.
const MAX_NAME: usize = 255;

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/endpoints/{name}/access/simulate",
    summary = "Try A Policy On A Person",
    description = "What the gateway would decide for a person, a group or role member, a ServiceAccount or the public on one Endpoint, and the Policy that decided it (API/01 §39, EP-103). Organization administrators only; recorded as an `access.simulated` activity event. Changes no data.",
    tag = "access",
    params(
        ("project" = String, Path, description = "Project name"),
        ("name" = String, Path, description = "Endpoint name"),
    ),
    request_body(
        content = SimulateRequest,
        description = "Who, which action and optionally which type.",
        content_type = "application/json",
        example = json!({
            "subject": { "kind": "person", "id": "8c0e5a1e-2b7d-4c55-9a43-6f0d2e1b9a10" },
            "action": "retrieveEntity",
            "type": "AirQualityObserved"
        })
    ),
    responses(
        (status = 200, description = "The gateway's decision and the Policy that decided", body = Simulated),
        (status = 400, description = "An unknown member or kind, or an action that is not a CIM 009 operation", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an organization administrator", body = crate::error::ProblemDetails),
        (status = 404, description = "No such Endpoint, person or ServiceAccount", body = crate::error::ProblemDetails),
        (status = 503, description = "No gateway, no realm admin client, or one of them did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn simulate_access(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((project, name)): Path<(String, String)>,
    body: Result<Json<SimulateRequest>, JsonRejection>,
) -> Result<Json<Simulated>, ApiError> {
    let identity = &user.0.identity;
    crate::permissions::require_organization_admin(
        &state,
        identity,
        "trying a Policy on a person",
    )?;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    if !jc_core::kinds::Operation::ALL
        .iter()
        .any(|operation| operation.as_str() == request.action)
    {
        return Err(ApiError::BadRequest(format!(
            "action {:?} is not a CIM 009 operation",
            request.action
        )));
    }
    if let Some(entity_type) = &request.entity_type {
        named("type", entity_type)?;
    }
    let slug = state
        .mirror
        .get(&project, "Endpoint", &name)
        .and_then(|endpoint| endpoint.spec["slug"].as_str().map(str::to_owned))
        .ok_or_else(|| ApiError::NotFound(format!("no Endpoint {name} in project {project}")))?;
    let subject = resolve(&state, &project, &request.subject).await?;
    let mut question = json!({ "subject": subject, "action": { "name": request.action } });
    if let Some(entity_type) = &request.entity_type {
        question["resource"] = json!({ "type": entity_type.trim() });
    }
    let answer = ask_gateway(&state, &slug, &question).await?;
    let context = &answer["context"];
    let simulated = Simulated {
        decision: answer["decision"] == json!(true),
        reason: context["reason"].as_str().unwrap_or("no_grant").to_owned(),
        policy: context["policy"].as_str().map(str::to_owned),
        assigner: context["assigner"].as_str().map(str::to_owned),
        subject,
    };
    record(&state, identity, &project, &name, &request, &simulated).await;
    Ok(Json(simulated))
}

/// A group, role, ServiceAccount or type name as the route passes it on: present, short, printable.
fn named<'a>(what: &str, value: &'a str) -> Result<&'a str, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_NAME || value.chars().any(char::is_control) {
        return Err(ApiError::BadRequest(format!(
            "{what} must be 1 to {MAX_NAME} printable characters"
        )));
    }
    Ok(value)
}

/// The subject the gateway is asked about, as a token of theirs would carry it.
pub(crate) async fn resolve(
    state: &AppState,
    project: &str,
    who: &Who,
) -> Result<Resolved, ApiError> {
    Ok(match who {
        Who::Person { id } => {
            let people = state.people.as_ref().ok_or_else(|| {
                ApiError::Unavailable(
                    "this Portal has no realm admin client to read people with".into(),
                )
            })?;
            let admin = people.admin().await?;
            let person = admin.get(id).await?;
            let (groups, roles) = admin.groups_and_realm_roles(id).await?;
            Resolved {
                user: Some(person.id),
                groups,
                roles,
                service_account: None,
            }
        }
        Who::Group { name } => Resolved {
            groups: vec![named("group", name)?.to_owned()],
            ..Resolved::default()
        },
        Who::Role { name } => Resolved {
            roles: vec![named("role", name)?.to_owned()],
            ..Resolved::default()
        },
        Who::ServiceAccount { name } => {
            let name = named("serviceAccount", name)?;
            if state.mirror.get(project, "ServiceAccount", name).is_none() {
                return Err(ApiError::NotFound(format!(
                    "no ServiceAccount {name} in project {project}"
                )));
            }
            Resolved {
                // The Keycloak client id is derived, never chosen (Architecture/12 §3).
                service_account: Some(format!("{project}-{name}")),
                ..Resolved::default()
            }
        }
        Who::Public {} => Resolved::default(),
    })
}

/// The gateway's `access/simulate`, with the Portal's own token: never the administrator's, whose
/// token would decide as them.
pub(crate) async fn ask_gateway(
    state: &AppState,
    slug: &str,
    body: &Value,
) -> Result<Value, ApiError> {
    let base = state.config.gateway_url.as_deref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no gateway address (JC_PORTAL_GATEWAY_URL)".into())
    })?;
    let oidc = state.oidc.as_ref().ok_or_else(|| {
        ApiError::Unavailable("this Portal has no Keycloak client to ask the gateway with".into())
    })?;
    let token = oidc.service_token().await?;
    let answer = crate::api::pipelines::http()
        .post(format!("{base}/api/endpoint/{slug}/access/simulate"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(10))
        .json(body)
        .send()
        .await
        .map_err(|err| {
            tracing::warn!(error = %err.without_url(), "the gateway did not answer access/simulate");
            ApiError::Unavailable("the gateway did not answer; try again shortly".into())
        })?;
    let status = answer.status().as_u16();
    let body: Value = answer.json().await.unwrap_or(Value::Null);
    let detail = || {
        body.get("detail")
            .and_then(Value::as_str)
            .unwrap_or("no reason given")
            .to_owned()
    };
    match status {
        200 => Ok(body),
        400 => Err(ApiError::BadRequest(detail())),
        404 => Err(ApiError::NotFound(
            "the gateway does not serve this Endpoint yet".into(),
        )),
        // A 401 or 403 here is this Portal's client, not the administrator: the realm's mapper or
        // the gateway's client id is wrong, which an operator fixes.
        _ => {
            tracing::warn!(status, "the gateway refused the Portal's access/simulate");
            Err(ApiError::Unavailable(format!(
                "the gateway refused the Portal's question ({status})"
            )))
        }
    }
}

/// The record of who asked about whom (EP-103): the administrator, the subject as resolved, the
/// Endpoint, the question and the answer. A record that cannot be written is logged, not a
/// failure of the answer the administrator already holds.
async fn record(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    endpoint: &str,
    request: &SimulateRequest,
    simulated: &Simulated,
) {
    let subject = &simulated.subject;
    let named = subject.label();
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "access.simulated".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} tried {} on {endpoint}: {} {}",
            identity.username,
            named,
            request.action,
            if simulated.decision {
                "allowed"
            } else {
                "refused"
            }
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({
            "actor": identity.username,
            "subject": subject,
            "endpoint": endpoint,
            "action": request.action,
            "type": request.entity_type,
            "decision": simulated.decision,
            "reason": simulated.reason,
            "policy": simulated.policy,
        }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, "the access simulation was not recorded");
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project}/endpoints/{name}/access/simulate",
        post(simulate_access),
    )
}
