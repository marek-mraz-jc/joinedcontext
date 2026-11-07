//! What a principal reaches (API/01 §41, EP-103, EP-59, T-3792).
//!
//! The Access explorer's one question: for one person, group member, role holder, ServiceAccount
//! or the public, every Endpoint of the project they reach and what they may do there. The Portal
//! decides nothing: it resolves the subject as the Policy simulator does (§39) and asks the
//! gateway's `access/simulate` with no action for each Endpoint, keeping those the gateway admits
//! the subject to and grants or prohibits something on. An Endpoint left out is absent, never
//! listed as refused (EP-59, SP-20).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use utoipa::ToSchema;

use crate::api::access_simulate::{ask_gateway, resolve, Resolved, Who};
use crate::auth::CurrentUser;
use crate::error::ApiError;
use crate::people::Consent;
use crate::state::AppState;
use crate::store::ListOptions;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExploreRequest {
    pub subject: Who,
}

/// One Endpoint the subject reaches, with the gateway's grant document for them.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Reached {
    pub name: String,
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub space: String,
    /// The surfaces the Endpoint serves; the page builds each URL under `/api/endpoint/{slug}`.
    pub representations: Vec<String>,
    /// The Policies in force for the subject here.
    pub policies: Vec<String>,
    /// The AuthZEN permissions as the gateway answered them (types, actions, attributes,
    /// constraints `q`, `scopeQ`, `geoQ`, `temporalQ`).
    #[schema(value_type = Vec<Object>)]
    pub permissions: Value,
    #[schema(value_type = Vec<Object>)]
    pub prohibitions: Value,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Explored {
    pub subject: Resolved,
    pub endpoints: Vec<Reached>,
    /// The project's McpServers with a member among `endpoints`.
    pub mcp_servers: Vec<String>,
    /// A person's Keycloak consents; empty for every other kind.
    pub consents: Vec<Consent>,
}

/// The gateway questions one exploration keeps in flight at once (T-4446).
pub const EXPLORE_AT_ONCE: usize = 16;

#[utoipa::path(
    post,
    path = "/api/v1/projects/{project}/access/explore",
    summary = "See What Someone Can Access",
    description = "Every Endpoint of the project a person, a group or role member, a ServiceAccount or the public reaches, with the gateway's grant document for them on each, the McpServers covering them and, for a person, their consents (API/01 §41, EP-103). Organization administrators only; recorded as an `access.explored` activity event. Changes no data.",
    tag = "access",
    params(("project" = String, Path, description = "Project name")),
    request_body(
        content = ExploreRequest,
        description = "Who.",
        content_type = "application/json",
        example = json!({ "subject": { "kind": "serviceAccount", "name": "etl" } })
    ),
    responses(
        (status = 200, description = "The Endpoints the subject reaches", body = Explored),
        (status = 400, description = "An unknown member or kind", body = crate::error::ProblemDetails),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
        (status = 403, description = "Not an organization administrator", body = crate::error::ProblemDetails),
        (status = 404, description = "No such person or ServiceAccount", body = crate::error::ProblemDetails),
        (status = 503, description = "No gateway, no realm admin client, or one of them did not answer", body = crate::error::ProblemDetails),
    )
)]
pub async fn explore_access(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(project): Path<String>,
    body: Result<Json<ExploreRequest>, JsonRejection>,
) -> Result<Json<Explored>, ApiError> {
    let identity = &user.0.identity;
    crate::permissions::require_organization_admin(
        &state,
        identity,
        "seeing what someone can access",
    )?;
    let Json(request) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let subject = resolve(&state, &project, &request.subject).await?;
    let question = json!({ "subject": subject });

    let endpoints = state
        .mirror
        .list(&project, "Endpoint", &ListOptions::default())
        .items;
    // At most EXPLORE_AT_ONCE questions in flight: a project of thousands of Endpoints would
    // otherwise open as many connections to the gateway at once (T-4446). `buffered` keeps the
    // answers in the Endpoints' order.
    let slugs: Vec<String> = endpoints
        .iter()
        .map(|endpoint| {
            endpoint.spec["slug"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    let answers: Vec<Result<Value, ApiError>> = futures_util::stream::iter(slugs)
        .map(|slug| {
            let question = &question;
            let state = &state;
            async move {
                if slug.is_empty() {
                    return Ok(Value::Null);
                }
                match ask_gateway(state, &slug, question).await {
                    // Not served by the gateway yet: nobody reaches it.
                    Err(ApiError::NotFound(_)) => Ok(Value::Null),
                    answer => answer,
                }
            }
        })
        .buffered(EXPLORE_AT_ONCE)
        .collect()
        .await;
    let mut reached = Vec::new();
    for (endpoint, answer) in endpoints.iter().zip(answers) {
        if let Some(row) = row(endpoint, answer?) {
            reached.push(row);
        }
    }

    let mcp_servers = mcp_servers(&state, &project, &reached);
    let consents = match &request.subject {
        Who::Person { id } => match &state.people {
            Some(people) => people.admin().await?.consents(id).await?,
            None => Vec::new(),
        },
        _ => Vec::new(),
    };
    record(&state, identity, &project, &subject, reached.len()).await;
    Ok(Json(Explored {
        subject,
        endpoints: reached,
        mcp_servers,
        consents,
    }))
}

/// The row of an Endpoint the gateway admits the subject to and grants or prohibits something
/// on; `None` for every other answer.
fn row(endpoint: &crate::resource::ResourceEnvelope, answer: Value) -> Option<Reached> {
    if answer["admitted"] != json!(true) {
        return None;
    }
    let empty = |member: &str| answer[member].as_array().is_none_or(Vec::is_empty);
    if empty("permissions") && empty("prohibitions") {
        return None;
    }
    let spec = &endpoint.spec;
    Some(Reached {
        name: endpoint.metadata.name.clone(),
        slug: spec["slug"].as_str().unwrap_or_default().to_owned(),
        title: spec["title"].as_str().map(str::to_owned),
        space: answer["resource"]["space"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        // One rule for what an Endpoint serves, the catalogue's and the instance's (T-5699).
        representations: crate::api::catalogue::served_representations(spec),
        policies: answer["policies"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        permissions: answer["permissions"].clone(),
        prohibitions: answer["prohibitions"].clone(),
    })
}

/// The project's McpServers naming one of the reached Endpoints as a member (ADR-N-043).
fn mcp_servers(state: &AppState, project: &str, reached: &[Reached]) -> Vec<String> {
    state
        .mirror
        .list(project, "McpServer", &ListOptions::default())
        .items
        .into_iter()
        .filter(|server| {
            server.spec["members"].as_array().is_some_and(|members| {
                members.iter().any(|member| {
                    member["kind"] == "Endpoint"
                        && member["namespace"]
                            .as_str()
                            .is_none_or(|namespace| namespace == project)
                        && reached
                            .iter()
                            .any(|row| member["name"] == row.name.as_str())
                })
            })
        })
        .map(|server| server.metadata.name)
        .collect()
}

/// Who looked at whose access (EP-103, T-3792). A record that cannot be written is logged, not a
/// failure of the answer.
async fn record(
    state: &AppState,
    identity: &crate::auth::Identity,
    project: &str,
    subject: &Resolved,
    reached: usize,
) {
    let named = subject.label();
    let event = crate::activity::ActivityEvent {
        time: chrono::Utc::now(),
        project: project.to_owned(),
        space: None,
        kind: "access.explored".to_owned(),
        source: "portal".to_owned(),
        summary: format!(
            "{} looked at what {named} can access: {reached} Endpoints",
            identity.username
        ),
        severity: "info".to_owned(),
        correlation_id: None,
        details: json!({ "actor": identity.username, "subject": subject, "endpoints": reached }),
    };
    if let Err(err) = state.activity.append(&[event]).await {
        tracing::warn!(error = %err, "the access explore was not recorded");
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/projects/{project}/access/explore", post(explore_access))
}
