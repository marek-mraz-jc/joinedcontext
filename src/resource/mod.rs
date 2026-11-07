pub mod filter;
pub mod selector;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// The manifest contract is jc-core's (T-0249): metadata, phases, conditions, scopes and the kind
// catalogue come from the tagged crate. What stays here is the kind-generic view the resource API
// needs and jc-core does not have: an envelope whose `spec` is untyped JSON, and the status the
// Portal computes for it.
pub use jc_core::{Condition, KindInfo, ObjectMeta, Phase, Scope, API_VERSION};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResourceEnvelope {
    pub api_version: String,
    pub kind: String,
    #[schema(schema_with = crate::openapi::object_meta_ref)]
    pub metadata: ObjectMeta,
    /// Free-form JSON whose shape the kind decides: an open map in the OpenAPI document, so a
    /// generated client can send a real spec without a cast (T-1488).
    #[serde(default)]
    #[schema(value_type = std::collections::BTreeMap<String, serde_json::Value>)]
    pub spec: serde_json::Value,
    /// Never read from Git, always computed by the Portal API (MF-04).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

impl ResourceEnvelope {
    pub fn strip_status(&mut self) {
        self.status = None;
    }

    /// The strong `ETag` of the manifest (API/13 section 3): a SHA-256 over its JSON without
    /// `status`, which the Portal computes and Git never holds. Every member is a struct field or
    /// a sorted map, so the same manifest always hashes the same.
    pub fn etag(&self) -> String {
        use sha2::{Digest, Sha256};
        let manifest = Self {
            status: None,
            ..self.clone()
        };
        let bytes = serde_json::to_vec(&manifest).unwrap_or_default();
        let digest = Sha256::digest(bytes);
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        format!("\"{hex}\"")
    }

    pub fn key(&self) -> ResourceKey {
        ResourceKey {
            namespace: self.metadata.namespace.clone().unwrap_or_default(),
            kind: self.kind.clone(),
            name: self.metadata.name.clone(),
        }
    }
}

/// Metadata the Portal accepts on a write or from the mirror: jc-core's DNS-1123 and namespace
/// rules, plus a namespace that is present. jc-core checks the namespace on its typed envelope;
/// the kind-generic envelope here has to ask for it itself.
pub fn validate_meta(meta: &ObjectMeta) -> Result<(), String> {
    meta.validate().map_err(|e| e.to_string())?;
    match &meta.namespace {
        Some(ns) if !ns.is_empty() => Ok(()),
        _ => Err("namespace must be set and non-empty".to_string()),
    }
}

pub fn is_dns1123(name: &str) -> bool {
    jc_core::names::validate_dns1123_label(name).is_ok()
}

/// Whether `name` is a `metadata.name` jc-core accepts for `kind`: a space's own form for a
/// ContextSpace, a DNS-1123 label for every other kind (MF-02, T-2544).
pub fn is_manifest_name(kind: &str, name: &str) -> bool {
    match kind {
        "ContextSpace" => jc_core::names::validate_space_name(name).is_ok(),
        _ => is_dns1123(name),
    }
}

/// The phase as it appears on the wire, for `fieldSelector=status.phase=Live`. Exhaustive on
/// purpose: a new jc-core phase must be given its name here before it compiles.
pub fn phase_str(phase: Phase) -> &'static str {
    match phase {
        Phase::Draft => "Draft",
        Phase::Pending => "Pending",
        Phase::Deploying => "Deploying",
        Phase::Live => "Live",
        Phase::Error => "Error",
        Phase::Drifted => "Drifted",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
/// The status the Portal API reports (MF-04). It is `jc_core::Status` plus `sourceUrl` and a phase
/// that is always known; it stays a Portal type until jc-core carries `sourceUrl` too (docs API/01
/// section 6 is the contract), then it becomes a re-export like its neighbours.
pub struct Status {
    // A manifest the build lane wrote carries `status.build` and nothing else (AP-13a), so the
    // phase is defaulted rather than required: the Portal computes the real one on every sync,
    // and a missing one must not cost the manifest its place in the mirror.
    #[serde(default = "pending")]
    #[schema(schema_with = crate::openapi::phase_ref)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_revision: Option<String>,
    /// Forge page of the file this manifest was read from, so a view can link "Source"
    /// without knowing where a kind lives in the repository. Computed, never read from Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(schema_with = crate::openapi::conditions_ref)]
    pub conditions: Vec<Condition>,
    /// What the build lane published for an `App`, and the only place its artifact is named
    /// (AP-13a). The one member of `status` a proposal may carry, and only from the role whose
    /// `propose` on `App` is constrained to it (AP-73).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub build: Option<jc_core::Build>,
    /// The WASM host shard a `wasm` App runs on, recorded at its first publish and kept; the
    /// Portal writes it in the publish commit, never a proposal (AP-149).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shard: Option<u32>,
    /// Whether an Organization owns the domain it declares (PF-41): computed by the reconciler,
    /// never read from Git, and only ever on an Organization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_verification: Option<crate::domain_verification::DomainVerification>,
    /// The last run of each scheduled job of a `wasm` App, as its shard reports it (AP-154):
    /// computed by the reconciler, never read from Git.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<JobRun>,
    /// What a `Subscription`'s space reports of its deliveries (T-4866): computed by the
    /// reconciler after each write, never read from Git, and only ever on a Subscription.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notifications: Option<crate::reconciler::subscriptions::Deliveries>,
}

/// One job's last run, in `status.jobs[]` (AP-154, AP-155): never a body or a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct JobRun {
    /// The job's `name` in `spec.server.jobs[]`.
    pub name: String,
    /// When the run was due, RFC 3339 UTC.
    pub last_run: String,
    /// `succeeded`, `failed`, `timedOut` or `skipped`.
    pub outcome: String,
    /// Why a run did not succeed, in words a person acts on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub duration_ms: u64,
    /// Runs that failed or timed out in a row; three raise a notice to the owners (AP-155).
    pub failures_in_a_row: u32,
}

/// The phase of a manifest whose status says nothing about one: the Portal has not reconciled
/// it yet, which is what `Pending` means.
fn pending() -> Phase {
    Phase::Pending
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceKey {
    pub namespace: String,
    pub kind: String,
    pub name: String,
}

/// Kinds the specification defines and the Portal already serves, but jc-core does not
/// implement yet: Entity seeds (Architecture/06 section 3). Subscription left with
/// jc-core-v0.7.30 (T-0913), Dashboard
/// and Layer left with jc-core-v0.7.7 (T-0528). Paths follow Architecture/06. Blueprint left
/// this list when jc-core-v0.4.0 took the kind over and ContextSourceRegistration when
/// jc-core-v0.6.0 did, which is exactly the move the next paragraph describes. That move is not
/// free: the placeholder row served the kind at `contextsourceregistrations` and jc-core gives it
/// the plural `csrs` (MF-36), so the URL a caller uses changes with it.
///
/// They live apart from [`jc_core::KINDS`] so the difference stays visible: when @platform adds a
/// kind to jc-core, its row moves out of this list and nothing else changes.
pub const PORTAL_ONLY_KINDS: &[KindInfo] = &[KindInfo {
    kind: "Entity",
    plural: "entities",
    scope: Scope::Project,
    path_template: "projects/{project}/spaces/{space}/entities/seed/{name}.yaml",
    project_path_template: None,
    spaceless_path_template: None,
}];

/// Every kind the resource API serves: the jc-core catalogue first, the Portal-only kinds after,
/// so a plural that exists in both always resolves to the crate's row.
pub fn kinds() -> impl Iterator<Item = &'static KindInfo> {
    jc_core::KINDS.iter().chain(PORTAL_ONLY_KINDS.iter())
}

pub fn by_plural(plural: &str) -> Option<&'static KindInfo> {
    kinds().find(|info| info.plural == plural)
}

pub fn by_kind(kind: &str) -> Option<&'static KindInfo> {
    kinds().find(|info| info.kind == kind)
}

/// Whether a manifest belongs to the organization rather than to one project (MF-06, PF-68).
///
/// A kind that lives in one place is answered by its scope alone. `Role` lives in either, so it
/// is answered by the namespace the manifest carries: that is what keeps an imported
/// organization role in `org` and a project's own role in its project (T-0820, T-0872).
pub fn belongs_to_the_organization(kind: &str, namespace: Option<&str>) -> bool {
    by_kind(kind).is_some_and(|info| match info.scope {
        Scope::Organization => true,
        Scope::Project => false,
        Scope::OrganizationOrProject => {
            namespace.is_none_or(|ns| ns.is_empty() || ns == crate::permissions::ORG_NAMESPACE)
        }
    })
}

/// The namespaces a kind's manifests live in for a call made inside `project`, nearest first.
///
/// One namespace for every kind that lives in one place. `Role` answers two, the project's own
/// and the organization's, because both are in force inside a project and a caller asking for
/// "the roles" there means both (PF-68, PF-69).
pub fn homes(info: &KindInfo, project: &str) -> Vec<String> {
    let org = crate::permissions::ORG_NAMESPACE.to_owned();
    match info.scope {
        Scope::Organization => vec![org],
        Scope::Project => vec![project.to_owned()],
        Scope::OrganizationOrProject if project == org => vec![org],
        Scope::OrganizationOrProject => vec![project.to_owned(), org],
    }
}

/// Where one manifest of this kind is written, for a call made inside `project`: the first of
/// [`homes`], which is the project's own copy for a kind that may live in either.
pub fn home(info: &KindInfo, project: &str) -> String {
    homes(info, project).remove(0)
}

/// [`KindInfo::repo_path`] with one refusal on top: a placeholder must never be left behind, since
/// a manifest written to `.../spaces/{space}/...` would be unreachable for the reconciler and
/// invisible in Gitea.
pub fn repository_path(
    info: &KindInfo,
    project: &str,
    space: Option<&str>,
    name: &str,
) -> Result<String, String> {
    if info.path_template.contains("{space}") && space.is_none_or(str::is_empty) {
        return Err(format!(
            "space is required for kind '{}' ({})",
            info.kind, info.path_template
        ));
    }
    Ok(info.repo_path(project, space.unwrap_or_default(), name))
}

/// The target Endpoint URNs a Pipeline's spec names, in either shape (PL-54): `targetEndpoint`,
/// or every `outputs[].targetEndpoint`.
pub fn pipeline_targets(spec: &serde_json::Value) -> Vec<&str> {
    match spec
        .get("targetEndpoint")
        .and_then(serde_json::Value::as_str)
    {
        Some(target) => vec![target],
        None => spec
            .get("outputs")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|output| output.get("targetEndpoint")?.as_str())
            .collect(),
    }
}

/// The name a reference field holds, written either way a manifest may write it: `"transport"` or
/// `{ kind: ContextSpace, name: transport }` (T-1483).
pub fn reference_name(reference: &serde_json::Value) -> Option<&str> {
    reference
        .as_str()
        .or_else(|| reference.get("name").and_then(serde_json::Value::as_str))
}

/// The namespace and name of the DataModel a ContextSpace's `dataModelRef` names: its own
/// `namespace` (an organization model, `org`), else the space's `project` (JC1-53).
pub fn model_reference<'a>(
    reference: &'a serde_json::Value,
    project: &'a str,
) -> Option<(&'a str, &'a str)> {
    let namespace = reference
        .get("namespace")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(project);
    Some((namespace, reference_name(reference)?))
}

/// A title written either way a manifest may write it: one string, or a text per language,
/// English first, else the first language's (F-R2POR1-016). A blank title is none.
pub fn title_text(title: &serde_json::Value) -> Option<String> {
    let text = match title {
        serde_json::Value::Object(texts) => texts
            .get("en")
            .and_then(serde_json::Value::as_str)
            .or_else(|| texts.values().find_map(serde_json::Value::as_str)),
        other => other.as_str(),
    }?;
    (!text.trim().is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_plain_string_or_the_english_one() {
        use serde_json::json;
        assert_eq!(title_text(&json!("Lab only")), Some("Lab only".to_owned()));
        assert_eq!(
            title_text(&json!({ "fi": "Vain", "en": "Only" })),
            Some("Only".to_owned())
        );
        assert_eq!(
            title_text(&json!({ "fi": "Vain" })),
            Some("Vain".to_owned())
        );
        assert_eq!(title_text(&json!("  ")), None);
        assert_eq!(title_text(&serde_json::Value::Null), None);
    }

    #[test]
    fn every_entry_round_trips_plural_kind_plural() {
        for entry in kinds() {
            let by_p = by_plural(entry.plural).expect("find by plural");
            // Every kind has a plural of its own (R19, T-2954), so each comes back as itself.
            assert_eq!(by_p.kind, entry.kind);
            let by_k = by_kind(entry.kind).expect("find by kind");
            assert_eq!(by_k.kind, entry.kind);
            assert_eq!(by_k.plural, entry.plural);
        }
    }

    #[test]
    fn every_kind_the_portal_serves_is_jc_core_or_declared_portal_only() {
        // No third list may creep in: a plural resolves through jc-core's catalogue or through
        // the explicit PORTAL_ONLY_KINDS rows, nothing else (T-0249).
        for info in kinds() {
            assert!(
                jc_core::KINDS.contains(info) || PORTAL_ONLY_KINDS.contains(info),
                "{} comes from neither list",
                info.kind
            );
        }
        let from_jc_core: Vec<&str> = jc_core::KINDS.iter().map(|k| k.kind).collect();
        assert_eq!(
            from_jc_core,
            vec![
                "Organization",
                "Project",
                "ContextSpace",
                "DataModel",
                "Mapping",
                "Policy",
                "ScopeDefinition",
                "Endpoint",
                // Arrived with jc-core-v0.7.12 (T-0563): the reusable subset of a model an
                // Endpoint exposes (MP-01).
                "ModelProjection",
                "SharedSpaceReference",
                "ContextSourceRegistration",
                // Arrived with jc-core-v0.7.30 (T-0913): the standing query a space declares,
                // reconciled into its broker (CC-72, DS-16).
                "Subscription",
                "ServiceAccount",
                "Pipeline",
                "DataSource",
                "App",
                "CkanInstance",
                // Arrived with jc-core-v0.7.75 (T-3051): the knowledge assistant's sources and
                // deployments (MF-51, MF-52); the Portal administers them (T-3057).
                "KnowledgeSource",
                "AssistantDeployment",
                "McpServer",
                // One MQTT broker a project's devices publish to (MF-54, ADR-N-048, T-3994).
                "MqttBroker",
                "Blueprint",
                // Arrived with jc-core-v0.7.8 (T-0537, T-0540): the builder profile every
                // agent run loads.
                "AgentProfile",
                "DataSpaceParticipant",
                "DataOffer",
                "DataAgreement",
                "SyncSource",
                "Bundle",
                // Arrived with jc-core-v0.7.0 (T-0451). The generic resource routes serve it
                // the moment it is in this catalogue, which is what the catalogue is for; what
                // the Portal does with `portal/forms/*.uischema.yaml` as forms is T-0452.
                "UiSchema",
                "Role",
                "RoleBinding",
                // Arrived with jc-core-v0.7.21 (T-0865): a named set of people a RoleBinding
                // may name instead of one user (PF-62).
                "Group",
                // Arrived with T-5952: who may set up AI connectors above the ceiling (AG-150).
                "DelegationPolicy",
                // One broker instance a space lives on (PF-110, ADR-N-047).
                "ContextBroker",
                // Arrived with jc-core-v0.7.24 (T-0882): the environment overlay the loader
                // merges before validation, one file per environment (CC-73).
                "Environment",
                "Dashboard",
                "Layer",
            ],
            "jc-core's catalogue changed: check PORTAL_ONLY_KINDS and the UI navigation"
        );
    }

    #[test]
    fn portal_only_kinds_never_shadow_a_jc_core_kind() {
        for extra in PORTAL_ONLY_KINDS {
            assert!(
                !jc_core::KINDS.iter().any(|k| k.kind == extra.kind),
                "{} is in jc-core now: move its row out of PORTAL_ONLY_KINDS",
                extra.kind
            );
            if let Some(clash) = jc_core::KINDS.iter().find(|k| k.plural == extra.plural) {
                panic!(
                    "portal-only {} claims the plural '{}' that jc-core gives {}",
                    extra.kind, extra.plural, clash.kind
                );
            }
        }
    }

    #[test]
    fn a_rendered_path_never_keeps_a_placeholder() {
        for info in kinds() {
            let path = repository_path(info, "ovzdusie", Some("ovzdusie"), "demo")
                .unwrap_or_else(|e| panic!("{}: {e}", info.kind));
            assert!(
                !path.contains('{') && !path.contains('}'),
                "{} rendered to {path}",
                info.kind
            );
        }
    }

    #[test]
    fn spaces_maps_to_context_space() {
        let info = by_plural("spaces").expect("find spaces");
        assert_eq!(info.kind, "ContextSpace");
    }

    #[test]
    fn demo_endpoint_path_is_expected() {
        let info = by_kind("Endpoint").expect("find Endpoint");
        let path = repository_path(info, "ovzdusie", Some("ovzdusie"), "public-air")
            .expect("endpoint path");
        assert_eq!(
            path,
            "projects/ovzdusie/spaces/ovzdusie/endpoints/public-air.yaml"
        );
    }

    #[test]
    fn space_scoped_kind_without_space_errors() {
        let info = by_kind("Endpoint").expect("find Endpoint");
        assert!(repository_path(info, "ovzdusie", None, "public-air").is_err());
    }

    #[test]
    fn dns1123_validation_rules() {
        assert!(is_dns1123("public-air"));
        assert!(is_dns1123("a"));
        assert!(is_dns1123("0"));
        assert!(is_dns1123("air-quality-01"));

        assert!(!is_dns1123("Public_Air"));
        assert!(!is_dns1123("-x"));
        assert!(!is_dns1123("x-"));
        assert!(!is_dns1123(""));
        let name_64 = "a".repeat(64);
        assert!(!is_dns1123(&name_64));
        let name_63 = "a".repeat(63);
        assert!(is_dns1123(&name_63));
    }

    #[test]
    fn object_meta_validate() {
        let valid = ObjectMeta {
            name: "public-air".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        };
        assert!(validate_meta(&valid).is_ok());

        let missing_ns = ObjectMeta {
            name: "public-air".into(),
            namespace: None,
            ..Default::default()
        };
        assert!(validate_meta(&missing_ns).is_err());

        let empty_ns = ObjectMeta {
            name: "public-air".into(),
            namespace: Some("".into()),
            ..Default::default()
        };
        assert!(validate_meta(&empty_ns).is_err());

        let bad_name = ObjectMeta {
            name: "Public_Air".into(),
            namespace: Some("ovzdusie".into()),
            ..Default::default()
        };
        assert!(validate_meta(&bad_name).is_err());
    }
}
