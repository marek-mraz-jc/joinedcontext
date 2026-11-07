//! Every `MqttBroker`'s release and Keycloak client (ADR-N-048 §2.6–§2.7, MF-54, PL-74, T-3994).
//!
//! The deployment renders the broker chart once with sentinels into the ConfigMap
//! [`TEMPLATE`] (key `objects.yaml`) in the brokers' namespace. Per broker the reconciler fills
//! the sentinels and applies every object by server-side apply, the way the App hosts copy the
//! edge Ingress, so the chart stays the one description of a broker. The names it fills are DNS
//! labels and the limits numbers, so a filled value can never open a sentinel or a YAML structure
//! of its own.
//!
//! A manifest the platform refuses (its kind's rules, a ServiceAccount its project does not
//! declare, a broker past the project's MQTT allowance, a dedicated port another broker holds) is not applied: the release it had keeps
//! running, and the error goes into the broker's status. A broker whose manifest is gone loses its
//! objects, and its client `mqtt-{project}-{name}`: bearer-only, no secret, the audience the
//! gateway checks a device's token against. The ServiceAccounts a broker's roles name get that
//! audience on their own clients (`workload_clients`).

use std::collections::{BTreeMap, BTreeSet};

use jc_core::kinds::mqtt_broker::{certificate_release_name, release_name};
use jc_core::kinds::MqttDeviceAuth;
use jc_core::kinds::{MqttAllowance, MqttBrokerSpec, MqttUsage, ServiceAccountSpec};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::app_clients::{drift, managed, Admin, ClientOutcome};
use super::groups::{MANAGED_BY, MANAGED_VALUE};
use crate::apps::kube::KubeClient;
use crate::store::{ListOptions, Mirror};

/// The ConfigMap the deployment renders the broker chart into, sentinels for the values. Not named
/// `mqtt-…`, so the client's `*_mqtt` methods can never write or remove it.
pub const TEMPLATE: &str = "broker-template";
/// Its key.
pub const TEMPLATE_KEY: &str = "objects.yaml";
/// The key of the same objects for a broker with a served `spec.port` (ADR-N-048 §2.5, T-5856):
/// a TLS listener in RMQTT and a passthrough route on the entrypoint `mqtt-__PORT__`, the pod
/// annotated with `__TLS_REVISION__`, its Certificate's `status.revision`, so a renewed
/// certificate rolls the broker, which reads it only at start.
pub const PORT_KEY: &str = "objects-port.yaml";
/// The key of the same objects for a broker on a custom hostname (ADR-N-048 §2.11, T-5874): the
/// 8883 route by `__HOSTNAME__` serving the Secret `__TLS_SECRET__`, and a Certificate issued by
/// HTTP-01 for that name, which the reconciler drops when the admin's certificateRef serves it.
pub const HOSTNAME_KEY: &str = "objects-hostname.yaml";
/// The key of a broker's certificate door (ADR-N-048 §2.12, T-5875): its own release
/// `<release>-cert` on `cert.<name>.<project>.mqtt.<domain>`, RMQTT asking each device for a
/// certificate of the project CA `__CLIENT_CA__` (key `__CLIENT_CA_KEY__`), stamped beside the
/// broker's own objects when its `deviceAuth` names `clientCertificate`.
pub const CERTIFICATE_KEY: &str = "objects-certificate.yaml";
/// The dedicated ports the edge serves: each one a Traefik entrypoint `mqtt-<port>`. The rest
/// of `DEDICATED_PORTS` stays reserved, so a broker given one of them is not applied.
pub const SERVED_PORTS: std::ops::RangeInclusive<u16> = 18830..=18839;
/// The labels every object of a broker the reconciler made carries, and the ones it retires by.
pub const SELECTOR: &str =
    "app.kubernetes.io/part-of=mqtt-brokers,app.kubernetes.io/managed-by=joinedcontext-portal";
/// The label naming the release an object belongs to.
const INSTANCE: &str = "app.kubernetes.io/instance";
/// The smallest packet a broker takes (T-6043): RMQTT's one `max_packet_size` caps CONNECT as well
/// as PUBLISH, and a device's CONNECT carries a Keycloak token as its password (about 1.2 KB), so a
/// broker of `maxMessageBytes` 1024 refused every device. RMQTT cannot cap PUBLISH apart, so such a
/// broker takes a publish up to this size; the gateway still decides every publish.
pub const CONNECT_ALLOWANCE: u32 = 4096;
/// The client attribute naming the broker a managed client belongs to, `{project}/{name}`.
pub const BROKER_ATTRIBUTE: &str = "joinedcontext.mqttbroker";

/// One broker of the mirror, its manifest read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broker {
    pub project: String,
    pub name: String,
    /// `mqtt-{project}-{name}`: its objects, its ServiceAccount and its Keycloak client.
    pub release: String,
    /// `Err` with the reason when the platform refuses the manifest.
    pub spec: Result<MqttBrokerSpec, String>,
    /// The admin's certificate Secret its custom hostname is served with; `None` while
    /// cert-manager issues one by HTTP-01, and for every other address.
    pub certificate: Option<String>,
    /// The certificate door, when the broker's `deviceAuth` names `clientCertificate`.
    pub door: Option<Door>,
}

/// A broker's certificate door (ADR-N-048 §2.12): its release and the project CA's Secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Door {
    /// `mqtt-{project}-{name}-cert`, bounded: its objects and its ServiceAccount.
    pub release: String,
    /// The Secret in the brokers' namespace holding the project's device client CA certificate.
    pub ca_secret: String,
    /// The key of that Secret holding the certificate, `ca.crt` unless clientCaRef names one.
    pub ca_key: String,
}

impl Broker {
    pub fn label(&self) -> String {
        format!("{}/{}", self.project, self.name)
    }
}

/// Every broker of the mirror, sorted by project and name, each judged as `jcctl validate` judges
/// it (MF-54): its kind's rules, every ServiceAccount its roles name declared in its project, and
/// a dedicated port held by the first broker in that order, the later ones refused with the holder
/// named. A port is the node's, so it is checked across projects.
pub fn declared(mirror: &Mirror) -> Vec<Broker> {
    let mut brokers: Vec<Broker> = mirror
        .namespaces()
        .into_iter()
        .flat_map(|project| {
            let accounts: BTreeSet<String> = mirror
                .list(&project, "ServiceAccount", &ListOptions::default())
                .items
                .into_iter()
                .map(|account| account.metadata.name)
                .collect();
            mirror
                .list(&project, "MqttBroker", &ListOptions::default())
                .items
                .into_iter()
                .map(move |envelope| {
                    let name = envelope.metadata.name;
                    let spec = serde_json::from_value::<MqttBrokerSpec>(envelope.spec)
                        .map_err(|err| format!("spec does not read: {err}"))
                        .and_then(|spec| {
                            spec.validate().map_err(|err| err.to_string())?;
                            known_accounts(&spec, &accounts)?;
                            Ok(spec)
                        });
                    Broker {
                        release: release_name(&project, &name),
                        project: project.clone(),
                        name,
                        spec,
                        certificate: None,
                        door: None,
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect();
    brokers.sort_by(|a, b| (&a.project, &a.name).cmp(&(&b.project, &b.name)));
    judge_allowances(mirror, &mut brokers);
    let mut ports: BTreeMap<u16, String> = BTreeMap::new();
    for broker in &mut brokers {
        let Some(port) = broker.spec.as_ref().ok().and_then(|spec| spec.port) else {
            continue;
        };
        if !SERVED_PORTS.contains(&port) {
            broker.spec = Err(format!(
                "spec.port {port} is reserved but not served: the edge serves {}-{} (ADR-N-048 \
                 §2.5); pick one of those",
                SERVED_PORTS.start(),
                SERVED_PORTS.end()
            ));
            continue;
        }
        match ports.get(&port) {
            Some(holder) => {
                broker.spec = Err(format!(
                    "spec.port {port} is already held by {holder}; pick another port from the \
                     reserved range (MF-54)"
                ))
            }
            None => {
                ports.insert(port, broker.label());
            }
        }
    }
    judge_hostnames(mirror, &mut brokers);
    judge_doors(mirror, &mut brokers);
    brokers
}

/// The certificate door of each broker whose `deviceAuth` names `clientCertificate`, with the
/// client CA its project's allowance names (ADR-N-048 §2.12, T-5875). A door whose release is
/// another broker's (one named `<name>-cert`) refuses the broker: the gateway would read that
/// release as the other broker's token door, and both would write the same objects.
fn judge_doors(mirror: &Mirror, brokers: &mut [Broker]) {
    let releases: BTreeMap<String, String> = brokers
        .iter()
        .map(|broker| (broker.release.clone(), broker.label()))
        .collect();
    for broker in brokers {
        let Ok(spec) = &broker.spec else { continue };
        if !spec
            .effective_device_auth()
            .contains(&MqttDeviceAuth::ClientCertificate)
        {
            continue;
        }
        let release = certificate_release_name(&broker.project, &broker.name);
        let (allowance, _) = crate::mqtt_allowance::effective(mirror, &broker.project);
        let judged = match (releases.get(&release), allowance.client_ca_ref) {
            (Some(holder), _) => Err(format!(
                "spec.deviceAuth clientCertificate needs the release {release} for the \
                 certificate door, which is the broker {holder}'s; rename one of the two"
            )),
            (None, None) => Err(format!(
                "spec.deviceAuth clientCertificate needs the project's clientCaRef: an \
                 organization admin sets spec.quotas.mqtt.clientCaRef on project `{}`",
                broker.project
            )),
            (None, Some(ca)) => ca_secret(&ca.name).map(|ca_secret| Door {
                release,
                ca_secret,
                ca_key: ca.key.unwrap_or_else(|| "ca.crt".to_owned()),
            }),
        };
        match judged {
            Ok(door) => broker.door = Some(door),
            Err(reason) => broker.spec = Err(reason),
        }
    }
}

/// The project CA's Secret, a DNS label in the brokers' namespace that is none of the platform's
/// own `mqtt-…` Secrets, so a quota can never mount another broker's certificate or key.
fn ca_secret(name: &str) -> Result<String, String> {
    if name.starts_with("mqtt-") || jc_core::names::validate_dns1123_label(name).is_err() {
        return Err(format!(
            "the clientCaRef `{name}` is no Secret a broker may mount: name a DNS label that \
             does not start with `mqtt-`, the admin's CA certificate Secret in the brokers' \
             namespace"
        ));
    }
    Ok(name.to_owned())
}

/// A custom hostname is served by one broker of the one project its quota assigns it to, the
/// first project in name order when two quotas list it (ADR-N-048 §2.11, T-5874). The refusal
/// names no other project: the broker's status is read by its own project's members.
fn judge_hostnames(mirror: &Mirror, brokers: &mut [Broker]) {
    let holders = crate::mqtt_allowance::hostname_holders(mirror);
    let mut served: BTreeMap<String, String> = BTreeMap::new();
    for broker in brokers {
        let Some(hostname) = broker
            .spec
            .as_ref()
            .ok()
            .and_then(|spec| spec.hostname.clone())
        else {
            continue;
        };
        let judged = match holders.get(&hostname).and_then(|all| all.first()) {
            None => Err(format!(
                "spec.hostname `{hostname}` is not assigned to project `{}`: an organization \
                 admin adds it to the project's spec.quotas.mqtt.customHostnames",
                broker.project
            )),
            Some(holder) if holder.project != broker.project => Err(format!(
                "spec.hostname `{hostname}` is assigned to another project as well, which \
                 serves it; a hostname serves one project only (ADR-N-048 §2.11): an \
                 organization admin takes it off one of the two"
            )),
            Some(_) if served.contains_key(&hostname) => Err(format!(
                "spec.hostname `{hostname}` is already served by {}; a hostname serves one \
                 broker",
                served[&hostname]
            )),
            Some(holder) => certificate_secret(holder.certificate.as_deref()),
        };
        match judged {
            Ok(certificate) => {
                served.insert(hostname, broker.label());
                broker.certificate = certificate;
            }
            Err(reason) => broker.spec = Err(reason),
        }
    }
}

/// The admin's certificate Secret, a DNS label in the brokers' namespace that is none of the
/// platform's own `mqtt-…` Secrets, so a quota can never serve another broker's certificate.
fn certificate_secret(name: Option<&str>) -> Result<Option<String>, String> {
    let Some(name) = name else { return Ok(None) };
    if name.starts_with("mqtt-") || jc_core::names::validate_dns1123_label(name).is_err() {
        return Err(format!(
            "the certificateRef of this hostname, `{name}`, is no Secret the edge may serve: \
             name a DNS label that does not start with `mqtt-`, the admin's own certificate \
             Secret in the brokers' namespace"
        ));
    }
    Ok(Some(name.to_owned()))
}

/// The project's MQTT allowance, judged in name order like a quota: a broker past what is left
/// is not rendered, and a release it already has keeps running (PF-100, PF-118).
fn judge_allowances(mirror: &Mirror, brokers: &mut [Broker]) {
    let mut allowances: BTreeMap<String, (MqttAllowance, MqttUsage)> = BTreeMap::new();
    for broker in brokers {
        let Ok(spec) = &broker.spec else {
            continue;
        };
        let (allowance, used) = allowances.entry(broker.project.clone()).or_insert_with(|| {
            let (allowance, _) = crate::mqtt_allowance::effective(mirror, &broker.project);
            (allowance, MqttUsage::default())
        });
        match allowance.refusal(&broker.project, used, spec, None) {
            Some(reason) => broker.spec = Err(reason),
            None => used.add(spec),
        }
    }
}

fn known_accounts(spec: &MqttBrokerSpec, accounts: &BTreeSet<String>) -> Result<(), String> {
    for role in &spec.roles {
        if let Some(missing) = role
            .service_accounts
            .iter()
            .find(|account| !accounts.contains(*account))
        {
            return Err(format!(
                "role `{}` names ServiceAccount `{missing}`, which no manifest of this project \
                 declares (MF-54)",
                role.name
            ));
        }
    }
    Ok(())
}

/// The broker clients a ServiceAccount's token names as audiences: every broker of its project
/// whose roles list it or that a binding `mqtt/<broker>/<role>` of the account names (ADR-N-048
/// §2.7, §2.13), whatever else the broker's manifest says, so a device keeps its audience while a
/// typo elsewhere in the manifest is fixed.
pub fn audiences_of(mirror: &Mirror, project: &str, account: &str) -> Vec<String> {
    let bound = mirror
        .get(project, "ServiceAccount", account)
        .and_then(|envelope| serde_json::from_value::<ServiceAccountSpec>(envelope.spec).ok());
    mirror
        .list(project, "MqttBroker", &ListOptions::default())
        .items
        .into_iter()
        .filter(|envelope| {
            serde_json::from_value::<MqttBrokerSpec>(envelope.spec.clone()).is_ok_and(|spec| {
                crate::mqtt_roles::names_account(
                    &envelope.metadata.name,
                    &spec,
                    account,
                    bound.as_ref(),
                )
            })
        })
        .map(|envelope| release_name(project, &envelope.metadata.name))
        .collect()
}

/// What a broker the platform applies is worth a look for (T-5876), one line each naming the
/// broker: a role still listing the deprecated `serviceAccounts`, which `jcctl migrate mqtt-roles`
/// moves to `mqtt/<broker>/<role>` bindings on the accounts, and two roles granting one filter
/// (`jc_core::mqtt::topics::overlaps`).
pub fn warnings(brokers: &[Broker]) -> Vec<String> {
    let mut found = Vec::new();
    for broker in brokers {
        let Ok(spec) = &broker.spec else { continue };
        for role in spec
            .roles
            .iter()
            .filter(|role| !role.service_accounts.is_empty())
        {
            found.push(format!(
                "MQTT broker {} role `{}` still lists serviceAccounts, deprecated since T-5872: \
                 run `jcctl migrate mqtt-roles` to move them to `mqtt/{}/{}` RoleBindings on \
                 each account",
                broker.label(),
                role.name,
                broker.name,
                role.name
            ));
        }
        found.extend(
            jc_core::mqtt::topics::overlaps(&spec.roles)
                .into_iter()
                .map(|overlap| format!("MQTT broker {}: {overlap}", broker.label())),
        );
    }
    found
}

/// Logs each of [`warnings`] the previous cycle did not have: the reconciler judges the brokers
/// every cycle, and a line repeated each cycle hides the next one. `seen` holds the last
/// cycle's warnings only, so it never outgrows the manifests.
pub fn log_warnings(brokers: &[Broker], seen: &mut BTreeSet<String>) {
    for warning in fresh(warnings(brokers), seen) {
        tracing::warn!(%warning, "MQTT broker manifest");
    }
}

/// The `current` warnings not in `seen`; `seen` becomes `current`.
fn fresh(current: Vec<String>, seen: &mut BTreeSet<String>) -> Vec<String> {
    let current: BTreeSet<String> = current.into_iter().collect();
    let new = current.difference(seen).cloned().collect();
    *seen = current;
    new
}

/// The broker's `max_packet_size`: its `maxMessageBytes`, and never under [`CONNECT_ALLOWANCE`].
fn packet_bytes(spec: &MqttBrokerSpec) -> u32 {
    spec.limits.max_message_bytes.max(CONNECT_ALLOWANCE)
}

/// The template's objects for one broker: the sentinels filled, and `checksum/config` the hash of
/// everything else, so a change of any value restarts the broker and nothing else does.
pub fn render(
    template: &str,
    broker: &Broker,
    spec: &MqttBrokerSpec,
) -> Result<Vec<Value>, String> {
    let filled = template
        .replace("__RELEASE__", &broker.release)
        .replace("__PROJECT__", &broker.project)
        .replace("__NAME__", &broker.name)
        .replace("__MAX_BYTES__", &packet_bytes(spec).to_string())
        .replace(
            "__PER_SECOND__",
            &spec.limits.messages_per_second.to_string(),
        )
        .replace("__CONNECTIONS__", &spec.limits.max_connections.to_string());
    let hash = hex(&Sha256::digest(filled.as_bytes()));
    let filled = filled.replace("__CONFIG_HASH__", &hash);
    let mut objects = Vec::new();
    for document in serde_norway::Deserializer::from_str(&filled) {
        let object: Value = serde::Deserialize::deserialize(document)
            .map_err(|err| format!("{TEMPLATE} does not read: {err}"))?;
        if object.is_null() {
            continue;
        }
        let instance = object.pointer(&format!("/metadata/labels/{}", INSTANCE.replace('/', "~1")));
        if instance.and_then(Value::as_str) != Some(broker.release.as_str()) {
            return Err(format!(
                "{TEMPLATE} holds an object not labelled {INSTANCE}={}: {}/{}",
                broker.release,
                object["kind"].as_str().unwrap_or("?"),
                object["metadata"]["name"].as_str().unwrap_or("?"),
            ));
        }
        objects.push(object);
    }
    if objects.is_empty() {
        return Err(format!("{TEMPLATE} holds no object"));
    }
    Ok(objects)
}

/// The three templates of [`TEMPLATE`]: the plain one, and the keys a deployment that predates a
/// door may lack.
#[derive(Debug, Clone, Default)]
pub struct Templates {
    pub plain: String,
    pub port: Option<String>,
    pub hostname: Option<String>,
    pub certificate: Option<String>,
}

/// Every object of one broker: the template's; for a broker with `spec.port` the one of
/// [`PORT_KEY`] with `__PORT__` and `__TLS_REVISION__` filled; for one with `spec.hostname` the
/// one of [`HOSTNAME_KEY`]. A template without the key the broker needs refuses it.
/// `tls_revision` is the Certificate's `status.revision`, a number or nothing yet.
pub fn objects_of(
    templates: &Templates,
    broker: &Broker,
    spec: &MqttBrokerSpec,
    tls_revision: Option<u64>,
) -> Result<Vec<Value>, String> {
    let mut objects = match (&spec.hostname, spec.port) {
        (Some(hostname), _) => hostname_objects(templates, broker, spec, hostname)?,
        (None, None) => render(&templates.plain, broker, spec)?,
        (None, Some(port)) => {
            let port_template = templates.port.as_deref().ok_or_else(|| {
                format!(
                    "{TEMPLATE} has no {PORT_KEY} for spec.port {port}: the deployment renders it"
                )
            })?;
            render(
                &port_template
                    .replace("__PORT__", &port.to_string())
                    .replace(
                        "__TLS_REVISION__",
                        &tls_revision.map(|r| r.to_string()).unwrap_or_default(),
                    ),
                broker,
                spec,
            )?
        }
    };
    fence_routes(&mut objects, broker, spec)?;
    Ok(objects)
}

/// The kind of the per-broker edge filters: a Traefik `MiddlewareTCP` whose `ipAllowList` admits
/// the broker's `allowedSources` and nothing else (PF-129), and one whose `inFlightConn` caps the
/// connections one source address holds at once (T-5888).
pub const SOURCES_KIND: (&str, &str) = ("traefik.io/v1alpha1", "MiddlewareTCP");

/// Fences every route of each of a broker's `IngressRouteTCP`s (8883 by SNI and the dedicated
/// port) with middlewares of the broker's own. A broker with `allowedSources` gets
/// `{release}-sources`, so Traefik closes a connection from any other address before TLS
/// (PF-129). Every broker gets `{release}-connections`, so one address holds at most
/// `limits.connectionsPerAddress` connections and cannot take the broker's `maxConnections`
/// (T-5888). The retirement removes a middleware the broker no longer renders.
fn fence_routes(
    objects: &mut Vec<Value>,
    broker: &Broker,
    spec: &MqttBrokerSpec,
) -> Result<(), String> {
    let ranges = spec.source_ranges().map_err(|err| err.to_string())?;
    // The allow-list goes first: an address outside it is closed before it counts.
    let mut fences = Vec::new();
    if !ranges.is_empty() {
        let ranges: Vec<String> = ranges
            .iter()
            .map(|range| format!("{}/{}", range.network, range.prefix))
            .collect();
        fences.push((
            format!("{}-sources", broker.release),
            json!({ "ipAllowList": { "sourceRange": ranges } }),
        ));
    }
    fences.push((
        format!("{}-connections", broker.release),
        json!({ "inFlightConn": { "amount": spec.limits.connections_per_address() } }),
    ));
    let named: Vec<Value> = fences
        .iter()
        .map(|(name, _)| json!({ "name": name }))
        .collect();
    let mut namespace = None;
    let mut first_route = None;
    for (at, object) in objects.iter_mut().enumerate() {
        if object["kind"] != "IngressRouteTCP" {
            continue;
        }
        namespace =
            namespace.or_else(|| object["metadata"]["namespace"].as_str().map(str::to_owned));
        let routes = object["spec"]["routes"]
            .as_array_mut()
            .ok_or_else(|| format!("{TEMPLATE}'s IngressRouteTCP holds no routes"))?;
        for route in routes {
            route["middlewares"] = json!(named);
            first_route = first_route.or(Some(at));
        }
    }
    // Applied before the routes that name them, so Traefik never meets a missing middleware.
    if let Some(first_route) = first_route {
        let stamped = fences
            .into_iter()
            .map(|(name, spec)| middleware(broker, namespace.as_deref(), &name, spec));
        objects.splice(first_route..first_route, stamped);
    }
    Ok(())
}

/// One of a broker's edge middlewares, labelled as its release so the retirement finds it.
fn middleware(broker: &Broker, namespace: Option<&str>, name: &str, spec: Value) -> Value {
    let mut labels: serde_json::Map<String, Value> = SELECTOR
        .split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.to_owned(), json!(value)))
        .collect();
    labels.insert(INSTANCE.to_owned(), json!(broker.release));
    let mut metadata = json!({ "name": name, "labels": labels });
    if let Some(namespace) = namespace {
        metadata["namespace"] = json!(namespace);
    }
    json!({
        "apiVersion": SOURCES_KIND.0,
        "kind": SOURCES_KIND.1,
        "metadata": metadata,
        "spec": spec,
    })
}

/// The objects of a broker's certificate door: the template of [`CERTIFICATE_KEY`] with the
/// door's release, the CA Secret and the door Certificate's `status.revision` filled, behind the
/// broker's `allowedSources` filter of its own.
pub fn door_objects(
    templates: &Templates,
    broker: &Broker,
    spec: &MqttBrokerSpec,
    door: &Door,
    tls_revision: Option<u64>,
) -> Result<Vec<Value>, String> {
    let template = templates.certificate.as_deref().ok_or_else(|| {
        format!(
            "{TEMPLATE} has no {CERTIFICATE_KEY} for spec.deviceAuth clientCertificate: the \
             deployment renders it"
        )
    })?;
    let at_door = Broker {
        release: door.release.clone(),
        ..broker.clone()
    };
    let mut objects = render(
        &template
            .replace("__CLIENT_CA_KEY__", &door.ca_key)
            .replace("__CLIENT_CA__", &door.ca_secret)
            .replace(
                "__TLS_REVISION__",
                &tls_revision.map(|r| r.to_string()).unwrap_or_default(),
            ),
        &at_door,
        spec,
    )?;
    // The door admits the broker's networks only, as its token release does (PF-129).
    fence_routes(&mut objects, &at_door, spec)?;
    Ok(objects)
}

/// A custom hostname's objects: the route serves `<release>-tls`, which its Certificate's HTTP-01
/// issue writes, or the admin's Secret, and then no Certificate is rendered.
fn hostname_objects(
    templates: &Templates,
    broker: &Broker,
    spec: &MqttBrokerSpec,
    hostname: &str,
) -> Result<Vec<Value>, String> {
    let template = templates.hostname.as_deref().ok_or_else(|| {
        format!(
            "{TEMPLATE} has no {HOSTNAME_KEY} for spec.hostname {hostname}: the deployment \
             renders it"
        )
    })?;
    let secret = broker
        .certificate
        .clone()
        .unwrap_or_else(|| format!("{}-tls", broker.release));
    let mut objects = render(
        &template
            .replace("__HOSTNAME__", hostname)
            .replace("__TLS_SECRET__", &secret),
        broker,
        spec,
    )?;
    if broker.certificate.is_some() {
        objects.retain(|object| object["kind"] != "Certificate");
    }
    Ok(objects)
}

/// What tells two objects of a release apart: kind and name.
fn identity(object: &Value) -> (String, String) {
    (
        object["kind"].as_str().unwrap_or_default().to_owned(),
        object["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

/// The kinds a release is made of, read from the template itself: what a retired broker left.
fn template_kinds(template: &str) -> BTreeSet<(String, String)> {
    serde_norway::Deserializer::from_str(template)
        .filter_map(|document| serde::Deserialize::deserialize(document).ok())
        .filter_map(|object: Value| {
            Some((
                object.get("apiVersion")?.as_str()?.to_owned(),
                object.get("kind")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The brokers' objects in their namespace, by the Portal's in-cluster identity.
pub struct MqttBrokers {
    kube: KubeClient,
    namespace: String,
}

impl MqttBrokers {
    pub fn new(kube: KubeClient, namespace: String) -> Self {
        Self { kube, namespace }
    }

    /// Applies every broker the platform accepts and, when `retire` holds, removes the objects of
    /// every release no manifest names any more, and those an applied broker no longer renders
    /// (a dropped port's route, listener and Server). Answers the failure of each broker that
    /// has one, by `{project}/{name}`, and `*` for one that touched them all.
    pub async fn converge(&self, brokers: &[Broker], retire: bool) -> BTreeMap<String, String> {
        let mut failed = BTreeMap::new();
        let map = match self.kube.get_config_map(&self.namespace, TEMPLATE).await {
            Ok(map) => map.unwrap_or_default(),
            Err(err) => {
                failed.insert("*".to_owned(), format!("reading {TEMPLATE}: {err}"));
                return failed;
            }
        };
        let key = |key: &str| map["data"][key].as_str().map(str::to_owned);
        let Some(plain) = key(TEMPLATE_KEY) else {
            failed.insert(
                "*".to_owned(),
                format!(
                    "{}/{TEMPLATE} has no {TEMPLATE_KEY}: the deployment renders it",
                    self.namespace
                ),
            );
            return failed;
        };
        let templates = Templates {
            plain,
            port: key(PORT_KEY),
            hostname: key(HOSTNAME_KEY),
            certificate: key(CERTIFICATE_KEY),
        };
        // The objects of each broker applied whole: whatever else its release holds is stale.
        let mut applied: BTreeMap<&str, BTreeSet<(String, String)>> = BTreeMap::new();
        for broker in brokers {
            let Ok(spec) = &broker.spec else { continue };
            match self.apply(&templates, broker, spec).await {
                Ok(kept) => {
                    applied.insert(&broker.release, kept);
                }
                Err(err) => {
                    failed.insert(broker.label(), err);
                }
            }
            let Some(door) = &broker.door else { continue };
            match self.apply_door(&templates, broker, spec, door).await {
                Ok(kept) => {
                    applied.insert(&door.release, kept);
                }
                Err(err) => {
                    failed.insert(broker.label(), err);
                }
            }
        }
        if !retire {
            return failed;
        }
        // A refused manifest keeps its release: only a broker no manifest names is gone.
        // A door is held while its broker names clientCertificate and the manifest is accepted;
        // a refused manifest keeps a door it had, as it keeps its release.
        let mut held: BTreeSet<String> = BTreeSet::new();
        for broker in brokers {
            held.insert(broker.release.clone());
            match (&broker.spec, &broker.door) {
                (Ok(_), Some(door)) => held.insert(door.release.clone()),
                (Err(_), _) => held.insert(certificate_release_name(&broker.project, &broker.name)),
                (Ok(_), None) => false,
            };
        }
        let mut kinds = template_kinds(&templates.plain);
        for extra in [&templates.port, &templates.hostname, &templates.certificate]
            .into_iter()
            .flatten()
        {
            kinds.extend(template_kinds(extra));
        }
        // Only a template with an edge route can have stamped a middleware: a cluster without
        // Traefik's CRDs (an nginx ingress class) has no such list to read.
        if kinds.iter().any(|(_, kind)| kind == "IngressRouteTCP") {
            kinds.insert((SOURCES_KIND.0.to_owned(), SOURCES_KIND.1.to_owned()));
        }
        for (api_version, kind) in &kinds {
            let listed = match self
                .kube
                .list_mqtt(api_version, kind, &self.namespace, SELECTOR)
                .await
            {
                Ok(listed) => listed,
                Err(err) => {
                    failed.insert("*".to_owned(), format!("listing {kind}: {err}"));
                    continue;
                }
            };
            for object in listed {
                let release = object["metadata"]["labels"][INSTANCE]
                    .as_str()
                    .unwrap_or_default();
                let Some(name) = object["metadata"]["name"].as_str() else {
                    continue;
                };
                let stale = applied
                    .get(release)
                    .is_some_and(|kept| !kept.contains(&(kind.clone(), name.to_owned())));
                if held.contains(release) && !stale {
                    continue;
                }
                if let Err(err) = self
                    .kube
                    .delete_mqtt(api_version, kind, &self.namespace, name)
                    .await
                {
                    failed.insert("*".to_owned(), format!("removing {kind} {name}: {err}"));
                }
            }
        }
        failed
    }

    /// Applies every object of one broker and answers what it applied, by kind and name.
    async fn apply(
        &self,
        templates: &Templates,
        broker: &Broker,
        spec: &MqttBrokerSpec,
    ) -> Result<BTreeSet<(String, String)>, String> {
        let revision = match spec.port {
            Some(_) => self.tls_revision(&broker.release).await?,
            None => None,
        };
        let objects = objects_of(templates, broker, spec, revision)?;
        for object in &objects {
            self.kube
                .apply_mqtt(&self.namespace, object)
                .await
                .map_err(|err| err.to_string())?;
        }
        Ok(objects.iter().map(identity).collect())
    }

    /// Applies every object of one broker's certificate door and answers what it applied.
    async fn apply_door(
        &self,
        templates: &Templates,
        broker: &Broker,
        spec: &MqttBrokerSpec,
        door: &Door,
    ) -> Result<BTreeSet<(String, String)>, String> {
        let revision = self.tls_revision(&door.release).await?;
        let objects = door_objects(templates, broker, spec, door, revision)?;
        for object in &objects {
            self.kube
                .apply_mqtt(&self.namespace, object)
                .await
                .map_err(|err| err.to_string())?;
        }
        Ok(objects.iter().map(identity).collect())
    }

    /// The `status.revision` of a broker's Certificate, `None` before its first issue.
    async fn tls_revision(&self, release: &str) -> Result<Option<u64>, String> {
        Ok(self
            .certificate(release)
            .await?
            .and_then(|c| c["status"]["revision"].as_u64()))
    }

    /// Each applied broker on a custom hostname whose HTTP-01 certificate is not issued yet, by
    /// `{project}/{name}`, with what the city does next ([`dns_wait`]). A broker whose
    /// Certificate cannot be read is left out: the next wave reads it again.
    pub async fn waiting_for_dns(&self, brokers: &[Broker]) -> BTreeMap<String, String> {
        let mut waiting = BTreeMap::new();
        for broker in brokers {
            let Ok(spec) = &broker.spec else { continue };
            let (Some(hostname), None) = (&spec.hostname, &broker.certificate) else {
                continue;
            };
            let Ok(certificate) = self.certificate(&broker.release).await else {
                continue;
            };
            if let Some(message) = dns_wait(certificate.as_ref(), hostname) {
                waiting.insert(broker.label(), message);
            }
        }
        waiting
    }

    /// A broker's Certificate, `None` when there is none.
    async fn certificate(&self, release: &str) -> Result<Option<Value>, String> {
        let selector = format!("{SELECTOR},{INSTANCE}={release}");
        let listed = self
            .kube
            .list_mqtt(
                "cert-manager.io/v1",
                "Certificate",
                &self.namespace,
                &selector,
            )
            .await
            .map_err(|err| format!("reading the Certificate of {release}: {err}"))?;
        Ok(listed
            .into_iter()
            .find(|c| c["metadata"]["name"].as_str() == Some(release)))
    }
}

/// What a custom hostname's Certificate says the city still has to do, or `None` once it is
/// `Ready`: cert-manager proves the name by HTTP-01 only after the name resolves to the edge.
pub fn dns_wait(certificate: Option<&Value>, hostname: &str) -> Option<String> {
    let ready = certificate
        .and_then(|c| c["status"]["conditions"].as_array())
        .into_iter()
        .flatten()
        .any(|condition| condition["type"] == "Ready" && condition["status"] == "True");
    (!ready).then(|| {
        format!(
            "waiting for DNS: point a CNAME from {hostname} at the broker's platform address; \
             the certificate for {hostname} is issued by HTTP-01 once the name resolves to the \
             edge, and devices then connect on {hostname}:8883"
        )
    })
}

/// The client a broker should have: bearer-only, so it signs in nobody and holds no secret; it is
/// the audience a device's token names (ADR-N-048 §2.7).
pub fn desired(broker: &Broker) -> Value {
    json!({
        "clientId": broker.release,
        "name": format!("MQTT broker {}", broker.label()),
        "protocol": "openid-connect",
        "enabled": true,
        "publicClient": false,
        "bearerOnly": true,
        "standardFlowEnabled": false,
        "implicitFlowEnabled": false,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": false,
        "attributes": {
            MANAGED_BY: MANAGED_VALUE,
            BROKER_ATTRIBUTE: broker.label(),
        },
    })
}

/// The realm's broker clients, as the reconciler's own client may write them.
pub struct MqttClientSync {
    admin: Admin,
}

impl MqttClientSync {
    /// `None` when the issuer is not a realm URL.
    pub fn new(issuer: &str, client_id: String, auth: crate::config::ClientAuth) -> Option<Self> {
        Some(Self {
            admin: Admin::new(issuer, client_id, auth)?,
        })
    }

    /// Brings every declared broker's client to [`desired`] and deletes a managed broker client
    /// no broker names. A client of that id this wave did not make is left alone and reported.
    pub async fn converge(&self, brokers: &[Broker]) -> Vec<ClientOutcome> {
        let token = match self.admin.token().await {
            Ok(token) => token,
            Err(err) => {
                let mut outcome = ClientOutcome::of("*");
                outcome.error = Some(err);
                return vec![outcome];
            }
        };
        let mut outcomes = Vec::new();
        for broker in brokers {
            outcomes.push(self.converge_one(&token, broker).await);
        }
        let listed = match self
            .admin
            .send(
                &token,
                reqwest::Method::GET,
                "/clients?briefRepresentation=false&max=2000",
                None,
            )
            .await
        {
            Ok(listed) => listed.unwrap_or(Value::Null),
            Err(err) => {
                let mut outcome = ClientOutcome::of("*");
                outcome.error = Some(format!("listing the broker clients: {err}"));
                outcomes.push(outcome);
                return outcomes;
            }
        };
        let wanted: BTreeSet<String> = brokers.iter().map(Broker::label).collect();
        for client in listed.as_array().into_iter().flatten() {
            let Some(owner) = client["attributes"][BROKER_ATTRIBUTE].as_str() else {
                continue;
            };
            if !managed(client) || wanted.contains(owner) {
                continue;
            }
            let mut outcome = ClientOutcome::of(owner);
            match client["id"].as_str() {
                Some(uuid) => match self
                    .admin
                    .send(
                        &token,
                        reqwest::Method::DELETE,
                        &format!("/clients/{uuid}"),
                        None,
                    )
                    .await
                {
                    Ok(_) => outcome
                        .drift
                        .push("the broker is gone, so its client was removed".to_owned()),
                    Err(err) => outcome.error = Some(err),
                },
                None => outcome.error = Some("the realm listed a client without an id".to_owned()),
            }
            outcomes.push(outcome);
        }
        outcomes
    }

    async fn converge_one(&self, token: &str, broker: &Broker) -> ClientOutcome {
        let label = broker.label();
        let mut outcome = ClientOutcome::of(&label);
        let want = desired(broker);
        let held = match self.admin.find(token, &broker.release).await {
            Ok(held) => held,
            Err(err) => {
                outcome.error = Some(err);
                return outcome;
            }
        };
        let result = match held {
            None => self
                .admin
                .send(token, reqwest::Method::POST, "/clients", Some(&want))
                .await
                .map(|_| ()),
            Some(client)
                if !managed(&client)
                    || client["attributes"][BROKER_ATTRIBUTE].as_str() != Some(label.as_str()) =>
            {
                Err(format!(
                    "the realm holds a client {} this wave did not create for this broker; it is \
                     left alone and no device token names the broker until it is renamed or \
                     removed (ADR-N-048)",
                    broker.release
                ))
            }
            Some(client) => {
                let found = drift(&want, &client);
                match client["id"].as_str() {
                    _ if found.is_empty() => Ok(()),
                    Some(uuid) => {
                        outcome.drift = found;
                        self.admin
                            .send(
                                token,
                                reqwest::Method::PUT,
                                &format!("/clients/{uuid}"),
                                Some(&want),
                            )
                            .await
                            .map(|_| ())
                    }
                    None => Err(format!(
                        "the realm answered {} without an id",
                        broker.release
                    )),
                }
            }
        };
        if let Err(err) = result {
            outcome.error = Some(err);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};

    #[test]
    fn a_warning_logs_once_and_seen_keeps_only_the_last_cycle() {
        let mut seen = BTreeSet::new();
        let a = || "MQTT broker x: a".to_owned();
        let b = || "MQTT broker x: b".to_owned();
        assert_eq!(fresh(vec![a(), b()], &mut seen), vec![a(), b()]);
        assert_eq!(fresh(vec![a(), b()], &mut seen), Vec::<String>::new());
        assert_eq!(fresh(vec![b()], &mut seen), Vec::<String>::new());
        assert_eq!(seen, BTreeSet::from([b()]));
        assert_eq!(fresh(vec![a(), b()], &mut seen), vec![a()]);
    }

    fn envelope(kind: &str, project: &str, name: &str, spec: Value) -> ResourceEnvelope {
        ResourceEnvelope {
            api_version: API_VERSION.to_owned(),
            kind: kind.to_owned(),
            metadata: ObjectMeta::new(name, project),
            spec,
            status: None,
        }
    }

    fn broker_spec(accounts: &[&str], port: Option<u16>) -> Value {
        let mut spec = json!({
            "roles": [{"name": "camera", "serviceAccounts": accounts, "publish": ["cameras/${clientid}/#"]}],
            "limits": {"maxMessageBytes": 2048},
        });
        if let Some(port) = port {
            spec["port"] = json!(port);
        }
        spec
    }

    fn mirror() -> Mirror {
        let mirror = Mirror::new();
        mirror.upsert(envelope(
            "Organization",
            crate::permissions::ORG_NAMESPACE,
            "hel",
            json!({ "domain": "hel.fi", "limits": { "mqtt": {
                "enabled": true, "brokers": 2, "connections": 20000, "messagesPerSecond": 100000,
                "addresses": ["platform", "dedicatedPort"], "ports": [18830]
            } } }),
        ));
        for (project, account) in [
            ("helsinki", "cam-north"),
            ("helsinki", "cam-south"),
            ("espoo", "cam-north"),
        ] {
            mirror.upsert(envelope(
                "ServiceAccount",
                project,
                account,
                json!({ "roles": [] }),
            ));
        }
        mirror.upsert(envelope(
            "MqttBroker",
            "helsinki",
            "cameras",
            broker_spec(&["cam-north"], Some(18830)),
        ));
        mirror.upsert(envelope(
            "MqttBroker",
            "helsinki",
            "doors",
            broker_spec(&["cam-south"], None),
        ));
        mirror.upsert(envelope(
            "MqttBroker",
            "espoo",
            "cameras",
            broker_spec(&["cam-north"], None),
        ));
        mirror
    }

    const TEMPLATE_TEXT: &str = "\
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/part-of: mqtt-brokers
    app.kubernetes.io/managed-by: joinedcontext-portal
    app.kubernetes.io/instance: __RELEASE__
data:
  rmqtt.toml: |
    listener.tcp.external.max_packet_size = \"__MAX_BYTES__B\"
    listener.tcp.external.max_connections = __CONNECTIONS__
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  template:
    metadata:
      labels:
        joinedcontext.com/project: \"__PROJECT__\"
      annotations:
        checksum/config: __CONFIG_HASH__
---
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  dnsNames: [\"__NAME__.__PROJECT__.mqtt.dev.example\"]
";

    #[test]
    fn two_brokers_in_one_project_and_one_in_another_are_three_releases() {
        let brokers = declared(&mirror());
        let releases: Vec<&str> = brokers.iter().map(|b| b.release.as_str()).collect();
        assert_eq!(
            releases,
            [
                "mqtt-espoo-cameras",
                "mqtt-helsinki-cameras",
                "mqtt-helsinki-doors"
            ]
        );
        assert!(brokers.iter().all(|b| b.spec.is_ok()), "{brokers:?}");
        let hosts: BTreeSet<String> = brokers
            .iter()
            .map(|b| {
                let objects =
                    render(TEMPLATE_TEXT, b, b.spec.as_ref().expect("valid")).expect("renders");
                objects[2]["spec"]["dnsNames"][0]
                    .as_str()
                    .expect("a host")
                    .to_owned()
            })
            .collect();
        assert_eq!(hosts.len(), 3, "no two brokers share a host: {hosts:?}");
    }

    #[test]
    fn a_dedicated_port_is_held_once_across_projects_and_the_holder_named() {
        let mirror = mirror();
        mirror.upsert(envelope(
            "MqttBroker",
            "espoo",
            "cameras",
            broker_spec(&["cam-north"], Some(18830)),
        ));
        let brokers = declared(&mirror);
        let espoo = brokers
            .iter()
            .find(|b| b.project == "espoo")
            .expect("espoo");
        let helsinki = brokers
            .iter()
            .find(|b| b.label() == "helsinki/cameras")
            .expect("helsinki");
        // espoo sorts first, so it holds the port and helsinki is refused.
        assert!(espoo.spec.is_ok());
        let refused = helsinki.spec.as_ref().expect_err("refused");
        assert!(
            refused.contains("18830 is already held by espoo/cameras"),
            "{refused}"
        );
    }

    #[test]
    fn a_reserved_port_the_edge_does_not_serve_leaves_the_broker_unapplied_with_the_range() {
        let mirror = mirror();
        mirror.upsert(envelope(
            "Organization",
            crate::permissions::ORG_NAMESPACE,
            "hel",
            json!({ "domain": "hel.fi", "limits": { "mqtt": {
                "enabled": true, "brokers": 2, "connections": 20000, "messagesPerSecond": 100000,
                "addresses": ["platform", "dedicatedPort"], "ports": [18830, 18845]
            } } }),
        ));
        mirror.upsert(envelope(
            "MqttBroker",
            "espoo",
            "cameras",
            broker_spec(&["cam-north"], Some(18845)),
        ));
        let brokers = declared(&mirror);
        let espoo = brokers
            .iter()
            .find(|b| b.project == "espoo")
            .expect("espoo");
        let refused = espoo.spec.as_ref().expect_err("refused");
        assert!(
            refused.contains("18845 is reserved but not served") && refused.contains("18830-18839"),
            "{refused}"
        );
        let helsinki = brokers
            .iter()
            .find(|b| b.label() == "helsinki/cameras")
            .expect("helsinki");
        assert_eq!(
            helsinki.spec.as_ref().ok().and_then(|s| s.port),
            Some(18830)
        );
    }

    /// [`TEMPLATE_TEXT`] with a port: the pod annotated with the certificate's revision, and
    /// the passthrough route on the port's entrypoint.
    fn port_text() -> String {
        TEMPLATE_TEXT.replace(
            "checksum/config: __CONFIG_HASH__",
            "checksum/config: __CONFIG_HASH__\n        joinedcontext.com/tls-revision: \"__TLS_REVISION__\"",
        ) + "\
---
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata:
  name: __RELEASE__-port
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  entryPoints: [mqtt-__PORT__]
  routes:
    - match: HostSNI(`*`)
  tls:
    passthrough: true
"
    }

    #[test]
    fn a_broker_with_a_port_takes_the_port_template_and_one_without_the_plain_one() {
        let brokers = declared(&mirror());
        let cameras = &brokers[1];
        let mut spec = cameras.spec.clone().expect("valid");
        spec.port = Some(18835);
        let port = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            port: Some(port_text()),
            hostname: None,
            certificate: None,
        };
        let objects = objects_of(&port, cameras, &spec, Some(2)).expect("renders");
        // ConfigMap, Deployment, the per-address cap before the route naming it (T-5888), the
        // port route.
        assert_eq!(objects.len(), 5);
        assert_eq!(objects[3]["kind"], "MiddlewareTCP");
        assert_eq!(objects[4]["metadata"]["name"], "mqtt-helsinki-cameras-port");
        assert_eq!(objects[4]["spec"]["entryPoints"], json!(["mqtt-18835"]));
        let annotation = |objects: &[Value]| {
            objects[1]["spec"]["template"]["metadata"]["annotations"]
                ["joinedcontext.com/tls-revision"]
                .clone()
        };
        assert_eq!(annotation(&objects), json!("2"));
        // Before the first issue there is no revision; a renewal changes the annotation, so the
        // Deployment's pod template, so the broker restarts and reads the new certificate.
        let first = objects_of(&port, cameras, &spec, None).expect("renders");
        assert_eq!(annotation(&first), json!(""));
        let renewed = objects_of(&port, cameras, &spec, Some(3)).expect("renders");
        assert_eq!(annotation(&renewed), json!("3"));
        assert_ne!(objects[1], renewed[1]);
        let plain_only = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            ..Templates::default()
        };
        let err = objects_of(&plain_only, cameras, &spec, None).expect_err("no port template");
        assert!(err.contains("has no objects-port.yaml"), "{err}");
        spec.port = None;
        let plain = objects_of(&port, cameras, &spec, Some(3)).expect("renders");
        assert_eq!(plain.len(), 3);
        assert_eq!(annotation(&plain), Value::Null);
    }

    /// PF-129: a broker with allowedSources gets its own MiddlewareTCP, named on both of its
    /// routes, with the ranges as jc-core reads them; one without the list gets none.
    #[test]
    fn allowed_sources_put_the_brokers_own_middleware_on_each_of_its_routes() {
        let brokers = declared(&mirror());
        let cameras = &brokers[1];
        let mut spec = cameras.spec.clone().expect("valid");
        spec.port = Some(18835);
        let sni = "\
---
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  entryPoints: [mqtts]
  routes:
    - match: HostSNI(`__NAME__.__PROJECT__.mqtt.dev.example`)
";
        let port = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            port: Some(port_text() + sni),
            hostname: None,
            certificate: None,
        };
        let plain = objects_of(&port, cameras, &spec, Some(1)).expect("renders");
        let middlewares = |objects: &[Value]| -> Vec<String> {
            objects
                .iter()
                .filter(|o| o["kind"] == "MiddlewareTCP")
                .map(|o| {
                    o["metadata"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect()
        };
        // Without the list only the per-address cap fences the routes (T-5888).
        assert_eq!(middlewares(&plain), ["mqtt-helsinki-cameras-connections"]);
        assert!(plain
            .iter()
            .filter(|o| o["kind"] == "IngressRouteTCP")
            .all(|o| o["spec"]["routes"][0]["middlewares"]
                == json!([{ "name": "mqtt-helsinki-cameras-connections" }])));

        spec.allowed_sources = vec!["203.0.113.0/24".into(), "2001:0db8::/48".into()];
        let objects = objects_of(&port, cameras, &spec, Some(1)).expect("renders");
        assert_eq!(
            middlewares(&objects),
            [
                "mqtt-helsinki-cameras-sources",
                "mqtt-helsinki-cameras-connections"
            ]
        );
        let middleware = objects
            .iter()
            .find(|o| o["kind"] == "MiddlewareTCP")
            .expect("a middleware");
        assert_eq!(middleware["apiVersion"], "traefik.io/v1alpha1");
        assert_eq!(
            middleware["metadata"]["name"],
            "mqtt-helsinki-cameras-sources"
        );
        assert_eq!(middleware["metadata"]["namespace"], "jc-mqtt");
        assert_eq!(
            middleware["metadata"]["labels"],
            json!({
                "app.kubernetes.io/part-of": "mqtt-brokers",
                "app.kubernetes.io/managed-by": "joinedcontext-portal",
                "app.kubernetes.io/instance": "mqtt-helsinki-cameras",
            })
        );
        assert_eq!(
            middleware["spec"],
            json!({ "ipAllowList": { "sourceRange": ["203.0.113.0/24", "2001:db8::/48"] } })
        );
        let routes: Vec<&Value> = objects
            .iter()
            .filter(|o| o["kind"] == "IngressRouteTCP")
            .collect();
        assert_eq!(routes.len(), 2, "8883 by SNI and the dedicated port");
        for route in routes {
            assert_eq!(
                route["spec"]["routes"][0]["middlewares"],
                json!([
                    { "name": "mqtt-helsinki-cameras-sources" },
                    { "name": "mqtt-helsinki-cameras-connections" },
                ]),
                "{}",
                route["metadata"]["name"]
            );
        }
        // The hash the broker restarts on is the template's alone: a new list moves no pod.
        let hash = |objects: &[Value]| {
            objects[1]["spec"]["template"]["metadata"]["annotations"]["checksum/config"].clone()
        };
        assert_eq!(hash(&plain), hash(&objects));
        // An unreadable list is refused whole, never applied open.
        spec.allowed_sources = vec!["203.0.113.1/24".into()];
        let err = objects_of(&port, cameras, &spec, Some(1)).expect_err("refused");
        assert!(err.contains("spec.allowedSources"), "{err}");
        // A custom hostname broker is fenced the same way on its 8883 route by the city's name.
        spec.allowed_sources = vec!["203.0.113.0/24".into()];
        spec.port = None;
        spec.hostname = Some("mqtt.helsinki.example".into());
        let templates = Templates {
            hostname: Some(hostname_text()),
            ..port
        };
        let objects = objects_of(&templates, cameras, &spec, None).expect("renders");
        assert!(objects.iter().any(|o| o["kind"] == "MiddlewareTCP"));
        let route = objects
            .iter()
            .find(|o| o["kind"] == "IngressRouteTCP")
            .expect("the hostname route");
        assert_eq!(
            route["spec"]["routes"][0]["middlewares"],
            json!([
                { "name": "mqtt-helsinki-cameras-sources" },
                { "name": "mqtt-helsinki-cameras-connections" },
            ])
        );
    }

    /// T-5888: every broker's routes carry its own `inFlightConn` cap, 20 per address by
    /// default and the manifest's `connectionsPerAddress` when it names one.
    #[test]
    fn every_route_caps_the_connections_of_one_address() {
        let brokers = declared(&mirror());
        let cameras = &brokers[1];
        let mut spec = cameras.spec.clone().expect("valid");
        spec.port = Some(18835);
        let templates = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            port: Some(port_text()),
            hostname: None,
            certificate: None,
        };
        let cap = |objects: &[Value]| {
            let middleware = objects
                .iter()
                .find(|o| o["metadata"]["name"] == "mqtt-helsinki-cameras-connections")
                .expect("the per-address cap");
            assert_eq!(middleware["kind"], "MiddlewareTCP");
            assert_eq!(
                middleware["metadata"]["labels"][INSTANCE],
                "mqtt-helsinki-cameras"
            );
            let routes: Vec<&Value> = objects
                .iter()
                .filter(|o| o["kind"] == "IngressRouteTCP")
                .collect();
            assert!(!routes.is_empty());
            for route in routes {
                assert_eq!(
                    route["spec"]["routes"][0]["middlewares"],
                    json!([{ "name": "mqtt-helsinki-cameras-connections" }])
                );
            }
            middleware["spec"].clone()
        };
        let objects = objects_of(&templates, cameras, &spec, Some(1)).expect("renders");
        assert_eq!(cap(&objects), json!({ "inFlightConn": { "amount": 20 } }));
        spec.limits.connections_per_address = Some(50);
        let objects = objects_of(&templates, cameras, &spec, Some(1)).expect("renders");
        assert_eq!(cap(&objects), json!({ "inFlightConn": { "amount": 50 } }));
    }

    /// [`TEMPLATE_TEXT`] on a custom hostname: the Certificate for the city's name, and the
    /// 8883 route by that name serving `__TLS_SECRET__`.
    fn hostname_text() -> String {
        TEMPLATE_TEXT.replace("__NAME__.__PROJECT__.mqtt.dev.example", "__HOSTNAME__")
            + "\
---
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata:
  name: __RELEASE__
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  routes:
    - match: HostSNI(`__HOSTNAME__`)
  tls:
    secretName: __TLS_SECRET__
"
    }

    /// Helsinki's quota assigns `mqtt.helsinki.example` (with the admin's certificate when
    /// `certificate` is set) and `mqtt.shared.example`, which `espoo`, first in name order, holds
    /// too; `cameras` serves `hostname`.
    fn hostname_mirror(hostname: &str, certificate: Option<&str>) -> Mirror {
        let mirror = mirror();
        let mut own = json!({"name": "mqtt.helsinki.example"});
        if let Some(certificate) = certificate {
            own["certificateRef"] = json!({"name": certificate});
        }
        let quota = |hostnames: Value| {
            json!({ "organizationRef": { "name": "hel" }, "quotas": { "mqtt": {
                "enabled": true, "brokers": 3, "connections": 20000, "messagesPerSecond": 100000, "addresses": ["platform", "customHostname"],
                "customHostnames": hostnames
            } } })
        };
        mirror.upsert(envelope(
            "Project",
            crate::permissions::ORG_NAMESPACE,
            "helsinki",
            quota(json!([own, {"name": "mqtt.shared.example"}])),
        ));
        mirror.upsert(envelope(
            "Project",
            crate::permissions::ORG_NAMESPACE,
            "espoo",
            quota(json!([{"name": "mqtt.shared.example"}])),
        ));
        let mut spec = broker_spec(&["cam-north"], None);
        spec["address"] = json!("customHostname");
        spec["hostname"] = json!(hostname);
        mirror.upsert(envelope("MqttBroker", "helsinki", "cameras", spec));
        mirror
    }

    #[test]
    fn a_custom_hostname_gets_its_route_and_an_http01_certificate_or_the_admins_secret() {
        let brokers = declared(&hostname_mirror("mqtt.helsinki.example", None));
        let cameras = brokers
            .iter()
            .find(|b| b.label() == "helsinki/cameras")
            .expect("held");
        let spec = cameras
            .spec
            .clone()
            .expect("the project's own hostname is served");
        assert_eq!(cameras.certificate, None);
        let templates = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            port: None,
            hostname: Some(hostname_text()),
            certificate: None,
        };
        let objects = objects_of(&templates, cameras, &spec, None).expect("renders");
        let kinds: Vec<&str> = objects
            .iter()
            .map(|o| o["kind"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(
            kinds,
            [
                "ConfigMap",
                "Deployment",
                "Certificate",
                "MiddlewareTCP",
                "IngressRouteTCP"
            ]
        );
        assert_eq!(
            objects[2]["spec"]["dnsNames"],
            json!(["mqtt.helsinki.example"])
        );
        assert_eq!(
            objects[4]["spec"]["routes"][0]["match"],
            "HostSNI(`mqtt.helsinki.example`)"
        );
        assert_eq!(
            objects[4]["spec"]["tls"]["secretName"],
            "mqtt-helsinki-cameras-tls"
        );

        // The admin's certificate: the route serves it, and no Certificate is issued.
        let brokers = declared(&hostname_mirror(
            "mqtt.helsinki.example",
            Some("helsinki-mqtt-cert"),
        ));
        let cameras = brokers
            .iter()
            .find(|b| b.label() == "helsinki/cameras")
            .expect("held");
        assert_eq!(cameras.certificate.as_deref(), Some("helsinki-mqtt-cert"));
        let objects = objects_of(&templates, cameras, &spec, None).expect("renders");
        assert!(objects.iter().all(|o| o["kind"] != "Certificate"));
        assert_eq!(
            objects[3]["spec"]["tls"]["secretName"],
            "helsinki-mqtt-cert"
        );

        let plain_only = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            ..Templates::default()
        };
        let err = objects_of(&plain_only, cameras, &spec, None).expect_err("no hostname template");
        assert!(err.contains("has no objects-hostname.yaml"), "{err}");
    }

    #[test]
    fn a_hostname_another_project_holds_or_a_platform_secret_is_refused() {
        let brokers = declared(&hostname_mirror("mqtt.shared.example", None));
        let cameras = brokers
            .iter()
            .find(|b| b.label() == "helsinki/cameras")
            .expect("held");
        let reason = cameras.spec.clone().expect_err("espoo holds it first");
        assert!(
            reason.contains("is assigned to another project as well"),
            "{reason}"
        );
        assert!(
            !reason.contains("espoo"),
            "the holder stays unnamed: {reason}"
        );

        let brokers = declared(&hostname_mirror(
            "mqtt.helsinki.example",
            Some("mqtt-espoo-gates-tls"),
        ));
        let cameras = brokers
            .iter()
            .find(|b| b.label() == "helsinki/cameras")
            .expect("held");
        let reason = cameras
            .spec
            .clone()
            .expect_err("a platform Secret is refused");
        assert!(reason.contains("does not start with `mqtt-`"), "{reason}");

        // Two brokers of the project on one hostname: the first in name order serves it.
        let mirror = hostname_mirror("mqtt.helsinki.example", None);
        let mut spec = broker_spec(&["cam-south"], None);
        spec["address"] = json!("customHostname");
        spec["hostname"] = json!("mqtt.helsinki.example");
        mirror.upsert(envelope("MqttBroker", "helsinki", "doors", spec));
        let brokers = declared(&mirror);
        let doors = brokers
            .iter()
            .find(|b| b.label() == "helsinki/doors")
            .expect("held");
        let reason = doors.spec.clone().expect_err("served by cameras");
        assert!(
            reason.contains("already served by helsinki/cameras"),
            "{reason}"
        );
    }

    #[test]
    fn a_hostname_waits_for_dns_until_its_certificate_is_ready() {
        let waiting = dns_wait(None, "mqtt.city.example").expect("no Certificate yet");
        assert!(
            waiting.starts_with("waiting for DNS: point a CNAME from mqtt.city.example"),
            "{waiting}"
        );
        let pending = json!({"status": {"conditions": [
            {"type": "Ready", "status": "False", "reason": "DoesNotExist"}
        ]}});
        assert!(dns_wait(Some(&pending), "mqtt.city.example").is_some());
        let ready = json!({"status": {"conditions": [{"type": "Ready", "status": "True"}]}});
        assert_eq!(dns_wait(Some(&ready), "mqtt.city.example"), None);
    }

    #[test]
    fn a_broker_past_the_projects_allowance_is_not_rendered_and_the_limit_named() {
        let mirror = mirror();
        mirror.upsert(envelope(
            "Project",
            crate::permissions::ORG_NAMESPACE,
            "helsinki",
            json!({ "organizationRef": { "name": "hel" }, "quotas": { "mqtt": {
                "enabled": true, "brokers": 1,
                "addresses": ["platform", "dedicatedPort"], "ports": [18830]
            } } }),
        ));
        mirror.upsert(envelope(
            "Project",
            crate::permissions::ORG_NAMESPACE,
            "espoo",
            json!({ "organizationRef": { "name": "hel" }, "quotas": { "mqtt": { "enabled": false } } }),
        ));
        let by_label: BTreeMap<String, Result<MqttBrokerSpec, String>> = declared(&mirror)
            .into_iter()
            .map(|b| (b.label(), b.spec))
            .collect();
        assert!(
            by_label["helsinki/cameras"].is_ok(),
            "the first by name fits"
        );
        let past = by_label["helsinki/doors"].as_ref().expect_err("refused");
        assert!(
            past.contains("the project may run 1 MQTT brokers (1 in use)"),
            "{past}"
        );
        let off = by_label["espoo/cameras"].as_ref().expect_err("refused");
        assert!(off.contains("MQTT is off for project espoo"), "{off}");
    }

    #[test]
    fn a_broker_naming_an_unknown_account_or_breaking_its_kind_is_refused_with_the_reason() {
        let mirror = mirror();
        mirror.upsert(envelope(
            "MqttBroker",
            "helsinki",
            "doors",
            broker_spec(&["cam-west"], None),
        ));
        mirror.upsert(envelope(
            "MqttBroker",
            "espoo",
            "cameras",
            json!({ "roles": [] }),
        ));
        mirror.upsert(envelope(
            "MqttBroker",
            "espoo",
            "typo",
            json!({ "rolez": [] }),
        ));
        let by_label: BTreeMap<String, Result<MqttBrokerSpec, String>> = declared(&mirror)
            .into_iter()
            .map(|b| (b.label(), b.spec))
            .collect();
        assert!(by_label["helsinki/doors"]
            .as_ref()
            .expect_err("refused")
            .contains("`cam-west`"));
        assert!(by_label["espoo/cameras"]
            .as_ref()
            .expect_err("refused")
            .contains("at least one role"));
        assert!(by_label["espoo/typo"]
            .as_ref()
            .expect_err("refused")
            .contains("does not read"));
        assert!(by_label["helsinki/cameras"].is_ok());
    }

    #[test]
    fn the_template_is_filled_and_a_change_of_limits_changes_the_hash() {
        let brokers = declared(&mirror());
        let broker = &brokers[1];
        let mut spec = broker.spec.clone().expect("valid");
        let objects = render(TEMPLATE_TEXT, broker, &spec).expect("renders");
        assert_eq!(objects.len(), 3);
        assert_eq!(objects[0]["metadata"]["name"], "mqtt-helsinki-cameras");
        let toml = objects[0]["data"]["rmqtt.toml"].as_str().expect("toml");
        assert!(
            toml.contains("\"4096B\"") && toml.contains("= 1000"),
            "a 2048-byte message cap still takes a token CONNECT: {toml}"
        );
        assert_eq!(
            objects[1]["spec"]["template"]["metadata"]["labels"]["joinedcontext.com/project"],
            "helsinki"
        );
        let hash =
            objects[1]["spec"]["template"]["metadata"]["annotations"]["checksum/config"].clone();
        assert_eq!(hash.as_str().map(str::len), Some(64));
        assert_eq!(
            render(TEMPLATE_TEXT, broker, &spec).expect("renders")[1],
            objects[1],
            "same input, same hash"
        );
        spec.limits.max_message_bytes = 65536;
        let large = render(TEMPLATE_TEXT, broker, &spec).expect("renders");
        let toml = large[0]["data"]["rmqtt.toml"].as_str().expect("toml");
        assert!(toml.contains("\"65536B\""), "{toml}");
        spec.limits.max_connections = 5;
        let changed = render(TEMPLATE_TEXT, broker, &spec).expect("renders");
        assert_ne!(
            changed[1]["spec"]["template"]["metadata"]["annotations"]["checksum/config"],
            hash
        );
    }

    #[test]
    fn a_template_object_of_another_release_or_none_at_all_is_refused() {
        let brokers = declared(&mirror());
        let spec = brokers[0].spec.clone().expect("valid");
        let foreign = format!("{TEMPLATE_TEXT}---\napiVersion: v1\nkind: Service\nmetadata:\n  name: portal\n  labels: {{}}\n");
        assert!(render(&foreign, &brokers[0], &spec)
            .expect_err("refused")
            .contains("portal"));
        assert!(render("---\n", &brokers[0], &spec)
            .expect_err("refused")
            .contains("no object"));
        assert!(render("a: [", &brokers[0], &spec).is_err());
    }

    #[test]
    fn a_retirement_reads_the_kinds_from_the_template_alone() {
        let kinds: Vec<(String, String)> = template_kinds(TEMPLATE_TEXT).into_iter().collect();
        let want = [
            ("apps/v1", "Deployment"),
            ("cert-manager.io/v1", "Certificate"),
            ("v1", "ConfigMap"),
        ];
        assert_eq!(kinds, want.map(|(v, k)| (v.to_owned(), k.to_owned())));
    }

    #[test]
    fn an_account_gets_the_audience_of_every_broker_of_its_project_naming_it() {
        let mirror = mirror();
        assert_eq!(
            audiences_of(&mirror, "helsinki", "cam-north"),
            ["mqtt-helsinki-cameras"]
        );
        assert_eq!(
            audiences_of(&mirror, "helsinki", "cam-south"),
            ["mqtt-helsinki-doors"]
        );
        assert_eq!(
            audiences_of(&mirror, "espoo", "cam-south"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_binding_alone_gives_the_account_the_brokers_audience() {
        let mirror = mirror();
        mirror.upsert(envelope(
            "ServiceAccount",
            "helsinki",
            "air-fleet",
            json!({
                "owner": {"user": "anna"}, "purpose": "air", "credentials": [],
                "roles": [{"role": "mqtt/doors/camera", "scope": {"project": "helsinki"}}]
            }),
        ));
        assert_eq!(
            audiences_of(&mirror, "helsinki", "air-fleet"),
            ["mqtt-helsinki-doors"]
        );
    }

    #[test]
    fn a_role_still_listing_accounts_is_a_deprecation_naming_broker_role_and_the_fix() {
        let found = warnings(&declared(&mirror()));
        let doors: Vec<&String> = found
            .iter()
            .filter(|w| w.starts_with("MQTT broker helsinki/doors role `camera`"))
            .collect();
        assert_eq!(doors.len(), 1, "{found:?}");
        assert!(
            doors[0].contains("jcctl migrate mqtt-roles"),
            "{}",
            doors[0]
        );
        assert!(doors[0].contains("mqtt/doors/camera"), "{}", doors[0]);
        assert_eq!(
            found.len(),
            3,
            "one per legacy role of the three brokers: {found:?}"
        );
    }

    /// `espoo/air` as [`declared`] judges it beside the [`mirror`]'s brokers, every mode allowed.
    fn with_air(spec: Value) -> Broker {
        let mirror = mirror();
        // Every mode, so the table's relay cases are judged by their topics alone.
        mirror.upsert(envelope(
            "Organization",
            crate::permissions::ORG_NAMESPACE,
            "hel",
            json!({ "domain": "hel.fi", "limits": { "mqtt": {
                "enabled": true, "brokers": 2, "connections": 20000, "messagesPerSecond": 100000,
                "addresses": ["platform", "dedicatedPort"], "ports": [18830],
                "modes": ["ingest", "relay", "capture"]
            } } }),
        ));
        mirror.upsert(envelope("MqttBroker", "espoo", "air", spec));
        declared(&mirror)
            .into_iter()
            .find(|broker| broker.label() == "espoo/air")
            .expect("declared")
    }

    #[test]
    fn two_roles_granting_one_filter_are_warned_about() {
        let air = with_air(json!({"roles": [
            {"name": "wide", "publish": ["air/${clientid}/#"]},
            {"name": "narrow", "publish": ["air/${clientid}/pm10"]},
        ]}));
        let found = warnings(&[air]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("MQTT broker espoo/air: "),
            "{}",
            found[0]
        );
        assert!(found[0].contains("covers"), "{}", found[0]);
    }

    /// The validator table jc-core and the gateway run (`topics_cases.json`, copied from the
    /// pinned platform rev, T-5877), through the Portal's two readings of a broker: the proposal
    /// check (`registry::validate_yaml`, step 4b) and the reconciler's [`declared`].
    #[test]
    fn every_case_of_the_shared_topic_table_gets_its_verdict_in_the_portal() {
        #[derive(serde::Deserialize)]
        struct Case {
            case: String,
            spec: Value,
            refused: Option<String>,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../../tests/fixtures/mqtt_topics_cases.json"))
                .expect("the table is JSON");
        assert!(cases.len() >= 10);
        for case in cases {
            let manifest = envelope("MqttBroker", "espoo", "air", case.spec.clone());
            let text = serde_json::to_string(&manifest).expect("json");
            let proposed = jc_core::registry::validate_yaml("MqttBroker", &text)
                .expect("a kind jc-core checks");
            let reconciled = with_air(case.spec).spec;
            match &case.refused {
                None => {
                    assert!(proposed.is_ok(), "{}: {proposed:?}", case.case);
                    assert!(reconciled.is_ok(), "{}: {reconciled:?}", case.case);
                }
                Some(field) => {
                    let proposed = proposed.expect_err(&case.case).to_string();
                    assert!(
                        proposed.contains(field.as_str()),
                        "{}: {proposed}",
                        case.case
                    );
                    let reconciled = reconciled.expect_err(&case.case);
                    assert!(
                        reconciled.contains(field.as_str()),
                        "{}: {reconciled}",
                        case.case
                    );
                }
            }
        }
    }

    #[test]
    fn the_client_is_bearer_only_with_no_secret_and_no_flow() {
        let broker = &declared(&mirror())[0];
        let want = desired(broker);
        assert_eq!(want["clientId"], "mqtt-espoo-cameras");
        assert_eq!(want["bearerOnly"], true);
        for flow in [
            "standardFlowEnabled",
            "directAccessGrantsEnabled",
            "serviceAccountsEnabled",
            "publicClient",
        ] {
            assert_eq!(want[flow], false, "{flow}");
        }
        assert!(want.get("secret").is_none());
        assert_eq!(want["attributes"][BROKER_ATTRIBUTE], "espoo/cameras");
    }

    /// The door's template as the chart renders it, reduced: the ConfigMap naming the CA's
    /// Secret and key, and the pod's revision annotation.
    const DOOR_TEXT: &str = "\
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: __RELEASE__
  labels:
    app.kubernetes.io/instance: __RELEASE__
data:
  ca: \"__CLIENT_CA__/__CLIENT_CA_KEY__\"
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: __RELEASE__
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  template:
    metadata:
      annotations:
        joinedcontext.com/tls-revision: \"__TLS_REVISION__\"
";

    /// Helsinki's quota lets devices use client certificates of the CA in `ca`; `cameras` names
    /// `deviceAuth`, and `extra` is one more broker of helsinki.
    fn door_mirror(ca: &str, device_auth: Value, extra: Option<&str>) -> Mirror {
        let mirror = mirror();
        mirror.upsert(envelope(
            "Project",
            crate::permissions::ORG_NAMESPACE,
            "helsinki",
            json!({ "organizationRef": { "name": "hel" }, "quotas": { "mqtt": {
                "enabled": true, "brokers": 3, "connections": 20000, "messagesPerSecond": 100000,
                "deviceAuth": ["token", "clientCertificate"], "clientCaRef": { "name": ca }
            } } }),
        ));
        let mut spec = broker_spec(&["cam-north"], None);
        spec["deviceAuth"] = device_auth;
        mirror.upsert(envelope("MqttBroker", "helsinki", "cameras", spec));
        if let Some(name) = extra {
            mirror.upsert(envelope(
                "MqttBroker",
                "helsinki",
                name,
                broker_spec(&["cam-north"], None),
            ));
        }
        mirror
    }

    fn helsinki_cameras(brokers: &[Broker]) -> &Broker {
        brokers
            .iter()
            .find(|b| b.label() == "helsinki/cameras")
            .expect("held")
    }

    /// T-5875: a broker whose deviceAuth names clientCertificate gets its certificate door, the
    /// release `<release>-cert` stamped from objects-certificate.yaml with the project's CA; one
    /// with tokens alone gets none, and a template without the key refuses the door by name.
    #[test]
    fn a_certificate_broker_gets_its_door_with_the_projects_client_ca() {
        let brokers = declared(&door_mirror(
            "helsinki-device-ca",
            json!(["token", "clientCertificate"]),
            None,
        ));
        let cameras = helsinki_cameras(&brokers);
        let spec = cameras.spec.clone().expect("accepted");
        let door = cameras.door.clone().expect("a door");
        assert_eq!(
            door,
            Door {
                release: "mqtt-helsinki-cameras-cert".into(),
                ca_secret: "helsinki-device-ca".into(),
                ca_key: "ca.crt".into(),
            }
        );
        let templates = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            certificate: Some(DOOR_TEXT.to_owned()),
            ..Templates::default()
        };
        let objects = door_objects(&templates, cameras, &spec, &door, Some(2)).expect("renders");
        assert_eq!(objects.len(), 2);
        for object in &objects {
            assert_eq!(object["metadata"]["name"], "mqtt-helsinki-cameras-cert");
        }
        assert_eq!(objects[0]["data"]["ca"], "helsinki-device-ca/ca.crt");
        assert_eq!(
            objects[1]["spec"]["template"]["metadata"]["annotations"]
                ["joinedcontext.com/tls-revision"],
            "2"
        );
        let plain_only = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            ..Templates::default()
        };
        let err =
            door_objects(&plain_only, cameras, &spec, &door, None).expect_err("no door template");
        assert!(err.contains("has no objects-certificate.yaml"), "{err}");

        let tokens = declared(&door_mirror("helsinki-device-ca", json!(["token"]), None));
        assert_eq!(helsinki_cameras(&tokens).door, None);
        assert!(helsinki_cameras(&tokens).spec.is_ok());
    }

    /// PF-129 at the door: a broker's allowedSources put the door's own MiddlewareTCP on the
    /// door's route, so a certificate device outside the networks is closed at the edge too.
    #[test]
    fn the_door_admits_only_the_brokers_allowed_sources() {
        let brokers = declared(&door_mirror(
            "helsinki-device-ca",
            json!(["clientCertificate"]),
            None,
        ));
        let cameras = helsinki_cameras(&brokers);
        let mut spec = cameras.spec.clone().expect("accepted");
        spec.allowed_sources = vec!["203.0.113.0/24".into()];
        let door = cameras.door.clone().expect("a door");
        let route = "\
---
apiVersion: traefik.io/v1alpha1
kind: IngressRouteTCP
metadata:
  name: __RELEASE__
  namespace: jc-mqtt
  labels:
    app.kubernetes.io/instance: __RELEASE__
spec:
  entryPoints: [mqtts]
  routes:
    - match: HostSNI(`cert.__NAME__.__PROJECT__.mqtt.dev.example`)
";
        let templates = Templates {
            plain: TEMPLATE_TEXT.to_owned(),
            certificate: Some(DOOR_TEXT.to_owned() + route),
            ..Templates::default()
        };
        let objects = door_objects(&templates, cameras, &spec, &door, None).expect("renders");
        let middleware = objects
            .iter()
            .find(|object| object["metadata"]["name"] == "mqtt-helsinki-cameras-cert-sources")
            .expect("the door's filter");
        assert_eq!(
            middleware["metadata"]["name"],
            "mqtt-helsinki-cameras-cert-sources"
        );
        assert_eq!(
            middleware["metadata"]["labels"][INSTANCE],
            "mqtt-helsinki-cameras-cert"
        );
        assert_eq!(
            middleware["spec"]["ipAllowList"]["sourceRange"],
            json!(["203.0.113.0/24"])
        );
        let route = objects
            .iter()
            .find(|object| object["kind"] == "IngressRouteTCP")
            .expect("the door's route");
        assert_eq!(
            route["spec"]["routes"][0]["middlewares"],
            json!([
                { "name": "mqtt-helsinki-cameras-cert-sources" },
                { "name": "mqtt-helsinki-cameras-cert-connections" },
            ])
        );
    }

    /// A door whose release is another broker's, or a CA Secret that is one of the platform's
    /// own, refuses the broker with the reason; the other broker keeps its release.
    #[test]
    fn a_door_on_another_brokers_release_or_a_platform_secret_is_refused() {
        let brokers = declared(&door_mirror(
            "helsinki-device-ca",
            json!(["clientCertificate"]),
            Some("cameras-cert"),
        ));
        let err = helsinki_cameras(&brokers)
            .spec
            .clone()
            .expect_err("refused");
        assert!(
            err.contains("mqtt-helsinki-cameras-cert") && err.contains("helsinki/cameras-cert"),
            "{err}"
        );
        let other = brokers
            .iter()
            .find(|b| b.label() == "helsinki/cameras-cert")
            .expect("held");
        assert!(other.spec.is_ok() && other.door.is_none());

        let brokers = declared(&door_mirror(
            "mqtt-helsinki-cameras-tls",
            json!(["clientCertificate"]),
            None,
        ));
        let err = helsinki_cameras(&brokers)
            .spec
            .clone()
            .expect_err("refused");
        assert!(
            err.contains("clientCaRef `mqtt-helsinki-cameras-tls`"),
            "{err}"
        );
    }
}
