//! `GET /organization/portal-hosts` (ODP-08, ODP-15, T-6875): for each Organization the caller may
//! make a `kind: Portal` in, the hosts the operator listed for it and its open data base address.
//! The Portals editor offers these hosts as its only choices; the judge checks the submitted host
//! again (`portals::check`), so this list is a convenience, never the authority.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;
use jc_core::kinds::Verb;

/// One Organization the caller manages portals of.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortalOrganization {
    /// The Organization's name, the `spec.organizationRef` of its portals, e.g. `bratislava`.
    pub organization: String,
    /// The hosts the operator listed for its portals, e.g. `mobility.bratislava.sk`.
    pub hosts: Vec<String>,
    /// Its open data portal's public base, e.g. `https://opendata.bratislava.sk`; absent when
    /// this Portal knows none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opendata: Option<String>,
}

/// The Organizations the caller makes portals in, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct PortalHosts {
    pub items: Vec<PortalOrganization>,
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/portal-hosts",
    summary = "List Portal Hosts",
    description = "Each Organization the caller may propose a Portal in (its administrators, the installation's for every one), with the hosts the operator listed for its portals and its open data base address (ODP-08, ODP-15). A Portal naming another host is refused at propose with 422 on spec.host.",
    tag = "system",
    responses(
        (status = 200, description = "The caller's Organizations, by name", body = PortalHosts),
        (status = 401, description = "Not signed in", body = ProblemDetails),
        (status = 403, description = "The caller makes portals in no Organization", body = ProblemDetails)
    )
)]
pub async fn list_portal_hosts(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<PortalHosts>, ApiError> {
    let effective = crate::permissions::for_request(&state, &user.0.identity, ORG_NAMESPACE);
    let settings = &state.config.portal_settings;
    let items: Vec<PortalOrganization> = crate::permissions::organizations(&state.mirror)
        .into_iter()
        .map(|(organization, _)| organization)
        .filter(|organization| {
            let probe = json!({
                "apiVersion": "joinedcontext.com/v1alpha1",
                "kind": "Portal",
                "metadata": { "name": format!("{organization}-probe"), "namespace": ORG_NAMESPACE },
                "spec": { "organizationRef": organization, "path": "probe" },
            });
            effective
                .check("Portal", Verb::Propose, Some(&probe))
                .is_ok()
        })
        .map(|organization| PortalOrganization {
            hosts: settings
                .hosts
                .get(&organization)
                .cloned()
                .unwrap_or_default(),
            opendata: state
                .config
                .organization_catalogues
                .iter()
                .find(|c| c.organization == organization)
                .map(|c| c.url.as_str().trim_end_matches('/').to_owned()),
            organization,
        })
        .collect();
    if items.is_empty() {
        return Err(ApiError::Denied(
            "only an Organization's administrator makes its open data portals: it needs propose \
             on Portal (ODP-12)"
                .into(),
        ));
    }
    Ok(Json(PortalHosts { items }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/organization/portal-hosts", get(list_portal_hosts))
}
