//! `/api/v1/connectors`: a person's own connectors (AG-142, AG-147, API/14 §2, T-5950).
//!
//! Scoped by the caller's Keycloak `sub`: another person's connector answers the same `404` as
//! an id that does not exist. A connector's own token is not a token for these routes (its
//! audience is `/c/{id}`), so no connector creates, widens or revokes a connector (AG-151); a
//! workload's account is refused too, since a connector is a person's grant.
//!
//! `/api/v1/organization/connectors` is the organization administrator's overview (T-5975): every
//! connector with its owner, scope, clients and the last day's calls and denials, and a revoke.
//! It shows the audit's tool and target counts only, never a call's input or a token.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::session::Identity;
use crate::auth::CurrentUser;
use crate::connectors::{self, Call, Ceiling, Connector, NewConnector, Overview};
use crate::error::{ApiError, ProblemDetails};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/connectors", get(list_connectors).post(create_connector))
        .route("/connectors/options", get(connector_options))
        .route(
            "/connectors/{id}",
            get(get_connector)
                .patch(edit_connector)
                .delete(revoke_connector),
        )
        .route("/connectors/{id}/activity", get(connector_activity))
        .route("/connectors/pending", get(pending_connectors))
        .route("/connectors/{id}/approve", post(approve_connector))
        .route(
            "/connectors/{id}/tab",
            axum::routing::post(pair_tab).delete(unpair_tab),
        )
        .route("/connectors/{id}/tab/{tab}/events", get(tab_events))
        .route("/organization/connectors", get(organization_connectors))
        .route(
            "/organization/connectors/{id}",
            axum::routing::delete(revoke_organization_connector),
        )
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorOverview {
    pub items: Vec<Overview>,
    /// Refused calls in a day that flag a connector.
    pub denials_flag: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectorList {
    pub items: Vec<Connector>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ConnectorCalls {
    pub items: Vec<Call>,
}

/// What the caller may put in a connector today: the ceiling, and per project they read the
/// spaces, Endpoints and pipelines they read there (the "My connectors" tree, T-5951).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorOptions {
    /// Every preset a connector may name; those outside `ceiling.segments` need approval.
    pub segments: Vec<String>,
    /// The kinds each preset that plans changes reaches, one row each in the dialog's verb table
    /// (T-6302); `data-read` proposes nothing and has none.
    pub segment_kinds: Vec<SegmentKinds>,
    pub ceiling: CeilingView,
    pub projects: Vec<ProjectReach>,
    /// The realm's MCP clients a connector may be bound to, e.g. `mcp-local` (Claude Code).
    pub clients: Vec<ClientChoice>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientChoice {
    pub id: String,
    /// Whether naming it binds the connector (a published client id) or only says whom it is
    /// meant for (a loopback client anyone can name, AG-153).
    pub is_boundary: bool,
    /// Why the caller's ceiling refuses it, e.g. `DelegationPolicy p does not allow the client
    /// mcp-hub`; absent when it may be named. Create checks it again (T-6309).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// One preset and the kinds it reaches, e.g. `pipeline`: `Pipeline`, `DataSource`, ...
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SegmentKinds {
    pub segment: String,
    pub kinds: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CeilingView {
    /// The presets a connector of the caller may carry, e.g. `["data-read"]`.
    pub segments: Vec<String>,
    /// Whether a connector of the caller may hold a write item.
    pub writes: bool,
    /// The longest life a connector may get, in days.
    pub max_days: i64,
    /// The verbs a connector may tick per kind of the presets; a verb left out is greyed.
    pub kinds: Vec<connectors::KindGrant>,
}

/// One project the caller reads, with the objects of it they read, in name order.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectReach {
    pub project: String,
    pub spaces: Vec<String>,
    pub endpoints: Vec<String>,
    pub pipelines: Vec<String>,
    /// Whether the caller's ceiling lets a connector reach this project at all; Create refuses
    /// an item of a project with `false`.
    pub allowed: bool,
    /// The Endpoints of `endpoints` the ceiling lets an item name when its policy lists
    /// Endpoints: then the whole project, its spaces and pipelines are refused. Absent when every
    /// object of an allowed project may be named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_endpoints: Option<Vec<String>>,
    /// Organization data (ADR-N-067 §2.4): the caller holds no right in the project and reads only
    /// `endpoints`, shared with their Organization or a group of theirs. An item names one of
    /// them, for reading, and the connector is `data-read` with no writes.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub organization_data: bool,
}

/// The projects the caller holds no right in but reads Endpoints of, shared with their
/// Organization or a group of theirs (ADR-N-067 §2.4, T-6413): only those Endpoints, for reading.
async fn organization_reach(
    state: &AppState,
    session: &crate::auth::session::Session,
    headers: &axum::http::HeaderMap,
    ceiling: &Ceiling,
    held: &[ProjectReach],
) -> Vec<ProjectReach> {
    let catalogue = crate::api::catalogue_readable::readable_by(state, session, headers).await;
    let mut out: Vec<ProjectReach> = Vec::new();
    for dataset in catalogue.datasets {
        if dataset.access == "public" || held.iter().any(|p| p.project == dataset.project) {
            continue;
        }
        let at = match out.iter().position(|p| p.project == dataset.project) {
            Some(at) => at,
            None => {
                out.push(ProjectReach {
                    allowed: ceiling.allows_project_name(&dataset.project),
                    project: dataset.project.clone(),
                    spaces: Vec::new(),
                    endpoints: Vec::new(),
                    pipelines: Vec::new(),
                    allowed_endpoints: None,
                    organization_data: true,
                });
                out.len() - 1
            }
        };
        out[at].endpoints.push(dataset.name);
    }
    for reach in &mut out {
        reach.endpoints.sort();
        reach.endpoints.dedup();
        if reach.allowed && !ceiling.allows_endpoint(&reach.project, None) {
            reach.allowed_endpoints = Some(
                reach
                    .endpoints
                    .iter()
                    .filter(|e| ceiling.allows_endpoint(&reach.project, Some(e)))
                    .cloned()
                    .collect(),
            );
        }
    }
    out.sort_by(|a, b| a.project.cmp(&b.project));
    out
}

/// The fields of a connector its owner may change (JSON merge patch, API/14 §2); the name stays.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorPatch {
    #[serde(default)]
    pub segments: Option<Vec<String>>,
    #[serde(default)]
    pub scope: Option<Vec<connectors::ScopeItem>>,
    /// Verbs per kind, replacing the whole list (T-6302).
    #[serde(default)]
    pub kinds: Option<Vec<connectors::KindGrant>>,
    #[serde(default)]
    pub writes: Option<bool>,
    /// Approve each applied change at once as the owner (T-6552), e.g. `true`.
    #[serde(default)]
    pub autonomous: Option<bool>,
    /// RFC 3339, e.g. `2026-11-27T00:00:00Z`.
    #[serde(default)]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub allowed_clients: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateParams {
    /// `All` validates only: nothing is kept.
    #[serde(default)]
    pub dry_run: Option<String>,
}

pub(crate) fn base(state: &AppState) -> String {
    state
        .config
        .public_base_url
        .as_str()
        .trim_end_matches('/')
        .to_owned()
}

fn not_found() -> ApiError {
    ApiError::NotFound("no such connector".into())
}

/// A connector is a person's grant; a workload's account holds none.
fn person(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    // Keycloak names a client's own user `service-account-<client>`, and a token naming no
    // user at all is its client's: either is a workload's even where no ServiceAccount
    // manifest claims it.
    let workload = identity.username.starts_with("service-account-")
        || identity.client.as_deref() == Some(identity.username.as_str());
    if workload || crate::permissions::is_service_account(&state.mirror, identity) {
        return Err(ApiError::Denied(
            "a connector is a person's grant; a service account holds none (AG-142)".into(),
        ));
    }
    Ok(())
}

/// A person in a Portal session: no client's token, an AI client's or a token-exchanged agent's,
/// creates, widens, decides or revokes a connector (AG-151); a Portal session carries no client.
fn in_the_portal(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    person(state, identity)?;
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "a client's token manages no connector; do it signed in to the Portal (AG-151)".into(),
        ));
    }
    Ok(())
}

/// An autonomous connector (T-6552) writes, and its owner may approve changes in every project
/// it writes: its approvals are theirs. `Err` is the 422 or 403 naming `autonomous`.
pub fn may_be_autonomous(
    state: &AppState,
    identity: &Identity,
    new: &NewConnector,
) -> Result<(), ApiError> {
    if !new.autonomous {
        return Ok(());
    }
    if !new.writes {
        return Err(ApiError::Unprocessable {
            detail: "autonomous: a connector approves only what it writes; turn writes on first"
                .into(),
            errors: vec!["autonomous".into()],
        });
    }
    crate::auth::session::require_second_factor(identity, &state.config)?;
    let written = new
        .scope
        .iter()
        .filter(|item| item.access == connectors::ItemAccess::Write);
    for item in written {
        crate::api::changes::may_approve_anything(state, identity, &item.project).map_err(
            |_| {
                ApiError::Denied(format!(
                    "autonomous: you may not approve changes in project {}, so a connector of yours cannot approve them for you",
                    item.project
                ))
            },
        )?;
    }
    Ok(())
}

/// The caller's connector `id`, or the one answer for every other case.
async fn own(state: &AppState, identity: &Identity, id: &str) -> Result<Connector, ApiError> {
    if !connectors::is_id(id) {
        return Err(not_found());
    }
    match state.connectors.get(id, &base(state)).await? {
        Some(c) if c.owner == identity.subject => {
            Ok(marked(c, &Ceiling::of(&state.mirror, identity)))
        }
        _ => Err(not_found()),
    }
}

/// The "narrowed by policy" marks of a live connector: what its owner's ceiling takes away today.
fn marked(mut connector: Connector, ceiling: &Ceiling) -> Connector {
    if connector.is_active() && !ceiling.honours(&connector) {
        connector.narrowed_by = ceiling.narrowing(&connector);
    }
    connector
}

/// The owner's half of a new connector: each scope item is in a project they read today, and a
/// named object exists there (AG-150: a connector never reaches past its owner).
///
/// An item of a project the owner holds no right in is organization data (ADR-N-067 §2.4,
/// T-6413): it names one Endpoint the gateway lets the owner read today, for reading, and the
/// connector then carries `data-read` alone and no writes, whatever the ceiling would allow.
async fn reachable(
    state: &AppState,
    session: &crate::auth::session::Session,
    headers: &axum::http::HeaderMap,
    new: &NewConnector,
) -> Result<(), ApiError> {
    let identity = &session.identity;
    let mut catalogue = None;
    for (i, item) in new.scope.iter().enumerate() {
        if crate::permissions::for_request(state, identity, &item.project).may_read_project() {
            reachable_item(state, identity, i, item)?;
            continue;
        }
        if catalogue.is_none() {
            catalogue =
                Some(crate::api::catalogue_readable::readable_by(state, session, headers).await);
        }
        // Public Endpoints are read through the public hub; an item names a shared one.
        let shared: Vec<&str> = catalogue
            .iter()
            .flat_map(|c| &c.datasets)
            .filter(|d| d.project == item.project && d.access != "public")
            .map(|d| d.name.as_str())
            .collect();
        // A project nothing of is shared keeps the plain refusal, telling nothing of its shapes.
        if !shared.is_empty() {
            organization_item(i, item, new)?;
        }
        if !item
            .endpoint
            .as_deref()
            .is_some_and(|e| shared.contains(&e))
        {
            return Err(ApiError::Denied(format!(
                "scope[{i}].project: you hold no right in project {}, and no Endpoint of it you name is shared with you, so a connector of yours cannot reach it",
                item.project
            )));
        }
    }
    Ok(())
}

/// The shape an organization-data item and its connector keep: one named Endpoint, read, under
/// `data-read` alone, with no write and no verb past `read`. `Err` is the 403 naming the field.
fn organization_item(
    i: usize,
    item: &connectors::ScopeItem,
    new: &NewConnector,
) -> Result<(), ApiError> {
    let refused = |field: String, why: &str| {
        Err(ApiError::Denied(format!(
            "{field}: {why}; a connector over organization data (project {}) only reads the Endpoints shared with you",
            item.project
        )))
    };
    if item.space.is_some() || item.pipeline.is_some() {
        return refused(
            format!("scope[{i}]"),
            "name an Endpoint, not a space or pipeline",
        );
    }
    if item.endpoint.as_deref().is_none_or(|e| e == "*") {
        return refused(format!("scope[{i}].endpoint"), "name one Endpoint");
    }
    if item.access != connectors::ItemAccess::Read {
        return refused(format!("scope[{i}].access"), "the access is read");
    }
    if new.segments.iter().any(|s| s != "data-read") {
        return refused("segments".into(), "the preset is data-read alone");
    }
    if new.writes {
        return refused("writes".into(), "it writes nothing");
    }
    let verbs = new.kinds.iter().flatten().flat_map(|k| &k.verbs);
    if verbs
        .into_iter()
        .any(|v| *v != jc_core::kinds::ConnectorVerb::Read)
    {
        return refused("kinds".into(), "every kind is read only");
    }
    Ok(())
}

/// [`reachable`] for the one scope item at index `i`.
pub(crate) fn reachable_item(
    state: &AppState,
    identity: &Identity,
    i: usize,
    item: &connectors::ScopeItem,
) -> Result<(), ApiError> {
    let rights = crate::permissions::for_request(state, identity, &item.project);
    if !rights.may_read_project() {
        return Err(ApiError::Denied(format!(
            "scope[{i}].project: you hold no right in project {}, so a connector of yours cannot reach it",
            item.project
        )));
    }
    let named = [
        ("space", "ContextSpace", &item.space),
        ("endpoint", "Endpoint", &item.endpoint),
        ("pipeline", "Pipeline", &item.pipeline),
    ];
    for (field, kind, name) in named {
        let Some(name) = name.as_deref().filter(|n| *n != "*") else {
            continue;
        };
        // One answer for an object that is not there and one the owner may not read, so a
        // refusal tells nothing about what the owner cannot see.
        let readable = state
            .mirror
            .get(&item.project, kind, name)
            .is_some_and(|env| rights.may_read_envelope(kind, &env));
        if !readable {
            return Err(ApiError::Unprocessable {
                detail: format!("project {} has no {kind} {name} you may read", item.project),
                errors: vec![format!("scope[{i}].{field}")],
            });
        }
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/connectors",
    summary = "List My Connectors",
    description = "The caller's connectors, newest first, revoked and expired ones included (API/14 §2).",
    tag = "connectors",
    responses(
        (status = 200, description = "The caller's connectors", body = ConnectorList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A service account holds no connector", body = ProblemDetails),
    )
)]
pub async fn list_connectors(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ConnectorList>, ApiError> {
    let identity = &user.0.identity;
    person(&state, identity)?;
    let ceiling = Ceiling::of(&state.mirror, identity);
    let items = state
        .connectors
        .of_owner(&identity.subject, &base(&state))
        .await?
        .into_iter()
        .map(|c| marked(c, &ceiling))
        .collect();
    Ok(Json(ConnectorList { items }))
}

/// The names of `kind` in `project` the caller reads, in name order.
fn readable_names(
    state: &AppState,
    rights: &crate::permissions::Effective,
    project: &str,
    kind: &str,
) -> Vec<String> {
    state
        .mirror
        .list(project, kind, &crate::store::ListOptions::default())
        .items
        .into_iter()
        .filter(|env| rights.may_read_envelope(kind, env))
        .map(|env| env.metadata.name)
        .collect()
}

#[utoipa::path(
    get,
    path = "/api/v1/connectors/options",
    summary = "What My Connector May Reach",
    description = "The presets, the caller's ceiling and, per project the caller reads, the spaces, Endpoints and pipelines they read: what a new connector may name. A project the caller holds no right in but reads Endpoints of, shared with their Organization or a group (ADR-N-067), comes with `organizationData` and only those Endpoints, for reading. Create checks every item again (T-5951, AG-150).",
    tag = "connectors",
    responses(
        (status = 200, description = "What a connector of the caller may name", body = ConnectorOptions),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A service account holds no connector", body = ProblemDetails),
    )
)]
pub async fn connector_options(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<ConnectorOptions>, ApiError> {
    let identity = &user.0.identity;
    person(&state, identity)?;
    let ceiling = Ceiling::of(&state.mirror, identity);
    let mut projects: Vec<ProjectReach> = state
        .mirror
        .namespaces()
        .into_iter()
        .filter_map(|project| {
            let rights = crate::permissions::for_request(&state, identity, &project);
            rights.may_read_project().then(|| {
                let endpoints = readable_names(&state, &rights, &project, "Endpoint");
                let allowed = ceiling.allows_project_name(&project);
                let allowed_endpoints =
                    (allowed && !ceiling.allows_endpoint(&project, None)).then(|| {
                        endpoints
                            .iter()
                            .filter(|e| ceiling.allows_endpoint(&project, Some(e)))
                            .cloned()
                            .collect()
                    });
                ProjectReach {
                    spaces: readable_names(&state, &rights, &project, "ContextSpace"),
                    pipelines: readable_names(&state, &rights, &project, "Pipeline"),
                    endpoints,
                    allowed,
                    allowed_endpoints,
                    organization_data: false,
                    project,
                }
            })
        })
        .collect();
    projects.extend(organization_reach(&state, &user.0, &headers, &ceiling, &projects).await);
    let mut segments: Vec<String> = connectors::SEGMENTS
        .iter()
        .map(|(name, _, _)| (*name).to_owned())
        .collect();
    segments.push(connectors::ADMIN.to_owned());
    let mut every_kind: Vec<&str> = connectors::SEGMENTS
        .iter()
        .flat_map(|(_, _, kinds)| kinds.iter().copied())
        .collect();
    every_kind.sort_unstable();
    every_kind.dedup();
    // Only presets that plan changes get verb rows: data-read reads, it proposes nothing.
    let mut segment_kinds: Vec<SegmentKinds> = connectors::SEGMENTS
        .iter()
        .filter(|(_, tools, _)| tools.contains(&connectors::PLAN))
        .map(|(segment, _, kinds)| SegmentKinds {
            segment: (*segment).to_owned(),
            kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
        })
        .collect();
    segment_kinds.push(SegmentKinds {
        segment: connectors::ADMIN.to_owned(),
        kinds: every_kind.iter().map(|k| (*k).to_owned()).collect(),
    });
    Ok(Json(ConnectorOptions {
        segments,
        segment_kinds,
        ceiling: CeilingView {
            segments: ceiling.segments(),
            writes: ceiling.writes(),
            max_days: ceiling.max_days(),
            kinds: ceiling.kind_verbs(&every_kind),
        },
        projects,
        clients: client_choices(&state.config.connector_clients, &ceiling),
    }))
}

/// The realm's connector clients, each marked when the ceiling refuses it, then every other
/// client a binding policy lists (a CIMD URL such as claude.ai's): offering one grants nothing,
/// create checks each again (T-6309).
fn client_choices(configured: &[String], ceiling: &Ceiling) -> Vec<ClientChoice> {
    let listed = ceiling.listed_clients();
    let extra = listed.iter().filter(|id| !configured.contains(id));
    configured
        .iter()
        .chain(extra)
        .map(|id| ClientChoice {
            id: id.clone(),
            is_boundary: connectors::binds(id),
            refused: (!ceiling.allows_client(id))
                .then(|| format!("{} does not allow the client {id}", ceiling.named())),
        })
        .collect()
}

#[utoipa::path(
    post,
    path = "/api/v1/connectors",
    summary = "Create a Connector",
    description = "Creates a connector of the caller: an MCP server at `/c/{id}` reaching only its scope, with the caller's own rights read again at every call. At most 20 live connectors per person (AG-142, API/14 §2).",
    tag = "connectors",
    params(CreateParams),
    request_body(
        content = NewConnector,
        content_type = "application/json",
        example = json!({ "name": "doprava-in-claude", "segments": ["data-read"],
                          "scope": [{ "project": "doprava", "endpoint": "bike-counters", "access": "read" }],
                          "expiresAt": "2026-11-27T00:00:00Z",
                          "allowedClients": ["https://claude.ai/oauth/mcp-client-metadata"] })
    ),
    responses(
        (status = 201, description = "Created: `active`, or `pending` when it asks above the caller's DelegationPolicy ceiling and that policy queues such requests (AG-150)", body = Connector),
        (status = 200, description = "dryRun=All: valid, nothing kept", body = Object),
        (status = 400, description = "A field out of shape; errors names it", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Above the organization's ceiling, or a project the caller holds no right in; the detail names the field", body = ProblemDetails),
        (status = 409, description = "A live connector of that name, or 20 live ones already", body = ProblemDetails),
        (status = 422, description = "A scope item names an object the project lacks", body = ProblemDetails),
    )
)]
pub async fn create_connector(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<CreateParams>,
    body: Result<Json<NewConnector>, JsonRejection>,
) -> Result<Response, ApiError> {
    let identity = &user.0.identity;
    in_the_portal(&state, identity)?;
    let Json(new) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let ceiling = Ceiling::of(&state.mirror, identity);
    let above = connectors::validate(&new, &ceiling, chrono::Utc::now())?;
    reachable(&state, &user.0, &headers, &new).await?;
    may_be_autonomous(&state, identity, &new)?;
    match params.dry_run.as_deref() {
        None => {}
        Some("All") => {
            return Ok(Json(serde_json::json!({
                "valid": true,
                "pending": !above.is_empty(),
                "aboveCeiling": above,
            }))
            .into_response());
        }
        Some(_) => {
            return Err(ApiError::Invalid {
                detail: "dryRun takes All".into(),
                errors: vec!["dryRun".into()],
            })
        }
    }
    let queued = !above.is_empty();
    let connector = state
        .connectors
        .create(&identity.subject, new, above, &base(&state))
        .await?;
    if queued {
        notify_admins(&state, identity, &connector).await;
    }
    Ok((StatusCode::CREATED, Json(connector)).into_response())
}

#[utoipa::path(
    get,
    path = "/api/v1/connectors/{id}",
    summary = "Get My Connector",
    tag = "connectors",
    params(("id" = String, Path, description = "The connector id")),
    responses(
        (status = 200, description = "The connector", body = Connector),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No connector of the caller's with this id", body = ProblemDetails),
    )
)]
pub async fn get_connector(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Connector>, ApiError> {
    Ok(Json(own(&state, &user.0.identity, &id).await?))
}

#[utoipa::path(
    patch,
    path = "/api/v1/connectors/{id}",
    summary = "Edit My Connector",
    description = "Changes the presets, scope, writes, expiry or allowed clients of the caller's connector that is not revoked; the result is checked as a new one is, against the ceiling and the caller's rights today (API/14 §2, T-5951).",
    tag = "connectors",
    params(("id" = String, Path, description = "The connector id")),
    request_body(
        content = ConnectorPatch,
        content_type = "application/merge-patch+json",
        example = json!({ "scope": [{ "project": "doprava", "access": "read" }], "expiresAt": "2026-11-27T00:00:00Z" })
    ),
    responses(
        (status = 200, description = "The connector as it is now", body = Connector),
        (status = 400, description = "A field out of shape; errors names it", body = ProblemDetails),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Above the ceiling, or a project the caller holds no right in; the detail names the field", body = ProblemDetails),
        (status = 404, description = "No live connector of the caller's with this id", body = ProblemDetails),
        (status = 422, description = "A scope item names an object the caller does not read", body = ProblemDetails),
    )
)]
pub async fn edit_connector(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    body: Result<Json<ConnectorPatch>, JsonRejection>,
) -> Result<Json<Connector>, ApiError> {
    let identity = &user.0.identity;
    in_the_portal(&state, identity)?;
    let current = own(&state, identity, &id).await?;
    let Json(patch) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let new = NewConnector {
        name: current.name,
        segments: patch.segments.unwrap_or(current.segments),
        scope: patch.scope.unwrap_or(current.scope),
        kinds: patch.kinds.or(current.kinds),
        writes: patch.writes.unwrap_or(current.writes),
        autonomous: patch.autonomous.unwrap_or(current.autonomous),
        expires_at: patch.expires_at.unwrap_or(current.expires_at),
        allowed_clients: patch.allowed_clients.unwrap_or(current.allowed_clients),
    };
    // Decided (worker-t5953, owner may override): an edit is judged against today's ceiling like a
    // create; above it under a queueing policy the connector waits again unless its approval
    // already covers every line, because an approval covers what was asked then; options: keep
    // any approval (an edit would widen past it), refuse every edit above the ceiling.
    let above = connectors::validate(
        &new,
        &Ceiling::of(&state.mirror, identity),
        chrono::Utc::now(),
    )?;
    reachable(&state, &user.0, &headers, &new).await?;
    may_be_autonomous(&state, identity, &new)?;
    let approval = current
        .approval
        .filter(|_| !above.is_empty() && above.iter().all(|l| current.above_ceiling.contains(l)));
    if !state
        .connectors
        .update(&id, &identity.subject, new, above, approval)
        .await?
    {
        return Err(not_found());
    }
    let connector = own(&state, identity, &id).await?;
    if connector.state == "pending" {
        notify_admins(&state, identity, &connector).await;
    }
    Ok(Json(connector))
}

#[utoipa::path(
    delete,
    path = "/api/v1/connectors/{id}",
    summary = "Revoke My Connector",
    description = "The connector turns `revoked`; the next call through it is refused (AG-145).",
    tag = "connectors",
    params(("id" = String, Path, description = "The connector id")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Refused by the request guard", body = ProblemDetails),
        (status = 404, description = "No live connector of the caller's with this id", body = ProblemDetails),
    )
)]
pub async fn revoke_connector(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let identity = &user.0.identity;
    in_the_portal(&state, identity)?;
    own(&state, identity, &id).await?;
    state.paired_tabs.unpair(&id, &identity.subject);
    if state.connectors.revoke(&id, &identity.subject).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/connectors/{id}/activity",
    summary = "My Connector's Calls",
    description = "Every call through the connector, newest first, at most 200 of the last 30 days: person, client, tool, target, verdict, duration; never a token or an input value (AG-147).",
    tag = "connectors",
    params(("id" = String, Path, description = "The connector id")),
    responses(
        (status = 200, description = "The audit rows", body = ConnectorCalls),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 404, description = "No connector of the caller's with this id", body = ProblemDetails),
    )
)]
pub async fn connector_activity(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ConnectorCalls>, ApiError> {
    let connector = own(&state, &user.0.identity, &id).await?;
    let items = state.connectors.calls(&connector.id).await?;
    Ok(Json(ConnectorCalls { items }))
}

/// Only an administrator of the organization's ceiling, a person, sees or decides requests above
/// it: whoever may approve a `DelegationPolicy` change (AG-150, AG-151).
fn administrator(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    in_the_portal(state, identity)?;
    let org = crate::permissions::for_request(state, identity, ORG_NAMESPACE);
    if !org.administers_organization()
        && !org.may("DelegationPolicy", jc_core::kinds::Verb::Approve)
    {
        return Err(ApiError::Denied(
            "only an administrator of the organization, or a person who may approve a DelegationPolicy change, decides connector requests above the ceiling (AG-150)".into(),
        ));
    }
    Ok(())
}

/// The approver holds every right the connector asks for, project by project (PF-51,
/// `within_own_rights`): `read` on each kind its segments reach, `propose` where an item writes,
/// and `delete` where an item writes a kind the connector may delete (JC2-17). Each kind is
/// judged in its home namespace for the item's project (`resource::home`), so an organization
/// kind needs the organization's grant, never the project's.
fn within_own_rights(
    state: &AppState,
    identity: &Identity,
    connector: &Connector,
) -> Result<(), ApiError> {
    use jc_core::kinds::{ConnectorVerb, Verb};
    let (_, kinds) = connector.reach();
    for (i, item) in connector.scope.iter().enumerate() {
        for kind in &kinds {
            let namespace = jc_core::registry::by_kind(kind).map_or_else(
                || item.project.clone(),
                |info| crate::resource::home(info, &item.project),
            );
            let rights = crate::permissions::for_request(state, identity, &namespace);
            let mut verbs = vec![Verb::Read];
            if item.access == connectors::ItemAccess::Write {
                verbs.push(Verb::Propose);
                if connector.may_verb(kind, "", ConnectorVerb::Delete).is_ok() {
                    verbs.push(Verb::Delete);
                }
            }
            if let Some(verb) = verbs.into_iter().find(|v| !rights.may(kind, *v)) {
                return Err(ApiError::Denied(format!(
                    "scope[{i}]: the connector asks {verb:?} on {kind} in {namespace} (project {}), which you do not hold; nobody approves above their own rights (PF-51)",
                    item.project
                )));
            }
        }
    }
    Ok(())
}

/// Only an administrator of the organization reads or revokes every person's connectors.
fn org_administrator(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    // Every person's grants are read, and one revoked, by a person in a Portal session; an
    // administrator's agent or a service account bound to the role never does it (AG-133).
    if identity.client.is_some() {
        return Err(ApiError::Denied(
            "every person's connectors are listed and revoked only by an administrator in the \
             Portal, never with a service account's or an agent's token (AG-133)"
                .into(),
        ));
    }
    if crate::permissions::for_request(state, identity, ORG_NAMESPACE).administers_organization() {
        return Ok(());
    }
    Err(ApiError::Denied(
        "only an administrator of the organization reads or revokes every person's connectors: \
         it needs approve and delete on RoleBinding at organization scope (PF-03)"
            .into(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/connectors/pending",
    summary = "Connector Requests Above the Ceiling",
    description = "Every live connector waiting for an administrator, oldest first, with what it asks above its owner's DelegationPolicy ceiling (AG-150). Only an administrator of the organization, or a person who may approve a DelegationPolicy change, reads it.",
    tag = "connectors",
    responses(
        (status = 200, description = "The pending connectors", body = ConnectorList),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an administrator of the ceiling, or an AI client's token", body = ProblemDetails),
    )
)]
pub async fn pending_connectors(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ConnectorList>, ApiError> {
    administrator(&state, &user.0.identity)?;
    let items = state.connectors.pending(&base(&state)).await?;
    Ok(Json(ConnectorList { items }))
}

#[utoipa::path(
    post,
    path = "/api/v1/connectors/{id}/approve",
    summary = "Approve a Connector Request",
    description = "Lets a `pending` connector through: it turns `active` and calls run with what it asked, while a DelegationPolicy binding its owner still queues requests. The approver is an administrator of the organization or a person who may approve a DelegationPolicy change, not the owner, and holds every right the connector asks for (AG-150, PF-51); an AI client's token is refused (AG-151).",
    tag = "connectors",
    params(("id" = String, Path, description = "The connector id")),
    responses(
        (status = 200, description = "Approved", body = Connector),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an administrator, the owner, a right the approver lacks, or an AI client's token; the detail says which", body = ProblemDetails),
        (status = 404, description = "No pending connector with this id", body = ProblemDetails),
    )
)]
pub async fn approve_connector(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Connector>, ApiError> {
    let identity = &user.0.identity;
    administrator(&state, identity)?;
    // An approval widens a delegation past the ceiling: a decision door like a Change approval,
    // so it wants the second factor too (JC2-17, T-5567).
    crate::auth::session::require_second_factor(identity, &state.config)?;
    if !connectors::is_id(&id) {
        return Err(not_found());
    }
    let connector = match state.connectors.get(&id, &base(&state)).await? {
        Some(c) if c.state == "pending" => c,
        _ => return Err(not_found()),
    };
    if connector.owner == identity.subject {
        return Err(ApiError::Denied(
            "you asked for this connector; another administrator approves it (AG-150)".into(),
        ));
    }
    within_own_rights(&state, identity, &connector)?;
    if !state.connectors.approve(&id, &identity.subject).await? {
        return Err(not_found());
    }
    state
        .connectors
        .get(&id, &base(&state))
        .await?
        .map(Json)
        .ok_or_else(not_found)
}

/// How far ahead the owner learns a connector ends (T-5979).
const EXPIRY_NOTICE_DAYS: i64 = 7;

/// The audiences whose notices `identity` may act on today (T-5979): the approvers of each
/// project where they hold `approve`, and the deciders of connector requests. Judged at read, so
/// a person who lost the right gets no new notice of it.
pub(crate) fn audiences(state: &AppState, identity: &Identity) -> Vec<String> {
    let mut audiences: Vec<String> = state
        .mirror
        .namespaces()
        .into_iter()
        .filter(|project| {
            crate::api::changes::may_approve_anything(state, identity, project).is_ok()
        })
        .map(|project| crate::entity_comments::approvers_of(&project))
        .collect();
    if administrator(state, identity).is_ok() {
        audiences.push(crate::entity_comments::CONNECTOR_ADMINS.to_owned());
    }
    audiences
}

/// Leaves `me` one notice per live connector of theirs that ends within
/// [`EXPIRY_NOTICE_DAYS`] days; a new expiry is a new notice.
pub(crate) async fn notify_expiring(
    state: &AppState,
    identity: &Identity,
    me: &str,
) -> Result<(), ApiError> {
    let now = chrono::Utc::now();
    let soon = now + chrono::Duration::days(EXPIRY_NOTICE_DAYS);
    for connector in state
        .connectors
        .of_owner(&identity.subject, &base(state))
        .await?
    {
        if !connector.is_active() || connector.expires_at <= now || connector.expires_at > soon {
            continue;
        }
        let key = format!(
            "connector.expiring:{}:{}",
            connector.id,
            connector.expires_at.timestamp()
        );
        state
            .comments
            .notify(
                me,
                crate::entity_comments::Notice {
                    kind: "connector.expiring",
                    project: "",
                    about: &connector.id,
                    author: "",
                    author_name: "",
                    connector: &connector.name,
                    link: CONNECTORS_PAGE,
                    key: &key,
                },
            )
            .await
            .map_err(|err| {
                tracing::warn!(error = %err, "notifications not reachable");
                ApiError::Unavailable("the notifications could not be written".into())
            })?;
    }
    Ok(())
}

/// Tells the deciders of connector requests that `connector` waits above the ceiling (T-5979):
/// the owner's name and the connector's, never its scope. A notice that cannot be written is
/// logged and the request stays queued.
async fn notify_admins(state: &AppState, identity: &Identity, connector: &Connector) {
    let author = identity.username.to_ascii_lowercase();
    // One notice per request: the same lines asked again are the same event (T-5979).
    use sha2::{Digest, Sha256};
    let asked = Sha256::digest(connector.above_ceiling.join("\n").as_bytes());
    let key = format!("connector.pending:{}:{:x}", connector.id, asked);
    let notice = crate::entity_comments::Notice {
        kind: "connector.pending",
        project: "",
        about: &connector.id,
        author: &author,
        author_name: identity.name.as_deref().unwrap_or(&identity.username),
        connector: &connector.name,
        link: CONNECTORS_PAGE,
        key: &key,
    };
    let audience = crate::entity_comments::CONNECTOR_ADMINS;
    if let Err(err) = state.comments.notify(audience, notice).await {
        tracing::warn!(error = %err, connector = %connector.id, "connector admins not notified");
    }
}

/// The page a person manages their connectors on.
pub(crate) const CONNECTORS_PAGE: &str = "/ai-connectors";

#[utoipa::path(
    get,
    path = "/api/v1/organization/connectors",
    summary = "List the Organization's Connectors",
    description = "Every person's connectors, live ones first, at most 500: owner, segments, scope, clients, state, last call, calls and denials of the last 24 hours, and `flagged` at `denialsFlag` denials. Never a call's input or a token (T-5975, API/14 §2).",
    tag = "connectors",
    responses(
        (status = 200, description = "The organization's connectors", body = ConnectorOverview),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an administrator of the organization", body = ProblemDetails),
    )
)]
pub async fn organization_connectors(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<ConnectorOverview>, ApiError> {
    org_administrator(&state, &user.0.identity)?;
    let items = state.connectors.overview(&base(&state)).await?;
    Ok(Json(ConnectorOverview {
        items,
        denials_flag: connectors::DENIALS_FLAG,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/v1/organization/connectors/{id}",
    summary = "Revoke a Person's Connector",
    description = "An administrator revokes any person's connector; the next call through it is refused, and the activity feed names who revoked it (T-5975).",
    tag = "connectors",
    params(("id" = String, Path, description = "The connector id")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "Not an administrator of the organization", body = ProblemDetails),
        (status = 404, description = "No live connector with this id", body = ProblemDetails),
    )
)]
pub async fn revoke_organization_connector(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let identity = &user.0.identity;
    org_administrator(&state, identity)?;
    if !connectors::is_id(&id) {
        return Err(not_found());
    }
    let Some(found) = state.connectors.get(&id, &base(&state)).await? else {
        return Err(not_found());
    };
    state.paired_tabs.unpair(&id, &found.owner);
    if !state.connectors.revoke(&id, &found.owner).await? {
        return Err(not_found());
    }
    let action = format!("revoked connector {} of", found.name);
    crate::api::people::record(&state, identity, &action, &found.owner).await;
    Ok(StatusCode::NO_CONTENT)
}

/// A Portal tab paired with a connector (T-5967).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PairedTab {
    /// The tab's id: the address of its event stream, valid only for this person's session.
    pub tab: String,
    pub connector: String,
    pub name: String,
    /// When the pairing ends with the session, unless the person disconnects before.
    #[schema(value_type = String, format = DateTime)]
    pub until: chrono::DateTime<chrono::Utc>,
}

/// The person signed in in a browser: a tab is paired by its person, never by a workload or by a
/// token a connector client holds (T-5967).
fn in_a_browser(state: &AppState, identity: &Identity) -> Result<(), ApiError> {
    person(state, identity)?;
    match identity.client.as_deref() {
        Some(client) if state.config.connector_clients.iter().any(|c| c == client) => {
            Err(ApiError::Denied(
                "a tab is connected by the person signed in in it, not by an MCP client".into(),
            ))
        }
        _ => Ok(()),
    }
}

/// One row of the connector's audit for its tab: who paired or let it go.
async fn audit_tab(state: &AppState, identity: &Identity, id: &str, tool: &str) {
    let call = Call {
        at: chrono::Utc::now(),
        subject: identity.subject.clone(),
        client: identity.client.clone(),
        tool: tool.to_owned(),
        target: None,
        verdict: "allowed".to_owned(),
        duration_ms: 0,
    };
    if let Err(err) = state.connectors.record(id, call).await {
        tracing::warn!(connector = %id, error = %err, "a tab pairing is not in its connector's audit");
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/connectors/{id}/tab",
    summary = "Connect This Tab",
    description = "Pairs the caller's open Portal tab with one of their live connectors: until the session ends or the person disconnects, the connector's MCP client may send this tab UI actions (navigate, open_form, highlight, show_diff, filter_table), which never save. A newer tab replaces the one paired before. The answer's `tab` addresses the tab's event stream for this session only (T-5967).",
    tag = "connectors",
    params(("id" = String, Path, description = "The connector id")),
    responses(
        (status = 201, description = "Paired", body = PairedTab),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A workload, an MCP client's token, or a connector that is not active", body = ProblemDetails),
        (status = 404, description = "No connector of the caller's with this id", body = ProblemDetails),
    )
)]
pub async fn pair_tab(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<PairedTab>), ApiError> {
    let session = &user.0;
    in_a_browser(&state, &session.identity)?;
    let connector = own(&state, &session.identity, &id).await?;
    if !connector.is_active() {
        return Err(ApiError::Denied(format!(
            "connector {} is {}; create a new one to connect this tab",
            connector.name, connector.state
        )));
    }
    let tab = state.paired_tabs.pair(
        &connector.id,
        &session.identity.subject,
        session.started_at(),
        session.expires_at,
    );
    audit_tab(&state, &session.identity, &connector.id, "tab/connect").await;
    let until = chrono::DateTime::from_timestamp(session.expires_at, 0).unwrap_or_default();
    Ok((
        StatusCode::CREATED,
        Json(PairedTab {
            tab,
            connector: connector.id,
            name: connector.name,
            until,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/v1/connectors/{id}/tab",
    summary = "Disconnect The Tab",
    description = "Ends the pairing of the caller's tab with the connector: its event stream closes and the connector's UI tools refuse until a tab is connected again (T-5967).",
    tag = "connectors",
    params(("id" = String, Path, description = "The connector id")),
    responses(
        (status = 204, description = "No tab is paired any more"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A workload or an MCP client's token", body = ProblemDetails),
        (status = 404, description = "No connector of the caller's with this id", body = ProblemDetails),
    )
)]
pub async fn unpair_tab(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let identity = &user.0.identity;
    in_a_browser(&state, identity)?;
    let connector = own(&state, identity, &id).await?;
    if state.paired_tabs.unpair(&connector.id, &identity.subject) {
        audit_tab(&state, identity, &connector.id, "tab/disconnect").await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/connectors/{id}/tab/{tab}/events",
    summary = "The Paired Tab's Actions",
    description = "Server-sent `action` events, one per UI action the connector sent: `frame` (`navigate` or `ui`), its checked `payload`, the `tool`, and `from` (connector name and MCP client). Only the paired tab of the session that paired it reads them; the stream ends when the pairing does (T-5967).",
    tag = "connectors",
    params(
        ("id" = String, Path, description = "The connector id"),
        ("tab" = String, Path, description = "The tab id `Connect This Tab` answered"),
    ),
    responses(
        (status = 200, description = "The event stream", content_type = "text/event-stream"),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "A workload or an MCP client's token", body = ProblemDetails),
        (status = 404, description = "No tab of this session is paired with the caller's connector", body = ProblemDetails),
    )
)]
pub async fn tab_events(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((id, tab)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use tokio_stream::StreamExt;

    let session = &user.0;
    in_a_browser(&state, &session.identity)?;
    let connector = own(&state, &session.identity, &id).await?;
    let now = crate::auth::session::now_unix();
    let (receiver, until) = state
        .paired_tabs
        .listen(
            &connector.id,
            &tab,
            &session.identity.subject,
            session.started_at(),
            session.expires_at,
            now,
        )
        .ok_or_else(|| {
            ApiError::NotFound(
                "this tab is not connected to the connector; connect it again".into(),
            )
        })?;
    let ends = tokio::time::Instant::now()
        + std::time::Duration::from_secs(u64::try_from(until.saturating_sub(now)).unwrap_or(0));
    // A tab that fell behind misses actions rather than the stream.
    let actions = tokio_stream::wrappers::BroadcastStream::new(receiver).filter_map(|item| {
        let action = item.ok()?;
        Some(Ok::<_, std::convert::Infallible>(
            Event::default()
                .event("action")
                .json_data(action.as_ref())
                .unwrap_or_else(|_| Event::default().comment("unserializable")),
        ))
    });
    // The stream ends with the session that paired the tab.
    let stream = futures_util::StreamExt::take_until(actions, tokio::time::sleep_until(ends));
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(20))
                .text("keep-alive"),
        )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::HeaderName::from_static("x-accel-buffering"),
        axum::http::HeaderValue::from_static("no"),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::permissions::ORG_NAMESPACE;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use crate::store::Mirror;
    use axum::http::HeaderMap;
    use serde_json::{json, Value};
    use std::sync::Arc;

    fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, namespace),
            spec,
            status: None,
        }
    }

    /// T-6413: an organization-data item names one Endpoint for reading under `data-read`
    /// alone; each other shape is the 403 naming its field.
    #[test]
    fn an_organization_data_item_reads_one_endpoint_and_nothing_else() {
        use jc_core::kinds::ConnectorVerb;
        let item = |endpoint: Option<&str>, access| connectors::ScopeItem {
            project: "helsinki".into(),
            space: None,
            endpoint: endpoint.map(str::to_owned),
            pipeline: None,
            access,
            personal_data: false,
        };
        let read = connectors::ItemAccess::Read;
        let ask = |segments: &[&str], writes: bool, verbs: Vec<ConnectorVerb>| NewConnector {
            name: "shared".into(),
            segments: segments.iter().map(|s| (*s).to_owned()).collect(),
            scope: vec![],
            kinds: Some(vec![connectors::KindGrant {
                kind: "Endpoint".into(),
                verbs,
            }]),
            writes,
            autonomous: false,
            expires_at: chrono::Utc::now(),
            allowed_clients: vec![],
        };
        let fine = ask(&["data-read"], false, vec![ConnectorVerb::Read]);
        assert!(organization_item(0, &item(Some("air"), read), &fine).is_ok());
        let refused = |item: connectors::ScopeItem, new: &NewConnector| match organization_item(
            0, &item, new,
        ) {
            Err(ApiError::Denied(why)) => why,
            other => panic!("{other:?}"),
        };
        assert!(refused(item(None, read), &fine).starts_with("scope[0].endpoint:"));
        assert!(refused(item(Some("*"), read), &fine).starts_with("scope[0].endpoint:"));
        let write = item(Some("air"), connectors::ItemAccess::Write);
        assert!(refused(write, &fine).starts_with("scope[0].access:"));
        let context = ask(&["data-read", "context"], false, vec![]);
        assert!(refused(item(Some("air"), read), &context).starts_with("segments:"));
        let writes = ask(&["data-read"], true, vec![]);
        assert!(refused(item(Some("air"), read), &writes).starts_with("writes:"));
        let create = ask(&["data-read"], false, vec![ConnectorVerb::Create]);
        assert!(refused(item(Some("air"), read), &create).starts_with("kinds:"));
        let mut spaced = item(Some("air"), read);
        spaced.space = Some("air".into());
        assert!(refused(spaced, &fine).starts_with("scope[0]:"));
    }

    /// A reader of project `helsinki` alone; `espoo` exists beside it.
    fn reader_world() -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        let org = ORG_NAMESPACE;
        state.mirror.upsert(envelope(
            "Role",
            "reader",
            org,
            json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace", "Pipeline"], "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(envelope(
            "RoleBinding",
            "reader",
            org,
            json!({ "subjects": [{ "user": "reader@hel.fi" }], "role": "reader", "scope": { "project": "helsinki" } }),
        ));
        for project in ["helsinki", "espoo"] {
            state
                .mirror
                .upsert(envelope("ContextSpace", "air", project, json!({})));
            state.mirror.upsert(envelope(
                "Endpoint",
                &format!("{project}-air"),
                project,
                json!({ "contextSpaceRef": "air" }),
            ));
        }
        state
    }

    fn manifest(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        envelope(kind, name, ORG_NAMESPACE, spec)
    }

    fn person(email: &str) -> Identity {
        Identity {
            client: None,
            subject: format!("sub-{email}"),
            username: email.to_owned(),
            email: Some(email.to_owned()),
            name: None,
            roles: vec![],
            groups: vec![],
            second_factor: false,
        }
    }

    /// Petra reads the data-read kinds in project doprava and proposes nothing there.
    fn world() -> AppState {
        let state = AppState::new(Config::for_tests(), None);
        state.mirror.upsert(manifest(
            "Role",
            "reader",
            json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace", "DataModel"], "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(manifest(
            "RoleBinding",
            "petra-reads-doprava",
            json!({ "subjects": [{ "user": "petra@hel.fi" }], "role": "reader",
                    "scope": { "project": "doprava" } }),
        ));
        state
    }

    async fn asked(state: &AppState, access: &str, writes: bool) -> Connector {
        let new: NewConnector = serde_json::from_value(json!({
            "name": format!("asks-{access}"), "segments": ["data-read"],
            "scope": [{ "project": "doprava", "access": access }], "writes": writes,
            "expiresAt": (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
        }))
        .expect("request");
        state
            .connectors
            .create(
                "sub-jana@hel.fi",
                new,
                vec!["writes: above".into()],
                "https://portal.test",
            )
            .await
            .expect("kept")
    }

    #[tokio::test]
    async fn an_approver_holds_every_right_the_connector_asks_for() {
        let state = world();
        let petra = person("petra@hel.fi");
        let reads = asked(&state, "read", false).await;
        let ok = within_own_rights(&state, &petra, &reads);
        assert!(ok.is_ok(), "{ok:?}");
        // A write item asks `propose`, which Petra does not hold in doprava.
        let writes = asked(&state, "write", true).await;
        match within_own_rights(&state, &petra, &writes) {
            Err(ApiError::Denied(why)) => {
                assert!(
                    why.starts_with("scope[0]") && why.contains("doprava"),
                    "{why}"
                )
            }
            other => panic!("approved above her rights: {other:?}"),
        }
        // Nobody holds anything in a project no binding names.
        let nobody = person("eva@hel.fi");
        assert!(within_own_rights(&state, &nobody, &reads).is_err());
    }

    /// JC2-17: a write item whose connector may delete a kind needs `delete` from the approver;
    /// `propose` covers create and edit only.
    #[tokio::test]
    async fn an_approver_without_delete_does_not_approve_a_deleting_connector() {
        let state = world();
        state.mirror.upsert(manifest(
            "Role",
            "editor",
            json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace", "DataModel"], "verbs": ["read", "propose"] }] }),
        ));
        state.mirror.upsert(manifest(
            "RoleBinding",
            "petra-edits-doprava",
            json!({ "subjects": [{ "user": "petra@hel.fi" }], "role": "editor",
                    "scope": { "project": "doprava" } }),
        ));
        let petra = person("petra@hel.fi");
        let with = |name: &str, kinds: Value| {
            let state = state.clone();
            let name = name.to_owned();
            async move {
                let new: NewConnector = serde_json::from_value(json!({
                    "name": name, "segments": ["data-read"], "kinds": kinds,
                    "scope": [{ "project": "doprava", "access": "write" }], "writes": true,
                    "expiresAt": (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339(),
                }))
                .expect("request");
                state
                    .connectors
                    .create(
                        "sub-jana@hel.fi",
                        new,
                        vec!["writes: above".into()],
                        "https://portal.test",
                    )
                    .await
                    .expect("kept")
            }
        };
        // Create and edit only: `propose` covers it.
        let edits = with(
            "edits",
            json!([{ "kind": "Endpoint", "verbs": ["read", "create", "edit"] }]),
        )
        .await;
        let ok = within_own_rights(&state, &petra, &edits);
        assert!(ok.is_ok(), "{ok:?}");
        // Delete on Endpoint, ticked or carried by `propose`: Petra holds no delete.
        for (name, verbs) in [
            ("ticks-delete", json!(["read", "delete"])),
            ("ticks-propose", json!(["propose"])),
        ] {
            let deletes = with(name, json!([{ "kind": "Endpoint", "verbs": verbs }])).await;
            match within_own_rights(&state, &petra, &deletes) {
                Err(ApiError::Denied(why)) => {
                    assert!(why.contains("Delete on Endpoint"), "{why}")
                }
                other => panic!("approved a delete she does not hold: {other:?}"),
            }
        }
        // Without a kinds list the connector changes everything its segments reach, delete too.
        let every = with("every", Value::Null).await;
        assert!(matches!(
            within_own_rights(&state, &petra, &every),
            Err(ApiError::Denied(_))
        ));
        // Granted delete, Petra approves it.
        state.mirror.upsert(manifest(
            "Role",
            "editor",
            json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace", "DataModel"], "verbs": ["read", "propose", "delete"] }] }),
        ));
        let ok = within_own_rights(&state, &petra, &every);
        assert!(ok.is_ok(), "{ok:?}");
    }

    #[test]
    fn only_a_person_who_approves_delegation_policies_decides_requests() {
        let state = world();
        // Petra reads but approves no DelegationPolicy change.
        assert!(matches!(
            administrator(&state, &person("petra@hel.fi")),
            Err(ApiError::Denied(_))
        ));
        let mut admin = person("admin@hel.fi");
        admin.groups = vec![state.config.bootstrap_admins.clone()];
        assert!(administrator(&state, &admin).is_ok());
        // The same administrator through an AI client's token decides nothing (AG-151).
        admin.client = Some("mcp-local".into());
        match administrator(&state, &admin) {
            Err(ApiError::Denied(why)) => assert!(why.contains("AG-151"), "{why}"),
            other => panic!("{other:?}"),
        }
        // Nor through an agent's token exchanged for the administrator (no connector client).
        admin.client = Some("agent-runner".into());
        match administrator(&state, &admin) {
            Err(ApiError::Denied(why)) => assert!(why.contains("AG-151"), "{why}"),
            other => panic!("an agent token decided a request: {other:?}"),
        }
        // A workload's account is no person.
        let mut workload = person("service-account-pipeline");
        workload.groups = vec![state.config.bootstrap_admins.clone()];
        assert!(administrator(&state, &workload).is_err());
    }

    fn user(username: &str) -> CurrentUser {
        CurrentUser(crate::auth::session::Session {
            identity: Identity {
                client: None,
                subject: format!("f:1:{username}"),
                username: username.to_owned(),
                email: None,
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

    fn body(name: &str) -> Result<Json<NewConnector>, JsonRejection> {
        let expires = chrono::Utc::now() + chrono::Duration::days(7);
        Ok(Json(
            serde_json::from_value(json!({
                "name": name, "segments": ["data-read"],
                "scope": [{ "project": "doprava", "access": "read" }],
                "expiresAt": expires.to_rfc3339(),
            }))
            .expect("a valid request"),
        ))
    }

    fn dry(run: Option<&str>) -> Query<CreateParams> {
        Query(CreateParams {
            dry_run: run.map(str::to_owned),
        })
    }

    /// `who`'s browser session issued at `issued_at`, ending in an hour; `client` as a bearer
    /// token's `azp`.
    fn signed_in(who: &str, issued_at: i64, client: Option<&str>) -> CurrentUser {
        let mut user = user(who);
        user.0.issued_at = issued_at;
        user.0.expires_at = crate::auth::session::now_unix() + 3600;
        user.0.identity.client = client.map(str::to_owned);
        user
    }

    #[tokio::test]
    async fn only_the_owner_signed_in_in_the_tab_pairs_it_and_only_that_session_listens() {
        let mirror = Arc::new(Mirror::new());
        mirror.upsert(envelope(
            "Role",
            "reader",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["*"], "verbs": ["read"] }] }),
        ));
        mirror.upsert(envelope(
            "RoleBinding",
            "reader",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "ana" }, { "user": "bob" }], "role": "reader", "scope": { "project": "doprava" } }),
        ));
        let state = AppState::new(Config::for_tests(), None).with_mirror(mirror);
        create_connector(
            user("ana"),
            State(state.clone()),
            HeaderMap::new(),
            dry(None),
            body("tab"),
        )
        .await
        .expect("created");
        let id = state
            .connectors
            .of_owner("f:1:ana", "")
            .await
            .expect("list")[0]
            .id
            .clone();
        let pair = |who: CurrentUser| pair_tab(who, State(state.clone()), Path(id.clone()));

        // Another person's connector answers as one that does not exist.
        assert!(matches!(
            pair(signed_in("bob", 5, None)).await,
            Err(ApiError::NotFound(_))
        ));
        // The connector's own client (Claude Code's token) cannot pair a tab for itself.
        assert!(matches!(
            pair(signed_in("ana", 5, Some("mcp-local"))).await,
            Err(ApiError::Denied(why)) if why.contains("signed in")
        ));
        let (status, Json(paired)) = pair(signed_in("ana", 5, None)).await.expect("paired");
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(
            (paired.connector.as_str(), paired.name.as_str()),
            (id.as_str(), "tab")
        );

        let listen = |who: CurrentUser, tab: &str| {
            tab_events(
                who,
                State(state.clone()),
                Path((id.clone(), tab.to_owned())),
            )
        };
        // A later sign-in of the same person, another person, a guessed tab: no stream.
        assert!(matches!(
            listen(signed_in("ana", 6, None), &paired.tab).await,
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            listen(signed_in("bob", 5, None), &paired.tab).await,
            Err(ApiError::NotFound(_))
        ));
        assert!(matches!(
            listen(signed_in("ana", 5, None), &"0".repeat(32)).await,
            Err(ApiError::NotFound(_))
        ));
        let stream = listen(signed_in("ana", 5, None), &paired.tab)
            .await
            .expect("the paired tab listens");
        assert_eq!(stream.status(), StatusCode::OK);

        // Disconnect ends it; a revoke ends a pairing made after.
        let unpaired = unpair_tab(
            signed_in("ana", 5, None),
            State(state.clone()),
            Path(id.clone()),
        )
        .await;
        assert_eq!(unpaired.expect("disconnected"), StatusCode::NO_CONTENT);
        assert!(matches!(
            listen(signed_in("ana", 5, None), &paired.tab).await,
            Err(ApiError::NotFound(_))
        ));
        let _again = pair(signed_in("ana", 5, None)).await.expect("paired again");
        assert!(state
            .paired_tabs
            .is_paired(&id, "f:1:ana", crate::auth::session::now_unix()));
        revoke_connector(user("ana"), State(state.clone()), Path(id.clone()))
            .await
            .expect("revoked");
        assert!(!state
            .paired_tabs
            .is_paired(&id, "f:1:ana", crate::auth::session::now_unix()));
        assert!(matches!(
            pair(signed_in("ana", 5, None)).await,
            Err(ApiError::Denied(why)) if why.contains("revoked")
        ));
        let audit: Vec<String> = state
            .connectors
            .calls(&id)
            .await
            .expect("audit")
            .into_iter()
            .map(|c| c.tool)
            .collect();
        assert!(
            audit.contains(&"tab/connect".to_owned())
                && audit.contains(&"tab/disconnect".to_owned()),
            "{audit:?}"
        );
    }

    #[tokio::test]
    async fn create_refuses_workloads_strangers_and_bad_dry_runs_then_keeps_the_owners() {
        let mirror = Arc::new(Mirror::new());
        mirror.upsert(envelope(
            "Role",
            "reader",
            ORG_NAMESPACE,
            json!({ "rules": [{ "kinds": ["*"], "verbs": ["read"] }] }),
        ));
        mirror.upsert(envelope(
            "RoleBinding",
            "reader",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "ana" }], "role": "reader", "scope": { "project": "doprava" } }),
        ));
        let state = AppState::new(Config::for_tests(), None).with_mirror(mirror);
        let create = |who: &str, run: Option<&str>, name: &str| {
            create_connector(
                user(who),
                State(state.clone()),
                HeaderMap::new(),
                dry(run),
                body(name),
            )
        };

        let workload = create("service-account-etl", None, "a").await;
        assert!(matches!(workload, Err(ApiError::Denied(why)) if why.contains("AG-142")));
        let stranger = create("bob", None, "a").await;
        assert!(
            matches!(stranger, Err(ApiError::Denied(why)) if why.starts_with("scope[0].project"))
        );
        let bad_name = create("ana", None, "Not A Label").await;
        assert!(matches!(bad_name, Err(ApiError::Invalid { errors, .. }) if errors == ["name"]));
        let bad_run = create("ana", Some("Some"), "a").await;
        assert!(matches!(bad_run, Err(ApiError::Invalid { errors, .. }) if errors == ["dryRun"]));

        let checked = create("ana", Some("All"), "a").await.expect("dry run");
        assert_eq!(checked.status(), StatusCode::OK);
        assert!(state
            .connectors
            .of_owner("f:1:ana", "")
            .await
            .expect("list")
            .is_empty());
        let created = create("ana", None, "a").await.expect("created");
        assert_eq!(created.status(), StatusCode::CREATED);
        let mine = state
            .connectors
            .of_owner("f:1:ana", "")
            .await
            .expect("list");
        assert_eq!(mine.len(), 1);

        // AG-151 (T-6226, F-R2POR2-004): a token-exchanged agent's token, or any client's that
        // is no configured connector client, manages no connector of the person it acts for.
        let mut agent = user("ana");
        agent.0.identity.client = Some("helsinki-jc-agent-proxy".into());
        let minted = create_connector(
            agent.clone(),
            State(state.clone()),
            HeaderMap::new(),
            dry(None),
            body("b"),
        )
        .await;
        assert!(matches!(minted, Err(ApiError::Denied(why)) if why.contains("AG-151")));
        let widened = edit_connector(
            agent.clone(),
            State(state.clone()),
            Path(mine[0].id.clone()),
            HeaderMap::new(),
            patch(json!({ "writes": true })),
        )
        .await;
        assert!(matches!(widened, Err(ApiError::Denied(why)) if why.contains("AG-151")));
        let revoked = revoke_connector(agent, State(state.clone()), Path(mine[0].id.clone())).await;
        assert!(matches!(revoked, Err(ApiError::Denied(why)) if why.contains("AG-151")));
        assert_eq!(
            state
                .connectors
                .of_owner("f:1:ana", "")
                .await
                .expect("list")
                .len(),
            1,
            "the agent minted or revoked a connector"
        );
    }

    fn patch(body: Value) -> Result<Json<ConnectorPatch>, JsonRejection> {
        Ok(Json(serde_json::from_value(body).expect("patch")))
    }

    #[tokio::test]
    async fn the_options_offer_the_clients_a_policy_lists_and_mark_the_ones_it_refuses() {
        const CLAUDE_AI: &str = "https://claude.ai/oauth/mcp-oauth-client-metadata";
        const CLAUDE_CODE: &str = "https://claude.ai/oauth/claude-code-client-metadata";
        let state = reader_world();
        let options = |state: AppState| async move {
            let Json(options) =
                connector_options(user("reader@hel.fi"), State(state), HeaderMap::new())
                    .await
                    .expect("options");
            options.clients
        };
        // Under the default every realm client may be named and nothing else is offered.
        let open = options(state.clone()).await;
        assert_eq!(
            open.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            state.config.connector_clients
        );
        assert!(open.iter().all(|c| c.refused.is_none()), "{open:?}");
        state.mirror.upsert(envelope(
            "DelegationPolicy",
            "claude-only",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "kind": "Group", "name": "readers" }], "projects": ["helsinki"],
                    "allowedClients": [CLAUDE_CODE, CLAUDE_AI] }),
        ));
        state.mirror.upsert(envelope(
            "Group",
            "readers",
            ORG_NAMESPACE,
            json!({ "members": [{ "user": "reader@hel.fi" }] }),
        ));
        let only = options(state.clone()).await;
        for id in &state.config.connector_clients {
            let choice = only
                .iter()
                .find(|c| &c.id == id)
                .expect("realm client kept");
            assert_eq!(
                choice.refused.as_deref(),
                Some(
                    format!("DelegationPolicy claude-only does not allow the client {id}").as_str()
                )
            );
        }
        let offered: Vec<(&str, bool)> = only
            .iter()
            .filter(|c| c.refused.is_none())
            .map(|c| (c.id.as_str(), c.is_boundary))
            .collect();
        assert_eq!(offered, vec![(CLAUDE_CODE, false), (CLAUDE_AI, true)]);
        // Offering a client grants nothing: create still refuses a realm client the policy omits.
        let days = (chrono::Utc::now() + chrono::Duration::days(5)).to_rfc3339();
        let ask = |client: &str| {
            let new: NewConnector = serde_json::from_value(json!({
                "name": "air", "segments": ["data-read"], "expiresAt": days,
                "scope": [{ "project": "helsinki", "access": "read" }], "allowedClients": [client]
            }))
            .expect("new");
            let ceiling = Ceiling::of(&state.mirror, &user("reader@hel.fi").0.identity);
            connectors::validate(&new, &ceiling, chrono::Utc::now()).map(|above| above.len())
        };
        assert_eq!(ask(CLAUDE_AI).ok(), Some(0));
        assert!(
            !matches!(ask("mcp-hub"), Ok(0)),
            "mcp-hub passed the ceiling"
        );
    }

    #[tokio::test]
    async fn the_options_list_only_what_the_reader_reads_and_an_edit_asks_again() {
        let state = reader_world();
        let Json(options) = connector_options(
            user("reader@hel.fi"),
            State(state.clone()),
            HeaderMap::new(),
        )
        .await
        .expect("options");
        assert_eq!(
            serde_json::to_value(&options.projects).expect("json"),
            json!([{ "project": "helsinki", "spaces": ["air"], "endpoints": ["helsinki-air"], "pipelines": [], "allowed": true }])
        );
        assert_eq!(options.ceiling.segments, vec!["data-read"]);

        let expires = (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339();
        let new: NewConnector = serde_json::from_value(json!({
            "name": "air", "segments": ["data-read"], "expiresAt": expires,
            "scope": [{ "project": "helsinki", "access": "read" }]
        }))
        .expect("new");
        let created = create_connector(
            user("reader@hel.fi"),
            State(state.clone()),
            HeaderMap::new(),
            Query(CreateParams { dry_run: None }),
            Ok(Json(new)),
        )
        .await
        .expect("created");
        assert_eq!(created.status(), StatusCode::CREATED);
        let id = state
            .connectors
            .of_owner("f:1:reader@hel.fi", "https://portal.test")
            .await
            .expect("list")[0]
            .id
            .clone();

        let edit = |body| {
            edit_connector(
                user("reader@hel.fi"),
                State(state.clone()),
                Path(id.clone()),
                HeaderMap::new(),
                patch(body),
            )
        };
        let Err(ApiError::Denied(detail)) =
            edit(json!({ "scope": [{ "project": "espoo", "access": "read" }] })).await
        else {
            panic!("another project is refused");
        };
        assert!(detail.starts_with("scope[0].project"), "{detail}");
        let Err(ApiError::Unprocessable { errors, .. }) = edit(
            json!({ "scope": [{ "project": "helsinki", "endpoint": "nope", "access": "read" }] }),
        )
        .await
        else {
            panic!("an Endpoint the project lacks is refused");
        };
        assert_eq!(errors, vec!["scope[0].endpoint"]);
        let Json(edited) = edit(json!({ "scope": [{ "project": "helsinki", "endpoint": "helsinki-air", "access": "read" }] }))
            .await
            .expect("edited");
        assert_eq!(edited.scope[0].endpoint.as_deref(), Some("helsinki-air"));
        assert_eq!(edited.name, "air");
    }

    /// T-6307: the options mark what the ceiling refuses, so the dialog greys it before Create
    /// instead of losing the whole form to one refusal.
    #[tokio::test]
    async fn the_options_mark_projects_and_endpoints_the_ceiling_refuses() {
        let state = reader_world();
        state.mirror.upsert(envelope(
            "RoleBinding",
            "reader-espoo",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "reader@hel.fi" }], "role": "reader", "scope": { "project": "espoo" } }),
        ));
        state.mirror.upsert(envelope(
            "Endpoint",
            "helsinki-noise",
            "helsinki",
            json!({ "contextSpaceRef": "air" }),
        ));
        state.mirror.upsert(envelope(
            "DelegationPolicy",
            "helsinki-air-only",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "kind": "Group", "name": "readers" }],
                    "projects": ["helsinki"], "endpoints": ["helsinki-air"] }),
        ));
        state.mirror.upsert(envelope(
            "Group",
            "readers",
            ORG_NAMESPACE,
            json!({ "members": [{ "user": "reader@hel.fi" }] }),
        ));
        let Json(options) = connector_options(
            user("reader@hel.fi"),
            State(state.clone()),
            HeaderMap::new(),
        )
        .await
        .expect("options");
        let reach: Vec<(String, bool, Option<Vec<String>>)> = options
            .projects
            .into_iter()
            .map(|p| (p.project, p.allowed, p.allowed_endpoints))
            .collect();
        assert_eq!(
            reach,
            vec![
                ("espoo".to_owned(), false, None),
                (
                    "helsinki".to_owned(),
                    true,
                    Some(vec!["helsinki-air".to_owned()])
                ),
            ]
        );
        assert!(!options.ceiling.writes);
    }

    #[tokio::test]
    async fn an_edit_above_a_queueing_ceiling_waits_again_unless_its_approval_covers_it() {
        let state = reader_world();
        state.mirror.upsert(envelope(
            "DelegationPolicy",
            "readers-a-week",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "kind": "Group", "name": "readers" }],
                    "projects": ["helsinki"], "maxExpiry": "P7D", "aboveCeiling": "queue" }),
        ));
        state.mirror.upsert(envelope(
            "Group",
            "readers",
            ORG_NAMESPACE,
            json!({ "members": [{ "user": "reader@hel.fi" }] }),
        ));
        // The reader also reads espoo, which the policy leaves out.
        state.mirror.upsert(envelope(
            "RoleBinding",
            "reader-espoo",
            ORG_NAMESPACE,
            json!({ "subjects": [{ "user": "reader@hel.fi" }], "role": "reader", "scope": { "project": "espoo" } }),
        ));
        let days = |n: i64| (chrono::Utc::now() + chrono::Duration::days(n)).to_rfc3339();
        let new: NewConnector = serde_json::from_value(json!({
            "name": "air", "segments": ["data-read"], "expiresAt": days(5),
            "scope": [{ "project": "helsinki", "access": "read" }]
        }))
        .expect("new");
        let created = state
            .connectors
            .create("f:1:reader@hel.fi", new, Vec::new(), "https://portal.test")
            .await
            .expect("created");
        let edit = |body| {
            edit_connector(
                user("reader@hel.fi"),
                State(state.clone()),
                Path(created.id.clone()),
                HeaderMap::new(),
                patch(body),
            )
        };
        let Json(longer) = edit(json!({ "expiresAt": days(20) })).await.expect("kept");
        assert_eq!(longer.state, "pending");
        assert_eq!(longer.above_ceiling.len(), 1, "{:?}", longer.above_ceiling);
        assert!(state
            .connectors
            .approve(&created.id, "sub-admin")
            .await
            .expect("approved"));
        // The same ask again keeps the approval; a new line above the ceiling waits again.
        let Json(again) = edit(json!({ "expiresAt": days(20) })).await.expect("kept");
        assert!(
            again.approval.is_some() && again.state != "pending",
            "{again:?}"
        );
        let both = json!([{ "project": "helsinki", "access": "read" }, { "project": "espoo", "access": "read" }]);
        let Json(wider) = edit(json!({ "scope": both, "expiresAt": days(20) }))
            .await
            .expect("kept");
        assert_eq!(wider.state, "pending");
        assert!(wider.approval.is_none());
        // Back inside the ceiling the connector lives with nothing above it.
        let one = json!([{ "project": "helsinki", "access": "read" }]);
        let Json(inside) = edit(json!({ "scope": one, "expiresAt": days(5) }))
            .await
            .expect("edited");
        assert!(
            inside.state != "pending" && inside.above_ceiling.is_empty(),
            "{inside:?}"
        );
    }

    fn administrator_user() -> CurrentUser {
        let mut admin = user("ada");
        // Config::for_tests names `portal-approver` its bootstrap administrators' group.
        admin.0.identity.groups = vec!["portal-approver".into()];
        admin
    }

    /// T-5953: a pending connector goes through once, approved by an administrator who did not
    /// ask for it; a stranger, the owner and a second approval do not let it through.
    #[tokio::test]
    async fn another_administrator_approves_a_pending_connector_once() {
        let state = AppState::new(Config::for_tests(), None);
        let new = body("ana-asks").expect("body").0;
        let made = state
            .connectors
            .create("f:1:ana", new, vec!["writes: above".into()], "")
            .await
            .expect("kept");
        assert_eq!(made.state, "pending");
        let approve = |who: CurrentUser, id: &str| {
            approve_connector(who, State(state.clone()), Path(id.to_owned()))
        };

        let stranger = approve(user("bob"), &made.id).await;
        assert!(matches!(stranger, Err(ApiError::Denied(why)) if why.contains("AG-150")));
        let mut owner = administrator_user();
        owner.0.identity.subject = "f:1:ana".into();
        let own = approve(owner, &made.id).await;
        assert!(matches!(own, Err(ApiError::Denied(why)) if why.contains("another administrator")));
        let unknown = approve(administrator_user(), &"a".repeat(26)).await;
        assert!(matches!(unknown, Err(ApiError::NotFound(_))));

        let Json(approved) = approve(administrator_user(), &made.id)
            .await
            .expect("approved");
        assert_eq!(approved.state, "active");
        let again = approve(administrator_user(), &made.id).await;
        assert!(matches!(again, Err(ApiError::NotFound(_))));
    }

    /// JC2-17: approving a connector above the ceiling is a decision: without the second factor
    /// in the session, where the deployment asks for one, it is refused and stays pending.
    #[tokio::test]
    async fn an_approval_above_the_ceiling_wants_the_second_factor() {
        let mut config = Config::for_tests();
        config.decisions_need_second_factor = true;
        let state = AppState::new(config, None);
        let new = body("ana-asks").expect("body").0;
        let made = state
            .connectors
            .create("f:1:ana", new, vec!["writes: above".into()], "")
            .await
            .expect("kept");
        let approve =
            |who: CurrentUser| approve_connector(who, State(state.clone()), Path(made.id.clone()));
        let refused = approve(administrator_user()).await;
        assert!(
            matches!(refused, Err(ApiError::SecondFactorRequired)),
            "{refused:?}"
        );
        let mut proven = administrator_user();
        proven.0.identity.second_factor = true;
        let Json(approved) = approve(proven).await.expect("approved");
        assert_eq!(approved.state, "active");
    }

    /// T-5975: only an administrator lists every person's connectors, with the day's calls and
    /// denials and the flag, and revokes one; nobody else reads or revokes them.
    #[tokio::test]
    async fn an_administrator_alone_lists_flags_and_revokes_every_persons_connectors() {
        let state = AppState::new(Config::for_tests(), None);
        let new = body("ana-reads").expect("body").0;
        let made = state
            .connectors
            .create("f:1:ana", new, Vec::new(), "")
            .await
            .expect("created");
        for verdict in std::iter::repeat_n("refused: outside the scope", 10).chain(["allowed"]) {
            let call = Call {
                at: chrono::Utc::now(),
                subject: "f:1:ana".into(),
                client: Some("mcp-local".into()),
                tool: "jc_resource_list".into(),
                target: Some("doprava/Endpoint/x".into()),
                verdict: verdict.into(),
                duration_ms: 3,
            };
            state
                .connectors
                .record(&made.id, call)
                .await
                .expect("audit");
        }

        let stranger = organization_connectors(user("bob"), State(state.clone())).await;
        assert!(matches!(stranger, Err(ApiError::Denied(why)) if why.contains("PF-03")));
        let refused =
            revoke_organization_connector(user("bob"), State(state.clone()), Path(made.id.clone()))
                .await;
        assert!(matches!(refused, Err(ApiError::Denied(_))));

        let mut agent = administrator_user();
        agent.0.identity.client = Some("helsinki-jc-agent-proxy".into());
        let listing = organization_connectors(agent.clone(), State(state.clone())).await;
        assert!(matches!(listing, Err(ApiError::Denied(why)) if why.contains("AG-133")));
        let revoking =
            revoke_organization_connector(agent, State(state.clone()), Path(made.id.clone())).await;
        assert!(matches!(revoking, Err(ApiError::Denied(why)) if why.contains("AG-133")));

        let Json(listed) = organization_connectors(administrator_user(), State(state.clone()))
            .await
            .expect("listed");
        assert_eq!(listed.items.len(), 1);
        let row = &listed.items[0];
        assert_eq!(
            (row.calls_per_day, row.denials_per_day, row.flagged),
            (11, 10, true)
        );
        let text = serde_json::to_string(&listed).expect("json");
        assert!(text.contains("\"owner\":\"f:1:ana\""), "{text}");

        let unknown = revoke_organization_connector(
            administrator_user(),
            State(state.clone()),
            Path("a".repeat(26)),
        )
        .await;
        assert!(matches!(unknown, Err(ApiError::NotFound(_))));
        let revoked = revoke_organization_connector(
            administrator_user(),
            State(state.clone()),
            Path(made.id.clone()),
        )
        .await
        .expect("revoked");
        assert_eq!(revoked, StatusCode::NO_CONTENT);
        let again = revoke_organization_connector(
            administrator_user(),
            State(state.clone()),
            Path(made.id.clone()),
        )
        .await;
        assert!(matches!(again, Err(ApiError::NotFound(_))));
        let Json(after) = organization_connectors(administrator_user(), State(state))
            .await
            .expect("listed");
        assert_eq!(after.items[0].connector.state, "revoked");
    }
}
