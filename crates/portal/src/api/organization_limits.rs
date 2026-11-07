//! `GET /api/v1/organization/limits`: what Organization settings shows for every policy and limit
//! (PF-96…PF-102, ADR-N-035; API/01 §28). The catalog of jc-core with the operator's bound on each
//! entry, the value the Organization sets and where it comes from, and each readable project's
//! quota with what it uses.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use jc_core::kinds::org_settings::{BoundKind, CATALOG};
use jc_core::kinds::OrganizationSpec;
use serde::Serialize;
use utoipa::ToSchema;

use crate::auth::session::CurrentUser;
use crate::error::ApiError;
use crate::permissions::ORG_NAMESPACE;
use crate::state::AppState;
use crate::store::ListOptions;

/// The Organization manifest's spec, when the organization repository holds one that parses.
pub(crate) fn organization_spec(state: &AppState) -> Option<OrganizationSpec> {
    state
        .mirror
        .list(ORG_NAMESPACE, "Organization", &ListOptions::default())
        .items
        .into_iter()
        .find_map(|envelope| serde_json::from_value::<OrganizationSpec>(envelope.spec).ok())
}

/// The organization's `spec.limits`, when its manifest sets any that parse. Only that subtree is
/// read, so a field this Portal does not know yet elsewhere in the manifest never turns the
/// organization's own limits back into the defaults.
fn organization_limits(state: &AppState) -> Option<jc_core::kinds::OrganizationLimits> {
    limits_in(&state.mirror)
}

/// [`organization_limits`] for a caller that holds a mirror and no state: the reconciler.
pub(crate) fn limits_in(
    mirror: &crate::store::Mirror,
) -> Option<jc_core::kinds::OrganizationLimits> {
    mirror
        .list(ORG_NAMESPACE, "Organization", &ListOptions::default())
        .items
        .into_iter()
        .find_map(|org| serde_json::from_value(org.spec.get("limits")?.clone()).ok())
}

const UPLOAD: &str = "spec.limits.data.uploadMegabytes";
const INVITATION: &str = "spec.limits.people.invitationHours";

/// How long the link in a realm e-mail the Portal asks for lives (`spec.limits.people.
/// invitationHours`, ADR-N-035): the organization's value, else the catalog's default, held
/// inside the operator's bound, so a bound lowered after the change was approved still holds.
/// `None` only if the catalog lost the entry, which leaves the realm's own lifespan.
pub(crate) fn invitation_lifespan(state: &AppState) -> Option<std::time::Duration> {
    let entry = jc_core::kinds::org_settings::entry_at(INVITATION)?;
    let set = organization_limits(state).and_then(|limits| limits.people.invitation_hours);
    let (min, max) = state.config.organization_bounds.range(entry);
    // `max`/`min` rather than `clamp`: an operator file with min above max must not panic here.
    let hours = set.or(entry.default)?.min(max.unwrap_or(u32::MAX)).max(min);
    Some(std::time::Duration::from_secs(u64::from(hours) * 3600))
}

/// Refuses an upload or import larger than the organization accepts (`spec.limits.data.
/// uploadMegabytes`, ADR-N-035): its own value, else the catalog's default, and never past the
/// Portal's own ceiling, which the edge's largest body sets.
pub(crate) fn within_upload_limit(state: &AppState, bytes: usize) -> Result<(), ApiError> {
    let set = organization_limits(state).and_then(|limits| limits.data.upload_megabytes);
    let megabytes =
        set.or_else(|| jc_core::kinds::org_settings::entry_at(UPLOAD).and_then(|e| e.default));
    let limit = megabytes
        .map_or(crate::api::import::MAX_UPLOAD_BYTES, |mb| {
            mb as usize * 1024 * 1024
        })
        .min(crate::api::import::MAX_UPLOAD_BYTES);
    if bytes > limit {
        return Err(ApiError::BadRequest(format!(
            "the upload is {:.1} MiB and the organization accepts at most {} MiB; make it smaller, \
             or ask an organization admin to raise the upload size in Organization settings \
             ({UPLOAD})",
            bytes as f64 / (1024.0 * 1024.0),
            limit / (1024 * 1024)
        )));
    }
    Ok(())
}

/// Where a value in force comes from (PF-101).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum LimitOrigin {
    /// The Project's own `spec.quotas`.
    Project,
    /// The Organization manifest.
    Organization,
    /// Nothing sets it: the catalog's default.
    Default,
}

/// One entry of the catalog as the settings page shows it (PF-102).
#[derive(Debug, Serialize, ToSchema)]
pub struct LimitEntry {
    /// The manifest path, e.g. `spec.limits.edge.requestsPerMinute.web`.
    pub path: String,
    /// The settings section: `projects`, `applications`, `edge`, `signIn`, `people`, `agents`,
    /// `pipelinesAndData`.
    pub section: String,
    /// Whether the operator may only tighten the bound (ADR-N-035 §3.2).
    pub security: bool,
    /// The catalog's default; `null` is no limit.
    pub default: Option<u32>,
    /// The smallest value the organization may set.
    pub min: u32,
    /// The largest value the organization may set; `null` where nobody sets a ceiling.
    pub max: Option<u32>,
    /// What the Organization manifest sets; `null` when it sets nothing.
    pub value: Option<u32>,
    /// `organization` or `default`.
    pub origin: LimitOrigin,
}

/// One quota dimension of a project.
#[derive(Debug, Serialize, ToSchema)]
pub struct QuotaUse {
    /// `null` is no limit.
    pub limit: Option<u32>,
    /// The manifest count for a countable dimension (PF-75); `null` for a runtime limit.
    pub used: Option<u32>,
}

/// The quota in force for one project the caller may read.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectQuota {
    pub project: String,
    pub origin: LimitOrigin,
    pub quota: BTreeMap<String, QuotaUse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OrganizationLimits {
    pub entries: Vec<LimitEntry>,
    pub projects: Vec<ProjectQuota>,
}

/// The section's name as the page and the API spell it: jc-core's own serialization.
fn section_name(section: jc_core::kinds::org_settings::Section) -> String {
    serde_json::to_value(section)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[utoipa::path(
    get,
    path = "/api/v1/organization/limits",
    summary = "Read Organization Limits",
    description = "Every policy and limit of the catalog with the operator's bound, the value in force and where it comes from, and the quota of each project the caller may read (PF-96…PF-102, ADR-N-035).",
    tag = "system",
    responses(
        (status = 200, description = "The catalog and the values in force", body = OrganizationLimits),
        (status = 401, description = "Not signed in", body = crate::error::ProblemDetails),
    )
)]
pub async fn get_limits(
    user: CurrentUser,
    State(state): State<AppState>,
) -> Result<Json<OrganizationLimits>, ApiError> {
    let organization = organization_spec(&state);
    let bounds = &state.config.organization_bounds;
    let entries = CATALOG
        .iter()
        .map(|entry| {
            let (min, max) = bounds.range(entry);
            let value = organization
                .as_ref()
                .and_then(|spec| spec.setting(entry.path));
            LimitEntry {
                path: entry.path.to_owned(),
                section: section_name(entry.section),
                security: entry.kind == BoundKind::Security,
                default: entry.default,
                min,
                max,
                value,
                origin: if value.is_some() {
                    LimitOrigin::Organization
                } else {
                    LimitOrigin::Default
                },
            }
        })
        .collect();

    let identity = &user.0.identity;
    let organization_set = organization
        .as_ref()
        .and_then(|spec| spec.projects.quota.as_ref())
        .is_some();
    // A BTreeSet: the rows come out ordered by name.
    let projects: Vec<ProjectQuota> = state
        .mirror
        .namespaces()
        .into_iter()
        .filter(|project| project != ORG_NAMESPACE)
        .chain(
            state
                .mirror
                .list(ORG_NAMESPACE, "Project", &ListOptions::default())
                .items
                .into_iter()
                .map(|envelope| envelope.metadata.name),
        )
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        // A project the caller may not read is left out, so nothing of its size is said (R20).
        .filter(|project| {
            crate::permissions::for_request(&state, identity, project).may_read_project()
        })
        .map(|project| {
            let own = state
                .mirror
                .get(ORG_NAMESPACE, "Project", &project)
                .and_then(|envelope| {
                    serde_json::from_value::<jc_core::kinds::Quotas>(
                        envelope.spec.get("quotas")?.clone(),
                    )
                    .ok()
                })
                .is_some_and(|quotas| quotas.sets_dimensions());
            let quotas = crate::quotas::effective(&state.mirror, &project);
            let used = crate::quotas::usage(&state.mirror, &project);
            let mut quota: BTreeMap<String, QuotaUse> = quotas
                .dimensions()
                .into_iter()
                .map(|(dimension, limit)| {
                    let used = used.get(dimension).copied();
                    (dimension.to_owned(), QuotaUse { limit, used })
                })
                .collect();
            // The MQTT allowance (PF-117, API/01 §47): its own origin counts as the project's.
            let (mqtt, mqtt_own) = crate::mqtt_allowance::quota_rows(&state.mirror, &project);
            for (dimension, limit, used) in mqtt {
                let (limit, used) = (Some(limit), Some(used));
                quota.insert(dimension.to_owned(), QuotaUse { limit, used });
            }
            ProjectQuota {
                origin: if own || mqtt_own {
                    LimitOrigin::Project
                } else if organization_set {
                    LimitOrigin::Organization
                } else {
                    LimitOrigin::Default
                },
                project,
                quota,
            }
        })
        .collect();
    Ok(Json(OrganizationLimits { entries, projects }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/organization/limits", get(get_limits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    const MIB: usize = 1024 * 1024;

    fn organization(spec: serde_json::Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "Organization".into(),
            metadata: ObjectMeta {
                name: "hel".into(),
                namespace: Some(ORG_NAMESPACE.into()),
                ..Default::default()
            },
            spec,
            status: None,
        }
    }

    /// T-2870 (ADR-N-035): the upload size is the catalog's 16 MiB until the organization sets
    /// one; what it sets holds, never past the Portal's ceiling, the edge's largest body.
    #[test]
    fn the_upload_size_is_the_organizations_the_default_or_the_ceiling() {
        let state = AppState::new(Config::for_tests(), None);
        assert!(within_upload_limit(&state, 16 * MIB).is_ok());
        assert!(within_upload_limit(&state, 16 * MIB + 1).is_err());

        state.mirror.upsert(organization(serde_json::json!({
            "domain": "hel.fi",
            "limits": { "data": { "uploadMegabytes": 40 } },
        })));
        assert!(within_upload_limit(&state, 40 * MIB).is_ok());
        let Err(ApiError::BadRequest(message)) = within_upload_limit(&state, 40 * MIB + MIB / 2)
        else {
            panic!("40.5 MiB is past the organization's 40");
        };
        assert!(message.contains("40.5 MiB"), "{message}");
        assert!(message.contains("at most 40 MiB"), "{message}");

        state.mirror.upsert(organization(serde_json::json!({
            "domain": "hel.fi",
            "limits": { "data": { "uploadMegabytes": 500 } },
        })));
        assert!(within_upload_limit(&state, crate::api::import::MAX_UPLOAD_BYTES).is_ok());
        assert!(within_upload_limit(&state, crate::api::import::MAX_UPLOAD_BYTES + 1).is_err());
    }
}
