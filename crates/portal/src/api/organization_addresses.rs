//! `GET /organization/addresses` (API/15 §2, EP-186, T-7013): every address of each Organization
//! the caller belongs to, as jc's host registry routes it (`JC_PORTAL_ORGANIZATION_ADDRESSES`):
//! its own hosts, its `/{id}/` path on a shared host, where its people sign in and its MCP
//! servers. Nobody types an address here; the page lists what the deployment serves.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::CurrentUser;
use crate::error::{ApiError, ProblemDetails};
use crate::organization_host::{AddressKind, OrganizationAddress};
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;

/// The addresses of one Organization.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationAddressList {
    /// The Organization's name, e.g. `bbsk`.
    pub organization: String,
    /// The title people know it by, e.g. `Banskobystrický kraj`; its name when it has none.
    pub title: String,
    /// Its id, e.g. `sk_bbsk` (EP-186); absent until the operator sets one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    pub addresses: Vec<OrganizationAddress>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationAddresses {
    /// One entry per Organization the caller belongs to, by name; empty when the deployment
    /// lists no Organization addresses.
    pub items: Vec<OrganizationAddressList>,
}

/// The title of Organization `name`: `metadata.title`, else `spec.appearance.instanceName`, else
/// the name.
fn title(state: &AppState, name: &str) -> String {
    state
        .mirror
        .get(ORG_NAMESPACE, "Organization", name)
        .and_then(|envelope| {
            let manifest = serde_json::to_value(&envelope).ok()?;
            crate::resource::title_text(&manifest["metadata"]["title"]).or_else(|| {
                manifest["spec"]["appearance"]["instanceName"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned)
            })
        })
        .unwrap_or_else(|| name.to_owned())
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/addresses",
    summary = "List Organization Addresses",
    description = "Every address of each Organization the caller belongs to, as the deployment routes it: its own hosts, its /{id}/ path on a shared host, where its people sign in and its MCP servers (API/15 §2, EP-186). On an Organization's own host, that Organization alone.",
    tag = "system",
    responses(
        (status = 200, description = "The caller's Organizations with their addresses", body = OrganizationAddresses),
        (status = 401, description = "Not signed in", body = ProblemDetails)
    )
)]
pub async fn list_addresses(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<OrganizationAddresses>, ApiError> {
    let identity = &user.0.identity;
    let pinned = crate::organization_host::pinned().map(|host| host.organization);
    let sign_in = state.config.oidc.as_ref().map(|oidc| oidc.issuer.clone());
    let items = state
        .config
        .organization_addresses
        .iter()
        .filter(|org| pinned.as_ref().is_none_or(|p| *p == org.organization))
        .filter(|org| {
            crate::permissions::belongs_to_organization(
                &state.mirror,
                &state.config.bootstrap_admins,
                identity,
                &org.organization,
            )
        })
        .map(|org| OrganizationAddressList {
            organization: org.organization.clone(),
            title: title(&state, &org.organization),
            organization_id: org.organization_id.clone(),
            addresses: org
                .addresses
                .iter()
                .cloned()
                .chain(sign_in.clone().map(|url| OrganizationAddress {
                    kind: AddressKind::SignIn,
                    url,
                }))
                .collect(),
        })
        .collect();
    Ok(Json(OrganizationAddresses { items }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/organization/addresses", get(list_addresses))
}
