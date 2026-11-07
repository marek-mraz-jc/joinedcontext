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
use jc_core::kinds::ContextSourceRegistrationSpec;
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
            http: reqwest::Client::builder()
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
            let members = self.members_of(mirror, &project);
            let body = match registration(&manifest, &members) {
                Ok(body) => body,
                Err(err) => {
                    outcomes.push((project, name, RegistrationOutcome::Error(err.to_string())));
                    continue;
                }
            };
            let Some(ngsi_ld) = self.ngsi_ld(mirror, &project, &hub) else {
                let reason = format!(
                    "the hub space '{hub}' lives on a broker this Portal has no address for, so \
                     nothing is registered; check its brokerRef (PF-111)"
                );
                outcomes.push((project, name, RegistrationOutcome::Error(reason)));
                continue;
            };
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
        outcomes
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
                        tenant: crate::spaces::segment(mirror, project, &space),
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
            // The broker tenant is the space's rendered segment (PF-84).
            let space = crate::spaces::segment(mirror, &namespace, spec.context_space_ref.name());
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
