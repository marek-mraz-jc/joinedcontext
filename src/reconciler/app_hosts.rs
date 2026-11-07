//! The host of every published App (ADR-N-037, AP-133, T-2838).
//!
//! An App is served on `{name}.apps.{domain}`, and the zone has no DNS API, so each host gets a
//! certificate of its own by HTTP-01. In the APISIX namespace this wave creates, per published
//! App, a cert-manager `Certificate` `app-{name}` from the issuer of the edge's own certificate
//! (`apisix-edge`), and an `Ingress` `app-{name}` copied from the chart's edge Ingress (`apisix`)
//! for that one host, which carries the challenge and then the App's traffic to APISIX.
//!
//! It creates both once, when the App is published, and never again on a reconcile: Let's
//! Encrypt issues 50 certificates per registered domain a week, and cert-manager renews one that
//! exists. It deletes both when the App is no longer published. An object of that name it did
//! not create is never read for its state, changed or deleted; the App is reported instead.
//!
//! Where the App zone is delegated to a DNS API, the chart's wildcard certificate
//! `apisix-apps-wildcard` serves every App host (ADR-N-037 §6, T-3013). Once it is issued and the
//! edge Ingress carries it, a published App's host is ready at once and gets no objects of its own.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::apps::kube::{KubeClient, KubeError};

/// The chart's edge Ingress, the one every App's Ingress is copied from.
pub const EDGE_INGRESS: &str = "apisix";
/// The chart's edge Certificate, whose issuer every App's certificate is requested from.
pub const EDGE_CERTIFICATE: &str = "apisix-edge";
/// The chart's certificate for `*.apps.{domain}`, when the installation has one (ADR-N-037 §6).
pub const APPS_WILDCARD_CERTIFICATE: &str = "apisix-apps-wildcard";
const MANAGED_BY: &str = "app.kubernetes.io/managed-by";
const MANAGED_VALUE: &str = "joinedcontext-portal";
const COMPONENT: &str = "app.kubernetes.io/component";
const COMPONENT_VALUE: &str = "app-host";
/// The component of the objects of a hostname on the Organization's own domain (AP-172).
const HOSTNAME_COMPONENT: &str = "app-hostname";
const APP_LABEL: &str = crate::apps::reconciler::APP_LABEL;
const CERTIFICATE_API: &str = "cert-manager.io/v1";
const INGRESS_API: &str = "networking.k8s.io/v1";

/// Where one App's host stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostState {
    /// The certificate is issued: the App answers on its host.
    Ready,
    /// The certificate is requested and not issued yet, with what cert-manager last said.
    Pending(String),
    /// The host cannot be served, and why. Never carries a secret.
    Failed(String),
}

/// The name of both objects of one App's host.
fn object_name(app: &str) -> String {
    format!("app-{app}")
}

fn labels(app: &str) -> Value {
    json!({ MANAGED_BY: MANAGED_VALUE, COMPONENT: COMPONENT_VALUE, APP_LABEL: app })
}

/// The name of both objects of one hostname of an App (AP-172), as its edge route is named.
fn hostname_object(app: &str, hostname: &str) -> String {
    format!(
        "app-{app}-at-{}",
        crate::reconciler::edge_file::hostname_key(hostname)
    )
}

fn hostname_labels(app: &str) -> Value {
    json!({ MANAGED_BY: MANAGED_VALUE, COMPONENT: HOSTNAME_COMPONENT, APP_LABEL: app })
}

/// The App this wave created `object` for, or `None` when it is somebody else's.
fn managed_app(object: &Value) -> Option<&str> {
    managed_as(object, COMPONENT_VALUE)
}

fn managed_as<'a>(object: &'a Value, component: &str) -> Option<&'a str> {
    let labels = &object["metadata"]["labels"];
    (labels[MANAGED_BY] == MANAGED_VALUE && labels[COMPONENT] == component)
        .then(|| labels[APP_LABEL].as_str())
        .flatten()
}

/// The certificate of one App's host, from the edge certificate's issuer.
pub fn certificate(namespace: &str, app: &str, host: &str, issuer_ref: &Value) -> Value {
    named_certificate(namespace, &object_name(app), labels(app), host, issuer_ref)
}

/// The certificate of one hostname of an App on its Organization's domain, by HTTP-01 from the
/// edge certificate's issuer: the wildcard names `*.apps.{domain}` only (AP-172).
pub fn hostname_certificate(
    namespace: &str,
    app: &str,
    hostname: &str,
    issuer_ref: &Value,
) -> Value {
    let name = hostname_object(app, hostname);
    named_certificate(namespace, &name, hostname_labels(app), hostname, issuer_ref)
}

fn named_certificate(
    namespace: &str,
    name: &str,
    labels: Value,
    host: &str,
    issuer_ref: &Value,
) -> Value {
    json!({
        "apiVersion": CERTIFICATE_API,
        "kind": "Certificate",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        "spec": {
            "secretName": format!("{name}-tls"),
            "issuerRef": issuer_ref,
            "dnsNames": [host],
        },
    })
}

/// The edge Ingress of one App's host: the chart's class, its TLS and listener annotations and
/// its backend, for this host alone. `None` when the template has no backend to copy.
pub fn ingress(template: &Value, namespace: &str, app: &str, host: &str) -> Option<Value> {
    named_ingress(template, namespace, &object_name(app), labels(app), host)
}

/// The edge Ingress of one hostname of an App (AP-172), as [`ingress`] is of its host.
pub fn hostname_ingress(
    template: &Value,
    namespace: &str,
    app: &str,
    hostname: &str,
) -> Option<Value> {
    let name = hostname_object(app, hostname);
    named_ingress(template, namespace, &name, hostname_labels(app), hostname)
}

fn named_ingress(
    template: &Value,
    namespace: &str,
    name: &str,
    labels: Value,
    host: &str,
) -> Option<Value> {
    let path = template["spec"]["rules"][0]["http"]["paths"][0].clone();
    if !path["backend"].is_object() {
        return None;
    }
    // The chart's Certificate is explicit and so is this one: an ingress-shim annotation would
    // make a second owner order the same name.
    let annotations: Map<String, Value> = template["metadata"]["annotations"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| !key.starts_with("cert-manager.io/"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut spec = json!({
        "tls": [{ "hosts": [host], "secretName": format!("{name}-tls") }],
        "rules": [{ "host": host, "http": { "paths": [path] } }],
    });
    if let Some(class) = template["spec"]["ingressClassName"].as_str() {
        spec["ingressClassName"] = json!(class);
    }
    Some(json!({
        "apiVersion": INGRESS_API,
        "kind": "Ingress",
        "metadata": {
            "name": name,
            "namespace": namespace,
            "labels": labels,
            "annotations": annotations,
        },
        "spec": spec,
    }))
}

/// What cert-manager says of one certificate.
pub fn state_of(certificate: &Value) -> HostState {
    let ready = certificate["status"]["conditions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|condition| condition["type"] == "Ready");
    match ready {
        Some(condition) if condition["status"] == "True" => HostState::Ready,
        Some(condition) => HostState::Pending(
            condition["message"]
                .as_str()
                .unwrap_or("cert-manager has not issued it yet")
                .to_owned(),
        ),
        None => HostState::Pending("cert-manager has not picked it up yet".to_owned()),
    }
}

/// Whether `wildcard` serves every App host under `apex`: issued, naming `*.apps.{apex}`, and the
/// edge Ingress `edge` terminating that name with the certificate's Secret. Anything less leaves the
/// hosts to certificates of their own, as before the wildcard.
pub fn wildcard_serves(wildcard: &Value, edge: &Value, apex: &str) -> bool {
    let name = format!("*.apps.{apex}");
    let names = |hosts: &Value| {
        hosts
            .as_array()
            .into_iter()
            .flatten()
            .any(|host| host.as_str() == Some(name.as_str()))
    };
    let secret = &wildcard["spec"]["secretName"];
    state_of(wildcard) == HostState::Ready
        && names(&wildcard["spec"]["dnsNames"])
        && secret.is_string()
        && edge["spec"]["tls"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|tls| names(&tls["hosts"]) && &tls["secretName"] == secret)
}

fn no_backend() -> HostState {
    HostState::Failed(format!(
        "the edge Ingress {EDGE_INGRESS} routes no backend to copy"
    ))
}

/// A kube error as a sentence that names the object and the status, never the body sent.
fn said(object: &str, err: &KubeError) -> String {
    match err {
        KubeError::Api { status, .. } => format!("the API server answered {status} for {object}"),
        other => format!("{object}: {other}"),
    }
}

/// The hosts of one installation's Apps, in the APISIX namespace.
pub struct AppHosts {
    kube: KubeClient,
    namespace: String,
}

impl AppHosts {
    pub fn new(kube: KubeClient, namespace: String) -> Self {
        Self { kube, namespace }
    }

    /// Brings the hosts to the published Apps, `apex` the domain their hosts sit under: creates
    /// what a newly published App lacks and reads what an App's certificate says. Removing the
    /// hosts of Apps no longer published is [`AppHosts::retire`], which the caller runs only
    /// when `published` is the whole organization's.
    pub async fn converge(
        &self,
        published: &BTreeSet<String>,
        apex: &str,
    ) -> BTreeMap<String, HostState> {
        let failed_all = |reason: String| {
            published
                .iter()
                .map(|app| (app.clone(), HostState::Failed(reason.clone())))
                .collect()
        };
        let ns = self.namespace.as_str();
        let template = match self
            .kube
            .get(INGRESS_API, "Ingress", ns, EDGE_INGRESS)
            .await
        {
            Ok(Some(template)) => template,
            Ok(None) => {
                return failed_all(format!(
                    "the edge Ingress {EDGE_INGRESS} is not in {ns}; the apisix chart renders it"
                ))
            }
            Err(err) => return failed_all(said(&format!("Ingress {EDGE_INGRESS}"), &err)),
        };
        match self
            .kube
            .get(
                CERTIFICATE_API,
                "Certificate",
                ns,
                APPS_WILDCARD_CERTIFICATE,
            )
            .await
        {
            Ok(Some(wildcard)) if wildcard_serves(&wildcard, &template, apex) => {
                return published
                    .iter()
                    .map(|app| (app.clone(), HostState::Ready))
                    .collect();
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(
                error = %said(&format!("Certificate {APPS_WILDCARD_CERTIFICATE}"), &err),
                "the wildcard certificate was not read; each App host keeps a certificate of its own"
            ),
        }
        let issuer = match self.issuer().await {
            Ok(issuer) => issuer,
            Err(reason) => return failed_all(reason),
        };

        let mut states = BTreeMap::new();
        for app in published {
            let host = crate::reconciler::edge_file::app_host(app, apex);
            let state = match ingress(&template, ns, app, &host) {
                Some(wanted) => {
                    let wanted_certificate = certificate(ns, app, &host, &issuer);
                    self.host(wanted, wanted_certificate, app, COMPONENT_VALUE)
                        .await
                }
                None => no_backend(),
            };
            states.insert(app.clone(), state);
        }
        states
    }

    /// The issuer of the edge's own certificate, which every App certificate is requested from.
    async fn issuer(&self) -> Result<Value, String> {
        let ns = self.namespace.as_str();
        match self
            .kube
            .get(CERTIFICATE_API, "Certificate", ns, EDGE_CERTIFICATE)
            .await
        {
            Ok(Some(edge)) if edge["spec"]["issuerRef"].is_object() => {
                Ok(edge["spec"]["issuerRef"].clone())
            }
            Ok(_) => Err(format!(
                "the edge Certificate {EDGE_CERTIFICATE} in {ns} names no issuer to request an App's from"
            )),
            Err(err) => Err(said(&format!("Certificate {EDGE_CERTIFICATE}"), &err)),
        }
    }

    /// Brings the hostnames of the Organization's domain to `wanted`, App → its routed names
    /// (AP-172): each gets an Ingress and a certificate of its own by HTTP-01, the wildcard or
    /// not, created once. Returns each name's state.
    pub async fn converge_hostnames(
        &self,
        wanted: &BTreeMap<String, Vec<String>>,
    ) -> BTreeMap<String, HostState> {
        let names = wanted.values().flatten();
        if wanted.values().all(Vec::is_empty) {
            return BTreeMap::new();
        }
        let ns = self.namespace.as_str();
        let parts = match self
            .kube
            .get(INGRESS_API, "Ingress", ns, EDGE_INGRESS)
            .await
        {
            Ok(Some(template)) => self.issuer().await.map(|issuer| (template, issuer)),
            Ok(None) => Err(format!(
                "the edge Ingress {EDGE_INGRESS} is not in {ns}; the apisix chart renders it"
            )),
            Err(err) => Err(said(&format!("Ingress {EDGE_INGRESS}"), &err)),
        };
        let (template, issuer) = match parts {
            Ok(parts) => parts,
            Err(reason) => {
                return names
                    .map(|name| (name.clone(), HostState::Failed(reason.clone())))
                    .collect()
            }
        };
        let mut states = BTreeMap::new();
        for (app, hostnames) in wanted {
            for hostname in hostnames {
                let state = match hostname_ingress(&template, ns, app, hostname) {
                    Some(wanted) => {
                        let wanted_certificate = hostname_certificate(ns, app, hostname, &issuer);
                        self.host(wanted, wanted_certificate, app, HOSTNAME_COMPONENT)
                            .await
                    }
                    None => no_backend(),
                };
                states.insert(hostname.clone(), state);
            }
        }
        states
    }

    /// Deletes the objects of every hostname not in `wanted`: removed from its App, its domain
    /// lapsed, or its App retired (AP-172). `wanted` must be complete, as for [`AppHosts::retire`].
    pub async fn retire_hostnames(&self, wanted: &BTreeMap<String, Vec<String>>) {
        let ns = self.namespace.as_str();
        let keep: BTreeSet<String> = wanted
            .iter()
            .flat_map(|(app, names)| names.iter().map(|name| hostname_object(app, name)))
            .collect();
        let selector = format!("{MANAGED_BY}={MANAGED_VALUE},{COMPONENT}={HOSTNAME_COMPONENT}");
        for (api, kind) in [(CERTIFICATE_API, "Certificate"), (INGRESS_API, "Ingress")] {
            let held = match self.kube.list(api, kind, ns, &selector).await {
                Ok(held) => held,
                Err(err) => {
                    tracing::warn!(error = %said(kind, &err), "app hostnames were not listed");
                    continue;
                }
            };
            for object in held {
                let Some(name) = object["metadata"]["name"].as_str() else {
                    continue;
                };
                if managed_as(&object, HOSTNAME_COMPONENT).is_none() || keep.contains(name) {
                    continue;
                }
                match self.kube.delete(api, kind, ns, name).await {
                    Ok(()) => tracing::info!(%name, kind, "App hostname no longer routed: removed"),
                    Err(err) => {
                        tracing::warn!(%name, error = %said(&format!("{kind} {name}"), &err), "App hostname not removed")
                    }
                }
            }
        }
    }

    /// Creates `wanted_certificate` and `wanted_ingress` once, and reads the certificate's state;
    /// an object of that name `component` did not make for `app` is never touched.
    async fn host(
        &self,
        wanted_ingress: Value,
        wanted_certificate: Value,
        app: &str,
        component: &str,
    ) -> HostState {
        let ns = self.namespace.as_str();
        let name = wanted_certificate["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let held = match self
            .kube
            .get(CERTIFICATE_API, "Certificate", ns, &name)
            .await
        {
            Ok(held) => held,
            Err(err) => return HostState::Failed(said(&format!("Certificate {name}"), &err)),
        };
        let state = match held {
            None => {
                // Requested once: from here on the Certificate exists and cert-manager renews it.
                if let Err(err) = self.kube.create(&wanted_certificate).await {
                    return HostState::Failed(said(&format!("Certificate {name}"), &err));
                }
                HostState::Pending("the certificate is requested".to_owned())
            }
            Some(held) if managed_as(&held, component) != Some(app) => {
                return HostState::Failed(format!(
                    "a Certificate {name} in {ns} exists that the Portal did not create; rename the App or remove it"
                ))
            }
            Some(held) => state_of(&held),
        };
        match self.kube.get(INGRESS_API, "Ingress", ns, &name).await {
            Ok(None) => match self.kube.create(&wanted_ingress).await {
                Ok(()) => state,
                Err(err) => HostState::Failed(said(&format!("Ingress {name}"), &err)),
            },
            Ok(Some(held)) if managed_as(&held, component) != Some(app) => HostState::Failed(format!(
                "an Ingress {name} in {ns} exists that the Portal did not create; rename the App or remove it"
            )),
            Ok(Some(_)) => state,
            Err(err) => HostState::Failed(said(&format!("Ingress {name}"), &err)),
        }
    }

    /// Deletes the host of every App this wave made one for that is no longer published. A
    /// failure is logged and tried again on the next run.
    ///
    /// `published` must be complete: an App missing from it only because its project's
    /// repository did not stage would lose its certificate, and every re-request counts against
    /// Let's Encrypt's five duplicate certificates per host and week.
    // ponytail: the TLS Secret stays behind; cert-manager's --enable-certificate-owner-ref
    // removes it with its Certificate, and the Portal holds no right to delete Secrets here.
    pub async fn retire(&self, published: &BTreeSet<String>) {
        let ns = self.namespace.as_str();
        let selector = format!("{MANAGED_BY}={MANAGED_VALUE},{COMPONENT}={COMPONENT_VALUE}");
        for (api, kind) in [(CERTIFICATE_API, "Certificate"), (INGRESS_API, "Ingress")] {
            let held = match self.kube.list(api, kind, ns, &selector).await {
                Ok(held) => held,
                Err(err) => {
                    tracing::warn!(error = %said(kind, &err), "app hosts were not listed");
                    continue;
                }
            };
            for object in held {
                let Some(app) = managed_app(&object) else {
                    continue;
                };
                if published.contains(app) {
                    continue;
                }
                let name = object_name(app);
                match self.kube.delete(api, kind, ns, &name).await {
                    Ok(()) => tracing::info!(%app, kind, "retired App's host removed"),
                    Err(err) => {
                        tracing::warn!(%app, error = %said(&format!("{kind} {name}"), &err), "retired App's host not removed")
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> Value {
        json!({
            "metadata": {
                "name": "apisix",
                "annotations": {
                    "traefik.ingress.kubernetes.io/router.tls.options": "apisix-bsi-tr-02102@kubernetescrd",
                    "cert-manager.io/cluster-issuer": "letsencrypt-prod",
                },
            },
            "spec": {
                "ingressClassName": "traefik",
                "rules": [{ "host": "city.example", "http": { "paths": [{
                    "path": "/", "pathType": "ImplementationSpecific",
                    "backend": { "service": { "name": "apisix-gateway", "port": { "number": 80 } } },
                }] } }],
                "tls": [{ "hosts": ["city.example"], "secretName": "apisix-edge-tls" }],
            },
        })
    }

    #[test]
    fn an_app_ingress_is_the_edges_for_its_host_alone() {
        let made =
            ingress(&template(), "apisix", "bikes", "bikes.apps.city.example").expect("an Ingress");
        assert_eq!(made["metadata"]["name"], "app-bikes");
        assert_eq!(made["metadata"]["namespace"], "apisix");
        assert_eq!(made["spec"]["ingressClassName"], "traefik");
        assert_eq!(
            made["spec"]["rules"],
            json!([{ "host": "bikes.apps.city.example", "http": { "paths": [{
                "path": "/", "pathType": "ImplementationSpecific",
                "backend": { "service": { "name": "apisix-gateway", "port": { "number": 80 } } },
            }] } }])
        );
        assert_eq!(
            made["spec"]["tls"],
            json!([{ "hosts": ["bikes.apps.city.example"], "secretName": "app-bikes-tls" }])
        );
        let annotations = made["metadata"]["annotations"]
            .as_object()
            .expect("annotations");
        assert!(annotations.contains_key("traefik.ingress.kubernetes.io/router.tls.options"));
        assert!(
            !annotations
                .keys()
                .any(|key| key.starts_with("cert-manager.io/")),
            "one owner orders the certificate: {annotations:?}"
        );
        assert_eq!(managed_app(&made), Some("bikes"));
        assert!(
            !made.to_string().contains("\"city.example\""),
            "the apex is not its host"
        );
    }

    #[test]
    fn a_template_without_a_backend_makes_no_ingress() {
        let mut bare = template();
        bare["spec"]["rules"] = json!([]);
        assert!(ingress(&bare, "apisix", "bikes", "bikes.apps.city.example").is_none());
    }

    #[test]
    fn an_app_certificate_names_its_host_and_the_edges_issuer() {
        let issuer = json!({ "name": "letsencrypt-prod", "kind": "ClusterIssuer" });
        let made = certificate("apisix", "bikes", "bikes.apps.city.example", &issuer);
        assert_eq!(made["apiVersion"], "cert-manager.io/v1");
        assert_eq!(made["metadata"]["name"], "app-bikes");
        assert_eq!(made["spec"]["dnsNames"], json!(["bikes.apps.city.example"]));
        assert_eq!(made["spec"]["secretName"], "app-bikes-tls");
        assert_eq!(made["spec"]["issuerRef"], issuer);
        assert_eq!(managed_app(&made), Some("bikes"));
    }

    /// AP-172: a hostname of the Organization's domain gets its own Ingress and HTTP-01
    /// certificate, named after its edge route and never taken for the App's host objects.
    #[test]
    fn a_hostname_gets_an_ingress_and_a_certificate_of_its_own() {
        let issuer = json!({ "name": "letsencrypt-prod", "kind": "ClusterIssuer" });
        let key = crate::reconciler::edge_file::hostname_key("bikes.hel.fi");
        let made = hostname_certificate("apisix", "bikes", "bikes.hel.fi", &issuer);
        assert_eq!(made["metadata"]["name"], format!("app-bikes-at-{key}"));
        assert_eq!(made["spec"]["dnsNames"], json!(["bikes.hel.fi"]));
        assert_eq!(
            made["spec"]["secretName"],
            format!("app-bikes-at-{key}-tls")
        );
        assert_eq!(made["spec"]["issuerRef"], issuer);
        assert_eq!(managed_as(&made, HOSTNAME_COMPONENT), Some("bikes"));
        assert_eq!(managed_app(&made), None, "not the App's host certificate");

        let made =
            hostname_ingress(&template(), "apisix", "bikes", "bikes.hel.fi").expect("an Ingress");
        assert_eq!(made["metadata"]["name"], format!("app-bikes-at-{key}"));
        assert_eq!(made["spec"]["rules"][0]["host"], "bikes.hel.fi");
        assert_eq!(
            made["spec"]["tls"],
            json!([{ "hosts": ["bikes.hel.fi"], "secretName": format!("app-bikes-at-{key}-tls") }])
        );
        assert!(!made.to_string().contains("cert-manager.io/"));
        assert_eq!(managed_as(&made, HOSTNAME_COMPONENT), Some("bikes"));
    }

    #[test]
    fn a_certificate_is_ready_only_when_cert_manager_says_so() {
        let with = |conditions: Value| json!({ "status": { "conditions": conditions } });
        assert_eq!(
            state_of(&with(json!([{ "type": "Ready", "status": "True" }]))),
            HostState::Ready
        );
        assert_eq!(
            state_of(&with(
                json!([{ "type": "Ready", "status": "False", "message": "Issuing certificate as Secret does not exist" }])
            )),
            HostState::Pending("Issuing certificate as Secret does not exist".into())
        );
        assert!(matches!(state_of(&json!({})), HostState::Pending(_)));
        assert!(matches!(
            state_of(&with(json!([{ "type": "Issuing", "status": "True" }]))),
            HostState::Pending(_)
        ));
    }

    fn wildcard(ready: &str, names: Value) -> Value {
        json!({
            "spec": { "secretName": "apisix-apps-wildcard-tls", "dnsNames": names },
            "status": { "conditions": [{ "type": "Ready", "status": ready }] },
        })
    }

    fn edge_with(tls: Value) -> Value {
        let mut edge = template();
        edge["spec"]["tls"] = tls;
        edge
    }

    #[test]
    fn the_wildcard_serves_app_hosts_only_issued_named_and_on_the_edge() {
        let edge = edge_with(json!([
            { "hosts": ["city.example"], "secretName": "apisix-edge-tls" },
            { "hosts": ["*.apps.city.example"], "secretName": "apisix-apps-wildcard-tls" },
        ]));
        let issued = wildcard("True", json!(["*.apps.city.example"]));
        assert!(wildcard_serves(&issued, &edge, "city.example"));

        // Not issued yet: the hosts keep their own certificates until it is.
        assert!(!wildcard_serves(
            &wildcard("False", json!(["*.apps.city.example"])),
            &edge,
            "city.example"
        ));
        // Issued for another domain, or for the apex's own wildcard, which no App host is under.
        assert!(!wildcard_serves(&issued, &edge, "other.example"));
        assert!(!wildcard_serves(
            &wildcard("True", json!(["*.city.example"])),
            &edge,
            "city.example"
        ));
        // Issued, but the edge does not terminate the App hosts with it.
        assert!(!wildcard_serves(&issued, &template(), "city.example"));
        assert!(!wildcard_serves(
            &issued,
            &edge_with(
                json!([{ "hosts": ["*.apps.city.example"], "secretName": "apisix-edge-tls" }])
            ),
            "city.example"
        ));
        // No Secret named: nothing to match the edge against.
        let mut nameless = issued.clone();
        nameless["spec"]["secretName"] = Value::Null;
        assert!(!wildcard_serves(
            &nameless,
            &edge_with(json!([{ "hosts": ["*.apps.city.example"] }])),
            "city.example"
        ));
    }

    #[test]
    fn an_object_somebody_else_made_is_nobodys_app() {
        assert_eq!(managed_app(&template()), None);
        let mut forged = template();
        forged["metadata"]["labels"] = json!({ MANAGED_BY: MANAGED_VALUE, APP_LABEL: "bikes" });
        assert_eq!(
            managed_app(&forged),
            None,
            "the component label is part of it"
        );
    }
}
