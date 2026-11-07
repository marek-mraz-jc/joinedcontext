//! Compiling `kind: App` into what runs it and what it may read (T-0227, AP-04, AP-05,
//! AP-25…AP-29, AP-13, AP-15, ADR-N-019).
//!
//! An app declares needs, never grants: `spec.dataNeeds` is the input, one `Endpoint` and one
//! `Policy` per need is the output, and the app reaches context data through nothing else
//! (AP-04). The pod-shaped half is the second half of the same idea: the app container is the
//! pod's only container, reachable from the APISIX edge alone, and the edge's `openid-connect`
//! plugin is the login front, so the application carries no authentication code at all (AP-26,
//! ADR-N-019).
//!
//! Rendering is a pure function of the manifest, the installation settings, the image CI built
//! and the reconciler-owned endpoint slug — spelled out because `apps/mod.rs` also documents
//! `pub mod reconciler;` from the outside, and rustdoc resolves the two doc blocks merged, in
//! the parent scope, where a bare name here is not an item. Nothing here reaches a cluster:
//! `render` returns the objects, and applying them is the caller's business.

use std::collections::BTreeSet;

use argon2::password_hash::rand_core::{OsRng, RngCore};
use jc_core::annotations::GENERATED_BY;
use jc_core::kinds::{
    AppClass, AppLifecycle, AppSpec, AppVisibility, DataNeed, EndpointSlug, Operation,
    OperationRef, Representation,
};
use jc_core::API_VERSION;
use jcctl::loader::{RawManifest, RawMetadata};
use serde_json::{json, Map, Value};

/// The tool the generated manifests name as their origin (MF-08).
pub const GENERATOR: &str = "portal/app-reconciler";

/// The port the app container serves on and the Service publishes; the only port of the pod
/// (AP-26).
///
/// `jcctl::apisix` already routes `/apps/{name}/*` to `app-{name}` on this port, so the Service
/// rendered here and the routing table rendered there have to agree on it.
pub const APP_PORT: u16 = 8080;

/// The address the app container must bind: every interface of the pod, because APISIX opens
/// the connection from another pod (AP-26). The NetworkPolicy is what keeps everyone else out.
pub const APP_ADDRESS: &str = "0.0.0.0";

/// The pod label the edge's own NetworkPolicy selects app pods by for its egress
/// (Deployment/10 §4): one value for every app, the name is in `app.kubernetes.io/name`.
pub const APP_LABEL: &str = "joinedcontext.com/app";
/// Where meshed traffic lands on a pod: the Linkerd inbound proxy, not the service port.
pub(crate) const LINKERD_INBOUND: u16 = 4143;

/// Base32 alphabet of RFC 4648 in the lowercase form [`EndpointSlug`] accepts (EP-02).
const SLUG_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// 26 characters of five bits each: 130 bits, over the 128 EP-02 asks for.
const SLUG_LEN: usize = 26;

/// Where the rendered objects run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The Portal's host, `portal.city.example.com`, where an App's `JC_ME_URL` is answered.
    pub host: String,
    /// The domain every App's own host sits under, `city.example.com`: App `name` is served on
    /// `{name}.apps.{apex}` (AP-133). The host of `JC_PORTAL_APPS_URL`, else the Portal's host
    /// without its `portal.` label.
    pub apex: String,
    /// The context gateway's Service in the cluster (`JC_PORTAL_GATEWAY_URL`), the only address
    /// an App pod reaches its endpoint on (AP-134). `None`: no pod-backed App runs.
    pub gateway_url: Option<String>,
    /// The Portal's own Service in the cluster (`JC_PORTAL_SERVICE_URL`), where an App pod asks
    /// `JC_ME_URL` for its caller's roles (AP-109, AP-134). `None`: no pod-backed App runs.
    pub service_url: Option<String>,
    /// The Portal's own namespace: where the pull Secret is kept and the Portal's
    /// ServiceAccount lives. No App object is read or written here since each project has its
    /// own (AP-116, T-4798).
    pub namespace: String,
    /// The installation's release name, the first part of each project's apps namespace
    /// `{release}-{project}-apps` (AP-116). `None`: no pod-backed App runs.
    pub release: Option<String>,
    /// The Portal's ServiceAccount in [`Settings::namespace`], which the RoleBinding of each
    /// project's apps namespace names (AP-116). `None`: no pod-backed App runs.
    pub service_account: Option<String>,
    /// The organization's domain, which becomes the policy assigner (`did:web:{domain}`).
    pub org_domain: String,
    /// The namespace the installation runs APISIX in, the only one whose pods reach an app pod
    /// (AP-108). `apisix` on an installation that sets nothing; `dev` runs it in `dev`.
    pub apisix_namespace: String,
    /// Where the Portal pushed the images, `{registry}/{organization}`, so an image reference
    /// is composed here and never read from a manifest (AP-108). `None`: no pod-backed App runs.
    pub image_repository: Option<String>,
    /// The `dockerconfigjson` Secret in [`Settings::namespace`] a node pulls app images with, a
    /// forge token that reads packages and nothing else (AP-108).
    pub pull_secret: Option<String>,
    /// The Portal's public base URL when it proxies a basemap (AP-67): a pod App's
    /// `JC_APP_CONFIG` names its project's style under it. `None`: no basemap is configured.
    pub basemap_base: Option<String>,
}

/// A platform Service an App pod calls, as its NetworkPolicy names it: the namespace, from the
/// Service's in-cluster host `{service}.{namespace}.svc…`, and the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterService {
    pub namespace: String,
    pub port: u16,
}

impl ClusterService {
    /// `None` for anything but `http(s)://{service}.{namespace}…[:port]`.
    fn parse(url: Option<&str>) -> Option<Self> {
        let url: url::Url = url?.parse().ok()?;
        let namespace = url
            .host_str()?
            .split('.')
            .nth(1)
            .filter(|namespace| crate::resource::is_dns1123(namespace))?
            .to_owned();
        let port = url.port_or_known_default()?;
        Some(Self { namespace, port })
    }
}

/// The label the gateway chart puts on its pods.
const GATEWAY_POD: &str = "context-gateway-gateway";
/// The label the Portal chart puts on its pods.
const PORTAL_POD: &str = "portal-portal";

/// The networks a declared destination may never reach, whatever its CIDR says (AP-134): the
/// private ranges, carrier-grade NAT and link-local (the cloud metadata address), and their IPv6
/// counterparts.
const PRIVATE_RANGES: [&str; 7] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "169.254.0.0/16",
    "fc00::/7",
    "fe80::/10",
];

/// A CIDR as a 128-bit network: IPv4 in the top 32 bits, with its family and prefix. `None`
/// for anything jc-core would have refused.
fn network(cidr: &str) -> Option<(bool, u128, u8)> {
    let (address, prefix) = cidr.split_once('/')?;
    let prefix: u8 = prefix.parse().ok()?;
    match address.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(v4) if prefix <= 32 => {
            Some((true, u128::from(u32::from(v4)) << 96, prefix))
        }
        std::net::IpAddr::V6(v6) if prefix <= 128 => Some((false, u128::from(v6), prefix)),
        _ => None,
    }
}

/// Whether network `inner` lies inside network `outer` (same family).
fn within(inner: (bool, u128, u8), outer: (bool, u128, u8)) -> bool {
    let mask = |prefix: u8| u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
    inner.0 == outer.0 && inner.2 >= outer.2 && inner.1 & mask(outer.2) == outer.1 & mask(outer.2)
}

/// One declared destination as a NetworkPolicy peer, the private ranges inside it excepted.
/// `None` for a destination that lies wholly inside a private range: nothing of it is reachable.
fn egress_block(cidr: &str) -> Option<Value> {
    let declared = network(cidr)?;
    let private: Vec<(&str, (bool, u128, u8))> = PRIVATE_RANGES
        .iter()
        .filter_map(|range| network(range).map(|net| (*range, net)))
        .collect();
    if private.iter().any(|(_, range)| within(declared, *range)) {
        return None;
    }
    let except: Vec<&str> = private
        .iter()
        .filter(|(_, range)| within(*range, declared))
        .map(|(text, _)| *text)
        .collect();
    let mut block = json!({ "cidr": cidr });
    if !except.is_empty() {
        block["except"] = json!(except);
    }
    Some(json!({ "ipBlock": block }))
}

impl Settings {
    /// The gateway an App pod calls, from [`Settings::gateway_url`] (AP-134).
    pub fn gateway(&self) -> Result<ClusterService, RenderError> {
        ClusterService::parse(self.gateway_url.as_deref()).ok_or(RenderError::NoGateway)
    }

    /// The Portal an App pod asks for its caller's roles, from [`Settings::service_url`]
    /// (AP-109, AP-134).
    pub fn portal_service(&self) -> Result<ClusterService, RenderError> {
        ClusterService::parse(self.service_url.as_deref()).ok_or(RenderError::NoPortalService)
    }

    /// `{release}-{project}-apps`, the namespace a project's pod-backed Apps run in (AP-116).
    pub fn apps_namespace(&self, project: &str) -> Result<String, RenderError> {
        let release = self.release.as_deref().ok_or(RenderError::NoRelease)?;
        let namespace = format!("{release}-{project}-apps");
        jc_core::names::validate_dns1123_label(&namespace).map_err(|_| {
            RenderError::NamespaceName {
                namespace: namespace.clone(),
            }
        })?;
        Ok(namespace)
    }

    /// `{registry}/{organization}/app-{name}@{digest}`, the one image an App runs (AP-108).
    pub fn image_of(&self, name: &str, digest: &str) -> Option<String> {
        self.image_repository
            .as_deref()
            .map(|repository| format!("{repository}/app-{name}@{digest}"))
    }
}

/// A fresh endpoint slug for an app that has none yet (EP-02).
///
/// The slug is the one value the reconciler owns and Git never sees: it addresses the app's
/// endpoint, it is unguessable, and it outlives a single render, so a second reconcile of an
/// unchanged app must pass the same one back rather than mint a new one — otherwise every run
/// would move the endpoint under its own users. The Secret `app-{name}-endpoint` is where it is
/// kept between runs.
pub fn generate_slug() -> EndpointSlug {
    let mut slug_bytes = [0u8; SLUG_LEN];
    OsRng.fill_bytes(&mut slug_bytes);
    // 32 is a divisor of 256, so masking five bits off a random byte draws from the alphabet
    // without bias and without a rejection loop.
    let slug: String = slug_bytes
        .iter()
        .map(|byte| char::from(SLUG_ALPHABET[usize::from(byte & 0x1f)]))
        .collect();
    EndpointSlug::new(&slug).expect("the alphabet and length are EP-02's")
}

/// The Kubernetes objects a pod-backed app needs; `static` apps have none (AP-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workload {
    /// The app container alone, on [`APP_PORT`] (AP-26).
    pub deployment: Value,
    /// `app-{name}` on [`APP_PORT`], the name the APISIX upstream points at.
    pub service: Value,
    /// `app-{name}-endpoint`: the endpoint slug, kept between runs (EP-02).
    pub secret: Value,
    /// Default-deny in both directions, with the two exceptions the app cannot work without
    /// (AP-15).
    pub network_policy: Value,
}

/// Everything one `App` manifest compiles into.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The pod-shaped objects, absent for a `static` app.
    pub workload: Option<Workload>,
    /// `app-{name}`, the one endpoint the app may reach (AP-04).
    pub endpoint: RawManifest,
    /// One policy per data need, in the order the needs are declared (AP-05).
    pub policies: Vec<RawManifest>,
}

/// Why an app did not compile.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The manifest is not an `App`.
    #[error("expected kind App, got {kind}")]
    NotAnApp {
        /// The kind the manifest actually declared.
        kind: String,
    },
    /// The manifest is an App whose spec does not hold.
    #[error("the app spec is not valid: {0}")]
    Spec(#[from] jc_core::Error),
    /// A project-scoped kind without a project.
    #[error("an app carries the project it belongs to in metadata.namespace")]
    NoProject,
    /// Only `preview` and `published` apps run (AP-18, AP-21).
    #[error("an app in state {lifecycle} is not deployed")]
    NotDeployable {
        /// The state the manifest is in.
        lifecycle: String,
    },
    /// A pod-backed app needs the image CI built for it.
    #[error("a {class} app renders a Deployment and needs the image built from its source")]
    NoImage {
        /// The app class that asked for an image.
        class: String,
    },
    /// An image that is not pinned by digest (AP-13).
    #[error("{image} is not pinned by digest; a tag is mutable (AP-13)")]
    UnpinnedImage {
        /// The reference as written.
        image: String,
    },
    /// Data needs reaching into a space the App cannot read: a pod-backed App reads its own space
    /// alone, a `ui` App two further spaces besides (AP-04).
    #[error("an app reads its own space {first}, and only a `ui` app up to two further spaces besides; its data needs also name {second} (AP-04)")]
    SeveralSpaces {
        /// The space the first need names.
        first: String,
        /// The first space that differs from it.
        second: String,
    },
    /// A need on a further space writes or names a role: that space is read through its public
    /// Endpoints and granted nothing (AP-04).
    #[error("the further space {space} is read only through its public endpoints: a need on it names read operations and no roles (AP-04)")]
    FurtherSpace {
        /// The further space.
        space: String,
    },
    /// No release name is configured, so a project's apps namespace cannot be named (AP-116).
    #[error("no release name is configured (JC_PORTAL_RELEASE), so the project's apps namespace cannot be named (AP-116)")]
    NoRelease,
    /// `{release}-{project}-apps` is not a namespace name: too long, most likely.
    #[error("{namespace} is not a namespace name (at most 63 lowercase letters, digits and '-'); a shorter project name fits (AP-116)")]
    NamespaceName {
        /// The name as composed.
        namespace: String,
    },
    /// The endpoint slug read back from the cluster is not a slug.
    #[error("the endpoint slug is not usable: {0}")]
    Slug(jc_core::Error),
    /// No gateway Service is configured, so the pod has no address to reach its endpoint on
    /// inside the cluster (AP-134).
    #[error("no context gateway is configured (JC_PORTAL_GATEWAY_URL, http://context-gateway.<namespace>.svc.cluster.local:8080), so a pod-backed App has no endpoint to call (AP-134)")]
    NoGateway,
    /// No Service of the Portal is configured, so the pod has no address inside the cluster to
    /// ask its caller's roles on: the public host is outside its NetworkPolicy (AP-109, AP-134).
    #[error("no Portal Service is configured (JC_PORTAL_SERVICE_URL, http://portal.<namespace>.svc.cluster.local:8080), so a pod-backed App cannot learn its caller's roles (AP-109, AP-134)")]
    NoPortalService,
}

/// Compiles one `App` manifest into its endpoint, its policies and, unless it is `static`, the
/// pod that serves it.
///
/// `image` is the digest-pinned reference CI built from `spec.source`; a `static` app ignores it.
/// `slug` is the app's endpoint slug, read back from the cluster or freshly generated.
pub fn render(
    manifest: &RawManifest,
    image: Option<&str>,
    slug: &EndpointSlug,
    settings: &Settings,
) -> Result<Rendered, RenderError> {
    let (name, project, spec) = deployable(manifest)?;
    let (endpoint, policies) =
        compiled_grants(manifest, name, project, &spec, slug, &settings.org_domain)?;
    let workload = match spec.class {
        // A wasm App's interface is static and its server the shared host's (AP-148): no pod.
        AppClass::Ui | AppClass::Wasm => None,
        class => {
            let image = image.ok_or_else(|| RenderError::NoImage {
                class: class.to_string(),
            })?;
            let config = app_config(name, project, &endpoint, &spec, slug, settings)?;
            Some(render_workload(
                name, project, &spec, image, slug, settings, &config,
            )?)
        }
    };
    Ok(Rendered {
        workload,
        endpoint,
        policies,
    })
}

/// What an App grants, without what runs it: its Endpoint and one Policy per need, or per role of
/// a role-gated need (AP-04, AP-05, AP-96). The Portal commits these beside the App in the same
/// change, since the gateway reads its endpoints and policies from the repository and nowhere
/// else (CC-61, T-2632).
pub fn grants(
    manifest: &RawManifest,
    slug: &EndpointSlug,
    org_domain: &str,
) -> Result<(RawManifest, Vec<RawManifest>), RenderError> {
    let (name, project, spec) = deployable(manifest)?;
    compiled_grants(manifest, name, project, &spec, slug, org_domain)
}

/// The name, project and valid spec of an App that runs (AP-18, AP-21).
fn deployable(manifest: &RawManifest) -> Result<(&str, &str, AppSpec), RenderError> {
    if manifest.kind != "App" {
        return Err(RenderError::NotAnApp {
            kind: manifest.kind.clone(),
        });
    }
    let spec: AppSpec = serde_json::from_value(manifest.spec.clone())
        .map_err(|err| jc_core::Error::Parse(err.to_string()))?;
    spec.validate()?;

    let name = manifest.metadata.name.as_str();
    let project = manifest
        .metadata
        .namespace
        .as_deref()
        .ok_or(RenderError::NoProject)?;

    match spec.lifecycle {
        AppLifecycle::Preview | AppLifecycle::Published => Ok((name, project, spec)),
        state => Err(RenderError::NotDeployable {
            lifecycle: state.as_str().to_owned(),
        }),
    }
}

fn compiled_grants(
    manifest: &RawManifest,
    name: &str,
    project: &str,
    spec: &AppSpec,
    slug: &EndpointSlug,
    org_domain: &str,
) -> Result<(RawManifest, Vec<RawManifest>), RenderError> {
    let (space, own) = own_needs(spec)?;
    // The endpoint is rendered from the own space's needs alone: a further need's representations
    // are served by that space's public endpoints, not added to this one.
    let own_spec = AppSpec {
        data_needs: own.iter().map(|(_, need)| (*need).clone()).collect(),
        ..spec.clone()
    };
    let mut endpoint = endpoint(name, project, space, &own_spec, slug);
    // The endpoints list named it `app-{name}` and nothing else (T-2759): it carries its App's
    // title, which is what a person knows the app by.
    if let Some(title) = manifest.metadata.rest.get("title") {
        endpoint
            .metadata
            .rest
            .insert("title".to_owned(), title.clone());
    }
    Ok((
        endpoint,
        own.into_iter()
            .flat_map(|(index, need)| policies(name, project, index, need, org_domain))
            .collect(),
    ))
}

/// The App's own space and its needs, each with its position in `dataNeeds`.
type OwnNeeds<'a> = (&'a str, Vec<(usize, &'a DataNeed)>);

/// How many further spaces a `ui` App may read besides its own (AP-04, T-2934).
const FURTHER_SPACES: usize = 2;

/// The App's own space, the first need's, with the needs on it and their positions (AP-04). A
/// `ui` App may name up to two further spaces of its project, read only and holding no role: those
/// needs compile into nothing, since the page reads those spaces through their public Endpoints,
/// which answer anyone already (Architecture/16 §2).
fn own_needs(spec: &AppSpec) -> Result<OwnNeeds<'_>, RenderError> {
    let own = spec.data_needs[0].context_space_ref.name();
    let mut further: Vec<&str> = Vec::new();
    let mut needs = Vec::new();
    for (index, need) in spec.data_needs.iter().enumerate() {
        let space = need.context_space_ref.name();
        if space == own {
            needs.push((index, need));
            continue;
        }
        let seen = further.contains(&space);
        if !matches!(spec.class, AppClass::Ui | AppClass::Wasm)
            || (!seen && further.len() == FURTHER_SPACES)
        {
            return Err(RenderError::SeveralSpaces {
                first: own.to_owned(),
                second: space.to_owned(),
            });
        }
        if need.has_write() || !need.roles.is_empty() {
            return Err(RenderError::FurtherSpace {
                space: space.to_owned(),
            });
        }
        if !seen {
            further.push(space);
        }
    }
    Ok((own, needs))
}

/// The `#jc-config` the static host writes for a `ui` App (AP-95), without `user`, for a pod
/// App's backend to write into its own page with `user` from its `/me` (Architecture/16 §13,
/// AP-126). A pod App reads through its one endpoint, so the list holds that one. The project's
/// basemap style goes in as the static host writes it, so a pod App's map draws on the platform's
/// basemap too (AP-67), and the project's page in the Portal, for the entity panel (SDK-40).
fn app_config(
    name: &str,
    project: &str,
    endpoint: &RawManifest,
    spec: &AppSpec,
    slug: &EndpointSlug,
    settings: &Settings,
) -> Result<Value, RenderError> {
    let space = single_space(spec)?;
    let endpoints = [crate::agents::endpoints::RunEndpoint {
        name: endpoint.metadata.name.clone(),
        slug: slug.as_str().to_owned(),
        space: space.to_owned(),
    }];
    let needs = serde_json::to_value(&spec.data_needs).unwrap_or_default();
    let mut config = json!({
        "slug": slug.as_str(),
        "orgDomain": settings.org_domain,
        "space": space,
        "transport": "origin",
        "appName": name,
        "endpointName": endpoint.metadata.name,
        "endpoints": crate::agents::endpoints::config(&endpoints, &needs),
    });
    if let Some(base) = &settings.basemap_base {
        config["basemap"] = json!(crate::api::basemap::style_url_at(base, project));
    }
    // Where the entity panel links an entity the reader changes in the Portal (SDK-40), as the
    // static host writes it for a `ui` App.
    config["portal"] = json!(format!("https://{}/projects/{project}", settings.host));
    Ok(config)
}

/// The one space every data need must name; two spaces cannot become one endpoint (AP-04).
fn single_space(spec: &AppSpec) -> Result<&str, RenderError> {
    let first = spec.data_needs[0].context_space_ref.name();
    for need in &spec.data_needs[1..] {
        let other = need.context_space_ref.name();
        if other != first {
            return Err(RenderError::SeveralSpaces {
                first: first.to_owned(),
                second: other.to_owned(),
            });
        }
    }
    Ok(first)
}

/// `repo@sha256:<64 hex>`; anything else is a tag, and a tag can be moved under the pod (AP-13).
fn pinned(image: &str) -> bool {
    match image.split_once("@sha256:") {
        Some((repository, digest)) => {
            !repository.is_empty()
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        }
        None => false,
    }
}

fn render_workload(
    name: &str,
    project: &str,
    spec: &AppSpec,
    image: &str,
    slug: &EndpointSlug,
    settings: &Settings,
    config: &Value,
) -> Result<Workload, RenderError> {
    if !pinned(image) {
        return Err(RenderError::UnpinnedImage {
            image: image.to_owned(),
        });
    }

    let namespace = settings.apps_namespace(project)?;
    let gateway = settings.gateway()?;
    let portal = settings.portal_service()?;
    let workload_name = format!("app-{name}");
    let labels = json!({
        "app.kubernetes.io/name": workload_name,
        "app.kubernetes.io/part-of": "joinedcontext",
        "app.kubernetes.io/managed-by": "joinedcontext-portal",
        APP_LABEL: "true",
        "joinedcontext.com/project": project,
    });
    let selector = json!({ "app.kubernetes.io/name": workload_name });
    let secret_name = format!("{workload_name}-endpoint");

    let deployment = json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": object_meta(&workload_name, &namespace, &labels),
        "spec": {
            "replicas": 1,
            "selector": { "matchLabels": selector },
            "template": {
                "metadata": {
                    "labels": labels,
                    "annotations": { "linkerd.io/inject": "enabled" },
                },
                "spec": {
                    // Nothing in the pod talks to the Kubernetes API, and a mounted token is
                    // the first thing a compromised app would use.
                    "automountServiceAccountToken": false,
                    "enableServiceLinks": false,
                    // A numeric user: the image is one layer on an empty base and names none, so
                    // `runAsNonRoot` alone cannot tell the kubelet it holds (AP-108).
                    "securityContext": {
                        "runAsNonRoot": true,
                        "runAsUser": 65532,
                        "runAsGroup": 65532,
                        "seccompProfile": { "type": "RuntimeDefault" },
                    },
                    "imagePullSecrets": settings
                        .pull_secret
                        .iter()
                        .map(|secret| json!({ "name": secret }))
                        .collect::<Vec<_>>(),
                    "containers": [app_container(name, project, image, spec, slug, settings, config)],
                    "volumes": [{ "name": "tmp-app", "emptyDir": {} }],
                },
            },
        },
    });

    let service = json!({
        "apiVersion": "v1",
        "kind": "Service",
        "metadata": object_meta(&workload_name, &namespace, &labels),
        "spec": {
            "type": "ClusterIP",
            "selector": selector,
            "ports": [{
                "name": "http",
                "port": APP_PORT,
                "targetPort": "http",
                "protocol": "TCP",
            }],
        },
    });

    let secret = json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": object_meta(&secret_name, &namespace, &labels),
        "type": "Opaque",
        // The slug is here and not only in the rendered Endpoint because this Secret is what
        // the reconciler owns and reads back: the slug outlives a render, and one regenerated
        // on the next run would move the app's endpoint under its own users (EP-02). No client
        // secret and no cookie secret beside it: the login front is the edge's, not the
        // pod's (AP-27, ADR-N-019).
        "stringData": { "endpoint-slug": slug.as_str() },
    });

    Ok(Workload {
        deployment,
        service,
        network_policy: network_policy(
            &workload_name,
            &namespace,
            settings,
            &gateway,
            &portal,
            &spec.egress,
            &labels,
        ),
        secret,
    })
}

fn object_meta(name: &str, namespace: &str, labels: &Value) -> Value {
    json!({
        "name": name,
        "namespace": namespace,
        "labels": labels,
        "annotations": { GENERATED_BY: GENERATOR },
    })
}

/// The app itself: the pod's only container, on [`APP_PORT`], with a readiness probe on
/// `/healthz` so no traffic reaches it before it can answer (Architecture/16 §5).
///
/// What a generated application is started with (AP-14, AP-28):
///
/// - `JC_BIND_ADDRESS` — where it listens, which is the port the Service routes to.
/// - `JC_BASE_PATH` — the path it is served under, `/`: the App is the whole of its host (AP-133).
/// - `JC_ENDPOINT_URL` — the one Endpoint it may read, on the gateway's Service in the cluster
///   (AP-134).
/// - `JC_ME_URL` — the Portal route that answers the caller's roles in this App, on the Portal's
///   Service in the cluster, called with the edge's `X-Access-Token` as the bearer (AP-109, AP-134).
/// - `JC_ANONYMOUS` — set to `true` for a public app, so its backend treats an absent
///   `X-Access-Token` as normal rather than as a bug.
/// - `JC_APP_CONFIG` — the `#jc-config` object the static host writes for a `ui` App, without
///   `user`, with the project's `basemap` style URL when one is configured (AP-67) and
///   `portal`, the project's page in the Portal for the entity panel's link (SDK-40): the
///   backend writes it into its page with `user` from `JC_ME_URL`, so the App SDK in `ui/`
///   reads its endpoints as it does on the static host (AP-95, AP-126).
///
/// Never a credential: an application calls its Endpoint with the caller's own token.
fn app_container(
    name: &str,
    project: &str,
    image: &str,
    spec: &AppSpec,
    slug: &EndpointSlug,
    settings: &Settings,
    config: &Value,
) -> Value {
    // `render` refuses a pod-backed App before this when there is no gateway to name.
    let gateway = settings.gateway_url.as_deref().unwrap_or_default();
    let portal = settings.service_url.as_deref().unwrap_or_default();
    let mut env = vec![
        json!({ "name": "JC_BIND_ADDRESS", "value": format!("{APP_ADDRESS}:{APP_PORT}") }),
        // The App is the whole of its own host (AP-133).
        json!({ "name": "JC_BASE_PATH", "value": "/" }),
        // The gateway's Service in the cluster, never the public host: the mesh carries the
        // person's token there as mTLS, and the NetworkPolicy can name the gateway's pods
        // (AP-134). A pod-backed App is not rendered without it.
        json!({
            "name": "JC_ENDPOINT_URL",
            "value": format!("{gateway}/api/endpoint/{}/", slug.as_str()),
        }),
        // The Portal's Service in the cluster, for the same reason: the public host is an
        // address the pod's NetworkPolicy never names, so every caller would read as role-less.
        json!({
            "name": "JC_ME_URL",
            "value": format!("{portal}/api/v1/projects/{project}/apps/{name}/me"),
        }),
        json!({ "name": "JC_APP_CONFIG", "value": config.to_string() }),
    ];
    if spec.visibility == AppVisibility::Public {
        // A public app is called by people who never logged in, so the backend has to know that
        // an absent `X-Access-Token` is normal rather than a bug (AP-28).
        env.push(json!({ "name": "JC_ANONYMOUS", "value": "true" }));
    }

    let mut container = json!({
        "name": "app",
        "image": image,
        "imagePullPolicy": "IfNotPresent",
        "env": env,
        "ports": [{ "name": "http", "containerPort": APP_PORT, "protocol": "TCP" }],
        "readinessProbe": {
            "httpGet": { "path": "/healthz", "port": "http" },
            "periodSeconds": 5,
            "timeoutSeconds": 3,
        },
        "securityContext": {
            "readOnlyRootFilesystem": true,
            "allowPrivilegeEscalation": false,
            "capabilities": { "drop": ["ALL"] },
        },
        "resources": {
            "requests": { "cpu": "50m", "memory": "128Mi" },
            "limits": { "cpu": "1", "memory": "512Mi" },
        },
        "volumeMounts": [{ "name": "tmp-app", "mountPath": "/tmp" }],
    });
    // A ui-rust image is the binary at `/app` and nothing else, no entrypoint (AP-105).
    if spec.class == jc_core::kinds::AppClass::UiRust {
        container["command"] = json!(["/app"]);
    }
    container
}

/// Default-deny in both directions: APISIX in; out, DNS, the Linkerd control plane, the
/// gateway's pods, the Portal's pods and the destinations `spec.egress` declares, nothing else
/// (AP-134).
fn network_policy(
    name: &str,
    namespace: &str,
    settings: &Settings,
    gateway: &ClusterService,
    portal: &ClusterService,
    declared: &[jc_core::kinds::AppEgress],
    labels: &Value,
) -> Value {
    let selector = json!({ "app.kubernetes.io/name": name });
    let mut egress = vec![
        // The Linkerd control plane (identity, destination, policy): without it the proxy never
        // learns its inbound policy and the pod never starts.
        json!({
            "to": [{ "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "linkerd" } } }],
            "ports": [
                { "protocol": "TCP", "port": 8080 },
                { "protocol": "TCP", "port": 8086 },
                { "protocol": "TCP", "port": 8090 },
            ],
        }),
        json!({
            "to": [{
                "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": "kube-system" } },
                "podSelector": { "matchLabels": { "k8s-app": "kube-dns" } },
            }],
            "ports": [
                { "protocol": "UDP", "port": 53 },
                { "protocol": "TCP", "port": 53 },
            ],
        }),
        // The one call an App pod makes to the platform, its own endpoint, on the gateway's
        // Service in the cluster: its pods on their port, and on the Linkerd inbound port the
        // meshed connection lands on.
        json!({
            "to": [{
                "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": gateway.namespace } },
                "podSelector": { "matchLabels": { "app.kubernetes.io/name": GATEWAY_POD } },
            }],
            "ports": [
                { "protocol": "TCP", "port": gateway.port },
                { "protocol": "TCP", "port": LINKERD_INBOUND },
            ],
        }),
        // `JC_ME_URL`, the caller's roles in this App (AP-109), on the Portal's Service: its pods
        // on that port and the Linkerd inbound port, nothing else of the Portal.
        json!({
            "to": [{
                "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": portal.namespace } },
                "podSelector": { "matchLabels": { "app.kubernetes.io/name": PORTAL_POD } },
            }],
            "ports": [
                { "protocol": "TCP", "port": portal.port },
                { "protocol": "TCP", "port": LINKERD_INBOUND },
            ],
        }),
    ];
    // What the manifest declares and a publisher approved (PF-71): each network on its own
    // ports, never a private range inside it.
    for destination in declared {
        if let Some(block) = egress_block(&destination.cidr) {
            let ports: Vec<Value> = destination
                .ports
                .iter()
                .map(|port| json!({ "protocol": "TCP", "port": port }))
                .collect();
            egress.push(json!({ "to": [block], "ports": ports }));
        }
    }
    json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": object_meta(name, namespace, labels),
        "spec": {
            "podSelector": { "matchLabels": selector },
            "policyTypes": ["Ingress", "Egress"],
            // Only the gateway may open a connection into the pod, and only on the app's port:
            // the edge is the login front, so the port is not a door for anyone else (AP-26).
            // The pod is meshed, and meshed traffic lands on the Linkerd inbound proxy (4143),
            // not on the app's port, so the one source is admitted on both.
            "ingress": [
                {
                    "from": [{
                        "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": settings.apisix_namespace } },
                        "podSelector": { "matchLabels": { "app.kubernetes.io/name": "apisix" } },
                    }],
                    "ports": [
                        { "protocol": "TCP", "port": APP_PORT },
                        { "protocol": "TCP", "port": LINKERD_INBOUND },
                    ],
                },
                // The proxy's own admin ports, which the kubelet probes; no request reaches
                // the app through them.
                {
                    "ports": [
                        { "protocol": "TCP", "port": 4190 },
                        { "protocol": "TCP", "port": 4191 },
                    ],
                },
            ],
            "egress": egress,
        },
    })
}

/// `app-{name}`: the one endpoint the app reads through (AP-04, AP-05).
fn endpoint(
    name: &str,
    project: &str,
    space: &str,
    spec: &AppSpec,
    slug: &EndpointSlug,
) -> RawManifest {
    let representations: Vec<&str> = {
        let mut declared: BTreeSet<Representation> = spec.representations();
        if declared.is_empty() {
            // An app that names no representation still needs one; NGSI-LD is the canonical
            // surface every other representation projects from (EP-05).
            declared.insert(Representation::NgsiLd);
        }
        // Every Endpoint answers MCP (T-4319, owner 2026-10-16); its audience and Policy still
        // decide who may call it.
        declared.insert(Representation::Mcp);
        declared.iter().map(Representation::as_str).collect()
    };

    let (audience, allowed_projects) = match spec.visibility {
        AppVisibility::Public => ("public", Vec::new()),
        AppVisibility::Organization => ("organization", Vec::new()),
        // `private` and `roles` have no audience of their own in the endpoint model; the owning
        // project is the narrowest one there is, and the app's policies bind who inside it may
        // act (AP-18, AP-96).
        AppVisibility::Project | AppVisibility::Private | AppVisibility::Roles => {
            ("project-list", vec![project])
        }
    };

    let mut endpoint_spec = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": space },
        "slug": slug.as_str(),
        "audience": audience,
        "enabledRepresentations": representations,
    });
    if !allowed_projects.is_empty() {
        endpoint_spec["allowedProjects"] = json!(allowed_projects);
    }
    // An app that names `ical` subscribes people to `ical/v1/events.ics`: a calendar is served
    // only for a listed output (EP-159, T-5208), so it gets the one the preset maps.
    if representations.contains(&Representation::Ical.as_str()) {
        endpoint_spec["representations"] = json!({ "ical": [{
            "name": jc_core::kinds::ical_mapping::LEGACY_OUTPUT,
            "sources": [{ "type": "Event", "preset": "sdm-event" }],
        }] });
    }
    // The app's grants are held on this endpoint and nowhere else of the space: every caller it
    // admits holds its caller role, and the members of an app role that role (AP-96, AP-97).
    endpoint_spec["callerRole"] = json!(true);
    let roles: Vec<Value> = spec
        .roles
        .iter()
        .filter_map(|role| {
            let subjects: Vec<&jc_core::kinds::Subject> = spec
                .access
                .iter()
                .filter(|access| access.role == role.name)
                .flat_map(|access| &access.subjects)
                .collect();
            // A role nobody holds gives nobody anything, and an Endpoint role needs a subject.
            (!subjects.is_empty()).then(|| json!({ "name": role.name, "subjects": subjects }))
        })
        .collect();
    if !roles.is_empty() {
        endpoint_spec["roles"] = json!(roles);
    }
    if let Some(rate) = spec.limits.as_ref().and_then(|l| l.requests_per_minute) {
        endpoint_spec["rateLimits"] = json!({ "requestsPerMinute": rate });
    }
    // Both halves of `spec.limits` are enforced on the app's own endpoint and nowhere else, so
    // an app's downloads never eat into what its author may pull elsewhere (AP-17, EP-44).
    if let Some(rows) = spec.limits.as_ref().and_then(|l| l.max_file_rows) {
        endpoint_spec["fileLimits"] = json!({ "maxFileRows": rows });
    }

    generated(format!("app-{name}"), project, "Endpoint", endpoint_spec)
}

/// One data need, compiled into the grants that carry it (AP-05, AP-96): one Policy to the
/// endpoint's caller role, or, for a need that names roles, one per role to that role's endpoint
/// role, each exactly the need and never more (AP-06).
fn policies(
    name: &str,
    project: &str,
    index: usize,
    need: &DataNeed,
    org_domain: &str,
) -> Vec<RawManifest> {
    // 1-based and in declaration order: stable across runs, so a second reconcile of an
    // unchanged app writes the same names and changes nothing.
    let base = format!("app-{name}-{}", index + 1);
    let holders: Vec<(String, Option<&str>)> = if need.roles.is_empty() {
        vec![(base, None)]
    } else {
        need.roles
            .iter()
            .map(|role| (format!("{base}-{role}"), Some(role.as_str())))
            .collect()
    };
    let parts = by_window(need);
    holders
        .iter()
        .flat_map(|(holder, role)| {
            parts.iter().map(move |(suffix, operations, temporal_q)| {
                policy(
                    format!("{holder}{suffix}"),
                    project,
                    need,
                    operations,
                    temporal_q.as_deref(),
                    assignee(name, project, *role),
                    org_domain,
                )
            })
        })
        .collect()
}

/// A need's time window bounds its history reads alone: CIM 009 has no `timerel` on a
/// current-state query, so one Policy holding the window beside `queryEntity` made every read of
/// the app a broker `400` (T-2672). With a window, the history reads get a Policy of their own,
/// `-history`, that carries it, and every other operation one without it; a window on a need with
/// no history read has nothing to bound.
fn by_window(need: &DataNeed) -> Vec<(&'static str, Vec<OperationRef>, Option<String>)> {
    let Some(window) = temporal_query(need) else {
        return vec![("", need.operations.clone(), None)];
    };
    let history =
        |op: &Operation| matches!(op, Operation::QueryTemporal | Operation::RetrieveTemporal);
    let every: Vec<Operation> = need
        .operations
        .iter()
        .flat_map(|op| match op {
            OperationRef::Single(single) => vec![*single],
            OperationRef::Group(group) => group.operations().to_vec(),
        })
        .collect();
    let (past, present): (Vec<Operation>, Vec<Operation>) = every.into_iter().partition(history);
    let refs = |ops: Vec<Operation>| {
        ops.into_iter()
            .map(OperationRef::Single)
            .collect::<Vec<_>>()
    };
    let mut parts = Vec::new();
    if !present.is_empty() {
        parts.push(("", refs(present), None));
    }
    if !past.is_empty() {
        parts.push(("-history", refs(past), Some(window)));
    }
    parts
}

fn policy(
    policy_name: String,
    project: &str,
    need: &DataNeed,
    operations: &[OperationRef],
    temporal_q: Option<&str>,
    assignee: Value,
    org_domain: &str,
) -> RawManifest {
    let mut information = json!({
        "entities": need
            .types
            .iter()
            .map(|entity_type| json!({ "type": entity_type }))
            .collect::<Vec<_>>(),
    });
    if !need.attrs.is_empty() {
        // ponytail: an app declares attributes as one flat list and nothing here knows which of
        // them are relationships, so both whitelists get the same names — a property name in
        // `relationshipNames` matches no relationship and widens nothing. Split them properly
        // when the space's DataModel is available to the reconciler.
        information["propertyNames"] = json!(need.attrs);
        information["relationshipNames"] = json!(need.attrs);
    }

    let mut policy_spec = json!({
        "contextSpaceRef": { "kind": "ContextSpace", "name": need.context_space_ref.name() },
        "assigner": format!("did:web:{org_domain}"),
        "assignee": assignee,
        "operations": operations,
        "information": [information],
    });
    if let Some(q) = &need.q {
        policy_spec["q"] = json!(q);
    }
    if let Some(scope_q) = scope_query(need) {
        policy_spec["scopeQ"] = json!(scope_q);
    }
    if let Some(temporal_q) = temporal_q {
        policy_spec["temporalQ"] = json!(temporal_q);
    }

    generated(policy_name, project, "Policy", policy_spec)
}

/// Who the grant is made to (AP-07, AP-08, AP-96).
fn assignee(name: &str, project: &str, role: Option<&str>) -> Value {
    // Everyone reaches the data with their own token through the app's endpoint, anonymous
    // callers of a public app included, and holds the grant there alone: the endpoint's caller
    // role, or the role of the app they hold (AP-07, AP-96, AP-97, GW22).
    let endpoint = format!("app-{name}");
    json!({
        "kind": "role",
        "id": jc_core::kinds::endpoint_role(project, &endpoint, role),
    })
}

/// The scope narrowing of a need: its own scope query and its geographic confinement are both
/// scope strings, so two of them are one conjunction rather than a lost constraint (AP-05).
fn scope_query(need: &DataNeed) -> Option<String> {
    let within = need.geo_q.as_ref().map(|geo| geo.within.scope_ref.as_str());
    match (need.scope_q.as_deref(), within) {
        (Some(declared), Some(confined)) => Some(format!("({declared});({confined})")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_owned()),
        (None, None) => None,
    }
}

/// `{ window: P1D }` becomes the temporal query the gateway already speaks (R7, EP-56).
fn temporal_query(need: &DataNeed) -> Option<String> {
    need.temporal_q.as_ref().map(|temporal| {
        let window = temporal
            .window
            .strip_prefix('P')
            .unwrap_or(&temporal.window);
        format!("timerel=after;timeAt=P-{window}")
    })
}

fn generated(name: String, project: &str, kind: &str, spec: Value) -> RawManifest {
    let mut rest = Map::new();
    rest.insert("annotations".to_owned(), json!({ GENERATED_BY: GENERATOR }));
    RawManifest {
        api_version: API_VERSION.to_owned(),
        kind: kind.to_owned(),
        metadata: RawMetadata {
            name,
            namespace: Some(project.to_owned()),
            rest,
        },
        spec,
        status: None,
    }
}
