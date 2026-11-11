//! An Organization's home page widgets (UIX-16, T-6779): the Dashboards its
//! `spec.appearance.homeWidgets` names, each `project/name` of one of its own projects.
//!
//! A reference is checked twice. When the Organization's manifest is proposed, each one must
//! name an existing Dashboard of that Organization the proposer may read (422 naming
//! `spec.appearance.homeWidgets[i]`). When a page asks for them, each one is read again under
//! the viewer's own rights and the Organization the request is pinned to: a Dashboard the viewer
//! may not read, or one of another Organization, is left out and never named.

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
use crate::store::ListOptions;

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
    }
}

/// Refuses an Organization manifest whose `spec.appearance.homeWidgets` names a Dashboard the
/// proposer may not read in that Organization (T-6779). Any other kind passes.
pub fn check(
    state: &AppState,
    identity: &Identity,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    if envelope.kind != "Organization" {
        return Ok(());
    }
    let widgets = envelope
        .spec
        .pointer("/appearance/homeWidgets")
        .and_then(Value::as_array);
    for (at, reference) in widgets.into_iter().flatten().enumerate() {
        let shown = reference
            .as_str()
            .and_then(|reference| readable(state, identity, &envelope.metadata.name, reference));
        if shown.is_none() {
            let line = format!(
                "spec.appearance.homeWidgets[{at}]: must name a Dashboard of this Organization's \
                 projects that you may read, as project/name"
            );
            return Err(ApiError::Unprocessable {
                detail: line.clone(),
                errors: vec![line],
            });
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
