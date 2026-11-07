//! `GET /api/v1/projects/{project}/ckan/status`: what this project publishes to its open-data
//! catalogue (T-0318, EP-62…EP-67, UI-05).
//!
//! Configuring a catalogue is not a second write path. A `CkanInstance` is a manifest like
//! every other one, so it is created, listed and changed through
//! `/api/v1/projects/{project}/ckaninstances`, which means a new catalogue arrives as a change
//! proposal a steward approves rather than as a setting somebody flips (CC-03, CC-08). What
//! only this route can answer is the picture across both kinds: which endpoints publish, to
//! which instance, and which URLs a citizen will click.
//!
//! The dataset shown here is the dataset the reconciler would write, because it is rendered by
//! the same [`jcctl::publish::ckan`] code, so the monitor cannot drift from the publisher. No
//! CKAN call is made and no credential is read: the API token is a `secretRef` the reconciler
//! resolves, and only the name of that reference ever reaches this answer (EP-67).

use axum::extract::{Path, State};
use axum::Json;
use jc_core::kinds::ckan::CkanInstanceSpec;
use jcctl::loader::{RawManifest, RawMetadata};
use jcctl::publish::ckan::{package, Settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::resource::ResourceEnvelope;
use crate::state::AppState;
use crate::store::ListOptions;

/// The CKAN picture of one project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CkanStatus {
    /// Every catalogue this project can publish to.
    pub instances: Vec<InstanceSummary>,
    /// One entry per endpoint that declares `spec.publish.ckan`.
    pub publications: Vec<PublicationStatus>,
}

/// One `CkanInstance`, without anything secret about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceSummary {
    /// Manifest name.
    pub name: String,
    /// Base URL of the catalogue.
    pub url: String,
    /// The organization a dataset lands in when the endpoint names none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_default: Option<String>,
    /// The name of the secret holding the API token. Never its value (EP-67).
    pub api_token_ref: String,
}

/// What one endpoint publishes, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicationStatus {
    /// The endpoint that declares the publication.
    pub endpoint: String,
    /// The endpoint's lifecycle phase, as the mirror reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The `CkanInstance` it publishes to.
    pub instance: String,
    /// `true` when that instance is missing from the project, which is why nothing publishes.
    pub instance_missing: bool,
    /// The CKAN organization the dataset lands in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// The CKAN dataset name.
    pub dataset: String,
    /// Where the dataset is in the catalogue, once it is published.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dataset_url: Option<String>,
    /// One resource per enabled representation, plus the schema index (EP-64).
    pub resources: Vec<ResourceLink>,
}

/// One CKAN resource of a dataset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLink {
    /// What the resource is called in CKAN.
    pub name: String,
    /// The URL a citizen clicks, always under the endpoint (EP-66).
    pub url: String,
    /// The CKAN format string.
    pub format: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/projects/{project}/ckan/status",
    summary = "Read Catalogue Publication",
    description = "The catalogues this project publishes to, and what each endpoint's publication is doing.",
    tag = "ckan",
    params(("project" = String, Path, description = "Project name")),
    responses(
        (status = 200, description = "Catalogues and publications of this project", body = CkanStatus),
        (status = 401, description = "Unauthorized", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_status(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(project): Path<String>,
) -> Result<Json<CkanStatus>, ApiError> {
    // The publication status is a read of the project's catalogue instances (PF-59, T-1401),
    // asked the way `jc_ckan_status` asks it; not readable is not there (R20).
    if !crate::permissions::for_request(&state, &user.0.identity, &project).may_read("CkanInstance")
    {
        return Err(ApiError::NotFound(format!("project '{project}' not found")));
    }
    let opts = ListOptions::default();
    let instances: Vec<(String, CkanInstanceSpec)> = state
        .mirror
        .list(&project, "CkanInstance", &opts)
        .items
        .into_iter()
        .filter_map(|env| {
            let spec: CkanInstanceSpec = serde_json::from_value(env.spec).ok()?;
            Some((env.metadata.name, spec))
        })
        .collect();

    let settings = Settings::new(host_of(&state));
    let publications = state
        .mirror
        .list(&project, "Endpoint", &opts)
        .items
        .into_iter()
        .filter_map(|env| publication(env, &project, &instances, &settings))
        .collect();

    Ok(Json(CkanStatus {
        instances: instances
            .iter()
            .map(|(name, spec)| InstanceSummary {
                name: name.clone(),
                url: spec.base_url().to_owned(),
                organization_default: spec.organization_default.clone(),
                api_token_ref: spec.api_token_ref.name.clone(),
            })
            .collect(),
        publications,
    }))
}

/// The host endpoints answer on, which is the host that served this page: APISIX routes
/// `/api/endpoint/` to the gateway and everything else to the Portal.
fn host_of(state: &AppState) -> String {
    state
        .config
        .public_base_url
        .host_str()
        .unwrap_or("localhost")
        .to_owned()
}

/// The status of one endpoint, or `None` when it publishes nowhere.
fn publication(
    env: ResourceEnvelope,
    project: &str,
    instances: &[(String, CkanInstanceSpec)],
    settings: &Settings,
) -> Option<PublicationStatus> {
    let declared = env.spec.get("publish")?.get("ckan")?.clone();
    let instance_name = declared
        .get("instanceRef")
        .and_then(crate::resource::reference_name)
        .unwrap_or_default()
        .to_owned();
    let instance = instances.iter().find(|(name, _)| name == &instance_name);
    let phase = env
        .status
        .as_ref()
        .map(|status| crate::resource::phase_str(status.phase).to_owned());

    // Without the instance there is no organization and no catalogue URL, but the endpoint
    // still says where it wants to go: a dangling reference is what this view has to show.
    let Some((_, spec)) = instance else {
        return Some(PublicationStatus {
            endpoint: env.metadata.name,
            phase,
            instance: instance_name,
            instance_missing: true,
            organization: None,
            dataset: declared
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            dataset_url: None,
            resources: Vec::new(),
        });
    };

    // The same rendering the publisher uses, so what the monitor lists is what will be
    // written: an empty DCAT record is enough, only the resources and the names are read here.
    let manifest = RawManifest {
        api_version: env.api_version,
        kind: env.kind,
        metadata: RawMetadata {
            name: env.metadata.name.clone(),
            namespace: Some(project.to_owned()),
            rest: serde_json::Map::new(),
        },
        spec: env.spec,
        status: None,
    };
    let dataset = package(&manifest, spec, &Value::Null, settings)
        .ok()
        .flatten()?;

    let name = dataset["name"].as_str().unwrap_or_default().to_owned();
    Some(PublicationStatus {
        endpoint: env.metadata.name,
        phase,
        instance: instance_name,
        instance_missing: false,
        organization: dataset["owner_org"].as_str().map(str::to_owned),
        dataset_url: Some(format!("{}/dataset/{name}", spec.base_url())),
        dataset: name,
        resources: dataset["resources"]
            .as_array()
            .map(|items| items.iter().map(link).collect())
            .unwrap_or_default(),
    })
}

fn link(resource: &Value) -> ResourceLink {
    ResourceLink {
        name: text(resource, "name"),
        url: text(resource, "url"),
        format: text(resource, "format"),
    }
}

/// A `Ref` is either a bare name or `{ kind, name }` (MF-09).
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route(
        "/projects/{project}/ckan/status",
        axum::routing::get(get_status),
    )
}
