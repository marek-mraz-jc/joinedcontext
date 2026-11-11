//! The one judge of a manifest in a project (P-2, P-15, CHG-03): the checks a Portal proposal
//! meets, run again at approval, at the reconciler's load of `main` and by
//! `joinedcontext-portal validate --as-portal`, so a file that reaches `main` by any door (a
//! seed Job, an import, a merge on the forge) meets the same refusal in the same words.
//!
//! What stays at the propose door alone needs the proposer or the forge: who may write the
//! kind (`groups::for_proposal`), the build lane's status write, a catalogue contact that is
//! the proposer's own address, the MQTT rights the proposer holds, and the two checks that read
//! a data model's LinkML source from the forge (`check_surfaces`, `check_projection`).

use crate::api::mutate;
use crate::error::ApiError;
use crate::resource::ResourceEnvelope;
use crate::store::Mirror;
use jc_core::kinds::OrganizationBounds;
use serde_json::Value;

/// What a manifest is judged against: the project as it stands without it, and the
/// deployment's settings the checks read.
pub struct Ground<'a> {
    /// Every other manifest of the installation, as it would stand.
    pub mirror: &'a Mirror,
    /// The operator's bounds on an Organization's limits (PF-97).
    pub bounds: &'a OrganizationBounds,
    /// The zone the installation's own App hosts sit under (AP-133), when the Portal serves Apps.
    pub apex: Option<&'a str>,
    /// The deployment's organization domain, when no Organization manifest names one (PF-121).
    pub domain: Option<&'a str>,
}

impl<'a> Ground<'a> {
    /// The Portal's own ground: its mirror of `main` and its configuration.
    pub fn of(state: &'a crate::state::AppState) -> Self {
        let settings = state.config.app_settings.as_ref();
        Ground {
            mirror: &state.mirror,
            bounds: &state.config.organization_bounds,
            apex: settings.map(|s| s.apex.as_str()),
            domain: settings.map(|s| s.org_domain.as_str()),
        }
    }

    /// The organization's domain Policy URNs are bound on, empty when none is known.
    pub fn org_domain(&self) -> String {
        crate::api::assistant::org_domain_in(self.mirror, self.domain, "")
    }
}

type Check = fn(&Ground<'_>, &str, &ResourceEnvelope) -> Result<(), ApiError>;

/// Every check, by name, in the order a proposal meets them. A deletion is never judged here:
/// the propose door checks what a deletion leaves behind itself.
pub const CHECKS: [(&str, Check); 17] = [
    ("literal-secret", literal_secret),
    ("kind", kind),
    ("endpoint-representations", endpoint_representations),
    ("spec", spec),
    ("organization-bounds", organization_bounds),
    ("public-app", public_app),
    ("app-hostnames", app_hostnames),
    ("service-account-client-id", service_account_client_id),
    ("agent-operations", agent_operations),
    ("endpoint-policy-refs", endpoint_policy_refs),
    ("pipeline-replace", pipeline_replace),
    ("one-data-model", one_data_model),
    ("unmade-writes", unmade_writes),
    ("public-dashboard", public_dashboard),
    ("quota", quota),
    ("mqtt-allowance", mqtt_allowance),
    ("catalogue-publication", catalogue_publication),
];

/// The first refusal `envelope`, written into `project`, meets.
pub fn manifest(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    CHECKS
        .iter()
        .try_for_each(|(_, check)| check(ground, project, envelope))
}

/// What approval does not judge again, each refused before and after it: the spec's own parse
/// (`spec`) reads the same bytes the propose door read, a write nobody can make
/// (`unmade-writes`, PL-111) and a second model of a space (`one-data-model`, DM-61) are
/// properties of the whole project once the change merged, read beside Endpoint and model
/// rights the approval checks itself (PL-94, DM-77). The propose door refuses them before the
/// Change exists and the reconciler's load refuses them on `main`, whoever pushed to the branch
/// between, so no path serves them.
pub const NOT_AT_APPROVAL: [&str; 3] = ["spec", "unmade-writes", "one-data-model"];

/// [`manifest`] at approval: every check but [`NOT_AT_APPROVAL`].
pub fn manifest_at_approval(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    CHECKS
        .iter()
        .filter(|(name, _)| !NOT_AT_APPROVAL.contains(name))
        .try_for_each(|(_, check)| check(ground, project, envelope))
}

/// The words of a refusal, without the HTTP status: what the reconciler and the offline
/// validation show.
pub fn reason(err: &ApiError) -> String {
    match err {
        ApiError::BadRequest(detail) | ApiError::Denied(detail) | ApiError::Conflict(detail) => {
            detail.clone()
        }
        ApiError::Invalid { detail, .. } | ApiError::Unprocessable { detail, .. } => detail.clone(),
        other => other.to_string(),
    }
}

/// Judges `envelopes` as if each were proposed, in turn, onto `base`: the organization's
/// manifests first, then each project's, a project's in the order a person would propose them
/// (spaces and models before what reads them), with the others of the same project standing as
/// one Change (`references::BUNDLE`). What passes joins `base`, so a second ServiceAccount
/// deriving one client id, or an App past the quota, is refused as its proposal would be; in
/// `base` it keeps the status the one it replaces had, which the checks read (a verified
/// domain). `envelopes` carry no status: a manifest never does (MF-04). Returns each refused
/// manifest with its reason.
pub fn replay(
    base: &Mirror,
    settings: (&OrganizationBounds, Option<&str>, Option<&str>),
    mut envelopes: Vec<ResourceEnvelope>,
) -> Vec<(ResourceEnvelope, String)> {
    let (bounds, apex, domain) = settings;
    let project_of = |e: &ResourceEnvelope| e.metadata.namespace.clone().unwrap_or_default();
    envelopes.sort_by(|a, b| {
        let org = |e: &ResourceEnvelope| project_of(e) != crate::permissions::ORG_NAMESPACE;
        (org(a), project_of(a), rank(&a.kind), &a.metadata.name).cmp(&(
            org(b),
            project_of(b),
            rank(&b.kind),
            &b.metadata.name,
        ))
    });
    let mut refused = Vec::new();
    for project in envelopes.chunk_by(|a, b| project_of(a) == project_of(b)) {
        let namespace = project_of(&project[0]);
        let bundle: Vec<Value> = project
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        crate::references::BUNDLE.sync_scope(bundle, || {
            for envelope in project {
                let ground = Ground {
                    mirror: base,
                    bounds,
                    apex,
                    domain,
                };
                match manifest(&ground, &namespace, envelope) {
                    Ok(()) => {
                        let mut accepted = envelope.clone();
                        accepted.status = base
                            .get(&namespace, &envelope.kind, &envelope.metadata.name)
                            .and_then(|held| held.status);
                        base.upsert(accepted);
                    }
                    Err(err) => refused.push((envelope.clone(), reason(&err))),
                }
            }
        });
    }
    refused
}

/// The order a person proposes a project's kinds in: what is named before what names it.
fn rank(kind: &str) -> usize {
    const FIRST: [&str; 6] = [
        "Organization",
        "Project",
        "ContextSpace",
        "DataModel",
        "Policy",
        "Endpoint",
    ];
    FIRST.iter().position(|k| *k == kind).unwrap_or(FIRST.len())
}

/// A literal secret anywhere in the manifest (MF-24): a credential goes through `secretRef`.
fn literal_secret(_: &Ground<'_>, _: &str, envelope: &ResourceEnvelope) -> Result<(), ApiError> {
    let value = serde_json::to_value(envelope).map_err(|e| ApiError::Internal(e.to_string()))?;
    match mutate::find_literal_secret(&value) {
        Some(key) => Err(ApiError::BadRequest(format!(
            "literal secret in field {key} is forbidden; use secretRef instead (MF-24)"
        ))),
        None => Ok(()),
    }
}

/// The kind is one the platform defines (CC-72, T-0833).
pub(crate) fn kind(_: &Ground<'_>, _: &str, envelope: &ResourceEnvelope) -> Result<(), ApiError> {
    let kind = &envelope.kind;
    match jc_core::registry::by_kind(kind) {
        Some(_) => Ok(()),
        None => Err(ApiError::BadRequest(format!(
            "kind '{kind}' is declared but not defined by the platform yet, and a repository \
             holding one stops loading for every component; it cannot be written (CC-72, T-0833)"
        ))),
    }
}

/// The spec parses with the kind's invariants (T-0412, CC-08), after the representation
/// outputs, which name the member a form shows the finding beside (EP-159).
pub(crate) fn spec(_: &Ground<'_>, _: &str, envelope: &ResourceEnvelope) -> Result<(), ApiError> {
    let kind = &envelope.kind;
    let yaml = serde_json::to_string(envelope).map_err(|e| ApiError::Internal(e.to_string()))?;
    match jc_core::registry::validate_yaml(kind, &yaml) {
        Some(Err(e)) => Err(ApiError::BadRequest(format!(
            "spec is not a valid {kind}: {e}"
        ))),
        _ => Ok(()),
    }
}

/// An Endpoint's representation outputs, member by member (API/11 EP-159).
fn endpoint_representations(
    _: &Ground<'_>,
    _: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    match envelope.kind.as_str() {
        "Endpoint" => mutate::check_representation_shape(&envelope.spec),
        _ => Ok(()),
    }
}

/// An Organization's limits inside the operator's bounds (PF-97).
fn organization_bounds(
    ground: &Ground<'_>,
    _: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    mutate::organization_within_bounds(
        ground.bounds,
        &envelope.kind,
        &envelope.spec,
        ApiError::BadRequest,
    )
}

/// A new public App while the organization refuses public Apps (PF-103).
fn public_app(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    mutate::public_app_allowed(ground, project, envelope, ApiError::BadRequest)
}

/// An App's hostnames inside the organization's verified domain, and no other App's (AP-172).
fn app_hostnames(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    if envelope.kind != "App" {
        return Ok(());
    }
    crate::apps::hostnames::check(
        ground.mirror,
        project,
        &envelope.metadata.name,
        &envelope.spec,
        ground.apex,
    )
    .map_err(ApiError::BadRequest)
}

/// A ServiceAccount whose derived Keycloak client id another project's account derives (T-1456).
/// The other account is not named: it may live in a project the author cannot read (PF-59).
fn service_account_client_id(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    if envelope.kind != "ServiceAccount" {
        return Ok(());
    }
    let name = &envelope.metadata.name;
    match mutate::service_account_id_taken(ground.mirror, project, name) {
        Some(id) => Err(ApiError::BadRequest(format!(
            "metadata.name '{name}' gives the Keycloak client id '{id}', which another \
             ServiceAccount already derives; choose another name (T-1456)"
        ))),
        None => Ok(()),
    }
}

/// An agent profile naming an operation this Portal does not register (MF-40).
fn agent_operations(_: &Ground<'_>, _: &str, envelope: &ResourceEnvelope) -> Result<(), ApiError> {
    if envelope.kind != "AgentProfile" {
        return Ok(());
    }
    match mutate::unknown_operation(&envelope.spec) {
        Some(name) => Err(ApiError::BadRequest(format!(
            "spec.access.operations names '{name}', which is not a registered operation (MF-40)"
        ))),
        None => Ok(()),
    }
}

/// An Endpoint's Policy references bind a Policy of its space (EP-117, T-6435).
fn endpoint_policy_refs(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    match envelope.kind.as_str() {
        "Endpoint" => mutate::check_policy_refs(ground, project, envelope),
        _ => Ok(()),
    }
}

/// A replace output meeting another Pipeline's writes of the same ids (PL-79, T-4409).
fn pipeline_replace(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    match envelope.kind.as_str() {
        "Pipeline" => {
            mutate::check_replace(ground, project, &envelope.metadata.name, &envelope.spec)
        }
        _ => Ok(()),
    }
}

/// A second data model of one space (DM-61, T-6394).
fn one_data_model(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    match envelope.kind.as_str() {
        "DataModel" => {
            mutate::check_one_model(ground, project, &envelope.metadata.name, &envelope.spec)
        }
        _ => Ok(()),
    }
}

/// A write nobody can make (PL-111, ADR-N-057 decision 5).
fn unmade_writes(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    mutate::check_unmade_writes(ground, project, crate::change::Operation::Update, envelope)
}

/// A public dashboard reading through a non-public Endpoint (UI-19, T-0528).
fn public_dashboard(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    crate::dashboards::check(
        ground.mirror,
        project,
        &envelope.kind,
        &envelope.metadata.name,
        &envelope.spec,
    )
}

/// What the project may hold (PF-73, PF-74).
fn quota(ground: &Ground<'_>, project: &str, envelope: &ResourceEnvelope) -> Result<(), ApiError> {
    crate::quotas::check(
        ground.mirror,
        project,
        &envelope.kind,
        &envelope.metadata.name,
        &envelope.spec,
        ground.bounds,
    )
}

/// A broker inside what the project's MQTT allowance leaves (PF-101, PF-117).
fn mqtt_allowance(
    ground: &Ground<'_>,
    project: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    match envelope.kind.as_str() {
        "MqttBroker" => crate::mqtt_allowance::check(
            ground.mirror,
            project,
            &envelope.metadata.name,
            &envelope.spec,
        ),
        _ => Ok(()),
    }
}

/// A CKAN publication of an Endpoint that is not public (EP-62).
fn catalogue_publication(
    _: &Ground<'_>,
    _: &str,
    envelope: &ResourceEnvelope,
) -> Result<(), ApiError> {
    crate::catalog::check_publication(&envelope.kind, &envelope.spec)
}

/// `joinedcontext-portal validate --as-portal` (P-15): what `jcctl validate` finds in an
/// organization checkout and its projects, and what this judge refuses of it, so a repository
/// can be judged offline exactly as this Portal's load would judge it on a first run. Each
/// manifest is judged as its proposal onto the ones before it ([`replay`]); a hostname needs a
/// verified domain, which no checkout holds, so an App listing one is refused here as a
/// proposal of it is before the TXT record is published.
pub fn validate(
    org_dir: &std::path::Path,
    projects: std::collections::BTreeMap<String, std::path::PathBuf>,
    settings: (&OrganizationBounds, Option<&str>, Option<&str>),
) -> jcctl::commands::validate::Report {
    use jcctl::commands::validate::Finding;
    let judge = |repository: &jcctl::loader::Repository| {
        let mut located = std::collections::BTreeMap::new();
        let mut envelopes = Vec::new();
        for (_, resource) in repository.iter() {
            let Ok(mut envelope) = serde_json::to_value(&resource.manifest)
                .and_then(serde_json::from_value::<ResourceEnvelope>)
            else {
                continue;
            };
            if envelope
                .metadata
                .namespace
                .as_deref()
                .is_none_or(str::is_empty)
            {
                envelope.metadata.namespace = Some(crate::reconciler::staging::namespace_of(
                    &resource.path.to_string_lossy(),
                ));
            }
            located.insert(key(&envelope), resource);
            envelopes.push(envelope);
        }
        replay(&Mirror::new(), settings, envelopes)
            .into_iter()
            .filter_map(|(envelope, reason)| {
                let resource = located.get(&key(&envelope))?;
                Some(Finding {
                    path: resource.path.clone(),
                    document: resource.document,
                    line: resource.line,
                    message: format!("the Portal refuses it: {reason}"),
                })
            })
            .collect()
    };
    jcctl::commands::validate::run_assembled_judged(
        org_dir,
        &jcctl::assemble::Directories(projects),
        &judge,
    )
}

/// `validate --as-portal --repo-dir <org> [--project-dir <slug>=<dir>]... [--apex <zone>]` of the
/// `joinedcontext-portal` and `jc` binaries, its exit code: 0 valid, 1 a finding, 2 a usage or
/// configuration error. The operator's bounds come from `JC_PORTAL_ORGANIZATION_BOUNDS_FILE`.
pub fn validate_command(args: &[String]) -> i32 {
    let usage = || {
        eprintln!(
            "usage: joinedcontext-portal validate --as-portal --repo-dir <org-dir> \
             [--project-dir <slug>=<dir>]... [--apex <zone>]"
        );
        2
    };
    let mut rest: Vec<&str> = args.iter().map(String::as_str).collect();
    let (Some(&"--as-portal"), Some(&"--repo-dir"), Some(org_dir)) =
        (rest.first(), rest.get(1), rest.get(2).copied())
    else {
        return usage();
    };
    rest.drain(..3);
    let (mut projects, mut apex) = (std::collections::BTreeMap::new(), None);
    while let [flag, value, tail @ ..] = rest.as_slice() {
        match *flag {
            "--project-dir" => match value.split_once('=') {
                Some((slug, dir)) => {
                    projects.insert(slug.to_owned(), std::path::PathBuf::from(dir));
                }
                None => return usage(),
            },
            "--apex" => apex = Some((*value).to_owned()),
            _ => return usage(),
        }
        rest = tail.to_vec();
    }
    if !rest.is_empty() {
        return usage();
    }
    let bounds = match crate::config::organization_bounds(&|key| std::env::var(key).ok()) {
        Ok(bounds) => bounds,
        Err(err) => {
            eprintln!("Configuration error: {err}");
            return 2;
        }
    };
    let report = validate(
        std::path::Path::new(org_dir),
        projects,
        (&bounds, apex.as_deref(), None),
    );
    for finding in &report.findings {
        eprintln!("{finding}");
    }
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    if report.is_valid() {
        println!("{} manifests valid", report.checked);
        0
    } else {
        eprintln!(
            "{} manifests valid, {} invalid",
            report.checked,
            report.findings.len()
        );
        1
    }
}

fn key(envelope: &ResourceEnvelope) -> crate::resource::ResourceKey {
    crate::resource::ResourceKey {
        namespace: envelope.metadata.namespace.clone().unwrap_or_default(),
        kind: envelope.kind.clone(),
        name: envelope.metadata.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::ORG_NAMESPACE;
    use crate::resource::ObjectMeta;
    use serde_json::json;

    fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: crate::resource::API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some(namespace.into()),
                ..Default::default()
            },
            spec,
            status: None,
        }
    }

    fn account() -> Value {
        json!({
            "owner": { "user": "jana.k@bb" }, "purpose": "pushes ParkingSpot updates",
            "roles": [{ "role": "space-writer", "scope": { "contextSpace": "ovzdusie" },
                        "types": ["ParkingSpot"], "operations": ["createEntity"] }],
            "credentials": [{ "kind": "oauth-client", "name": "main" }]
        })
    }

    fn space_policy(assignee: Value, operations: &[&str]) -> Value {
        json!({
            "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
            "assigner": "did:web:hel.fi",
            "assignee": assignee,
            "operations": operations,
        })
    }

    fn endpoint(extra: Value) -> Value {
        let mut spec = json!({
            "contextSpaceRef": "ovzdusie", "slug": "zt4qm7ge2xdv6ksb3ncf5arw2y",
            "audience": "public", "enabledRepresentations": ["ngsi-ld"]
        });
        for (key, value) in extra.as_object().into_iter().flatten() {
            spec[key] = value.clone();
        }
        spec
    }

    fn pipeline(mode: &str) -> Value {
        json!({
            "class": "scheduled", "schedule": "0 * * * *",
            "source": { "dataSourceRef": "feed" },
            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:ovzdusie:public-air",
            "output": { "type": "AirQualityObserved", "mode": mode, "id": {
                "mint": "template", "template": "urn:ngsi-ld:AirQualityObserved:{station.code}"
            } }
        })
    }

    /// One manifest per check that this check refuses, with what the mirror holds beside it.
    fn fixture(check: &str) -> (Vec<ResourceEnvelope>, ResourceEnvelope) {
        let org = |spec: Value| envelope("Organization", "hel", ORG_NAMESPACE, spec);
        let project = |quotas: Value| {
            envelope(
                "Project",
                "ovzdusie",
                ORG_NAMESPACE,
                json!({ "organizationRef": { "name": "hel" }, "quotas": quotas }),
            )
        };
        let air = |spec: Value| envelope("Endpoint", "public-air", "ovzdusie", spec);
        match check {
            "literal-secret" => (
                vec![],
                envelope("DataSource", "feed", "ovzdusie", json!({ "password": "p" })),
            ),
            "kind" => (vec![], envelope("NoSuchKind", "x", "ovzdusie", json!({}))),
            "spec" => (
                vec![],
                envelope("DataSource", "feed", "ovzdusie", json!({ "notAField": 1 })),
            ),
            "endpoint-representations" => (
                vec![],
                air(endpoint(
                    json!({ "representations": { "notAnOutput": {} } }),
                )),
            ),
            "organization-bounds" => (
                vec![],
                org(
                    json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
                            "projects": { "quota": { "contextSpaces": 50 } } }),
                ),
            ),
            "public-app" => (
                vec![org(
                    json!({ "domain": "hel.fi", "policies": { "apps": { "public": "refused" } } }),
                )],
                envelope("App", "map", "ovzdusie", json!({ "visibility": "public" })),
            ),
            "app-hostnames" => (
                vec![org(json!({ "domain": "hel.fi" }))],
                envelope(
                    "App",
                    "map",
                    "ovzdusie",
                    json!({ "hostnames": ["map.hel.fi"] }),
                ),
            ),
            "service-account-client-id" => (
                vec![envelope("ServiceAccount", "c", "a-b", account())],
                envelope("ServiceAccount", "b-c", "a", account()),
            ),
            "agent-operations" => (
                vec![],
                envelope(
                    "AgentProfile",
                    "helper",
                    "ovzdusie",
                    json!({ "access": { "operations": ["no.such.operation"] } }),
                ),
            ),
            "endpoint-policy-refs" => (
                vec![org(json!({ "domain": "hel.fi", "installation": true }))],
                air(endpoint(
                    json!({ "policyRef": "urn:ngsi-ld:Policy:hel.fi:ovzdusie-ovzdusie:nobody" }),
                )),
            ),
            "pipeline-replace" => (
                vec![
                    air(endpoint(json!({}))),
                    envelope("Pipeline", "enrich", "ovzdusie", pipeline("upsert")),
                ],
                envelope("Pipeline", "stations", "ovzdusie", pipeline("replace")),
            ),
            "one-data-model" => {
                let model = |space: &str| {
                    json!({ "contextSpaceRef": space, "linkml": "./m.linkml.yaml",
                            "version": "0.1.0", "lifecycle": "draft" })
                };
                (
                    vec![envelope("DataModel", "walk", "ovzdusie", model("ovzdusie"))],
                    envelope("DataModel", "second", "ovzdusie", model("ovzdusie")),
                )
            }
            "unmade-writes" => (
                vec![
                    air(endpoint(json!({
                        "policyRef": "urn:ngsi-ld:Policy:hel.fi:ovzdusie-ovzdusie:public-read"
                    }))),
                    envelope(
                        "Policy",
                        "public-read",
                        "ovzdusie",
                        space_policy(json!({ "kind": "role", "id": "public" }), &["retrieveOps"]),
                    ),
                ],
                envelope(
                    "Policy",
                    "sensors-write",
                    "ovzdusie",
                    space_policy(
                        json!({ "kind": "serviceAccount", "id": "sensors" }),
                        &["upsertBatch"],
                    ),
                ),
            ),
            "public-dashboard" => (
                vec![],
                envelope(
                    "Dashboard",
                    "air",
                    "ovzdusie",
                    json!({ "visibility": "public", "pages": [{ "layers": ["missing"] }] }),
                ),
            ),
            "quota" => {
                let resident = |name: &str| {
                    envelope(
                        "Pipeline",
                        name,
                        "ovzdusie",
                        json!({
                            "class": "resident",
                            "source": { "dataSourceRef": { "kind": "DataSource", "name": "mqtt" } },
                            "compute": { "kind": "bloblang", "bloblang": "root = this" },
                            "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:ovzdusie:air"
                        }),
                    )
                };
                (
                    vec![
                        org(json!({ "domain": "hel.fi",
                                    "projects": { "quota": { "residentPipelines": 1 } } })),
                        project(json!({})),
                        envelope(
                            "DataSource",
                            "mqtt",
                            "ovzdusie",
                            json!({ "type": "mqtt", "mqtt": {
                                "urls": ["tcp://mqtt.hel.fi:1883"], "topics": ["air/#"] } }),
                        ),
                        resident("one"),
                    ],
                    resident("two"),
                )
            }
            "mqtt-allowance" => (
                vec![
                    org(json!({ "domain": "hel.fi", "limits": { "mqtt": { "enabled": false } } })),
                    project(json!({})),
                ],
                envelope(
                    "MqttBroker",
                    "cams",
                    "ovzdusie",
                    crate::mqtt_allowance::tests::broker(10),
                ),
            ),
            "catalogue-publication" => (
                vec![],
                air(endpoint(
                    json!({ "audience": "organization", "publish": { "ckan": { "instanceRef": "opendata" } } }),
                )),
            ),
            other => panic!("no fixture for check {other}"),
        }
    }

    /// P-2: every check is registered in the one judge, and each refuses its fixture through
    /// [`manifest`], the function propose, approve, load and `validate --as-portal` call.
    #[test]
    fn every_check_is_in_the_judge_and_refuses_its_fixture() {
        let bounds: OrganizationBounds = serde_json::from_value(json!({
            "spec.projects.quota.contextSpaces": { "max": 20 }
        }))
        .expect("bounds");
        for (name, check) in CHECKS {
            let (held, proposed) = fixture(name);
            let mirror = Mirror::new();
            held.into_iter().for_each(|e| mirror.upsert(e));
            let ground = Ground {
                mirror: &mirror,
                bounds: &bounds,
                apex: None,
                domain: None,
            };
            let project = proposed.metadata.namespace.clone().unwrap_or_default();
            let alone = check(&ground, &project, &proposed)
                .expect_err(&format!("{name} refuses its fixture"));
            manifest(&ground, &project, &proposed)
                .expect_err(&format!("the judge refuses the {name} fixture"));
            assert!(!reason(&alone).is_empty(), "{name}");
        }
        let names: std::collections::BTreeSet<&str> = CHECKS.iter().map(|(n, _)| *n).collect();
        assert_eq!(names.len(), CHECKS.len(), "one entry per check");
    }

    /// P-19 (T-6713): one credential typed under a name that does not say so (an API key in a
    /// DataSource's query) is refused at propose, at approval and at load, each naming the
    /// field path and the rule and never the value.
    #[test]
    fn a_credential_shaped_value_is_refused_at_every_door() {
        let token = format!("ghp_{}", "a1B2c3D4".repeat(5));
        let feed = envelope(
            "DataSource",
            "feed",
            "ovzdusie",
            json!({ "input": { "query": token.clone() } }),
        );
        let bounds = OrganizationBounds::default();
        let mirror = Mirror::new();
        let ground = Ground {
            mirror: &mirror,
            bounds: &bounds,
            apex: None,
            domain: None,
        };
        let said = |refusal: &str| {
            assert!(
                refusal.contains("'spec.input.query' (a credential-shaped value)")
                    && !refusal.contains(&token),
                "{refusal}"
            );
        };
        said(&reason(
            &manifest(&ground, "ovzdusie", &feed).expect_err("propose refuses it"),
        ));
        said(&reason(
            &manifest_at_approval(&ground, "ovzdusie", &feed).expect_err("approval refuses it"),
        ));
        let refused = replay(&Mirror::new(), (&bounds, None, None), vec![feed]);
        assert_eq!(refused.len(), 1, "load refuses it: {refused:?}");
        said(&refused[0].1);
    }

    /// P-2: what reaches `main` past the propose door is judged as its proposals would be: of
    /// two ServiceAccounts deriving one Keycloak client id the second is refused, and a public
    /// App the organization refuses never passes, while one already public stays.
    #[test]
    fn a_replay_refuses_what_a_proposal_of_it_would() {
        let bounds = OrganizationBounds::default();
        let settings = (&bounds, None, None);
        let refused = replay(
            &Mirror::new(),
            settings,
            vec![
                envelope("ServiceAccount", "c", "a-b", account()),
                envelope("ServiceAccount", "b-c", "a", account()),
            ],
        );
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].0.metadata.name, "c", "the first in order stays");
        assert!(refused[0].1.contains("a-b-c"), "{}", refused[0].1);

        let org = envelope(
            "Organization",
            "hel",
            ORG_NAMESPACE,
            json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en",
                    "policies": { "apps": { "public": "refused" } } }),
        );
        let public = envelope(
            "App",
            "map",
            "ovzdusie",
            json!({
                "kind": "static", "source": { "path": "." }, "build": { "node": "22" },
                "visibility": "public", "lifecycle": "draft",
                "dataNeeds": [{ "contextSpaceRef": { "kind": "ContextSpace", "name": "ovzdusie" },
                                "types": ["AirQualityObserved"], "operations": ["queryEntity"] }]
            }),
        );
        let refused = replay(&Mirror::new(), settings, vec![public.clone(), org.clone()]);
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert!(refused[0].1.contains("PF-103"), "{}", refused[0].1);

        let serving = Mirror::new();
        serving.upsert(public.clone());
        assert!(replay(&serving, settings, vec![org, public]).is_empty());
    }

    /// P-15: `validate --as-portal` finds in a checkout what the Portal would refuse at load,
    /// beside what `jcctl validate` finds, naming the file.
    #[test]
    fn an_offline_validation_refuses_what_the_portal_would() {
        let dir = std::env::temp_dir().join(format!("jc-judge-{}", std::process::id()));
        let file = dir.join("projects/city/spaces/air/endpoints/air.yaml");
        std::fs::create_dir_all(file.parent().expect("a parent")).expect("the tree");
        let write = |audience: &str| {
            let manifest = json!({
                "apiVersion": crate::resource::API_VERSION,
                "kind": "Endpoint",
                "metadata": { "name": "air", "namespace": "city" },
                "spec": {
                    "contextSpaceRef": "air", "slug": "dddddddddddddddddddddddddd",
                    "audience": audience, "enabledRepresentations": ["ngsi-ld"],
                    "publish": { "ckan": { "instanceRef": "opendata" } }
                }
            });
            std::fs::write(&file, manifest.to_string()).expect("the manifest");
        };
        let bounds = OrganizationBounds::default();
        let settings = (&bounds, None, None);

        write("organization");
        let report = validate(&dir, Default::default(), settings);
        let found: Vec<String> = report.findings.iter().map(ToString::to_string).collect();
        assert!(
            found.iter().any(
                |f| f.starts_with("projects/city/spaces/air/endpoints/air.yaml")
                    && f.contains("the Portal refuses it")
                    && f.contains("EP-62")
            ),
            "{found:?}"
        );

        write("public");
        let report = validate(&dir, Default::default(), settings);
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.message.contains("the Portal refuses it")),
            "{:?}",
            report.findings
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
