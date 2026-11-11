//! Every Pipeline's own principal and Policies, from a loaded repository (PL-19, PL-20, T-1508).
//!
//! The derivation itself is jc-core's ([`jc_core::kinds::pipeline_identity::derive`]); this reads
//! the repository it needs: each Pipeline of a project and the space each Endpoint of that
//! project serves. Nothing derived is a file: the gateway adds the accounts and Policies to its
//! tables, and `plan`, `apply` and `validate` never see them, so no commit carries them.

use crate::loader::Repository;
use jc_core::kinds::pipeline_identity::{derive, Derived};
use jc_core::kinds::{EndpointSpec, PipelineSpec, PolicySpec};
use std::collections::BTreeMap;

/// The runner's namespace, where every pipeline's Kubernetes ServiceAccount lives.
pub const RUNNER_NAMESPACE: &str = "pipeline-runner";

/// The derived identity of every Pipeline of `repo`, with its project, in name order. A
/// Pipeline whose spec does not parse derives nothing: `validate` reports it.
pub fn derived(repo: &Repository) -> Vec<(String, Derived)> {
    let org_domain = repo.org_domain().unwrap_or_default().to_owned();
    let mut spaces: BTreeMap<(String, String), String> = BTreeMap::new();
    for (id, resource) in repo.iter().filter(|(id, _)| id.kind == "Endpoint") {
        if let Ok(spec) = serde_json::from_value::<EndpointSpec>(resource.manifest.spec.clone()) {
            spaces.insert(
                (id.namespace.clone().unwrap_or_default(), id.name.clone()),
                spec.context_space_ref.name().to_owned(),
            );
        }
    }
    repo.iter()
        .filter(|(id, _)| id.kind == "Pipeline")
        .filter_map(|(id, resource)| {
            let spec =
                serde_json::from_value::<PipelineSpec>(resource.manifest.spec.clone()).ok()?;
            let project = id.namespace.clone().unwrap_or_default();
            let derived = derive(
                &project,
                &id.name,
                &spec,
                RUNNER_NAMESPACE,
                &org_domain,
                |endpoint| spaces.get(&(project.clone(), endpoint.to_owned())).cloned(),
            );
            Some((project, derived))
        })
        .collect()
}

/// The Policies each Pipeline of `derived` grants on an Endpoint it names, by project and
/// Endpoint name: its write on each `targetEndpoint`, its read on each `endpointRef` source.
/// Such a grant binds the Endpoint it was derived for whatever `policyRefs` the Endpoint lists:
/// the Pipeline's manifest names that door, so it opens no other one (P-18, T-6736).
pub fn endpoint_grants(
    derived: &[(String, Derived)],
) -> BTreeMap<(String, String), Vec<(String, PolicySpec)>> {
    let mut grants: BTreeMap<(String, String), Vec<(String, PolicySpec)>> = BTreeMap::new();
    for (project, derived) in derived {
        for (name, spec) in &derived.policies {
            if let Some(endpoint) = derived.endpoints.get(name) {
                grants
                    .entry((project.clone(), endpoint.clone()))
                    .or_default()
                    .push((name.clone(), spec.clone()));
            }
        }
    }
    grants
}
