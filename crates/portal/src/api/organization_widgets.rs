//! An Organization's home page widgets (UIX-16, T-6779): the Dashboards its
//! `spec.appearance.homeWidgets` names, each `project/name` of one of its own projects.
//!
//! A reference is checked twice. When the Organization's manifest is proposed, each one must
//! name an existing Dashboard of that Organization the proposer may read (422 naming
//! `spec.appearance.homeWidgets[i]`). When a page asks for them, each one is read again under
//! the viewer's own rights and the Organization the request is pinned to: a Dashboard the viewer
//! may not read, or one of another Organization, is left out and never named.
//!
//! `spec.appearance.opendataWidgets` (T-6780) names Dashboards the Organization's open data
//! portal shows to anonymous readers: each must also carry `visibility: public`, so it reads only
//! public Endpoints (UI-19). The catalogue home asks [`public_dashboards`] on every render.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use crate::auth::session::{CurrentUser, Identity};
use crate::error::ApiError;
use crate::permissions::{self, ORG_NAMESPACE};
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::{ListOptions, Mirror};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/organization/widgets", get(list_widgets))
        .route("/organization/widgets/options", get(list_options))
}

/// One Dashboard a home page shows or a form offers.
#[derive(Debug, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HomeWidget {
    /// `project/name`, as `spec.appearance.homeWidgets` writes it.
    pub reference: String,
    pub project: String,
    pub name: String,
    /// The Dashboard's title: one string, or one per locale.
    #[schema(value_type = Object)]
    pub title: Value,
    /// How many pages it has.
    pub pages: usize,
    /// Whether it carries `visibility: public`, so the open data portal may show it (T-6780).
    pub public: bool,
}

/// The widgets of the request's Organization this viewer may read, in the manifest's order.
#[derive(Debug, Serialize, ToSchema)]
pub struct HomeWidgets {
    pub widgets: Vec<HomeWidget>,
}

/// Every Dashboard of one Organization the caller may read: what the form offers.
#[derive(Debug, Serialize, ToSchema)]
pub struct WidgetOptions {
    pub options: Vec<HomeWidget>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct OptionsQuery {
    /// The Organization the form edits, by `metadata.name`.
    pub organization: String,
}

/// The Dashboard `reference` names, when it is one of `organization`'s projects and `identity`
/// may read it; `None` for every other case alike, so nothing tells a missing Dashboard from an
/// unreadable one or one of another Organization.
fn readable(
    state: &AppState,
    identity: &Identity,
    organization: &str,
    reference: &str,
) -> Option<HomeWidget> {
    let (project, name) = reference.split_once('/')?;
    // A project without its Project manifest would count as the installation's.
    state.mirror.get(ORG_NAMESPACE, "Project", project)?;
    if permissions::organization_of(&state.mirror, project).as_deref() != Some(organization) {
        return None;
    }
    let dashboard = state.mirror.get(project, "Dashboard", name)?;
    permissions::for_request(state, identity, project)
        .may_read_envelope("Dashboard", &dashboard)
        .then(|| widget(project, &dashboard))
}

fn widget(project: &str, dashboard: &ResourceEnvelope) -> HomeWidget {
    let name = dashboard.metadata.name.clone();
    HomeWidget {
        reference: format!("{project}/{name}"),
        project: project.to_owned(),
        name,
        title: dashboard.spec.get("title").cloned().unwrap_or(Value::Null),
        pages: dashboard
            .spec
            .get("pages")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        public: crate::dashboards::is_public_dashboard(&dashboard.spec),
    }
}

/// The Dashboard `reference` names when it is one of `organization`'s projects and carries
/// `visibility: public`: what an anonymous reader of its open data portal may see.
fn public_dashboard(
    mirror: &Mirror,
    organization: &str,
    reference: &str,
) -> Option<(String, ResourceEnvelope)> {
    let (project, name) = reference.split_once('/')?;
    mirror.get(ORG_NAMESPACE, "Project", project)?;
    if permissions::organization_of(mirror, project).as_deref() != Some(organization) {
        return None;
    }
    let dashboard = mirror.get(project, "Dashboard", name)?;
    crate::dashboards::is_public_dashboard(&dashboard.spec).then(|| (project.to_owned(), dashboard))
}

/// The Dashboards `organization`'s `spec.appearance.opendataWidgets` names that are still its own
/// and public, in the manifest's order, each with its project: read on every render of its open
/// data portal's home, so a Dashboard made private or moved away disappears at once (T-6780).
pub fn public_dashboards(mirror: &Mirror, organization: &str) -> Vec<(String, ResourceEnvelope)> {
    mirror
        .get(ORG_NAMESPACE, "Organization", organization)
        .and_then(|org| {
            org.spec
                .pointer("/appearance/opendataWidgets")
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|reference| public_dashboard(mirror, organization, reference))
        .collect()
}

/// Refuses an Organization manifest whose `spec.appearance.homeWidgets` names a Dashboard the
/// proposer may not read in that Organization (T-6779), or whose `opendataWidgets` names one that
/// is also not public (T-6780). Any other kind passes.
pub fn check(
    state: &AppState,
    identity: &Identity,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    if envelope.kind != "Organization" {
        return Ok(());
    }
    let organization = &envelope.metadata.name;
    for (member, public) in [("homeWidgets", false), ("opendataWidgets", true)] {
        let widgets = envelope
            .spec
            .pointer(&format!("/appearance/{member}"))
            .and_then(Value::as_array);
        for (at, reference) in widgets.into_iter().flatten().enumerate() {
            let shown = reference
                .as_str()
                .and_then(|reference| readable(state, identity, organization, reference))
                .filter(|widget| widget.public || !public);
            if shown.is_none() {
                let named = reference.as_str().unwrap_or_default();
                let kind = if public {
                    "public Dashboard"
                } else {
                    "Dashboard"
                };
                let line = format!(
                    "spec.appearance.{member}[{at}]: {named} is not a {kind} of this \
                     Organization's projects that you may read; choose one the list offers"
                );
                return Err(ApiError::Unprocessable {
                    detail: line.clone(),
                    errors: vec![line],
                });
            }
        }
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/widgets",
    summary = "List The Home Page Widgets",
    description = "The Dashboards the request's Organization shows on its Portal home page (spec.appearance.homeWidgets, UIX-16), each read again under the caller's rights: one the caller may not read, or one of another Organization, is left out (T-6779).",
    tag = "system",
    responses(
        (status = 200, description = "The widgets the caller may read, in the manifest's order", body = HomeWidgets),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_widgets(user: CurrentUser, State(state): State<AppState>) -> Json<HomeWidgets> {
    let identity = &user.0.identity;
    let organization = crate::organization_host::organization()
        .or_else(|| permissions::installation_organization(&state.mirror));
    let references = organization
        .as_deref()
        .and_then(|name| state.mirror.get(ORG_NAMESPACE, "Organization", name))
        .and_then(|org| {
            org.spec
                .pointer("/appearance/homeWidgets")
                .and_then(Value::as_array)
                .cloned()
        })
        .unwrap_or_default();
    let widgets = organization
        .map(|organization| {
            references
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|reference| readable(&state, identity, &organization, reference))
                .collect()
        })
        .unwrap_or_default();
    Json(HomeWidgets { widgets })
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/widgets/options",
    summary = "List The Dashboards A Home Page May Show",
    description = "Every Dashboard of the named Organization's projects the caller may read: the choices of spec.appearance.homeWidgets, which the propose door checks again (T-6779). An Organization the caller reads nothing in answers an empty list.",
    tag = "system",
    params(OptionsQuery),
    responses(
        (status = 200, description = "The Dashboards the caller may name", body = WidgetOptions),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
    )
)]
pub async fn list_options(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<OptionsQuery>,
) -> Json<WidgetOptions> {
    let identity = &user.0.identity;
    let organization = query.organization;
    let mut options = Vec::new();
    for project in state
        .mirror
        .list(ORG_NAMESPACE, "Project", &ListOptions::default())
        .items
    {
        let project = project.metadata.name;
        if permissions::organization_of(&state.mirror, &project).as_deref()
            != Some(organization.as_str())
        {
            continue;
        }
        let effective = permissions::for_request(&state, identity, &project);
        options.extend(
            state
                .mirror
                .list(&project, "Dashboard", &ListOptions::default())
                .items
                .iter()
                .filter(|dashboard| effective.may_read_envelope("Dashboard", dashboard))
                .map(|dashboard| widget(&project, dashboard)),
        );
    }
    Json(WidgetOptions { options })
}
