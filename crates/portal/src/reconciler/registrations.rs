//! What a `ContextSourceRegistration` manifest declares, written into the hub space's tenant
//! (T-0345, MF-36, PF-48, SP-08, SP-09).
//!
//! A hub is a Context Space that holds registrations and one Endpoint; the reads over it are
//! CIM 009 clause 4.3.6 distributed operations the broker performs by itself. Nothing of that
//! is here. What is here is the projection the reconciler owes the broker: one manifest, one
//! `csourceRegistration`, in the tenant of the space the manifest names and no other.
//!
//! Three rules, and they are the reason this wave does not go through the space surface like
//! `Subscription` next door:
//!
//! - a registration is a control-plane act, not a data write. Registering a member is what
//!   exposes it to the hub's audience (PF-48), which is why the manifest is Red-lane and why
//!   there is no gateway operation a Policy could ever grant for it;
//! - the tenant travels as the CIM 009 header on this internal hop and on no other (SP-08,
//!   SP-09): no client sends it and no client sees it;
//! - the hop carries no subject, so the broker admits it only with its admin bearer token
//!   (`JC_PORTAL_BROKER_ADMIN_TOKEN_FILE`, the one the view writer sends; Antares T-5032): a pod
//!   that merely reaches the broker port registers nothing. No credential lands in a
//!   registration: a member is read by the broker as a tenant of itself, so there is nothing
//!   to store.
//!
//! Only a registration this reconciler declared in a previous run is ever deleted, so a
//! registration somebody wrote straight into the broker is left alone.

use crate::capped::refusal;
use crate::store::Mirror;
use jc_core::kinds::{ContextSourceRegistrationSpec, FederationIdentity};
use jcctl::csr::{registration, registration_id, MemberSource};
use jcctl::loader::{RawManifest, RawMetadata};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

/// A broker call that has not answered in this long is a failed sync, not a slow one.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The tenant header of the internal hop (CIM 009 clause 6.3.5).
const TENANT_HEADER: &str = "NGSILD-Tenant";

/// What happened to one `ContextSourceRegistration` in a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The hub's tenant holds what the manifest declares.
    Written,
    /// Nothing was written and why: a member that does not resolve, a refusal from the broker,
    /// or a broker that did not answer.
    Error(String),
}

/// One HTTP client and the brokers' addresses.
pub struct RegistrationSync {
    http: reqwest::Client,
    /// The `main` broker as this Portal reaches it, without a trailing slash.
    broker: String,
    /// Every other broker's address, `{broker}` replaced by its name (PF-111).
    template: Option<String>,
    /// The Context Gateway as this Portal reaches it, without a trailing slash: where a
    /// caller-identity registration names its member Endpoint (T-3999, PF-113).
    gateway: Option<String>,
    /// The broker's admin token (`JC_PORTAL_BROKER_ADMIN_TOKEN_FILE`), read on every call and
    /// sent only as the bearer of this hop. `None` sends none, for a broker without one.
    token_file: Option<PathBuf>,
}

impl RegistrationSync {
    /// A sync against one broker.
    pub fn new(broker: impl Into<String>) -> Self {
        Self {
            http: crate::reach::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
            broker: broker.into().trim_end_matches('/').to_owned(),
            template: None,
            gateway: None,
            token_file: None,
        }
    }

    /// Sends the broker's admin token, read from `token_file` on every call, as the bearer of
    /// each registration write and delete (T-5032).
    pub fn with_admin_token(mut self, token_file: Option<PathBuf>) -> Self {
        self.token_file = token_file;
        self
    }

    /// `request` with the admin bearer, when a token file is configured. A configured file
    /// that cannot be read fails the call instead of sending it without the credential.
    fn authorized(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, String> {
        let Some(file) = &self.token_file else {
            return Ok(request);
        };
        std::fs::read_to_string(file)
            .ok()
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .map(|token| request.bearer_auth(token))
            .ok_or_else(|| "the broker's admin token is not readable".to_owned())
    }

    /// Names each member Endpoint on the gateway at `gateway` (`JC_PORTAL_GATEWAY_URL`), so a
    /// `federation.identity: caller` registration can render; without it such a registration is
    /// refused as unresolved (T-4022).
    pub fn with_gateway(mut self, gateway: Option<String>) -> Self {
        self.gateway = gateway.map(|g| g.trim_end_matches('/').to_owned());
        self
    }

    /// Reaches a space on a broker other than `main` at `template` (`JC_PORTAL_BROKER_URL_TEMPLATE`).
    pub fn with_broker_template(mut self, template: Option<String>) -> Self {
        self.template = template;
        self
    }

    /// The NGSI-LD base of the broker the space `name` of `project` lives on (PF-111), which is
    /// where its registrations are written and where it is read as a member. `None` for a
    /// broker this Portal cannot reach.
    fn ngsi_ld(&self, mirror: &Mirror, project: &str, space: &str) -> Option<String> {
        let broker = crate::spaces::broker(mirror, project, space);
        crate::spaces::broker_address(&broker, Some(&self.broker), self.template.as_deref())
            .map(|base| format!("{base}/ngsi-ld/v1"))
    }

    /// Brings every hub space's registrations to what the repository declares.
    ///
    /// Returns one outcome per declared manifest, by `(project, name)`.
    pub async fn converge(
        &self,
        mirror: &Mirror,
        previous: &Mirror,
    ) -> Vec<(String, String, RegistrationOutcome)> {
        let mut outcomes = Vec::new();
        let mut declared_ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        // The export grants this run wrote, by (broker, provider tenant, hub tenant), with the
        // view each names.
        let mut grants: BTreeMap<GrantKey, String> = BTreeMap::new();

        let (found, refused) = declared(mirror);
        // A manifest that could not be read is said to be not written, so its status does not
        // keep the `Live` the load gave it (T-2543).
        for (project, name, reason) in refused {
            outcomes.push((project, name, RegistrationOutcome::Error(reason)));
        }
        for (project, name, space, hub, manifest) in found {
            let id = registration_id(&name);
            declared_ids
                .entry(space.clone())
                .or_default()
                .insert(id.clone());
            let Some(ngsi_ld) = self.ngsi_ld(mirror, &project, &hub) else {
                let reason = format!(
                    "the hub space '{hub}' lives on a broker this Portal has no address for, so \
                     nothing is registered; check its brokerRef (PF-111)"
                );
                outcomes.push((project, name, RegistrationOutcome::Error(reason)));
                continue;
            };
            let shared = match self.shared_member(mirror, &project, &manifest, &ngsi_ld) {
                Ok(shared) => shared,
                Err(reason) => {
                    outcomes.push((project, name, RegistrationOutcome::Error(reason)));
                    continue;
                }
            };
            let body = match &shared {
                Some(shared) => {
                    let member = shared.member.clone();
                    registration(&manifest, &move |_: &str| Some(member.clone()))
                }
                None => registration(&manifest, &self.members_of(mirror, &project)),
            };
            let body = match body {
                Ok(body) => body,
                Err(err) => {
                    outcomes.push((project, name, RegistrationOutcome::Error(err.to_string())));
                    continue;
                }
            };
            // The provider space's consent goes first: the broker admits a registration naming
            // itself only under it (ADR-0025 rule 5).
            if let Some(shared) = &shared {
                let key = shared.grant_key(&space);
                if let Some(other) = grants.get(&key).filter(|view| **view != shared.slug) {
                    let reason = format!(
                        "another registration of this space already reads the provider's space \
                         through Endpoint slug {other}; one shared Endpoint per provider space and \
                         hub space"
                    );
                    outcomes.push((project, name, RegistrationOutcome::Error(reason)));
                    continue;
                }
                if let Err(reason) = self.put_grant(shared, &space).await {
                    outcomes.push((project, name, RegistrationOutcome::Error(reason)));
                    continue;
                }
                grants.insert(key, shared.slug.clone());
            }
            let outcome = match self.write(&ngsi_ld, &space, &id, &body).await {
                Ok(()) => RegistrationOutcome::Written,
                Err(reason) => RegistrationOutcome::Error(reason),
            };
            outcomes.push((project, name, outcome));
        }

        // A manifest that is gone takes its registration with it, and with it the exposure it
        // was: a member that stays registered after its manifest is deleted keeps answering
        // the hub's audience.
        for (project, name, space, hub, _) in declared(previous).0 {
            let id = registration_id(&name);
            if declared_ids
                .get(&space)
                .is_some_and(|declared| declared.contains(&id))
            {
                continue;
            }
            let Some(ngsi_ld) = self.ngsi_ld(previous, &project, &hub) else {
                tracing::warn!(%space, %id, "registration not removed: its broker has no address");
                continue;
            };
            match self.delete(&ngsi_ld, &space, &id).await {
                Ok(()) => tracing::info!(%space, %id, "registration removed with its manifest"),
                Err(reason) => tracing::warn!(%space, %id, %reason, "registration not removed"),
            }
        }

        // A shared read that is gone, or that no SharedSpaceReference covers any more, takes
        // the provider's grant with it: the broker refuses every forward of a registration
        // still stored at once (SHR-07).
        for (project, _, space, hub, manifest) in declared(previous).0 {
            let Some(ngsi_ld) = self.ngsi_ld(previous, &project, &hub) else {
                continue;
            };
            let Ok(Some(shared)) = self.shared_member(previous, &project, &manifest, &ngsi_ld)
            else {
                continue;
            };
            let key = shared.grant_key(&space);
            if grants.contains_key(&key) {
                continue;
            }
            match self.delete_grant(&shared, &space).await {
                Ok(()) => {
                    grants.insert(key, shared.slug.clone());
                    tracing::info!(%space, provider = %shared.tenant, "export grant removed with its shared read");
                }
                Err(reason) => {
                    tracing::warn!(%space, provider = %shared.tenant, %reason, "export grant not removed")
                }
            }
        }
        outcomes
    }

    /// The Endpoint another project shares with `project`, when `manifest` reads one
    /// (SHR-15 `live`, T-7422): an anonymous registration whose `endpointRef` names another
    /// project. It is read in process on this broker, through the shared Endpoint's view and
    /// decided as an anonymous caller (SHR-17), and only while `project` holds a
    /// `SharedSpaceReference` to it. `Ok(None)` for every other registration.
    fn shared_member(
        &self,
        mirror: &Mirror,
        project: &str,
        manifest: &RawManifest,
        hub_ngsi_ld: &str,
    ) -> Result<Option<SharedMember>, String> {
        let Ok(spec) =
            serde_json::from_value::<ContextSourceRegistrationSpec>(manifest.spec.clone())
        else {
            return Ok(None);
        };
        let Some(reference) = spec.endpoint_ref.as_ref() else {
            return Ok(None);
        };
        let Some(provider) = reference.namespace().filter(|ns| *ns != project) else {
            return Ok(None);
        };
        if spec.federation.identity != FederationIdentity::Anonymous {
            return Ok(None);
        }
        let name = reference.name();
        let endpoint = mirror.get(provider, "Endpoint", name).ok_or_else(|| {
            format!(
                "endpointRef names the Endpoint {provider}/{name}, which does not exist, so \
                 nothing is registered"
            )
        })?;
        let slug = endpoint
            .spec
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("the Endpoint {provider}/{name} has no slug yet, so nothing is registered")
            })?
            .to_owned();
        if !referenced(mirror, project, provider, name, &slug) {
            return Err(format!(
                "no SharedSpaceReference of project {project} names the Endpoint \
                 {provider}/{name}; a live read of another project's Endpoint runs under one \
                 (SHR-15)"
            ));
        }
        let space = endpoint
            .spec
            .get("contextSpaceRef")
            .and_then(crate::resource::reference_name)
            .ok_or_else(|| format!("the Endpoint {provider}/{name} names no Context Space"))?
            .to_owned();
        let ngsi_ld = self.ngsi_ld(mirror, provider, &space).ok_or_else(|| {
            format!(
                "the Endpoint {provider}/{name} lives on a broker this Portal has no address \
                 for, so nothing is registered (PF-111)"
            )
        })?;
        // In process means on one broker: the hub reads the provider's tenant beside its own.
        if ngsi_ld != hub_ngsi_ld {
            return Err(format!(
                "the Endpoint {provider}/{name} lives on another broker than the hub space, and a \
                 shared read runs in process on one broker (SHR-17)"
            ));
        }
        let tenant = crate::spaces::tenant(mirror, provider, &space)?;
        Ok(Some(SharedMember {
            broker: ngsi_ld
                .strip_suffix("/ngsi-ld/v1")
                .unwrap_or(&ngsi_ld)
                .to_owned(),
            member: MemberSource {
                endpoint: ngsi_ld,
                tenant: tenant.clone(),
                gateway: None,
            },
            tenant,
            slug,
        }))
    }

    /// Writes the export grant the provider's tenant holds for the hub's (Antares ADR-0025
    /// rule 5): through the shared Endpoint's view, reads only, decided as an anonymous caller.
    async fn put_grant(&self, shared: &SharedMember, hub: &str) -> Result<(), String> {
        let response = self
            .authorized(self.http.put(shared.grant_url(hub)))?
            .json(&shared.grant(hub))
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match response.status().is_success() {
            true => Ok(()),
            false => Err(refusal(
                "the broker refused the provider's export grant",
                response,
                None,
            )
            .await),
        }
    }

    async fn delete_grant(&self, shared: &SharedMember, hub: &str) -> Result<(), String> {
        let response = self
            .authorized(self.http.delete(shared.grant_url(hub)))?
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND
        {
            true => Ok(()),
            false => Err(refusal(
                "the broker refused to remove the export grant",
                response,
                None,
            )
            .await),
        }
    }

    /// Where every Endpoint of one project is read: the broker holding it, and the tenant its
    /// space is. A member of another project resolves to nothing, which is PF-48's boundary
    /// again after the manifest's own validation (`validate_scope`).
    fn members_of(&self, mirror: &Mirror, project: &str) -> impl Fn(&str) -> Option<MemberSource> {
        let spaces: BTreeMap<String, MemberSource> = mirror
            .list(project, "Endpoint", &crate::store::ListOptions::default())
            .items
            .into_iter()
            .filter_map(|envelope| {
                let space = envelope
                    .spec
                    .get("contextSpaceRef")
                    .and_then(crate::resource::reference_name)?
                    .to_owned();
                let gateway = envelope
                    .spec
                    .get("slug")
                    .and_then(Value::as_str)
                    .zip(self.gateway.as_deref())
                    .map(|(slug, base)| format!("{base}/api/endpoint/{slug}"));
                Some((
                    envelope.metadata.name,
                    MemberSource {
                        endpoint: self.ngsi_ld(mirror, project, &space)?,
                        tenant: crate::spaces::tenant(mirror, project, &space).ok()?,
                        gateway,
                    },
                ))
            })
            .collect();
        move |name: &str| spaces.get(name).cloned()
    }

    /// Creates the registration, or patches the one that is already there.
    async fn write(
        &self,
        ngsi_ld: &str,
        space: &str,
        id: &str,
        body: &Value,
    ) -> Result<(), String> {
        let created = self
            .authorized(self.http.post(format!("{ngsi_ld}/csourceRegistrations")))?
            .header(TENANT_HEADER, space)
            .json(body)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if created.status().is_success() {
            return Ok(());
        }
        if created.status() != reqwest::StatusCode::CONFLICT {
            return Err(refusal("the broker refused the registration", created, None).await);
        }
        // Already there: the manifest is still the truth. The id is not sent again — a PATCH
        // that renames the resource is not what this is.
        let mut patch = body.clone();
        if let Some(object) = patch.as_object_mut() {
            object.remove("id");
            object.remove("type");
        }
        let updated = self
            .authorized(
                self.http
                    .patch(format!("{ngsi_ld}/csourceRegistrations/{id}")),
            )?
            .header(TENANT_HEADER, space)
            .json(&patch)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match updated.status().is_success() {
            true => Ok(()),
            false => Err(refusal("the broker refused the update", updated, None).await),
        }
    }

    async fn delete(&self, ngsi_ld: &str, space: &str, id: &str) -> Result<(), String> {
        let response = self
            .authorized(
                self.http
                    .delete(format!("{ngsi_ld}/csourceRegistrations/{id}")),
            )?
            .header(TENANT_HEADER, space)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        match response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND
        {
            true => Ok(()),
            false => Err(refusal("the broker refused the delete", response, None).await),
        }
    }
}

/// Every declared registration of the mirror: `(project, name, hub space, manifest)`, and the
/// ones that cannot be written: `(project, name, reason)`.
///
/// The manifest is rebuilt in the shape `jcctl::csr` reads, because the body it builds is the
/// one the specification defines and writing a second one here would be a second place for a
/// registration to stop matching.
fn declared(mirror: &Mirror) -> (Vec<Declared>, Vec<Refused>) {
    let mut found = Vec::new();
    let mut refused = Vec::new();
    for namespace in mirror.namespaces() {
        let page = mirror.list(
            &namespace,
            "ContextSourceRegistration",
            &crate::store::ListOptions::default(),
        );
        for envelope in page.items {
            let name = envelope.metadata.name.clone();
            let spec: ContextSourceRegistrationSpec =
                match serde_json::from_value(envelope.spec.clone()) {
                    Ok(spec) => spec,
                    Err(err) => {
                        tracing::warn!(
                            namespace = %namespace,
                            registration = %name,
                            error = %err,
                            "a ContextSourceRegistration manifest does not parse and is not written"
                        );
                        refused.push((
                            namespace.clone(),
                            name,
                            format!("the manifest does not parse, so it is not registered: {err}"),
                        ));
                        continue;
                    }
                };
            // The hub's own project is the only one a member may live in (PF-48). A manifest
            // that names another is refused here as well as at validation, because the mirror
            // holds what Git holds and Git may hold a file written before the rule.
            if let Err(err) = spec.validate_scope(Some(&namespace)) {
                tracing::warn!(
                    namespace = %namespace,
                    registration = %name,
                    error = %err,
                    "a ContextSourceRegistration reaches outside its project and is not written"
                );
                refused.push((namespace.clone(), name, format!("the manifest reaches outside its project, so it is not registered (PF-48): {err}")));
                continue;
            }
            let metadata = match serde_json::to_value(&envelope.metadata)
                .and_then(serde_json::from_value::<RawMetadata>)
            {
                Ok(metadata) => metadata,
                Err(err) => {
                    tracing::warn!(
                        namespace = %namespace,
                        registration = %name,
                        error = %err,
                        "a ContextSourceRegistration's metadata does not parse"
                    );
                    refused.push((namespace.clone(), name, format!("the manifest's metadata does not parse, so it is not registered: {err}")));
                    continue;
                }
            };
            // The broker tenant the gateway pins for the space (PF-130, ARCH/TEN-10).
            let space =
                match crate::spaces::tenant(mirror, &namespace, spec.context_space_ref.name()) {
                    Ok(tenant) => tenant,
                    Err(reason) => {
                        refused.push((
                            namespace.clone(),
                            name,
                            format!("not registered: {reason}"),
                        ));
                        continue;
                    }
                };
            found.push((
                namespace.clone(),
                name,
                space,
                spec.context_space_ref.name().to_owned(),
                RawManifest {
                    api_version: envelope.api_version.clone(),
                    kind: envelope.kind.clone(),
                    metadata,
                    spec: envelope.spec.clone(),
                    status: None,
                },
            ));
        }
    }
    (found, refused)
}

/// An Endpoint another project shares, read in process by a hub (SHR-15 `live`).
struct SharedMember {
    /// The broker both tenants live on, without `/ngsi-ld/v1`.
    broker: String,
    /// Where the broker reads it: itself, as the provider's tenant.
    member: MemberSource,
    /// The provider space's tenant.
    tenant: String,
    /// The shared Endpoint's slug, which is its view's id.
    slug: String,
}

/// `(broker, provider tenant, hub tenant)`: one export grant.
type GrantKey = (String, String, String);

/// The caller a shared read is decided as: the gateway's anonymous caller, so the provider's
/// public Policy holds and no person or token of the consumer reaches it (SHR-17).
const ANONYMOUS_ROLES: (&str, &str) = ("x-jc-roles", "public");

impl SharedMember {
    fn grant_key(&self, hub: &str) -> GrantKey {
        (self.broker.clone(), self.tenant.clone(), hub.to_owned())
    }

    fn grant_url(&self, hub: &str) -> String {
        format!("{}/q/grants/{}/{hub}", self.broker, self.tenant)
    }

    /// The grant document. Two tenants of different Organizations need the mark that the
    /// grant crosses them (Antares T-7306); the shared Endpoint's own Policy is what admits
    /// the consumer, and its view what is exported.
    fn grant(&self, hub: &str) -> Value {
        let organization = |tenant: &str| tenant.split_once("__").map(|(org, _)| org.to_owned());
        let mut grant = serde_json::json!({
            "target": self.tenant,
            "source": hub,
            "view": self.slug,
            "access": "read",
            "subject": { ANONYMOUS_ROLES.0: ANONYMOUS_ROLES.1 },
        });
        if organization(&self.tenant) != organization(hub) {
            grant["crossOrganization"] = Value::Bool(true);
        }
        grant
    }
}

/// Whether `project` holds a `SharedSpaceReference` to the Endpoint `provider`/`name`, by its
/// slug or by its `endpointRef`.
fn referenced(mirror: &Mirror, project: &str, provider: &str, name: &str, slug: &str) -> bool {
    mirror
        .list(
            project,
            "SharedSpaceReference",
            &crate::store::ListOptions::default(),
        )
        .items
        .into_iter()
        .any(|envelope| {
            let spec = &envelope.spec;
            spec.get("endpointSlug").and_then(Value::as_str) == Some(slug)
                || spec.get("endpointRef").is_some_and(|r| {
                    r.get("project").and_then(Value::as_str) == Some(provider)
                        && r.get("name").and_then(Value::as_str) == Some(name)
                })
        })
}

/// A declared registration: `(project, name, hub tenant, hub space name, manifest)`.
type Declared = (String, String, String, String, RawManifest);

/// A declared registration that cannot be written: `(project, name, reason)`.
type Refused = (String, String, String);

/// Each ContextSourceRegistration's status from what its hub's tenant answered (T-0345, PF-48).
pub(super) fn record(fresh_mirror: &Mirror, outcomes: Vec<(String, String, RegistrationOutcome)>) {
    for (namespace, name, outcome) in outcomes {
        let Some(mut envelope) = fresh_mirror.get(&namespace, "ContextSourceRegistration", &name)
        else {
            continue;
        };
        if let Some(status) = envelope.status.as_mut() {
            match &outcome {
                RegistrationOutcome::Written => {
                    status.phase = crate::resource::Phase::Live;
                    status.conditions = Vec::new();
                }
                RegistrationOutcome::Error(reason) => {
                    status.phase = crate::resource::Phase::Error;
                    status.conditions = vec![super::streams::make_condition(
                        "RegistrationWritten",
                        "False",
                        "BrokerRefused",
                        reason,
                    )];
                }
            }
        }
        fresh_mirror.upsert(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
    use serde_json::json;

    fn envelope(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: kind.into(),
            metadata: ObjectMeta {
                name: name.into(),
                namespace: Some("helsinki".into()),
                ..Default::default()
            },
            spec,
            status: None,
        }
    }

    fn mirror_with(items: Vec<ResourceEnvelope>) -> Mirror {
        let mirror = Mirror::new();
        for item in items {
            mirror.upsert(item);
        }
        mirror
    }

    fn csr(name: &str, endpoint_ref: Value) -> ResourceEnvelope {
        envelope(
            "ContextSourceRegistration",
            name,
            json!({
                "contextSpaceRef": "hub",
                "endpointRef": endpoint_ref,
                "information": [{ "entities": [{ "type": "Vehicle" }] }],
                "federation": {
                    "identity": "serviceAccount",
                    "serviceAccountRef": { "kind": "ServiceAccount", "name": "hub-reader" }
                }
            }),
        )
    }

    #[test]
    fn a_member_of_this_platform_is_read_as_its_own_tenant_of_this_broker() {
        let sync = RegistrationSync::new("http://context-broker.brokers.svc.cluster.local:8080/");
        let mirror = mirror_with(vec![
            csr(
                "transport",
                json!({ "kind": "Endpoint", "name": "transport-read" }),
            ),
            envelope(
                "Endpoint",
                "transport-read",
                json!({ "contextSpaceRef": "transport" }),
            ),
            // The hub predates PF-84 and pins its segment; `transport` renders its own.
            envelope("ContextSpace", "hub", json!({ "urnSegment": "hub" })),
        ]);

        let (declared, _) = declared(&mirror);
        assert_eq!(declared.len(), 1);
        let (project, name, space, _, manifest) = &declared[0];
        assert_eq!(
            (project.as_str(), name.as_str(), space.as_str()),
            ("helsinki", "transport", "hub")
        );

        let body = registration(manifest, &sync.members_of(&mirror, "helsinki")).expect("builds");
        assert_eq!(
            body["endpoint"],
            json!("http://context-broker.brokers.svc.cluster.local:8080/ngsi-ld/v1")
        );
        assert_eq!(body["tenant"], json!("helsinki-transport"));
        assert_eq!(
            body["id"],
            json!("urn:ngsi-ld:ContextSourceRegistration:transport")
        );
        // No credential and no header of the hop are in anything the broker stores.
        let text = serde_json::to_string(&body).expect("serialises");
        for member in ["Authorization", TENANT_HEADER] {
            assert!(!text.contains(member), "{member} is in the body: {text}");
        }
    }

    /// T-4022 (PF-113): read as the caller, the member is its Endpoint on the gateway with the
    /// request's own `Authorization` passed on, and no tenant; read as a ServiceAccount it is
    /// still the broker's tenant. Without the gateway's address a caller registration does not
    /// render.
    #[test]
    fn a_caller_registration_names_the_member_endpoint_on_the_gateway() {
        let caller = |name: &str| {
            let mut manifest = csr(
                name,
                json!({ "kind": "Endpoint", "name": "transport-read" }),
            );
            manifest.spec["federation"] = json!({ "identity": "caller" });
            manifest
        };
        let mirror = mirror_with(vec![
            caller("as-caller"),
            csr(
                "as-account",
                json!({ "kind": "Endpoint", "name": "transport-read" }),
            ),
            envelope(
                "Endpoint",
                "transport-read",
                json!({ "contextSpaceRef": "transport", "slug": "k7m2qz4tv6xh3n5jb2ryd3wcfa" }),
            ),
            envelope("ContextSpace", "hub", json!({ "urnSegment": "hub" })),
        ]);
        let (declared, _) = declared(&mirror);
        let manifest = |name: &str| {
            declared
                .iter()
                .find(|d| d.1 == name)
                .map(|d| d.4.clone())
                .expect("declared")
        };
        let sync = RegistrationSync::new("http://context-broker:8080").with_gateway(Some(
            "http://context-gateway.platform.svc.cluster.local:8080/".into(),
        ));
        let members = sync.members_of(&mirror, "helsinki");

        let body = registration(&manifest("as-caller"), &members).expect("renders");
        assert_eq!(
            body["endpoint"],
            json!("http://context-gateway.platform.svc.cluster.local:8080/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa")
        );
        assert_eq!(
            body["contextSourceInfo"],
            json!([{ "key": "Authorization", "value": "urn:ngsi-ld:request" }])
        );
        assert!(body.get("tenant").is_none(), "{body}");
        // the marker only: no token and no account in what the broker stores
        let text = body.to_string();
        assert!(
            !text.contains("Bearer") && !text.contains("hub-reader"),
            "{text}"
        );

        let body = registration(&manifest("as-account"), &members).expect("renders");
        assert_eq!(
            body["endpoint"],
            json!("http://context-broker:8080/ngsi-ld/v1")
        );
        assert_eq!(body["tenant"], json!("helsinki-transport"));
        assert!(body.get("contextSourceInfo").is_none(), "{body}");

        let without = RegistrationSync::new("http://context-broker:8080");
        assert!(matches!(
            registration(&manifest("as-caller"), &without.members_of(&mirror, "helsinki")),
            Err(jcctl::csr::CsrError::UnresolvedEndpoint(name)) if name == "transport-read"
        ));
    }

    /// PF-111: a member on broker `canary` is read at the canary's address; one whose space
    /// names no broker resolves to nothing rather than to `main`.
    #[test]
    fn a_member_is_read_on_the_broker_its_space_lives_on() {
        let sync = RegistrationSync::new("http://context-broker:8080")
            .with_broker_template(Some("http://broker-{broker}:8080".into()));
        let image = format!("ghcr.io/x/antares@sha256:{}", "a".repeat(64));
        let mirror = mirror_with(vec![
            envelope(
                "ContextBroker",
                "main",
                json!({ "default": true, "image": image }),
            ),
            envelope("ContextBroker", "canary", json!({ "image": image })),
            envelope(
                "ContextSpace",
                "transport",
                json!({ "brokerRef": "canary" }),
            ),
            envelope("ContextSpace", "lost", json!({ "brokerRef": "gone" })),
            envelope(
                "Endpoint",
                "on-canary",
                json!({ "contextSpaceRef": "transport" }),
            ),
            envelope("Endpoint", "on-lost", json!({ "contextSpaceRef": "lost" })),
        ]);
        let members = sync.members_of(&mirror, "helsinki");
        let member = members("on-canary").expect("resolves");
        assert_eq!(member.endpoint, "http://broker-canary:8080/ngsi-ld/v1");
        assert_eq!(member.tenant, "helsinki-transport");
        assert!(members("on-lost").is_none());
    }

    #[test]
    fn an_endpoint_this_project_does_not_have_resolves_to_nothing() {
        let sync = RegistrationSync::new("http://context-broker:8080");
        let mirror = mirror_with(vec![csr(
            "transport",
            json!({ "kind": "Endpoint", "name": "somewhere-else" }),
        )]);

        let (declared, _) = declared(&mirror);
        let refused = registration(&declared[0].4, &sync.members_of(&mirror, "helsinki"))
            .expect_err("no such Endpoint in this project");
        assert!(refused.to_string().contains("somewhere-else"), "{refused}");
    }

    /// PF-48: registering a member is what exposes it, so it stays inside one steward's project.
    #[test]
    fn a_member_named_in_another_project_is_never_written() {
        let mirror = mirror_with(vec![csr(
            "transport",
            json!({ "kind": "Endpoint", "name": "transport-read", "namespace": "espoo" }),
        )]);
        let (found, refused) = declared(&mirror);
        assert!(found.is_empty());
        assert!(refused[0].2.contains("outside its project"), "{refused:?}");
    }

    fn in_project(project: &str, mut item: ResourceEnvelope) -> ResourceEnvelope {
        item.metadata.namespace = Some(project.into());
        item
    }

    const SHARED_SLUG: &str = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

    /// Espoo shares Endpoint `shared` over its space `src`; Helsinki's hub reads it anonymously
    /// and holds a SharedSpaceReference to it when `referenced`.
    fn shared_read(referenced: bool) -> Mirror {
        let mut anonymous = csr(
            "from-espoo",
            json!({ "kind": "Endpoint", "name": "shared", "namespace": "espoo" }),
        );
        anonymous.spec["federation"] = json!({ "identity": "anonymous" });
        let mut items = vec![
            anonymous,
            envelope("ContextSpace", "hub", json!({ "urnSegment": "hub" })),
            in_project(
                "espoo",
                envelope(
                    "Endpoint",
                    "shared",
                    json!({ "contextSpaceRef": "src", "slug": SHARED_SLUG }),
                ),
            ),
            in_project(
                "espoo",
                envelope("ContextSpace", "src", json!({ "urnSegment": "src" })),
            ),
        ];
        if referenced {
            items.push(envelope(
                "SharedSpaceReference",
                "espoo",
                json!({ "alias": "espoo", "endpointSlug": SHARED_SLUG }),
            ));
        }
        mirror_with(items)
    }

    /// SHR-15 `live`, SHR-17 (T-7422): an anonymous registration of another project's Endpoint
    /// under a SharedSpaceReference is read on this broker as the provider's tenant, and the
    /// provider's grant names the shared Endpoint's view and the anonymous caller alone: no
    /// gateway address, no token marker, no person of the hub.
    #[test]
    fn a_shared_endpoint_of_another_project_is_read_in_process_as_an_anonymous_caller() {
        let sync = RegistrationSync::new("http://127.0.0.1:42101/broker")
            .with_gateway(Some("http://127.0.0.1:42101/api".into()));
        let mirror = shared_read(true);
        let (found, refused) = declared(&mirror);
        assert!(refused.is_empty(), "{refused:?}");
        let (project, _, hub, _, manifest) = &found[0];
        let ngsi_ld = "http://127.0.0.1:42101/broker/ngsi-ld/v1";
        let shared = sync
            .shared_member(&mirror, project, manifest, ngsi_ld)
            .expect("resolves")
            .expect("a shared read");
        let member = shared.member.clone();
        let body = registration(manifest, &move |_: &str| Some(member.clone())).expect("renders");
        assert_eq!(body["endpoint"], json!(ngsi_ld));
        // Both spaces pin their segment, which is their tenant.
        assert_eq!(body["tenant"], json!("src"));
        assert!(body.get("contextSourceInfo").is_none(), "{body}");

        assert_eq!(
            shared.grant_url(hub),
            "http://127.0.0.1:42101/broker/q/grants/src/hub"
        );
        assert_eq!(
            shared.grant(hub),
            json!({ "target": "src", "source": "hub", "view": SHARED_SLUG,
                    "access": "read", "subject": { "x-jc-roles": "public" } })
        );
        let across = SharedMember {
            tenant: "sk_ba__src".into(),
            ..shared
        };
        assert_eq!(
            across.grant("sk_bbsk__hub")["crossOrganization"],
            json!(true)
        );

        // On another broker than the hub's, nothing is read in process.
        let refused = sync
            .shared_member(&mirror, project, manifest, "http://other:8080/ngsi-ld/v1")
            .err()
            .unwrap_or_default();
        assert!(refused.contains("another broker"), "{refused}");
    }

    /// SHR-15: without a SharedSpaceReference to it, another project's Endpoint is not read.
    #[test]
    fn a_shared_read_needs_a_shared_space_reference() {
        let sync = RegistrationSync::new("http://127.0.0.1:42101/broker");
        let mirror = shared_read(false);
        let (found, _) = declared(&mirror);
        let refused = sync
            .shared_member(
                &mirror,
                &found[0].0,
                &found[0].4,
                "http://127.0.0.1:42101/broker/ngsi-ld/v1",
            )
            .err()
            .unwrap_or_default();
        assert!(refused.contains("SharedSpaceReference"), "{refused}");
    }

    /// One request the fake broker saw: method and path.
    type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>;

    async fn fake_broker() -> (String, Seen) {
        let seen: Seen = Seen::default();
        let log = seen.clone();
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let log = log.clone();
            async move {
                log.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((req.method().to_string(), req.uri().path().to_owned()));
                axum::http::StatusCode::CREATED
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds");
        let base = format!("http://{}/broker", listener.local_addr().expect("addr"));
        tokio::spawn(async move { axum::serve(listener, app).await });
        (base, seen)
    }

    /// T-7422, SHR-07: the provider's grant is written before the registration the broker
    /// admits only under it, and taken away once no SharedSpaceReference covers the read.
    #[tokio::test]
    async fn the_grant_goes_before_the_registration_and_leaves_with_the_reference() {
        let (base, seen) = fake_broker().await;
        let sync = RegistrationSync::new(base);
        let outcomes = sync.converge(&shared_read(true), &Mirror::new()).await;
        assert_eq!(outcomes[0].2, RegistrationOutcome::Written, "{outcomes:?}");
        let calls = std::mem::take(
            &mut *seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        assert_eq!(
            calls,
            [
                ("PUT".into(), "/broker/q/grants/src/hub".into()),
                (
                    "POST".into(),
                    "/broker/ngsi-ld/v1/csourceRegistrations".into()
                ),
            ]
        );

        let outcomes = sync.converge(&shared_read(false), &shared_read(true)).await;
        assert!(
            matches!(&outcomes[0].2, RegistrationOutcome::Error(r) if r.contains("SharedSpaceReference")),
            "{outcomes:?}"
        );
        let calls = std::mem::take(
            &mut *seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        assert_eq!(
            calls,
            [("DELETE".into(), "/broker/q/grants/src/hub".into())]
        );
    }

    #[test]
    fn a_registration_that_does_not_parse_is_left_out_rather_than_half_written() {
        let mirror = mirror_with(vec![envelope(
            "ContextSourceRegistration",
            "broken",
            json!({ "contextSpaceRef": "hub" }),
        )]);
        let (found, refused) = declared(&mirror);
        assert!(found.is_empty());
        assert_eq!(refused[0].1, "broken");
    }

    #[test]
    fn a_reference_reads_the_same_written_either_way() {
        use crate::resource::reference_name;
        assert_eq!(reference_name(&json!("transport")), Some("transport"));
        assert_eq!(
            reference_name(&json!({ "kind": "ContextSpace", "name": "transport" })),
            Some("transport")
        );
        assert_eq!(reference_name(&json!(7)), None);
    }
}
