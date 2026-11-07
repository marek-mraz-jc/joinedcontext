//! The APISIX file the edge serves (ADR-N-030, AP-112, T-2668).
//!
//! Helm renders the platform's shared routes into the ConfigMap `apisix-standalone-base`. Every
//! run, this wave reads that base, adds the routes of each published App whose Keycloak client
//! is in place, and writes the result into the Secret `apisix-standalone-config`, which APISIX
//! mounts and reloads. It writes only when the file changed, so an unchanged run is no reload,
//! and a base change reaches the edge on the next run.
//!
//! Every App is served on a host of its own, `{name}.apps.{domain}` (ADR-N-037, AP-133): the
//! browser's same-origin policy is then what keeps one App's storage, cookies and frames from
//! every other App, the forge and the endpoints. The App's host routes the App and the slugs of
//! the endpoints it reads, nothing else, and its old path on the apex answers a `308` there.
//!
//! An App's routes copy the base's own plugin chains, the `apps-surface` plugin config for the
//! App and `context-endpoint` for its endpoint, and change only the login: the App's client, its
//! secret, its cookie host-only on its host. Header stripping, security headers and rate limits
//! therefore stay where helm renders them. The composed file holds every App's client secret,
//! which is why it is a Secret; nothing here logs it or puts it in an error.
//!
//! The limits are the one thing of helm's entries this changes (ADR-N-035): every
//! `limit-count` whose entry carries the label [`RATE_CLASS`] counts the Organization's
//! `spec.limits.edge.requestsPerMinute` for that class, and the global rule [`EDGE_BODY_RULE`]
//! holds bodies to `spec.limits.edge.maxRequestBodyMegabytes`, so a changed limit reaches the
//! edge on the next run and needs no helm release.

use std::collections::BTreeMap;

use base64::Engine as _;
use jc_core::kinds::org_settings::entry_at;
use jc_core::kinds::{AppLifecycle, OrganizationBounds, OrganizationLimits};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::app_clients::{client_id, ClientSecret};
use crate::apps::kube::{KubeClient, KubeError};
use crate::apps::reconciler::{Settings, APP_PORT};
use crate::store::Mirror;

/// The ConfigMap helm renders the shared routes into.
pub const BASE_CONFIG_MAP: &str = "apisix-standalone-base";
/// The Secret APISIX mounts its rule file from.
pub const SERVED_SECRET: &str = "apisix-standalone-config";
/// The key both objects keep the file under.
pub const FILE_KEY: &str = "apisix.yaml";
/// Set on the Secret once this wave wrote it: from then on helm no longer renders it.
pub const COMPOSED_BY: &str = "joinedcontext.com/composed-by";
const COMPOSED_BY_VALUE: &str = "portal";
/// APISIX standalone reads a file only when it ends with this line.
const END_MARKER: &str = "#END";
const APP_PRIORITY: u64 = 30;
const ENDPOINT_PRIORITY: u64 = 35;
/// A wasm App's server, `/api/*` on its host: above the App's page, below its endpoint and
/// services routes (AP-143).
const SERVER_PRIORITY: u64 = 32;
/// The port of a WASM shard's Service on which the Apps' APIs answer (AP-143, T-3857).
const WASM_HOST_PORT: u16 = 8080;
/// The base's plugin config an App's own route is modelled on: the edge login, the header strip,
/// the rate limit and the security headers of a page.
const APPS_SURFACE: &str = "apps-surface";
/// The base's route and plugin config an App's endpoint route is modelled on; its host is the
/// apex, the domain every App's host sits under.
const CONTEXT_ENDPOINT: &str = "context-endpoint";
/// The base's route whose upstream is the Portal, which serves a `static` App's files.
const PORTAL_UI: &str = "portal-ui";
/// The label of a base entry whose `limit-count` the Organization sets, naming its route class.
pub const RATE_CLASS: &str = "jc-rate-class";

/// Each route class and the setting that sets its rate (ADR-N-035).
const RATE_CLASSES: [&str; 5] = ["web", "api", "dataRead", "dataWrite", "publicEndpoint"];

/// The base's global rule whose `client-control` holds every request body (ADR-N-035).
pub const EDGE_BODY_RULE: &str = "edge-body";

/// What the Organization sets at the edge (ADR-N-035): requests a minute per route class and
/// the largest request body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdgeLimits {
    rates: BTreeMap<&'static str, u32>,
    body_bytes: Option<u64>,
}

/// The organization's value, else the catalog's default, held inside the operator's bound, so a
/// bound lowered after the change was approved still holds. `max`/`min` rather than `clamp`:
/// an operator file with min above max must not panic here.
fn effective(path: &str, own: Option<u32>, bounds: &OrganizationBounds) -> Option<u32> {
    let entry = entry_at(path)?;
    let (min, max) = bounds.range(entry);
    Some(own.or(entry.default)?.min(max.unwrap_or(u32::MAX)).max(min))
}

impl EdgeLimits {
    pub fn of(limits: Option<&OrganizationLimits>, bounds: &OrganizationBounds) -> Self {
        let edge = limits.map(|limits| &limits.edge);
        let set = edge.map(|edge| &edge.requests_per_minute);
        let mut rates = BTreeMap::new();
        for class in RATE_CLASSES {
            let own = set.and_then(|rates| match class {
                "web" => rates.web,
                "api" => rates.api,
                "dataRead" => rates.data_read,
                "dataWrite" => rates.data_write,
                _ => rates.public_endpoint,
            });
            let path = format!("spec.limits.edge.requestsPerMinute.{class}");
            if let Some(rate) = effective(&path, own, bounds) {
                // APISIX refuses a count of 0, and one refused entry stops the whole file.
                rates.insert(class, rate.max(1));
            }
        }
        let own = edge.and_then(|edge| edge.max_request_body_megabytes);
        let body_bytes = effective(jc_core::kinds::org_settings::EDGE_BODY, own, bounds)
            .map(|megabytes| u64::from(megabytes.max(1)) * 1024 * 1024);
        Self { rates, body_bytes }
    }

    /// The rate of one class, `None` for a class the catalog does not know.
    pub fn of_class(&self, class: &str) -> Option<u32> {
        self.rates.get(class).copied()
    }

    /// The largest request body, in bytes.
    pub fn body_bytes(&self) -> Option<u64> {
        self.body_bytes
    }
}

/// Sets every labelled `limit-count` of the base to its class's rate, and the `edge-body` rule to
/// the body. An entry of a class this Portal does not know keeps helm's count.
fn apply_limits(document: &mut Value, limits: &EdgeLimits) {
    for list in ["plugin_configs", "routes"] {
        let Some(items) = document.get_mut(list).and_then(Value::as_array_mut) else {
            continue;
        };
        for item in items {
            let Some(rate) = item["labels"][RATE_CLASS]
                .as_str()
                .and_then(|class| limits.of_class(class))
            else {
                continue;
            };
            if let Some(limit) = item
                .get_mut("plugins")
                .and_then(|plugins| plugins.get_mut("limit-count"))
                .filter(|limit| limit.is_object())
            {
                limit["count"] = json!(rate);
            }
        }
    }
    let Some(bytes) = limits.body_bytes else {
        return;
    };
    let rule = document
        .get_mut("global_rules")
        .and_then(Value::as_array_mut)
        .and_then(|rules| rules.iter_mut().find(|rule| rule["id"] == EDGE_BODY_RULE));
    if let Some(control) = rule
        .and_then(|rule| rule.get_mut("plugins"))
        .and_then(|plugins| plugins.get_mut("client-control"))
        .filter(|control| control.is_object())
    {
        control["max_body_size"] = json!(bytes);
    }
}

/// Where an App's own route sends its requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Upstream {
    /// A `ui-rust` App: its Service `app-{name}` in this namespace, and the App's AP-12
    /// Content-Security-Policy, which its route sets because the App's server sends none of the
    /// platform's (a static App's host writes the policy itself).
    Pod { namespace: String, csp: String },
    /// A `static` App: the Portal's static host, the upstream of `portal-ui`.
    Static,
}

/// One published App, as the edge file needs it.
#[derive(Debug, Clone)]
pub struct EdgeApp {
    pub name: String,
    /// `visibility: public`: an anonymous request passes through (AP-28).
    pub public: bool,
    pub upstream: Upstream,
    pub secret: ClientSecret,
    /// The endpoints the App reads, the only ones its host routes (AP-133).
    pub slugs: Vec<String>,
    /// The names of the Organization's verified domain the App also answers on, each routed
    /// exactly as its host (AP-172).
    pub hostnames: Vec<String>,
    /// A `wasm` App's server: its shard's Service, `host:port`, which serves `/apps/{name}/api`
    /// (AP-143); `None` for any other App, or one not placed on a shard yet.
    pub server: Option<String>,
}

/// The host an App is served on, under the apex (AP-133).
pub fn app_host(name: &str, apex: &str) -> String {
    format!("{name}.apps.{apex}")
}

/// The id part of one of an App's hostnames: short, stable while the name is listed, and the
/// same whatever the order of the list (AP-172).
pub fn hostname_key(hostname: &str) -> String {
    Sha256::digest(hostname.as_bytes())[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Why the base could not be composed. Never carries a client secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ComposeError {
    #[error("the base {BASE_CONFIG_MAP} is not an APISIX file: {0}")]
    Unreadable(String),
    #[error("the base {BASE_CONFIG_MAP} has no {0}, which the App routes are modelled on")]
    Missing(String),
}

/// The composed file, and the Apps that got no route with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composed {
    pub file: String,
    pub skipped: Vec<(String, String)>,
}

fn by_id<'a>(document: &'a Value, list: &str, id: &str) -> Result<&'a Value, ComposeError> {
    document
        .get(list)
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find(|item| item["id"] == id))
        .ok_or_else(|| ComposeError::Missing(format!("{list} entry {id}")))
}

fn ids(document: &Value, list: &str) -> Vec<String> {
    document
        .get(list)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item["id"].as_str().map(str::to_owned))
        .collect()
}

fn push(document: &mut Value, list: &str, item: Value) {
    if let Some(items) = document.get_mut(list).and_then(Value::as_array_mut) {
        items.push(item);
    } else {
        document[list] = json!([item]);
    }
}

/// The session cookie's encryption key for one App: its own, so a cookie of one App decrypts at
/// no other App's route, and derived from the client secret, so it survives restarts and
/// changes when the secret does.
fn session_secret(app: &str, secret: &ClientSecret) -> String {
    let mut hash = Sha256::new();
    hash.update(b"joinedcontext app session\0");
    hash.update(app.as_bytes());
    hash.update(b"\0");
    hash.update(secret.expose().as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The base's `openid-connect` login made the App's own, on its own host (ADR-N-030, AP-29):
/// the cookie is `__Host-`, host-only on `/` and `Secure`, so another App, which may set a
/// cookie for the shared parent domain, cannot plant one of the same name beside it (T-4395).
fn login(template: &Value, host: &str, app: &EdgeApp, unauth_action: &str) -> Value {
    let name = app.name.as_str();
    let mut plugin = template.clone();
    plugin["client_id"] = json!(client_id(name));
    plugin["client_secret"] = json!(app.secret.expose());
    plugin["redirect_uri"] = json!(format!("https://{host}/callback"));
    plugin["logout_path"] = json!("/logout");
    plugin["post_logout_redirect_uri"] = json!(format!("https://{host}/"));
    plugin["unauth_action"] = json!(unauth_action);
    if !plugin["session"].is_object() {
        plugin["session"] = json!({});
    }
    plugin["session"]["secret"] = json!(session_secret(name, &app.secret));
    plugin["session"]["cookie_name"] = json!(format!("__Host-jc_app_{}", name.replace('-', "_")));
    plugin["session"]["cookie_path"] = json!("/");
    plugin["session"]["cookie_secure"] = json!(true);
    if let Some(session) = plugin["session"].as_object_mut() {
        session.remove("cookie_domain");
    }
    plugin
}

/// Sets `proxy-rewrite.regex_uri`, keeping whatever else the template's `proxy-rewrite` does.
fn rewrite_uri(plugins: &mut Value, from: String, to: String) {
    if !plugins["proxy-rewrite"].is_object() {
        plugins["proxy-rewrite"] = json!({});
    }
    plugins["proxy-rewrite"]["regex_uri"] = json!([from, to]);
}

/// Only the App's own origin and the Portal frame an App (AP-122, OPS-34): the App route drops
/// `X-Frame-Options`, whose `SAMEORIGIN` would refuse the Portal. A static App's route adds
/// `frame-ancestors 'self'` plus the Portal's host beside the policy the static host writes: a
/// browser enforces both, so that policy's `connect-src` and the rest stay as sent. A pod App's
/// route sets `policy`, its whole AP-12 policy, `frame-ancestors` included, in place of anything
/// the App's server sends: the manifest decides what the page loads, not the App's code (T-3011).
fn framed_by_portal(plugins: &mut Value, host: &str, policy: Option<&str>) {
    if !plugins["response-rewrite"].is_object() {
        plugins["response-rewrite"] = json!({});
    }
    let rewrite = &mut plugins["response-rewrite"];
    if !rewrite["headers"].is_object() {
        rewrite["headers"] = json!({});
    }
    let headers = &mut rewrite["headers"];
    if let Some(set) = headers.get_mut("set").and_then(Value::as_object_mut) {
        set.retain(|name, _| !name.eq_ignore_ascii_case("x-frame-options"));
    }
    match policy {
        // `set`, not `add`: an `add` entry may hold one `:` only, and the policy's `https:`,
        // `data:` and `blob:` sources need more. APISIX sets after it adds, so the one policy
        // this route sends is this one.
        Some(policy) => {
            if !headers["set"].is_object() {
                headers["set"] = json!({});
            }
            headers["set"]["Content-Security-Policy"] = json!(policy);
        }
        // No scheme: APISIX refuses an `add` entry with a second `:` (its pattern is
        // `^[^:]+:[^:]*[^/]$`) and drops the whole plugin config, so every App route answered
        // 503. A source without a scheme takes the App page's own, https.
        None => {
            let policy = format!("Content-Security-Policy: frame-ancestors 'self' portal.{host}");
            push(headers, "add", json!(policy));
        }
    }
    push(headers, "remove", json!("X-Frame-Options"));
}

/// The base with the Organization's rates and every App's routes added, ending `#END`
/// (AP-112, AP-133, ADR-N-035).
pub fn compose(base: &str, apps: &[EdgeApp], rates: &EdgeLimits) -> Result<Composed, ComposeError> {
    let mut document: Value =
        serde_norway::from_str(base).map_err(|err| ComposeError::Unreadable(err.to_string()))?;
    if !document.is_object() {
        return Err(ComposeError::Unreadable("not a mapping".into()));
    }
    // Before the App routes copy the base's chains, so they count the same rates.
    apply_limits(&mut document, rates);
    let endpoint_route = by_id(&document, "routes", CONTEXT_ENDPOINT)?.clone();
    let portal_route = by_id(&document, "routes", PORTAL_UI)?.clone();
    let surface_plugins = by_id(&document, "plugin_configs", APPS_SURFACE)?["plugins"].clone();
    let endpoint_plugins = by_id(&document, "plugin_configs", CONTEXT_ENDPOINT)?["plugins"].clone();
    let template = surface_plugins
        .get("openid-connect")
        .filter(|plugin| plugin.is_object())
        .ok_or_else(|| ComposeError::Missing(format!("openid-connect on {APPS_SURFACE}")))?
        .clone();
    let apex = endpoint_route["host"]
        .as_str()
        .ok_or_else(|| ComposeError::Missing(format!("host on route {CONTEXT_ENDPOINT}")))?
        .to_owned();
    let static_upstream = portal_route["upstream_id"].clone();
    let endpoint_upstream = endpoint_route["upstream_id"].clone();
    let portal = static_upstream
        .as_str()
        .and_then(|id| by_id(&document, "upstreams", id).ok())
        .and_then(first_node);

    let mut taken: Vec<String> = ["routes", "upstreams", "plugin_configs"]
        .iter()
        .flat_map(|list| ids(&document, list))
        .collect();
    let mut apps: Vec<&EdgeApp> = apps.iter().collect();
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    let mut skipped = Vec::new();
    for app in apps {
        let name = app.name.as_str();
        let own = format!("app-{name}");
        let endpoint = format!("app-{name}-endpoint");
        let moved = format!("app-{name}-moved");
        let services = format!("app-{name}-services");
        let server = format!("app-{name}-server");
        let host = app_host(name, &apex);
        // The App's host, then each hostname of its Organization's domain with the same three
        // routes and a login that returns to it (AP-133, AP-167, AP-172).
        let hosts: Vec<[String; 4]> = std::iter::once([
            host.clone(),
            own.clone(),
            endpoint.clone(),
            services.clone(),
        ])
        .chain(app.hostnames.iter().map(|hostname| {
            let at = format!("app-{name}-at-{}", hostname_key(hostname));
            [
                hostname.clone(),
                at.clone(),
                format!("{at}-endpoint"),
                format!("{at}-services"),
            ]
        }))
        .collect();
        let server_routes: Vec<String> = hosts
            .iter()
            .filter(|_| app.server.is_some())
            .map(|[_, route, _, _]| format!("{route}-server"))
            .collect();
        if let Some(id) = hosts
            .iter()
            .flat_map(|[_, route, endpoint, services]| [route, endpoint, services])
            .chain(&server_routes)
            .chain([&moved])
            .find(|id| taken.contains(id))
        {
            skipped.push((
                name.to_owned(),
                format!("the base or another App already has an entry {id}"),
            ));
            continue;
        }
        // App `foo-endpoint`'s route is App `foo`'s endpoint route by name: the first App by
        // name keeps its ids, so one App's login never replaces another's.
        taken.extend(
            hosts
                .iter()
                .flat_map(|[_, route, endpoint, services]| [route, endpoint, services])
                .chain(&server_routes)
                .chain([&moved, &server])
                .cloned(),
        );

        // A pod App's server and a wasm App's shard serve whoever passes the login, so the edge
        // asks the Portal first whether the App's visibility admits the caller (T-4922).
        let guarded = !app.public && (app.upstream != Upstream::Static || app.server.is_some());
        let admit = match (&portal, guarded) {
            (_, false) => None,
            (Some(portal), true) => Some(admission(portal, name)),
            (None, true) => {
                skipped.push((
                    name.to_owned(),
                    format!(
                        "the base's upstream {PORTAL_UI} names no node to ask who may open the App"
                    ),
                ));
                continue;
            }
        };

        let upstream_id = match &app.upstream {
            Upstream::Pod { namespace, .. } => {
                let mut nodes = Map::new();
                nodes.insert(
                    format!("app-{name}.{namespace}.svc.cluster.local:{APP_PORT}"),
                    json!(1),
                );
                push(
                    &mut document,
                    "upstreams",
                    json!({
                        "id": own,
                        "type": "roundrobin",
                        "nodes": nodes,
                        "timeout": { "connect": 6, "send": 30, "read": 30 },
                    }),
                );
                json!(own)
            }
            Upstream::Static => static_upstream.clone(),
        };
        if let Some(node) = &app.server {
            push(
                &mut document,
                "upstreams",
                json!({
                    "id": server,
                    "type": "roundrobin",
                    "nodes": { node.as_str(): 1 },
                    "timeout": { "connect": 6, "send": 30, "read": 30 },
                }),
            );
        }
        for [host, own, endpoint, services] in &hosts {
            let mut plugins = surface_plugins.clone();
            let policy = match &app.upstream {
                Upstream::Pod { csp, .. } => Some(csp.as_str()),
                Upstream::Static => None,
            };
            framed_by_portal(&mut plugins, &apex, policy);
            plugins["openid-connect"] = login(
                &template,
                host,
                app,
                if app.public { "pass" } else { "auth" },
            );
            if app.upstream == Upstream::Static {
                // The static host keeps a bundle under `/apps/{name}/`; the browser only ever
                // sees the App's host. A bundle built for the old path asks for `/apps/{name}/…`
                // itself, which lands on the same file.
                // ponytail: the optional prefix goes once every App is rebuilt for its host.
                rewrite_uri(
                    &mut plugins,
                    format!("^/(?:apps/{name}/)?(.*)$"),
                    format!("/apps/{name}/$1"),
                );
            } else if let Some(admit) = &admit {
                plugins["forward-auth"] = admit.clone();
            }
            push(
                &mut document,
                "plugin_configs",
                json!({ "id": own, "desc": format!("App {name} behind its own login"), "plugins": plugins }),
            );

            // The endpoint route answers the App's own requests: a request without a session
            // is a 401 for a non-public App, not a redirect a fetch cannot follow. The session's
            // token reaches the gateway as the bearer it verifies, and the prefix is stripped.
            let mut plugins = endpoint_plugins.clone();
            let mut oidc = login(
                &template,
                host,
                app,
                if app.public { "pass" } else { "deny" },
            );
            oidc["access_token_in_authorization_header"] = json!(true);
            plugins["openid-connect"] = oidc;
            // An SDK built before the App had a host sends its calls under `/apps/{name}`; the
            // prefix is stripped, and the slug is still one of the App's own.
            rewrite_uri(
                &mut plugins,
                format!("^(?:/apps/{name})?(/api/endpoint/.*)$"),
                "$1".to_owned(),
            );
            // The session cookie stays at the edge: the gateway takes the bearer and drops any
            // cookie, and the cookie (two chunks, about 6 KB) beside a long id list pushed a read
            // past the gateway's header limit (praha-odpad's isle names answered 431, T-3579).
            // `openid-connect` reads the session before `proxy-rewrite` runs.
            let remove = &mut plugins["proxy-rewrite"]["headers"]["remove"];
            match remove.as_array_mut() {
                Some(list) => list.push(json!("Cookie")),
                None => *remove = json!(["Cookie"]),
            }
            if !app.slugs.is_empty() {
                push(
                    &mut document,
                    "plugin_configs",
                    json!({ "id": endpoint, "desc": format!("App {name}'s endpoint behind its own login"), "plugins": plugins }),
                );
            }
            // A wasm App's server answers the rest of `/api/` on its host, under the same login
            // and with the same headers stripped as the endpoint; its shard routes by the
            // `/apps/{name}/api` prefix. Without this route the static host answered every call
            // of kpi-forecast's and news-topics' servers 404 (T-3579).
            if app.server.is_some() {
                let id = format!("{own}-server");
                let mut plugins = plugins.clone();
                rewrite_uri(
                    &mut plugins,
                    format!("^(?:/apps/{name})?/api/(.*)$"),
                    format!("/apps/{name}/api/$1"),
                );
                if let Some(admit) = &admit {
                    plugins["forward-auth"] = admit.clone();
                }
                push(
                    &mut document,
                    "plugin_configs",
                    json!({ "id": id, "desc": format!("App {name}'s server behind its own login"), "plugins": plugins }),
                );
                push(
                    &mut document,
                    "routes",
                    json!({
                        "id": id,
                        "name": id,
                        "desc": format!("App {name}'s server"),
                        "uris": ["/api/*", format!("/apps/{name}/api/*")],
                        "host": host,
                        "priority": SERVER_PRIORITY,
                        "vars": [["uri", "!", "~~", format!("^(?:/apps/{name})?/api/(?:endpoint|services)/")]],
                        "upstream_id": server,
                        "plugin_config_id": id,
                    }),
                );
            }

            push(
                &mut document,
                "routes",
                json!({
                    "id": own,
                    "name": own,
                    "desc": format!("App {name}"),
                    "uri": "/*",
                    "host": host,
                    "priority": APP_PRIORITY,
                    "upstream_id": upstream_id,
                    "plugin_config_id": own,
                }),
            );
            // The App's own endpoints and no other slug: a page of one App cannot reach another
            // App's data through its host (AP-133). An App that reads none gets no route, so
            // every `/api/endpoint/` path on its host is the App's own 404. The delivery path
            // the base refuses on `/api/endpoint/` is refused here too and never reaches the
            // gateway (R46).
            if !app.slugs.is_empty() {
                let uris: Vec<String> = app
                    .slugs
                    .iter()
                    .flat_map(|slug| {
                        [
                            format!("/api/endpoint/{slug}/*"),
                            format!("/apps/{name}/api/endpoint/{slug}/*"),
                        ]
                    })
                    .collect();
                push(
                    &mut document,
                    "routes",
                    json!({
                        "id": endpoint,
                        "name": endpoint,
                        "desc": format!("App {name}'s endpoint"),
                        "uris": uris,
                        "host": host,
                        "priority": ENDPOINT_PRIORITY,
                        "vars": [["uri", "!", "~~", format!("^(?:/apps/{name})?/api/endpoint/[^/]+/egress/")]],
                        "upstream_id": endpoint_upstream,
                        "plugin_config_id": endpoint,
                    }),
                );
            }
            // The platform services on each of the App's hosts, answered by the Portal for every App
            // shape (AP-167, API/06 §3): `/*` would hand them to a pod App's own server, and a fetch
            // cannot follow a login redirect. The session's token reaches the Portal as the bearer.
            let mut plugins = endpoint_plugins.clone();
            let mut oidc = login(
                &template,
                host,
                app,
                if app.public { "pass" } else { "deny" },
            );
            oidc["access_token_in_authorization_header"] = json!(true);
            plugins["openid-connect"] = oidc;
            rewrite_uri(
                &mut plugins,
                format!("^(?:/apps/{name})?/api/services/(.*)$"),
                format!("/apps/{name}/api/services/$1"),
            );
            push(
                &mut document,
                "plugin_configs",
                json!({ "id": services, "desc": format!("App {name}'s platform services"), "plugins": plugins }),
            );
            push(
                &mut document,
                "routes",
                json!({
                    "id": services,
                    "name": services,
                    "desc": format!("App {name}'s platform services"),
                    "uris": ["/api/services/*", format!("/apps/{name}/api/services/*")],
                    "host": host,
                    "priority": ENDPOINT_PRIORITY,
                    "upstream_id": static_upstream.clone(),
                    "plugin_config_id": services,
                }),
            );
        }
        // The old address: a `308` to the host that reaches no upstream and runs no login, so
        // a bookmark still arrives and the apex never sets an App's cookie (ADR-N-037 §3).
        push(
            &mut document,
            "routes",
            json!({
                "id": moved,
                "name": moved,
                "desc": format!("App {name} moved to its own host"),
                "uris": [format!("/apps/{name}"), format!("/apps/{name}/*")],
                "host": apex,
                "priority": APP_PRIORITY,
                "plugins": {
                    "redirect": {
                        "regex_uri": [format!("^/apps/{name}/?(.*)$"), format!("https://{host}/$1")],
                        "ret_code": 308,
                        "append_query_string": true,
                    },
                },
            }),
        );
    }
    let body = serde_norway::to_string(&document)
        .map_err(|err| ComposeError::Unreadable(err.to_string()))?;
    Ok(Composed {
        file: format!("{body}{END_MARKER}\n"),
        skipped,
    })
}

/// The first node of an upstream, `host:port`, in either form APISIX takes (a map or a list).
fn first_node(upstream: &Value) -> Option<String> {
    match &upstream["nodes"] {
        Value::Object(nodes) => nodes.keys().next().cloned(),
        Value::Array(nodes) => nodes.first().and_then(|node| {
            Some(format!(
                "{}:{}",
                node["host"].as_str()?,
                node["port"].as_u64()?
            ))
        }),
        _ => None,
    }
}

/// The `forward-auth` that asks the Portal at `portal` whether App `name` admits the caller: the
/// login's token goes along, a refusal reaches the browser as the static host wrote it, and a
/// Portal that does not answer refuses (T-4922).
// ponytail: one Portal call per request of a non-public pod App; cache by token if it shows.
fn admission(portal: &str, name: &str) -> Value {
    json!({
        "uri": format!("http://{portal}/edge/apps/{name}/admit"),
        "request_method": "GET",
        "request_headers": ["Authorization", "X-Access-Token", "Accept", "Accept-Language"],
        "client_headers": ["Content-Type", "Cache-Control", "Content-Security-Policy"],
        "timeout": 3000,
        "keepalive": true,
        "allow_degradation": false,
        "status_on_error": 503,
    })
}

/// The published Apps whose client is in place, as the edge file routes them.
pub fn edge_apps(
    mirror: &Mirror,
    secrets: &BTreeMap<String, ClientSecret>,
    settings: &Settings,
    wasm_host: Option<&str>,
) -> (Vec<EdgeApp>, Vec<(String, String)>) {
    let mut apps = Vec::new();
    let mut skipped = Vec::new();
    for envelope in mirror.matching(|envelope| {
        envelope.kind == "App"
            && envelope.spec.get("lifecycle").and_then(Value::as_str)
                == Some(AppLifecycle::Published.as_str())
    }) {
        let name = envelope.metadata.name.clone();
        let Some(secret) = secrets.get(&name) else {
            // No client yet (or a blocked one, AP-114): the shared surface keeps answering.
            continue;
        };
        let project = envelope.metadata.namespace.clone().unwrap_or_default();
        let public = envelope.spec.get("visibility").and_then(Value::as_str) == Some("public");
        if apps.iter().any(|app: &EdgeApp| app.name == name) {
            continue;
        }
        let spec: jc_core::kinds::AppSpec = match serde_json::from_value(envelope.spec.clone()) {
            Ok(spec) => spec,
            Err(err) => {
                skipped.push((name, format!("the App does not parse: {err}")));
                continue;
            }
        };
        // Read through jc-core, so `ui-rust` and the old `fullstack` both reach the pod (AP-124).
        let upstream = match spec.class {
            jc_core::kinds::AppClass::UiRust => match settings.apps_namespace(&project) {
                Ok(namespace) => Upstream::Pod {
                    namespace,
                    csp: pod_policy(&spec, settings, &project),
                },
                Err(err) => {
                    skipped.push((name, err.to_string()));
                    continue;
                }
            },
            // A wasm App's interface is served as a ui App's is (AP-148).
            jc_core::kinds::AppClass::Ui | jc_core::kinds::AppClass::Wasm => Upstream::Static,
        };
        let slugs = crate::apps::static_host::served_endpoints(mirror, &project, &name, &spec)
            .into_iter()
            .map(|endpoint| endpoint.slug)
            .collect();
        let hostnames =
            crate::apps::hostnames::routed(mirror, &project, &name, &envelope.spec, &settings.apex);
        let shard = envelope.status.as_ref().and_then(|status| status.shard);
        let server = match (spec.class, wasm_host, shard) {
            (jc_core::kinds::AppClass::Wasm, Some(namespace), Some(shard)) => Some(format!(
                "jc-wasm-host-{shard}.{namespace}.svc.cluster.local:{WASM_HOST_PORT}"
            )),
            _ => None,
        };
        apps.push(EdgeApp {
            name,
            public,
            upstream,
            secret: secret.clone(),
            slugs,
            hostnames,
            server,
        });
    }
    (apps, skipped)
}

/// A pod App's AP-12 policy, as the static host writes it for a static App with the same
/// manifest: framed by the Portal's host, and the project's basemap route when one is configured.
fn pod_policy(spec: &jc_core::kinds::AppSpec, settings: &Settings, project: &str) -> String {
    let portal = format!("https://{}", settings.host);
    let basemap = settings
        .basemap_base
        .as_deref()
        .map(|base| crate::api::basemap::route_prefix_at(base, project));
    crate::apps::static_host::content_security_policy(spec, Some(&portal), basemap.as_deref())
}

/// What one run did to the served file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeOutcome {
    /// The Secret already held this file.
    Unchanged { apps: usize },
    /// The Secret was rewritten; APISIX reloads it.
    Written { apps: usize },
    /// Nothing was written, and why. The edge keeps serving the last file.
    Failed(String),
}

/// The served file of one installation, in the APISIX namespace.
pub struct EdgeFile {
    kube: KubeClient,
    /// Where APISIX runs, and both objects live.
    namespace: String,
    /// The operator's bounds the Organization's rates are held inside (ADR-N-035).
    bounds: OrganizationBounds,
    /// The WASM shards' namespace (`JC_PORTAL_WASM_HOST_NAMESPACE`): a wasm App's server is
    /// routed only when it is set.
    wasm_host: Option<String>,
}

/// A kube error as a sentence that names the object and the status, never the body sent.
fn said(object: &str, err: &KubeError) -> String {
    match err {
        KubeError::Api { status, .. } => {
            format!("the API server answered {status} for {object}")
        }
        other => format!("{object}: {other}"),
    }
}

impl EdgeFile {
    pub fn new(kube: KubeClient, namespace: String) -> Self {
        Self {
            kube,
            namespace,
            bounds: OrganizationBounds::default(),
            wasm_host: None,
        }
    }

    /// The WASM shards' namespace, so a wasm App's `/api` reaches its shard (AP-143).
    pub fn with_wasm_host(mut self, namespace: Option<String>) -> Self {
        self.wasm_host = namespace;
        self
    }

    pub fn wasm_host(&self) -> Option<&str> {
        self.wasm_host.as_deref()
    }

    /// The operator's bounds (`JC_ORGANIZATION_BOUNDS`) in place of the built-in ones.
    pub fn with_bounds(mut self, bounds: OrganizationBounds) -> Self {
        self.bounds = bounds;
        self
    }

    /// Composes the file for these Apps and the Organization's limits, and writes it when it
    /// differs from what is served.
    pub async fn converge(
        &self,
        apps: &[EdgeApp],
        limits: Option<&OrganizationLimits>,
    ) -> (EdgeOutcome, Vec<(String, String)>) {
        let rates = EdgeLimits::of(limits, &self.bounds);
        let base = match self
            .kube
            .get_config_map(&self.namespace, BASE_CONFIG_MAP)
            .await
        {
            Ok(Some(map)) => map["data"][FILE_KEY].as_str().map(str::to_owned),
            Ok(None) => None,
            Err(err) => {
                return (
                    EdgeOutcome::Failed(said(&format!("ConfigMap {BASE_CONFIG_MAP}"), &err)),
                    Vec::new(),
                )
            }
        };
        let Some(base) = base else {
            return (
                EdgeOutcome::Failed(format!(
                    "the ConfigMap {BASE_CONFIG_MAP} in {} has no {FILE_KEY}; the apisix configuration chart renders it",
                    self.namespace
                )),
                Vec::new(),
            );
        };
        let composed = match compose(&base, apps, &rates) {
            Ok(composed) => composed,
            Err(err) => return (EdgeOutcome::Failed(err.to_string()), Vec::new()),
        };
        let routed = apps.len() - composed.skipped.len();
        let served = match self
            .kube
            .get("v1", "Secret", &self.namespace, SERVED_SECRET)
            .await
        {
            Ok(Some(secret)) => secret,
            Ok(None) => {
                return (
                    EdgeOutcome::Failed(format!(
                        "the Secret {SERVED_SECRET} in {} is not there; the apisix configuration chart seeds it",
                        self.namespace
                    )),
                    composed.skipped,
                )
            }
            Err(err) => {
                return (
                    EdgeOutcome::Failed(said(&format!("Secret {SERVED_SECRET}"), &err)),
                    composed.skipped,
                )
            }
        };
        let engine = base64::engine::general_purpose::STANDARD;
        let current = served["data"][FILE_KEY]
            .as_str()
            .and_then(|encoded| engine.decode(encoded).ok());
        let marked = served["metadata"]["annotations"][COMPOSED_BY] == COMPOSED_BY_VALUE;
        if marked && current.as_deref() == Some(composed.file.as_bytes()) {
            return (EdgeOutcome::Unchanged { apps: routed }, composed.skipped);
        }
        let mut next = served;
        if let Some(object) = next.as_object_mut() {
            object.remove("stringData");
        }
        next["data"] = json!({ FILE_KEY: engine.encode(composed.file.as_bytes()) });
        if !next["metadata"]["annotations"].is_object() {
            next["metadata"]["annotations"] = json!({});
        }
        next["metadata"]["annotations"][COMPOSED_BY] = json!(COMPOSED_BY_VALUE);
        match self.kube.replace(&next).await {
            Ok(()) => (EdgeOutcome::Written { apps: routed }, composed.skipped),
            Err(err) => (
                EdgeOutcome::Failed(said(&format!("Secret {SERVED_SECRET}"), &err)),
                composed.skipped,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"plugin_configs:
  - id: apps-surface
    desc: "Static apps"
    plugins:
      request-id:
        include_in_response: true
      openid-connect:
        client_id: edge
        client_secret: ${{EDGE_CLIENT_SECRET}}
        discovery: https://idm.city.example/realms/city/.well-known/openid-configuration
        unauth_action: auth
        redirect_uri: https://city.example/apps/callback
        logout_path: /apps/logout
        session:
          secret: ${{OIDC_SESSION_SECRET}}
          cookie_name: __Host-jc_edge_apps
          cookie_path: /
          idling_timeout: 3600
          absolute_timeout: 36000
      response-rewrite:
        headers:
          set:
            X-Content-Type-Options: nosniff
            X-Frame-Options: SAMEORIGIN
  - id: context-endpoint
    plugins:
      serverless-pre-function:
        phase: rewrite
        functions: ["return function() end"]
      proxy-rewrite:
        headers:
          set:
            X-Forwarded-Proto: https
upstreams:
  - id: portal-ui
    type: roundrobin
    nodes:
      "portal.jc.svc.cluster.local:8080": 1
  - id: context-endpoint
    type: roundrobin
    nodes:
      "context-gateway.jc.svc.cluster.local:8080": 1
routes:
  - id: portal-ui
    uri: /*
    host: portal.city.example
    priority: 1
    upstream_id: portal-ui
  - id: context-endpoint
    uri: /api/endpoint/*
    host: city.example
    priority: 20
    upstream_id: context-endpoint
    plugin_config_id: context-endpoint
#END
"#;

    /// A pod App's policy as `edge_apps` computes it, with a declared source and the basemap.
    const POD_POLICY: &str = "default-src 'self'; base-uri 'self'; object-src 'none'; \
        script-src 'self'; connect-src 'self' https://api.example.fi \
        https://portal.city.example/api/v1/projects/helsinki/basemap/; \
        frame-ancestors https://portal.city.example";

    fn pod(namespace: &str) -> Upstream {
        Upstream::Pod {
            namespace: namespace.into(),
            csp: POD_POLICY.into(),
        }
    }

    fn app(name: &str, public: bool, upstream: Upstream) -> EdgeApp {
        EdgeApp {
            name: name.into(),
            public,
            upstream,
            secret: ClientSecret::from(format!("secret-of-{name}")),
            slugs: vec![format!("slug-of-{name}")],
            hostnames: Vec::new(),
            server: None,
        }
    }

    fn parsed(file: &str) -> Value {
        serde_norway::from_str(file).expect("the composed file is YAML")
    }

    #[test]
    fn an_app_whose_ids_another_app_takes_is_skipped_and_every_id_stays_unique() {
        let composed = compose(
            BASE,
            &[
                app("foo-endpoint", false, pod("jc-helsinki-apps")),
                app("foo", false, pod("jc-helsinki-apps")),
            ],
            &EdgeLimits::default(),
        )
        .expect("composes");
        assert_eq!(
            composed.skipped,
            vec![(
                "foo-endpoint".to_owned(),
                "the base or another App already has an entry app-foo-endpoint".to_owned()
            )]
        );
        let file = parsed(&composed.file);
        for list in ["routes", "upstreams", "plugin_configs"] {
            let all = ids(&file, list);
            let unique: std::collections::BTreeSet<&String> = all.iter().collect();
            assert_eq!(unique.len(), all.len(), "{list}: {all:?}");
        }
    }

    #[test]
    fn no_app_leaves_the_base_as_it_is_and_ends_the_file() {
        let composed = compose(BASE, &[], &EdgeLimits::default()).expect("composes");
        assert!(composed.file.ends_with("\n#END\n"), "{}", composed.file);
        assert_eq!(
            parsed(&composed.file),
            parsed(BASE),
            "the base changed on the way"
        );
        assert!(composed.file.contains("${{EDGE_CLIENT_SECRET}}"));
    }

    #[test]
    fn the_endpoint_route_keeps_the_headers_the_base_strips_beside_the_cookie() {
        let base = BASE.replace(
            "          set:\n            X-Forwarded-Proto: https\n",
            "          set:\n            X-Forwarded-Proto: https\n          remove: [X-Userinfo]\n",
        );
        let composed = compose(
            &base,
            &[app("air-quality", false, pod("jc-helsinki-apps"))],
            &EdgeLimits::default(),
        )
        .expect("composes");
        let file = parsed(&composed.file);
        let plugins = &by_id(&file, "plugin_configs", "app-air-quality-endpoint")
            .expect("its plugins")["plugins"];
        assert_eq!(
            plugins["proxy-rewrite"]["headers"]["remove"],
            json!(["X-Userinfo", "Cookie"])
        );
        // The App's own page keeps its cookie: only the endpoint route drops it.
        let page =
            &by_id(&file, "plugin_configs", "app-air-quality").expect("its plugins")["plugins"];
        assert!(page["proxy-rewrite"]["headers"]["remove"].is_null());
    }

    /// T-3579: a wasm App's `/api` reaches its shard under the App's own login, with the
    /// session's token as the bearer and the cookie kept at the edge; the endpoint and the
    /// services keep their own routes. Before, only the page route matched and the static host
    /// answered every server call 404.
    /// T-4922: the pod and the wasm shard of a non-public App answer only after the Portal admits
    /// the caller; the static host checks by itself, and a public App asks nobody.
    #[test]
    fn a_non_public_pod_or_wasm_server_asks_the_portal_who_may_open_it() {
        let mut wasm = app("kpi-forecast", false, Upstream::Static);
        wasm.server = Some("jc-wasm-host-0.jc-apps.svc.cluster.local:8080".into());
        let mut open_wasm = app("news-topics", true, Upstream::Static);
        open_wasm.server = Some("jc-wasm-host-1.jc-apps.svc.cluster.local:8080".into());
        let apps = [
            app("air-quality", false, pod("jc-helsinki-apps")),
            app("hsl-transport", true, pod("jc-helsinki-apps")),
            app("praha-mesto", false, Upstream::Static),
            wasm,
            open_wasm,
        ];
        let composed = compose(BASE, &apps, &EdgeLimits::default()).expect("composes");
        assert!(composed.skipped.is_empty(), "{:?}", composed.skipped);
        let file = parsed(&composed.file);
        let guard = |id: &str| {
            by_id(&file, "plugin_configs", id).expect("its plugins")["plugins"]["forward-auth"]
                .clone()
        };
        for (id, name) in [
            ("app-air-quality", "air-quality"),
            ("app-kpi-forecast-server", "kpi-forecast"),
        ] {
            let auth = guard(id);
            assert_eq!(
                auth["uri"],
                format!("http://portal.jc.svc.cluster.local:8080/edge/apps/{name}/admit"),
                "{id}"
            );
            assert_eq!(auth["allow_degradation"], false, "{id}");
            assert_eq!(
                auth["request_headers"],
                json!([
                    "Authorization",
                    "X-Access-Token",
                    "Accept",
                    "Accept-Language"
                ]),
                "{id}"
            );
        }
        for id in [
            "app-hsl-transport",
            "app-praha-mesto",
            "app-kpi-forecast",
            "app-news-topics-server",
            "app-air-quality-endpoint",
        ] {
            assert!(guard(id).is_null(), "{id} asks the Portal");
        }
    }

    /// A base whose Portal upstream names no node leaves a guarded App without a route of its own,
    /// so nothing serves it past the check (T-4922).
    #[test]
    fn a_guarded_app_gets_no_route_when_the_portal_has_no_node() {
        let base = BASE.replace(
            "    nodes:\n      \"portal.jc.svc.cluster.local:8080\": 1\n",
            "    nodes: {}\n",
        );
        assert_ne!(base, BASE);
        let apps = [
            app("air-quality", false, pod("jc-helsinki-apps")),
            app("hsl-transport", true, pod("jc-helsinki-apps")),
        ];
        let composed = compose(&base, &apps, &EdgeLimits::default()).expect("composes");
        assert_eq!(composed.skipped.len(), 1, "{:?}", composed.skipped);
        assert_eq!(composed.skipped[0].0, "air-quality");
        let file = parsed(&composed.file);
        assert!(by_id(&file, "routes", "app-air-quality").is_err());
        assert!(by_id(&file, "routes", "app-hsl-transport").is_ok());
    }

    #[test]
    fn a_wasm_apps_server_is_routed_to_its_shard_on_each_of_its_hosts() {
        let mut kpi = app("kpi-forecast", false, Upstream::Static);
        kpi.server = Some("jc-wasm-host-0.wasm-host.svc.cluster.local:8080".into());
        kpi.hostnames = vec!["kpi.city.example".into()];
        let composed = compose(
            BASE,
            &[kpi, app("praha-mesto", true, Upstream::Static)],
            &EdgeLimits::default(),
        )
        .expect("composes");
        assert!(composed.skipped.is_empty(), "{:?}", composed.skipped);
        let file = parsed(&composed.file);

        let upstream = by_id(&file, "upstreams", "app-kpi-forecast-server").expect("its upstream");
        assert_eq!(
            upstream["nodes"],
            json!({ "jc-wasm-host-0.wasm-host.svc.cluster.local:8080": 1 })
        );
        let at = format!(
            "app-kpi-forecast-at-{}-server",
            hostname_key("kpi.city.example")
        );
        for (id, host) in [
            ("app-kpi-forecast-server", "kpi-forecast.apps.city.example"),
            (at.as_str(), "kpi.city.example"),
        ] {
            let route = by_id(&file, "routes", id).expect("its server route");
            assert_eq!(route["host"], host);
            assert_eq!(route["uris"], json!(["/api/*", "/apps/kpi-forecast/api/*"]));
            assert_eq!(route["upstream_id"], "app-kpi-forecast-server");
            assert_eq!(route["plugin_config_id"], id);
            let priority = route["priority"].as_u64().expect("a priority");
            assert!(
                priority > APP_PRIORITY && priority < ENDPOINT_PRIORITY,
                "{priority}"
            );
            assert_eq!(
                route["vars"],
                json!([[
                    "uri",
                    "!",
                    "~~",
                    "^(?:/apps/kpi-forecast)?/api/(?:endpoint|services)/"
                ]])
            );
            let plugins = &by_id(&file, "plugin_configs", id).expect("its plugins")["plugins"];
            let oidc = &plugins["openid-connect"];
            assert_eq!(oidc["unauth_action"], "deny");
            assert_eq!(oidc["access_token_in_authorization_header"], true);
            assert_eq!(oidc["client_id"], client_id("kpi-forecast"));
            assert_eq!(
                plugins["proxy-rewrite"]["regex_uri"],
                json!([
                    "^(?:/apps/kpi-forecast)?/api/(.*)$",
                    "/apps/kpi-forecast/api/$1"
                ])
            );
            assert_eq!(
                plugins["proxy-rewrite"]["headers"]["remove"],
                json!(["Cookie"])
            );
            assert_eq!(
                plugins["serverless-pre-function"],
                by_id(&file, "plugin_configs", "context-endpoint").expect("base")["plugins"]
                    ["serverless-pre-function"],
                "the base's header strip runs on the server route too"
            );
        }
        // An App without a server gets no server route and no upstream.
        assert!(by_id(&file, "routes", "app-praha-mesto-server").is_err());
        assert!(by_id(&file, "upstreams", "app-praha-mesto-server").is_err());
    }

    #[test]
    fn each_app_gets_its_own_login_route_and_endpoint_route() {
        let composed = compose(
            BASE,
            &[app("air-quality", false, pod("jc-helsinki-apps"))],
            &EdgeLimits::default(),
        )
        .expect("composes");
        let file = parsed(&composed.file);

        let route = by_id(&file, "routes", "app-air-quality").expect("the App route");
        assert_eq!(route["uri"], "/*");
        assert_eq!(route["host"], "air-quality.apps.city.example");
        assert_eq!(route["priority"], 30);
        assert_eq!(route["upstream_id"], "app-air-quality");
        let upstream = by_id(&file, "upstreams", "app-air-quality").expect("its upstream");
        assert_eq!(
            upstream["nodes"],
            json!({ "app-air-quality.jc-helsinki-apps.svc.cluster.local:8080": 1 })
        );

        let plugins =
            &by_id(&file, "plugin_configs", "app-air-quality").expect("its plugins")["plugins"];
        assert_eq!(
            plugins["request-id"],
            json!({ "include_in_response": true })
        );
        let oidc = &plugins["openid-connect"];
        assert_eq!(oidc["client_id"], "app-air-quality");
        assert_eq!(oidc["client_secret"], "secret-of-air-quality");
        assert_eq!(oidc["unauth_action"], "auth");
        assert_eq!(
            oidc["redirect_uri"],
            "https://air-quality.apps.city.example/callback"
        );
        assert_eq!(oidc["logout_path"], "/logout");
        assert_eq!(
            oidc["post_logout_redirect_uri"],
            "https://air-quality.apps.city.example/"
        );
        assert_eq!(oidc["session"]["cookie_name"], "__Host-jc_app_air_quality");
        assert_eq!(oidc["session"]["cookie_path"], "/");
        // T-4395: what a `__Host-` cookie needs, whatever the base's template said.
        assert_eq!(oidc["session"]["cookie_secure"], true);
        assert!(oidc["session"].get("cookie_domain").is_none());
        let session = oidc["session"]["secret"]
            .as_str()
            .expect("a session secret");
        assert_eq!(session.len(), 64);
        assert!(
            !session.contains("secret-of"),
            "the client secret is the cookie key"
        );

        let endpoint =
            by_id(&file, "routes", "app-air-quality-endpoint").expect("the endpoint route");
        assert_eq!(endpoint["host"], "air-quality.apps.city.example");
        assert_eq!(
            endpoint["uris"],
            json!([
                "/api/endpoint/slug-of-air-quality/*",
                "/apps/air-quality/api/endpoint/slug-of-air-quality/*"
            ])
        );
        assert_eq!(endpoint["priority"], 35);
        assert_eq!(endpoint["upstream_id"], "context-endpoint");
        assert_eq!(
            endpoint["vars"],
            json!([[
                "uri",
                "!",
                "~~",
                "^(?:/apps/air-quality)?/api/endpoint/[^/]+/egress/"
            ]])
        );
        let plugins = &by_id(&file, "plugin_configs", "app-air-quality-endpoint")
            .expect("its plugins")["plugins"];
        assert_eq!(plugins["serverless-pre-function"]["phase"], "rewrite");
        assert_eq!(
            plugins["proxy-rewrite"]["regex_uri"],
            json!(["^(?:/apps/air-quality)?(/api/endpoint/.*)$", "$1"])
        );
        assert_eq!(
            plugins["proxy-rewrite"]["headers"]["set"]["X-Forwarded-Proto"],
            "https"
        );
        // T-3579: the session cookie stays at the edge; the gateway gets the bearer.
        assert_eq!(
            plugins["proxy-rewrite"]["headers"]["remove"],
            json!(["Cookie"])
        );
        assert_eq!(plugins["openid-connect"]["unauth_action"], "deny");
        assert_eq!(
            plugins["openid-connect"]["access_token_in_authorization_header"],
            true
        );
        assert_eq!(
            plugins["openid-connect"]["session"]["cookie_name"],
            "__Host-jc_app_air_quality"
        );

        let services =
            by_id(&file, "routes", "app-air-quality-services").expect("the services route");
        assert_eq!(
            services["uris"],
            json!(["/api/services/*", "/apps/air-quality/api/services/*"])
        );
        assert_eq!(services["host"], "air-quality.apps.city.example");
        assert_eq!(services["priority"], 35);
        let portal = by_id(&file, "routes", "portal-ui").expect("the Portal route");
        assert_eq!(
            services["upstream_id"], portal["upstream_id"],
            "the Portal answers"
        );
        let plugins = &by_id(&file, "plugin_configs", "app-air-quality-services")
            .expect("its plugins")["plugins"];
        assert_eq!(
            plugins["proxy-rewrite"]["regex_uri"],
            json!([
                "^(?:/apps/air-quality)?/api/services/(.*)$",
                "/apps/air-quality/api/services/$1"
            ])
        );
        assert_eq!(plugins["openid-connect"]["unauth_action"], "deny");
        assert_eq!(
            plugins["openid-connect"]["access_token_in_authorization_header"],
            true
        );
        assert_eq!(
            plugins["openid-connect"]["client_id"], "app-air-quality",
            "the App's own client, so the Portal believes the token (AP-92)"
        );

        // The template is untouched, and a pod App's path is its own.
        let surface = by_id(&file, "plugin_configs", "apps-surface").expect("the surface");
        assert_eq!(surface["plugins"]["openid-connect"]["client_id"], "edge");
        let own = &by_id(&file, "plugin_configs", "app-air-quality").expect("plugins")["plugins"];
        assert!(own.get("proxy-rewrite").is_none(), "{own}");
    }

    /// AP-172: each hostname of the Organization's domain gets the App's two routes on it alone,
    /// with the App's login returning there and the App's own cookie, host-only.
    #[test]
    fn a_hostname_gets_the_apps_routes_and_login_on_it_alone() {
        let mut bikes = app("bikes", false, Upstream::Static);
        bikes.hostnames = vec!["bikes.hel.fi".into()];
        let composed = compose(BASE, &[bikes], &EdgeLimits::default()).expect("composes");
        let file = parsed(&composed.file);
        let at = format!("app-bikes-at-{}", hostname_key("bikes.hel.fi"));

        let own = by_id(&file, "routes", "app-bikes").expect("the App's host route");
        let named = by_id(&file, "routes", &at).expect("the hostname's route");
        assert_eq!(named["host"], "bikes.hel.fi");
        assert_eq!(own["host"], "bikes.apps.city.example");
        for field in ["uri", "priority", "upstream_id"] {
            assert_eq!(named[field], own[field], "{field}");
        }
        let endpoint = by_id(&file, "routes", &format!("{at}-endpoint")).expect("its endpoint");
        assert_eq!(endpoint["host"], "bikes.hel.fi");
        assert_eq!(
            endpoint["uris"],
            by_id(&file, "routes", "app-bikes-endpoint").expect("the host's")["uris"]
        );

        let oidc = |id: &str| {
            by_id(&file, "plugin_configs", id).expect(id)["plugins"]["openid-connect"].clone()
        };
        let (named, own) = (oidc(&at), oidc("app-bikes"));
        assert_eq!(named["redirect_uri"], "https://bikes.hel.fi/callback");
        assert_eq!(named["post_logout_redirect_uri"], "https://bikes.hel.fi/");
        for field in ["client_id", "unauth_action"] {
            assert_eq!(named[field], own[field], "{field}");
        }
        assert_eq!(
            named["session"]["cookie_name"],
            own["session"]["cookie_name"]
        );
        assert!(named["session"].get("cookie_domain").is_none());
        assert_eq!(
            oidc(&format!("{at}-endpoint"))["redirect_uri"],
            "https://bikes.hel.fi/callback"
        );
        // The platform services answer on the hostname too, with its own login (AP-167).
        let services = by_id(&file, "routes", &format!("{at}-services")).expect("its services");
        let theirs = by_id(&file, "routes", "app-bikes-services").expect("the host's");
        assert_eq!(services["host"], "bikes.hel.fi");
        for field in ["uris", "priority", "upstream_id"] {
            assert_eq!(services[field], theirs[field], "{field}");
        }
        assert_eq!(
            oidc(&format!("{at}-services"))["redirect_uri"],
            "https://bikes.hel.fi/callback"
        );
        // The key is the name's, whatever its place in the list.
        assert_eq!(hostname_key("bikes.hel.fi"), hostname_key("bikes.hel.fi"));
        assert_ne!(hostname_key("bikes.hel.fi"), hostname_key("events.hel.fi"));
    }

    #[test]
    fn a_public_app_passes_and_a_static_app_is_served_by_the_static_host() {
        let composed = compose(
            BASE,
            &[app("hsl-transport", true, Upstream::Static)],
            &EdgeLimits::default(),
        )
        .expect("composes");
        let file = parsed(&composed.file);
        let route = by_id(&file, "routes", "app-hsl-transport").expect("the App route");
        assert_eq!(route["upstream_id"], "portal-ui");
        assert!(by_id(&file, "upstreams", "app-hsl-transport").is_err());
        // The static host keeps the bundle under its folder; a path already there lands on the
        // same file.
        let own = &by_id(&file, "plugin_configs", "app-hsl-transport").expect("plugins")["plugins"];
        let rewrite = &own["proxy-rewrite"]["regex_uri"];
        assert_eq!(rewrite[1], "/apps/hsl-transport/$1");
        let pattern = regex::Regex::new(rewrite[0].as_str().expect("a pattern")).expect("compiles");
        for (path, rest) in [
            ("/", ""),
            ("/assets/index.js", "assets/index.js"),
            ("/apps/hsl-transport/assets/index.js", "assets/index.js"),
            ("/api/functions/count", "api/functions/count"),
        ] {
            assert_eq!(&pattern.captures(path).expect(path)[1], rest, "{path}");
        }
        for id in ["app-hsl-transport", "app-hsl-transport-endpoint"] {
            let plugins = &by_id(&file, "plugin_configs", id).expect("plugins")["plugins"];
            assert_eq!(plugins["openid-connect"]["unauth_action"], "pass", "{id}");
        }
    }

    #[test]
    fn only_the_portal_frames_an_app_and_the_shared_surface_keeps_its_frame_rule() {
        let composed = compose(
            BASE,
            &[
                app("hsl-transport", true, Upstream::Static),
                app("air-quality", false, pod("jc-helsinki-apps")),
            ],
            &EdgeLimits::default(),
        )
        .expect("composes");
        let file = parsed(&composed.file);
        let headers = |id: &str| {
            by_id(&file, "plugin_configs", id).expect("plugins")["plugins"]["response-rewrite"]
                ["headers"]
                .clone()
        };
        // A static App: its host writes the policy, the route adds who may frame it.
        let static_app = headers("app-hsl-transport");
        assert_eq!(
            static_app["set"],
            json!({ "X-Content-Type-Options": "nosniff" })
        );
        assert_eq!(
            static_app["add"],
            json!(["Content-Security-Policy: frame-ancestors 'self' portal.city.example"])
        );
        // APISIX's schema for `response-rewrite.headers.add`: one `:` and no trailing `/`.
        for entry in static_app["add"].as_array().expect("add") {
            let entry = entry.as_str().expect("a string");
            assert_eq!(entry.matches(':').count(), 1, "{entry}");
            assert!(!entry.ends_with('/'), "{entry}");
        }
        // A pod App: the route sets its whole policy, and adds none that would be overwritten.
        let pod_app = headers("app-air-quality");
        assert_eq!(
            pod_app["set"],
            json!({
                "X-Content-Type-Options": "nosniff",
                "Content-Security-Policy": POD_POLICY,
            })
        );
        assert!(pod_app.get("add").is_none(), "{pod_app}");
        for headers in [static_app, pod_app] {
            assert_eq!(headers["remove"], json!(["X-Frame-Options"]), "{headers}");
        }
        // The endpoint route answers JSON and is framed by nobody; the fallback keeps SAMEORIGIN.
        let endpoint = &by_id(&file, "plugin_configs", "app-air-quality-endpoint")
            .expect("plugins")["plugins"];
        assert!(endpoint.get("response-rewrite").is_none());
        let surface = &by_id(&file, "plugin_configs", "apps-surface").expect("the surface")
            ["plugins"]["response-rewrite"]["headers"]["set"];
        assert_eq!(surface["X-Frame-Options"], "SAMEORIGIN");
    }

    #[test]
    fn a_surface_without_a_response_rewrite_still_gets_the_frame_rule() {
        let base = BASE.replace(
            "      response-rewrite:\n        headers:\n          set:\n            X-Content-Type-Options: nosniff\n            X-Frame-Options: SAMEORIGIN\n",
            "",
        );
        assert_ne!(base, BASE, "the fixture changed");
        let composed = compose(
            &base,
            &[app("a", false, Upstream::Static)],
            &EdgeLimits::default(),
        )
        .expect("composes");
        let file = parsed(&composed.file);
        let headers = &by_id(&file, "plugin_configs", "app-a").expect("plugins")["plugins"]
            ["response-rewrite"]["headers"];
        assert_eq!(headers["remove"], json!(["X-Frame-Options"]));
        assert!(headers["add"][0]
            .as_str()
            .is_some_and(|value| value.ends_with("frame-ancestors 'self' portal.city.example")));
    }

    #[test]
    fn two_apps_never_share_a_cookie_or_its_key() {
        let composed = compose(
            BASE,
            &[
                app("b-app", false, Upstream::Static),
                app("a-app", false, Upstream::Static),
            ],
            &EdgeLimits::default(),
        )
        .expect("composes");
        let file = parsed(&composed.file);
        let session = |id: &str| {
            by_id(&file, "plugin_configs", id).expect("plugins")["plugins"]["openid-connect"]
                ["session"]
                .clone()
        };
        assert_ne!(
            session("app-a-app")["secret"],
            session("app-b-app")["secret"]
        );
        assert_ne!(
            session("app-a-app")["cookie_name"],
            session("app-b-app")["cookie_name"]
        );
        // An App's session lives as long as the edge session it was copied from, so a person who
        // is signed in to the Portal is not signed out of its Apps sooner (T-3034).
        for app in ["app-a-app", "app-b-app"] {
            assert_eq!(session(app)["idling_timeout"], 3600, "{app}");
            assert_eq!(session(app)["absolute_timeout"], 36000, "{app}");
        }
        // Sorted by name, so the same Apps always compose the same file.
        let again = compose(
            BASE,
            &[
                app("a-app", false, Upstream::Static),
                app("b-app", false, Upstream::Static),
            ],
            &EdgeLimits::default(),
        )
        .expect("composes");
        assert_eq!(composed.file, again.file);
    }

    #[test]
    fn a_base_without_the_surfaces_is_refused_by_name() {
        let without = BASE.replace("id: context-endpoint", "id: something-else");
        assert_eq!(
            compose(&without, &[], &EdgeLimits::default()),
            Err(ComposeError::Missing(
                "routes entry context-endpoint".into()
            ))
        );
        assert!(matches!(
            compose("routes: [", &[], &EdgeLimits::default()),
            Err(ComposeError::Unreadable(_))
        ));
    }

    #[test]
    fn an_app_whose_id_the_base_holds_gets_no_route() {
        let base = BASE.replace(
            "routes:\n",
            "routes:\n  - id: app-portal\n    uri: /x\n    upstream_id: portal-ui\n",
        );
        let composed = compose(
            &base,
            &[app("portal", false, Upstream::Static)],
            &EdgeLimits::default(),
        )
        .expect("composes");
        assert_eq!(composed.skipped.len(), 1);
        assert!(!composed.file.contains("secret-of-portal"));
    }

    #[test]
    fn a_base_without_the_portal_route_is_refused_by_name() {
        let without = BASE.replace("id: portal-ui\n    uri", "id: elsewhere\n    uri");
        assert_eq!(
            compose(&without, &[], &EdgeLimits::default()),
            Err(ComposeError::Missing("routes entry portal-ui".into()))
        );
    }

    /// AP-133: an App's host routes its own slugs and no other App's; an App that reads no
    /// endpoint gets no endpoint route at all.
    #[test]
    fn an_app_host_routes_its_own_slugs_only() {
        let mut reads_none = app("bare", false, Upstream::Static);
        reads_none.slugs.clear();
        let composed = compose(
            BASE,
            &[
                app("a-app", false, Upstream::Static),
                app("b-app", true, Upstream::Static),
                reads_none,
            ],
            &EdgeLimits::default(),
        )
        .expect("composes");
        let file = parsed(&composed.file);
        for route in file["routes"].as_array().expect("routes") {
            let text = route.to_string();
            match route["host"].as_str() {
                Some("a-app.apps.city.example") => assert!(!text.contains("b-app"), "{text}"),
                Some("b-app.apps.city.example") => assert!(!text.contains("a-app"), "{text}"),
                _ => {}
            }
        }
        assert!(by_id(&file, "routes", "app-bare-endpoint").is_err());
        assert!(by_id(&file, "plugin_configs", "app-bare-endpoint").is_err());
        assert!(by_id(&file, "routes", "app-bare").is_ok());
    }

    /// ADR-N-037 §3: the old path on the apex is a 308 to the host with no login and no
    /// upstream, the rest of the path and the query carried over.
    #[test]
    fn the_old_path_moves_to_the_host_without_a_session() {
        let composed = compose(
            BASE,
            &[app("air-quality", false, Upstream::Static)],
            &EdgeLimits::default(),
        )
        .expect("composes");
        let file = parsed(&composed.file);
        let moved = by_id(&file, "routes", "app-air-quality-moved").expect("the redirect");
        assert_eq!(moved["host"], "city.example");
        assert_eq!(
            moved["uris"],
            json!(["/apps/air-quality", "/apps/air-quality/*"])
        );
        assert!(moved.get("upstream_id").is_none());
        assert!(moved.get("plugin_config_id").is_none());
        let plugins = moved["plugins"].as_object().expect("plugins");
        assert_eq!(plugins.keys().collect::<Vec<_>>(), ["redirect"]);
        let redirect = &plugins["redirect"];
        assert_eq!(redirect["ret_code"], 308);
        assert_eq!(redirect["append_query_string"], true);
        assert_eq!(
            redirect["regex_uri"][1],
            "https://air-quality.apps.city.example/$1"
        );
        let pattern = regex::Regex::new(redirect["regex_uri"][0].as_str().expect("a pattern"))
            .expect("compiles");
        for (path, rest) in [
            ("/apps/air-quality", ""),
            ("/apps/air-quality/", ""),
            ("/apps/air-quality/stations/5", "stations/5"),
        ] {
            assert_eq!(&pattern.captures(path).expect(path)[1], rest, "{path}");
        }
        assert!(!moved.to_string().contains("secret-of"));
    }

    #[test]
    fn a_compose_error_never_carries_a_secret() {
        let err = compose(
            "[",
            &[app("a", false, Upstream::Static)],
            &EdgeLimits::default(),
        )
        .expect_err("unreadable");
        assert!(!err.to_string().contains("secret-of"));
    }

    /// [`BASE`] with its chains labelled by class, a labelled route of its own chain, and two
    /// limit-counts the Organization does not set: one unlabelled, one of an unknown class.
    fn rated() -> String {
        let limit = |count: u32| json!({"count": count, "time_window": 60, "key": "remote_addr"});
        let mut document = parsed(BASE);
        let configs = document["plugin_configs"]
            .as_array_mut()
            .expect("plugin_configs");
        configs[0]["labels"] = json!({RATE_CLASS: "web"});
        configs[0]["plugins"]["limit-count"] = limit(300);
        configs[1]["labels"] = json!({RATE_CLASS: "publicEndpoint"});
        configs[1]["plugins"]["limit-count"] = limit(5000);
        configs.push(json!({"id": "keycloak", "plugins": {"limit-count": limit(600)}}));
        let routes = document["routes"].as_array_mut().expect("routes");
        routes.push(
            json!({"id": "context-space-write", "uri": "/cs/*", "upstream_id": "portal-ui",
            "labels": {RATE_CLASS: "dataWrite"}, "plugins": {"limit-count": limit(1200)}}),
        );
        routes.push(
            json!({"id": "odd", "uri": "/odd/*", "upstream_id": "portal-ui",
            "labels": {RATE_CLASS: "nosuchclass"}, "plugins": {"limit-count": limit(7)}}),
        );
        document["global_rules"] = json!([{"id": EDGE_BODY_RULE,
            "plugins": {"client-control": {"max_body_size": 16 * 1024 * 1024}}}]);
        serde_norway::to_string(&document).expect("yaml") + "#END\n"
    }

    fn body(file: &Value) -> Value {
        by_id(file, "global_rules", EDGE_BODY_RULE).expect("the rule")["plugins"]["client-control"]
            ["max_body_size"]
            .clone()
    }

    #[test]
    fn the_organizations_body_limit_reaches_the_edge_body_rule() {
        let mib = 1024 * 1024;
        let compose_with = |json: Option<Value>| {
            let set = json.map(limits);
            let edge = EdgeLimits::of(set.as_ref(), &OrganizationBounds::default());
            parsed(&compose(&rated(), &[], &edge).expect("composes").file)
        };
        let set = |megabytes: u32| Some(json!({"edge": {"maxRequestBodyMegabytes": megabytes}}));
        assert_eq!(
            body(&compose_with(None)),
            json!(16 * mib),
            "the catalog default"
        );
        assert_eq!(
            body(&compose_with(set(32))),
            json!(32 * mib),
            "above the old fixed 16"
        );
        assert_eq!(body(&compose_with(set(2))), json!(2 * mib));
        assert_eq!(
            body(&compose_with(set(1000))),
            json!(64 * mib),
            "the ceiling"
        );
        // A base without the rule stays without it: the server block's number holds.
        let edge = EdgeLimits::of(None, &OrganizationBounds::default());
        let file = parsed(&compose(BASE, &[], &edge).expect("composes").file);
        assert!(file.get("global_rules").is_none());
    }

    fn count(file: &Value, list: &str, id: &str) -> Value {
        by_id(file, list, id).expect("the entry")["plugins"]["limit-count"]["count"].clone()
    }

    fn limits(json: Value) -> OrganizationLimits {
        serde_json::from_value(json).expect("limits")
    }

    #[test]
    fn the_organizations_rates_reach_every_labelled_limit_count() {
        let set = limits(json!({"edge": {"requestsPerMinute": {"web": 900, "dataWrite": 50}}}));
        let rates = EdgeLimits::of(Some(&set), &OrganizationBounds::default());
        let file = parsed(&compose(&rated(), &[], &rates).expect("composes").file);

        assert_eq!(count(&file, "plugin_configs", "apps-surface"), json!(900));
        assert_eq!(count(&file, "routes", "context-space-write"), json!(50));
        // Not set by the organization: the catalog's default.
        assert_eq!(
            count(&file, "plugin_configs", "context-endpoint"),
            json!(5000)
        );
        // No label, or a class this Portal does not know: helm's count stays.
        assert_eq!(count(&file, "plugin_configs", "keycloak"), json!(600));
        assert_eq!(count(&file, "routes", "odd"), json!(7));
    }

    #[test]
    fn without_an_organization_every_class_counts_its_default() {
        let rates = EdgeLimits::of(None, &OrganizationBounds::default());
        assert_eq!(rates.of_class("web"), Some(300));
        assert_eq!(rates.of_class("api"), Some(1200));
        assert_eq!(rates.of_class("dataRead"), Some(1200));
        assert_eq!(rates.of_class("dataWrite"), Some(1200));
        assert_eq!(rates.of_class("publicEndpoint"), Some(5000));
        assert_eq!(rates.of_class("nosuchclass"), None);
    }

    #[test]
    fn a_rate_above_the_ceiling_counts_the_ceiling() {
        let set = limits(json!({"edge": {"requestsPerMinute": {"web": 1_000_000, "api": 5000}}}));
        // The built-in ceiling for web, and an operator's tighter one for api.
        let bounds = OrganizationBounds(BTreeMap::from([(
            "spec.limits.edge.requestsPerMinute.api".to_string(),
            jc_core::kinds::org_settings::Bound {
                min: Some(10),
                max: Some(2000),
            },
        )]));
        let rates = EdgeLimits::of(Some(&set), &bounds);
        assert_eq!(rates.of_class("web"), Some(3000));
        assert_eq!(rates.of_class("api"), Some(2000));
    }

    #[test]
    fn apps_copy_the_rated_surface_chain() {
        let set = limits(json!({"edge": {"requestsPerMinute": {"web": 42}}}));
        let rates = EdgeLimits::of(Some(&set), &OrganizationBounds::default());
        let file = parsed(
            &compose(
                &rated(),
                &[app("air-quality", false, Upstream::Static)],
                &rates,
            )
            .expect("composes")
            .file,
        );
        assert_eq!(count(&file, "plugin_configs", "app-air-quality"), json!(42));
    }
}
