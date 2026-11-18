#![forbid(unsafe_code)]
//! `jc-router` (W9): Mount table routing requests across modules by `(host?, path_prefix?)`.
//! - Supports one domain with different slugs (e.g. `city.example/login`, `/portal`, `/opendata`,
//!   `/dsp`, `/mcp`), or a host per module (`id.city.example`), or both.
//! - Refuses overlapping mounts at start.
//! - Strips the path prefix before handing the request to the module router, and injects [`Mount`]
//!   into the request extensions so every absolute URL, redirect, issuer, cookie path, CSP and
//!   CORS origin honours the mount.
//! - Trusts `X-Forwarded-Proto`, `X-Forwarded-Host` and `X-Forwarded-For` only when the peer IP is
//!   in the configured trusted proxy list, and injects the resolved [`ClientAddr`].
//! - Answers 404 for unmounted paths.
//! - A host `*.{suffix}` matches any host one label under `{suffix}`, after every exact host and
//!   before a host-less entry (the App hosts `*.apps.{domain}`, edge-session-design S4a).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::{Arc, PoisonError, RwLock};
use tower::ServiceExt;

mod hosts;
mod projects;
pub mod registry;
pub use hosts::{HOST_ROLE_HEADER, HostRole, HostRow, HostTable, Organization, RegistryHost};
pub use projects::{IdAddress, ProjectTable, RegistryProject, branch_name};

/// The client's address as the router resolved it: the TCP peer, or, when the peer is a trusted
/// proxy, the right-most `X-Forwarded-For` hop that is not itself a trusted proxy. Modules key
/// per-address rate limits on it; it is absent when the peer is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddr(pub IpAddr);

/// The prefix of the mount entry a request matched, as the router stripped it from the path.
/// [`Mount::path_prefix`] is the public one, which also carries the Organization's segment on a
/// shared host; a module that routes on the whole path puts this one back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePrefix(pub String);

/// Resolved mount context passed to a module router via request extensions and at router build time (W9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    pub module: String,
    pub scheme: String,
    pub host: String,
    /// Normalized prefix: `""` for root mount, or `"/login"` (leading slash, no trailing slash).
    pub path_prefix: String,
}

impl Default for Mount {
    fn default() -> Self {
        Self {
            module: "kaisatsu".into(),
            scheme: "https".into(),
            host: "idm.localhost".into(),
            path_prefix: String::new(),
        }
    }
}

impl Mount {
    pub fn new(
        module: impl Into<String>,
        scheme: impl Into<String>,
        host: impl Into<String>,
        path_prefix: impl Into<String>,
    ) -> Result<Self, RouterError> {
        let prefix = normalize_prefix(&path_prefix.into())?;
        Ok(Self {
            module: module.into(),
            scheme: scheme.into(),
            host: host.into(),
            path_prefix: prefix,
        })
    }

    /// Public base URL including prefix (`https://city.example/login` or `https://login.city.example`).
    #[must_use]
    pub fn public_base(&self) -> String {
        format!("{}://{}{}", self.scheme, self.host, self.path_prefix)
    }

    /// Realm issuer string: `{public base}{prefix}/realms/{realm}` (W1, W9).
    #[must_use]
    pub fn realm_issuer(&self, realm: &str) -> String {
        format!("{}/realms/{realm}", self.public_base())
    }

    /// Cookie prefix rule of W9: `__Host-wk_` only when mounted at the host root (`path_prefix == ""`),
    /// otherwise `__Secure-wk_` with `Path={prefix}/realms/{realm}`.
    #[must_use]
    pub fn cookie_name(&self, suffix: &str) -> String {
        if self.path_prefix.is_empty() {
            format!("__Host-wk_{suffix}")
        } else {
            format!("__Secure-wk_{suffix}")
        }
    }
}

/// Declarative entry in the mount table config (W9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MountSpec {
    pub module: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouterError {
    #[error("invalid mount prefix '{0}': must start with '/' and contain no '..' or query")]
    InvalidPrefix(String),
    #[error(
        "overlapping mounts between module '{first}' and '{second}' on host '{host}' prefix '{prefix}'"
    )]
    OverlappingMounts {
        first: String,
        second: String,
        host: String,
        prefix: String,
    },
    #[error("invalid host row: {0}")]
    InvalidHostRow(String),
    #[error("Organization '{organization}' is also the first path segment of module '{module}'")]
    OrganizationShadowsMount {
        organization: String,
        module: String,
    },
}

pub fn normalize_prefix(raw: &str) -> Result<String, RouterError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Ok(String::new());
    }
    if !trimmed.starts_with('/')
        || trimmed.contains("..")
        || trimmed.contains('?')
        || trimmed.contains("//")
    {
        return Err(RouterError::InvalidPrefix(raw.to_owned()));
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// The suffix of a wildcard host `*.{suffix}` (with its leading dot), `None` for a plain host.
fn wildcard_suffix(host: &str) -> Option<&str> {
    host.strip_prefix('*').filter(|rest| rest.starts_with('.'))
}

/// Whether `host` is exactly one non-empty label in front of `suffix` (`.apps.city.example`):
/// `a.apps.city.example` is, `apps.city.example` and `a.b.apps.city.example` are not.
fn one_label_under(host: &str, suffix: &str) -> bool {
    host.strip_suffix(suffix)
        .is_some_and(|label| !label.is_empty() && !label.contains('.'))
}

/// The header the Linkerd inbound proxy sets to the caller's mTLS identity (and removes when there
/// is none): linkerd2-proxy `linkerd/app/inbound/src/http/set_identity_header.rs`.
const MESH_IDENTITY: &str = "l5d-client-id";

/// The headers a proxy names the client, its host and its scheme in.
const FORWARDING_HEADERS: [&str; 5] = [
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
];

struct MountedEntry {
    module: String,
    host: Option<String>,
    prefix: String,
    router: Router,
}

#[derive(Clone)]
pub struct JcRouter {
    entries: Arc<Vec<MountedEntry>>,
    default_scheme: String,
    default_host: String,
    trusted_proxies: Arc<Vec<IpNet>>,
    trusted_identities: Arc<Vec<String>>,
    hosts: Option<HostSwap>,
}

/// The router's host table, swapped at run time from the host registry (T-7129 §1, T-7136):
/// every request reads the table current when it arrives. [`HostSwap::rebuild`] applies the
/// registry's rows to the table the configuration built; a build that fails keeps the table
/// that answers now.
#[derive(Clone)]
pub struct HostSwap {
    /// The table of the configuration alone, which every rebuild starts from.
    base: Arc<HostTable>,
    current: Arc<RwLock<Arc<HostTable>>>,
    /// `(first path segment, module)` of each mount: no Organization may take one.
    segments: Arc<Vec<(String, String)>>,
    /// The router each module answers with ([`JcRouterBuilder::module_router`]), for the hosts
    /// the registry adds.
    routers: Arc<std::collections::BTreeMap<String, Router>>,
    /// Every host an entry of the configuration names, without its port: the registry mounts
    /// nothing there.
    fixed: Arc<std::collections::BTreeSet<String>>,
    /// The entries of the hosts the registry added ([`HostSwap::mount_hosts`]), longest prefix
    /// first.
    added: Arc<RwLock<Arc<Vec<MountedEntry>>>>,
    /// The live projects by id once id addresses are on ([`HostSwap::replace_projects`]); while
    /// `None` no path is read as an id address (RTE-05, `project_addresses` off).
    projects: Arc<RwLock<Option<Arc<ProjectTable>>>>,
}

impl HostSwap {
    /// Turns id addresses on with the live projects of `rows` (RTE-05, T-7194), replacing the
    /// last set whole.
    pub fn replace_projects(&self, rows: &[RegistryProject<'_>]) {
        *self
            .projects
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(ProjectTable::new(rows)));
    }

    /// The project table that answers now; `None` while id addresses are off.
    #[must_use]
    pub fn projects(&self) -> Option<Arc<ProjectTable>> {
        self.projects
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The table that answers now.
    #[must_use]
    pub fn current(&self) -> Arc<HostTable> {
        // A writer cannot panic while holding the lock (it only assigns), so a poisoned lock
        // still holds a whole table.
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Builds the table of the configuration with `rows` applied and swaps it in. On an error
    /// the table that answers now stays, and the error says which row broke the build.
    pub fn rebuild(&self, rows: &[RegistryHost<'_>]) -> Result<(), RouterError> {
        self.rebuild_without(rows, &[])
    }

    /// [`HostSwap::rebuild`] with the Organizations `removed` taken out of the configuration's
    /// table first (`HostTable::without`): none of their hosts or paths answers.
    pub fn rebuild_without(
        &self,
        rows: &[RegistryHost<'_>],
        removed: &[&str],
    ) -> Result<(), RouterError> {
        let table = self.base.without(removed).with_registry(rows)?;
        shadow_check(&table, &self.segments)?;
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(table);
        Ok(())
    }

    /// Mounts each module of `specs` on its host, with the router that module answers with
    /// elsewhere (T-7184): the entries of the hosts the registry serves beyond the
    /// configuration's, replacing the last set. A spec on a host the configuration mounts, on a
    /// wildcard host or of a module with no router here is left out; two specs on one host and
    /// prefix refuse the whole set, and the entries that answer now stay. The entries placed.
    pub fn mount_hosts(&self, specs: &[MountSpec]) -> Result<usize, RouterError> {
        let mut entries: Vec<MountedEntry> = Vec::new();
        for spec in specs {
            let Some(host) = spec
                .host
                .as_deref()
                .map(|h| h.trim().to_ascii_lowercase())
                .filter(|h| !h.is_empty() && wildcard_suffix(h).is_none())
            else {
                continue;
            };
            let Some(router) = self.routers.get(&spec.module) else {
                continue;
            };
            if self.fixed.contains(hosts::without_port(&host)) {
                continue;
            }
            let prefix = normalize_prefix(spec.path_prefix.as_deref().unwrap_or(""))?;
            if let Some(first) = entries
                .iter()
                .find(|e| e.host.as_deref() == Some(host.as_str()) && e.prefix == prefix)
            {
                return Err(RouterError::OverlappingMounts {
                    first: first.module.clone(),
                    second: spec.module.clone(),
                    host,
                    prefix: if prefix.is_empty() {
                        "/".into()
                    } else {
                        prefix
                    },
                });
            }
            entries.push(MountedEntry {
                module: spec.module.clone(),
                host: Some(host),
                prefix,
                router: router.clone(),
            });
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.prefix.len()));
        let placed = entries.len();
        *self.added.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(entries);
        Ok(placed)
    }

    fn added(&self) -> Arc<Vec<MountedEntry>> {
        Arc::clone(&self.added.read().unwrap_or_else(PoisonError::into_inner))
    }
}

/// No Organization of `table` is the first path segment of a mount: `/{org}/` would hide it.
fn shadow_check(table: &HostTable, segments: &[(String, String)]) -> Result<(), RouterError> {
    for org in table.organizations() {
        if let Some((_, module)) = segments.iter().find(|(first, _)| first == org) {
            return Err(RouterError::OrganizationShadowsMount {
                organization: org.to_owned(),
                module: module.clone(),
            });
        }
    }
    Ok(())
}

pub struct JcRouterBuilder {
    specs: Vec<(MountSpec, Router)>,
    default_scheme: String,
    default_host: String,
    trusted_proxies: Vec<IpNet>,
    trusted_identities: Vec<String>,
    hosts: Option<HostTable>,
    routers: std::collections::BTreeMap<String, Router>,
}

impl Default for JcRouterBuilder {
    fn default() -> Self {
        Self::new("https", "localhost")
    }
}

impl JcRouterBuilder {
    #[must_use]
    pub fn new(default_scheme: impl Into<String>, default_host: impl Into<String>) -> Self {
        Self {
            specs: Vec::new(),
            default_scheme: default_scheme.into(),
            default_host: default_host.into(),
            trusted_proxies: Vec::new(),
            trusted_identities: Vec::new(),
            hosts: None,
            routers: std::collections::BTreeMap::new(),
        }
    }

    /// The host registry (docs/API/15-addresses-v1.md §2): once set, a host outside it answers
    /// `421` and every request's [`Organization`] is decided from it.
    #[must_use]
    pub fn hosts(mut self, table: HostTable) -> Self {
        self.hosts = Some(table);
        self
    }

    #[must_use]
    pub fn trust_proxy(mut self, net: IpNet) -> Self {
        self.trusted_proxies.push(net);
        self
    }

    /// The mesh identity (`l5d-client-id`) of the edge. Once one is named, a trusted proxy's
    /// forwarding headers count only on a request that carries one of them: in a mesh every caller
    /// reaches jc from its own proxy's address, so the address alone names no edge. The Linkerd
    /// inbound proxy sets the header from the caller's mTLS identity and removes it otherwise.
    #[must_use]
    pub fn trust_identity(mut self, identity: impl Into<String>) -> Self {
        self.trusted_identities.push(identity.into());
        self
    }

    /// The router `module` answers with on a host the registry adds at run time
    /// ([`HostSwap::mount_hosts`]); the first one named for a module stays.
    #[must_use]
    pub fn module_router(mut self, module: &str, router: Router) -> Self {
        self.routers.entry(module.to_owned()).or_insert(router);
        self
    }

    #[must_use]
    pub fn mount(mut self, spec: MountSpec, router: Router) -> Self {
        self.specs.push((spec, router));
        self
    }

    /// Validates that no two mounts overlap and builds the [`JcRouter`] (W9).
    pub fn build(self) -> Result<JcRouter, RouterError> {
        let mut entries: Vec<MountedEntry> = Vec::with_capacity(self.specs.len());
        for (spec, router) in self.specs {
            let host = spec
                .host
                .as_deref()
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .map(|h| h.to_ascii_lowercase());
            let prefix = normalize_prefix(spec.path_prefix.as_deref().unwrap_or(""))?;
            for existing in &entries {
                let same_host = match (&existing.host, &host) {
                    (Some(a), Some(b)) => a == b,
                    (None, None) => true,
                    _ => false,
                };
                if same_host && existing.prefix == prefix {
                    return Err(RouterError::OverlappingMounts {
                        first: existing.module.clone(),
                        second: spec.module,
                        host: host.unwrap_or_else(|| "*".into()),
                        prefix: if prefix.is_empty() {
                            "/".into()
                        } else {
                            prefix
                        },
                    });
                }
            }
            entries.push(MountedEntry {
                module: spec.module,
                host,
                prefix,
                router,
            });
        }
        // Sort most-specific host and longest path prefix first so `/login/sub` matches before `/login` or `/`:
        // an exact host, then a wildcard host, then none.
        let rank = |host: &Option<String>| match host.as_deref() {
            Some(h) if wildcard_suffix(h).is_some() => 1,
            Some(_) => 2,
            None => 0,
        };
        let segments: Vec<(String, String)> = entries
            .iter()
            .filter_map(|e| {
                let first = e.prefix.trim_start_matches('/').split('/').next()?;
                (!first.is_empty()).then(|| (first.to_owned(), e.module.clone()))
            })
            .collect();
        if let Some(table) = &self.hosts {
            shadow_check(table, &segments)?;
        }
        let fixed: std::collections::BTreeSet<String> = entries
            .iter()
            .filter_map(|e| e.host.as_deref())
            .map(|h| hosts::without_port(h).to_owned())
            .collect();
        entries.sort_by(|a, b| {
            rank(&b.host)
                .cmp(&rank(&a.host))
                .then_with(|| b.prefix.len().cmp(&a.prefix.len()))
        });
        Ok(JcRouter {
            entries: Arc::new(entries),
            default_scheme: self.default_scheme,
            default_host: self.default_host,
            trusted_proxies: Arc::new(self.trusted_proxies),
            trusted_identities: Arc::new(self.trusted_identities),
            hosts: self.hosts.map(|table| {
                let base = Arc::new(table);
                HostSwap {
                    current: Arc::new(RwLock::new(Arc::clone(&base))),
                    base,
                    segments: Arc::new(segments),
                    routers: Arc::new(self.routers),
                    fixed: Arc::new(fixed),
                    added: Arc::default(),
                    projects: Arc::default(),
                }
            }),
        })
    }
}

/// The first of `entries` a request on `host_with_port` (`host_only` without its port) for
/// `path` goes to.
fn find_in<'a>(
    entries: &'a [MountedEntry],
    host_with_port: &str,
    host_only: &str,
    path: &str,
) -> Option<&'a MountedEntry> {
    entries.iter().find(|entry| {
        let host_ok = match &entry.host {
            Some(h) => match wildcard_suffix(h) {
                Some(suffix) => {
                    one_label_under(host_with_port, suffix) || one_label_under(host_only, suffix)
                }
                None => h == host_with_port || h == host_only,
            },
            None => true,
        };
        if !host_ok {
            return false;
        }
        // `path` is the prefix or continues it with a whole segment (no allocation per entry).
        entry.prefix.is_empty()
            || path
                .strip_prefix(entry.prefix.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// Whether a browser asked for a page: a `GET` or `HEAD` that accepts HTML. An API client asks
/// for JSON or anything, and is served in place (docs/API/15-addresses-v1.md §2).
fn is_page(req: &Request<Body>) -> bool {
    matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) && req
        .headers()
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

impl JcRouter {
    /// The handle that swaps the host table at run time; `None` without a host table.
    #[must_use]
    pub fn host_swap(&self) -> Option<HostSwap> {
        self.hosts.clone()
    }

    fn is_trusted_proxy(&self, peer: Option<IpAddr>) -> bool {
        let Some(ip) = peer else {
            return false;
        };
        self.trusted_proxies.iter().any(|net| net.contains(&ip))
    }

    /// Whether `req` came from the edge: a trusted proxy's address and, once an identity is
    /// named, the edge's mesh identity.
    fn came_from_edge(&self, req: &Request<Body>, peer: Option<IpAddr>) -> bool {
        self.is_trusted_proxy(peer)
            && (self.trusted_identities.is_empty()
                || req
                    .headers()
                    .get(MESH_IDENTITY)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|id| self.trusted_identities.iter().any(|t| t == id)))
    }

    /// The client address of `req`, and on an edge request the `X-Forwarded-For` chain cut right
    /// after it, so a module reading the chain's last entry (the gateway's and the basemap's
    /// limits) reads the client and never one of our own proxies (T-6788).
    fn resolve_client_addr(
        &self,
        req: &mut Request<Body>,
        peer: Option<IpAddr>,
        from_edge: bool,
    ) -> Option<IpAddr> {
        let peer = peer?;
        if !from_edge {
            return Some(peer);
        }
        let hops: Vec<IpAddr> = req
            .headers()
            .get_all("x-forwarded-for")
            .iter()
            // By bytes: a client's entry that is no text must not take the edge's appended
            // entry of the same line with it.
            .flat_map(|v| v.as_bytes().split(|b| *b == b','))
            .filter_map(|h| std::str::from_utf8(h).ok())
            .filter_map(|h| h.trim().parse::<IpAddr>().ok())
            .collect();
        let at = hops
            .iter()
            .rposition(|ip| !self.is_trusted_proxy(Some(*ip)))
            .unwrap_or(0);
        let chain = hops[..hops.len().min(at + 1)]
            .iter()
            .map(IpAddr::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let headers = req.headers_mut();
        headers.remove("x-forwarded-for");
        if !chain.is_empty()
            && let Ok(value) = axum::http::HeaderValue::from_str(&chain)
        {
            headers.insert("x-forwarded-for", value);
        }
        hops.get(at).copied().or(Some(peer))
    }

    fn resolve_scheme_and_host(&self, req: &Request<Body>, trust_fwd: bool) -> (String, String) {
        let scheme = if trust_fwd {
            req.headers()
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .map(str::trim)
                .filter(|s| matches!(*s, "http" | "https"))
                .unwrap_or(&self.default_scheme)
                .to_owned()
        } else {
            self.default_scheme.clone()
        };

        let forwarded_host = if trust_fwd {
            req.headers()
                .get("x-forwarded-host")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        } else {
            None
        };

        let host = forwarded_host
            .or_else(|| {
                req.headers()
                    .get(axum::http::header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| req.uri().host())
            .unwrap_or(&self.default_host)
            .to_ascii_lowercase();

        (scheme, host)
    }

    /// The entry a request on `host_with_port` (`host_only` without its port) for `path` goes to:
    /// a linear scan, most specific host and longest prefix first.
    fn find(&self, host_with_port: &str, host_only: &str, path: &str) -> Option<&MountedEntry> {
        find_in(&self.entries, host_with_port, host_only, path)
    }

    pub async fn dispatch(&self, mut req: Request<Body>, peer: Option<IpAddr>) -> Response<Body> {
        let peer = peer.or_else(|| {
            req.extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|c| c.0.ip())
        });
        let from_edge = self.came_from_edge(&req, peer);
        let client = self.resolve_client_addr(&mut req, peer, from_edge);
        let (scheme, host_with_port) = self.resolve_scheme_and_host(&req, from_edge);
        // A module reads `X-Forwarded-For` itself (rate limits, the assistant's visitor): one that
        // did not come from the edge reaches no module with a forwarding header of its own.
        if !from_edge {
            for name in FORWARDING_HEADERS {
                req.headers_mut().remove(name);
            }
        }
        if !hosts::well_formed(&host_with_port) {
            return (StatusCode::BAD_REQUEST, "malformed Host header").into_response();
        }
        let host_only = hosts::without_port(&host_with_port);
        // One table for the whole request, however often the registry swaps it meanwhile.
        let table = self.hosts.as_ref().map(HostSwap::current);
        // The role is the table's word alone: a client's own header reaches no module.
        req.headers_mut().remove(HOST_ROLE_HEADER);
        let role = table
            .as_deref()
            .and_then(|t| t.role_of(&host_with_port, host_only));
        if let Some(role) = role {
            if let Some(allow) = role.allowed_methods()
                && !matches!(
                    *req.method(),
                    axum::http::Method::GET
                        | axum::http::Method::HEAD
                        | axum::http::Method::OPTIONS
                )
            {
                return (
                    StatusCode::METHOD_NOT_ALLOWED,
                    [(axum::http::header::ALLOW, allow)],
                    "this host answers reads only",
                )
                    .into_response();
            }
            for name in role.removed_request_headers() {
                req.headers_mut().remove(*name);
            }
            req.headers_mut().insert(
                HOST_ROLE_HEADER,
                axum::http::HeaderValue::from_static(role.as_str()),
            );
        }
        let public_path = req.uri().path().to_owned();
        let projects = self.hosts.as_ref().and_then(HostSwap::projects);
        let mut project_address = false;
        // The project door's address when its metadata is asked (MCP-33).
        let mut door_metadata: Option<String> = None;
        // The project id an id address placed the request under (RTE-05, T-7212).
        let mut placed_id: Option<String> = None;
        let credentials = req
            .headers()
            .contains_key(axum::http::header::AUTHORIZATION)
            || req.headers().contains_key(axum::http::header::COOKIE);
        let (path, organization, via) = match table.as_deref() {
            None => (req.uri().path().to_owned(), None, hosts::Via::Plain),
            Some(table) => {
                match table.place(&host_with_port, host_only, req.uri().path(), is_page(&req)) {
                    hosts::Placed::Route {
                        path,
                        organization,
                        via,
                    } => {
                        // An id address (RTE-05): placed under the project's name, or refused
                        // with the one answer every refusal there gets.
                        let placed = match (&organization, &via, &projects) {
                            (Some(org), hosts::Via::Prefixed(_), Some(projects)) => projects
                                .place(org, &path, credentials)
                                .map(|placed| (org, placed)),
                            _ => None,
                        };
                        match placed {
                            None => (path, organization, via),
                            Some((_, Err(()))) => return projects::not_found(),
                            Some((org, Ok(placed))) => {
                                project_address = true;
                                let id = path.split('/').nth(1).unwrap_or_default();
                                let door_base = format!("{scheme}://{host_with_port}/{org}/{id}");
                                match placed {
                                    projects::IdPlace::Named(named) => {
                                        placed_id = Some(id.to_owned());
                                        (named, organization, via)
                                    }
                                    projects::IdPlace::Challenge => {
                                        return projects::challenge(&door_base);
                                    }
                                    projects::IdPlace::Metadata => {
                                        door_metadata = Some(door_base);
                                        (projects::PORTAL_METADATA.to_owned(), organization, via)
                                    }
                                }
                            }
                        }
                    }
                    hosts::Placed::Canonical(path) => {
                        let location = match req.uri().query() {
                            Some(q) => format!("{path}?{q}"),
                            None => path,
                        };
                        return (
                            StatusCode::PERMANENT_REDIRECT,
                            [(axum::http::header::LOCATION, location)],
                        )
                            .into_response();
                    }
                    hosts::Placed::Misdirected => {
                        return (
                            StatusCode::MISDIRECTED_REQUEST,
                            "this host is not served here",
                        )
                            .into_response();
                    }
                    hosts::Placed::NotFound => {
                        return (StatusCode::NOT_FOUND, "unmounted path").into_response();
                    }
                }
            }
        };

        // A name never resolves to a branch: only its own id address reaches it (RTE-05,
        // TEN-12; T-7212), whether id addresses are on or not.
        let placed_branch = placed_id.as_ref().map(|id| format!("b-{id}"));
        if projects::names_other_branch(&path, req.uri().query(), placed_branch.as_deref()) {
            return projects::not_found();
        }
        if let Some(id) = placed_id {
            req.extensions_mut().insert(projects::IdAddress(id));
        }

        // A host the registry added answers its own entries, exact hosts all, so they come first
        // as the configuration's exact hosts do (T-7184).
        let added = self.hosts.as_ref().map(HostSwap::added);
        let entry = added
            .as_deref()
            .and_then(|added| find_in(added, &host_with_port, host_only, &path))
            .or_else(|| self.find(&host_with_port, host_only, &path));
        // A page asked on another group's host moves to the configured host that serves it:
        // the Location names a host of the table, never the request's (T-6991 §7).
        if let Some(table) = table.as_deref()
            && !project_address
            && is_page(&req)
            && let Some((to, rest)) =
                table.page_move(&host_with_port, host_only, &public_path, entry.is_some())
        {
            let location = match req.uri().query() {
                Some(q) => format!("{scheme}://{to}{rest}?{q}"),
                None => format!("{scheme}://{to}{rest}"),
            };
            return (
                StatusCode::PERMANENT_REDIRECT,
                [(axum::http::header::LOCATION, location)],
            )
                .into_response();
        }
        let Some(entry) = entry else {
            if project_address {
                return projects::not_found();
            }
            return (StatusCode::NOT_FOUND, "unmounted path").into_response();
        };

        let stripped_path = if entry.prefix.is_empty() {
            path.clone()
        } else {
            let rest = &path[entry.prefix.len()..];
            if rest.is_empty() {
                "/".to_owned()
            } else {
                rest.to_owned()
            }
        };

        let new_pq = match req.uri().query() {
            Some(q) => format!("{stripped_path}?{q}"),
            None => stripped_path,
        };
        if let Ok(new_uri) = Uri::try_from(new_pq) {
            *req.uri_mut() = new_uri;
        }

        // Without a host table an entry with no host answers any Host, and a module builds its
        // issuer and links from the Mount (I-7): a Host the edge did not vouch for names the base
        // host there, so a forged one never reaches an issuer, a token or a link.
        let host = if table.is_none() && entry.host.is_none() && !from_edge {
            self.default_host.clone()
        } else {
            host_with_port
        };
        let mount = Mount {
            module: entry.module.clone(),
            scheme,
            host,
            path_prefix: via.public_prefix(&entry.prefix),
        };
        req.extensions_mut().insert(mount);
        req.extensions_mut()
            .insert(RoutePrefix(entry.prefix.clone()));
        match organization {
            Some(org) => {
                req.extensions_mut().insert(Organization(org));
            }
            None => {
                req.extensions_mut().remove::<Organization>();
            }
        }
        match client {
            Some(ip) => {
                req.extensions_mut().insert(ClientAddr(ip));
            }
            None => {
                req.extensions_mut().remove::<ClientAddr>();
            }
        }

        let mut resp = entry
            .router
            .clone()
            .oneshot(req)
            .await
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        if role.is_some_and(HostRole::drops_set_cookie) {
            resp.headers_mut().remove(axum::http::header::SET_COOKIE);
        }
        if let Some(door_base) = door_metadata {
            return projects::metadata(resp, &door_base).await;
        }
        if project_address && projects::refused(resp.status()) {
            return projects::not_found();
        }
        resp
    }

    pub fn into_axum_router(self) -> Router {
        let this = self.clone();
        Router::new().fallback(move |req: Request<Body>| {
            let router = this.clone();
            async move { router.dispatch(req, None).await }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Extension;
    use axum::routing::get;
    use http_body_util::BodyExt;

    async fn echo_mount(Extension(mount): Extension<Mount>, uri: Uri) -> String {
        format!(
            "module={} issuer={} cookie={} path={}",
            mount.module,
            mount.realm_issuer("dev"),
            mount.cookie_name("sso"),
            uri.path()
        )
    }

    async fn echo_client(client: Option<Extension<ClientAddr>>) -> String {
        client.map_or_else(|| "none".to_owned(), |c| c.0.0.to_string())
    }

    async fn client_of(router: &JcRouter, peer: Option<&str>, xff: Option<&str>) -> String {
        let mut b = Request::builder().uri("/ip").header("host", "city.example");
        if let Some(x) = xff {
            b = b.header("x-forwarded-for", x);
        }
        let resp = router
            .dispatch(
                b.body(Body::empty()).unwrap(),
                peer.map(|p| p.parse().unwrap()),
            )
            .await;
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn client_addr_trusts_forwarded_for_only_from_trusted_proxies() {
        let router = JcRouterBuilder::new("https", "city.example")
            .trust_proxy("10.0.0.0/8".parse().unwrap())
            .mount(
                MountSpec {
                    module: "m".into(),
                    host: None,
                    path_prefix: None,
                },
                Router::new().route("/ip", get(echo_client)),
            )
            .build()
            .unwrap();
        // Untrusted peer: its forged header is ignored.
        assert_eq!(
            client_of(&router, Some("203.0.113.9"), Some("198.51.100.1")).await,
            "203.0.113.9"
        );
        // Trusted proxy: the right-most hop that is not a proxy.
        assert_eq!(
            client_of(
                &router,
                Some("10.0.0.5"),
                Some("198.51.100.1, 203.0.113.7, 10.1.1.1")
            )
            .await,
            "203.0.113.7"
        );
        // Trusted proxy without a header: the peer itself.
        assert_eq!(client_of(&router, Some("10.0.0.5"), None).await, "10.0.0.5");
        // Unknown peer: no address, never a shared placeholder.
        assert_eq!(client_of(&router, None, Some("198.51.100.1")).await, "none");
    }

    /// Every caller on the internet reaches jc through the same edge hops, so a module that keys
    /// a limit on the last `X-Forwarded-For` entry (the gateway's anonymous MCP bucket) must find
    /// the client there, never the in-cluster proxy that forwarded it (T-6788): one flooding client
    /// is refused while a second one behind the same proxy is still answered.
    #[tokio::test]
    async fn the_forwarded_chain_ends_at_the_client_so_each_client_has_its_own_bucket() {
        use std::collections::HashMap;
        use std::sync::Mutex;
        let spent: Arc<Mutex<HashMap<String, u32>>> = Arc::default();
        let limit = {
            let spent = Arc::clone(&spent);
            move |headers: axum::http::HeaderMap| async move {
                let last = headers
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.rsplit(',').next())
                    .map_or("unknown", str::trim)
                    .to_owned();
                let mut spent = spent.lock().unwrap();
                let used = spent.entry(last.clone()).or_default();
                *used += 1;
                let status = if *used > 3 {
                    StatusCode::TOO_MANY_REQUESTS
                } else {
                    StatusCode::OK
                };
                (status, last)
            }
        };
        let router = JcRouterBuilder::new("https", "city.example")
            .trust_proxy("10.42.0.0/16".parse().unwrap())
            .mount(
                MountSpec {
                    module: "gateway".into(),
                    host: None,
                    path_prefix: None,
                },
                Router::new().route("/mcp", get(limit)),
            )
            .build()
            .unwrap();
        let ask = |xff: &str| {
            let req = Request::builder()
                .uri("/mcp")
                .header("host", "city.example")
                .header("x-forwarded-for", xff)
                .body(Body::empty())
                .unwrap();
            router.dispatch(req, Some("10.42.0.2".parse().unwrap()))
        };
        let body = |resp: Response<Body>| async move {
            let status = resp.status();
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8(bytes.to_vec()).unwrap())
        };
        // A floods through the edge's hops; a forged leftmost entry buys it no new bucket.
        for forged in ["1.1.1.1", "2.2.2.2", "3.3.3.3"] {
            let chain = format!("{forged}, 198.51.100.1, 10.42.0.7, 10.42.0.2");
            assert_eq!(
                body(ask(&chain).await).await,
                (StatusCode::OK, "198.51.100.1".into())
            );
        }
        assert_eq!(
            body(ask("9.9.9.9, 198.51.100.1, 10.42.0.7, 10.42.0.2").await).await,
            (StatusCode::TOO_MANY_REQUESTS, "198.51.100.1".into())
        );
        // B behind the same proxies is its own bucket.
        assert_eq!(
            body(ask("203.0.113.7, 10.42.0.7, 10.42.0.2").await).await,
            (StatusCode::OK, "203.0.113.7".into())
        );
        // A chain of proxies alone names its first hop, as `ClientAddr` does.
        assert_eq!(
            body(ask("10.42.0.9, 10.42.0.2").await).await,
            (StatusCode::OK, "10.42.0.9".into())
        );
        // The same chain from a peer that is not the edge reaches the module with no header.
        let req = Request::builder()
            .uri("/mcp")
            .header("host", "city.example")
            .header("x-forwarded-for", "203.0.113.7")
            .body(Body::empty())
            .unwrap();
        let spoofed = router.dispatch(req, Some("198.51.100.200".parse().unwrap()));
        assert_eq!(
            body(spoofed.await).await,
            (StatusCode::OK, "unknown".into())
        );
    }

    /// In a mesh every caller reaches jc from its own proxy's address, so the edge is named by its
    /// mTLS identity: a meshed tenant workload (a wasm shard) on the same address names no client,
    /// host or scheme, and no module sees the forwarding headers it sent.
    /// T-6788: a client's entry that is no text, on the line the edge appended the client's
    /// address to, still leaves the client's address as the chain's last entry.
    #[tokio::test]
    async fn an_opaque_client_entry_keeps_the_edges_appended_address() {
        let router = JcRouterBuilder::new("https", "city.example")
            .trust_proxy("10.42.0.0/16".parse().unwrap())
            .mount(
                MountSpec {
                    module: "gateway".into(),
                    host: None,
                    path_prefix: None,
                },
                Router::new().route(
                    "/mcp",
                    get(|headers: axum::http::HeaderMap| async move {
                        headers
                            .get("x-forwarded-for")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("none")
                            .to_owned()
                    }),
                ),
            )
            .build()
            .unwrap();
        let req = Request::builder()
            .uri("/mcp")
            .header("host", "city.example")
            .header(
                "x-forwarded-for",
                axum::http::HeaderValue::from_bytes(b"\xff, 203.0.113.9").unwrap(),
            )
            .body(Body::empty())
            .unwrap();
        let resp = router
            .dispatch(req, Some("10.42.0.2".parse().unwrap()))
            .await;
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"203.0.113.9");
    }

    #[tokio::test]
    async fn forwarding_headers_count_only_from_the_edges_mesh_identity() {
        const EDGE: &str = "apisix.jc-prod.serviceaccount.identity.linkerd.cluster.local";
        async fn seen(
            client: Option<Extension<ClientAddr>>,
            headers: axum::http::HeaderMap,
        ) -> String {
            let names: Vec<&str> = FORWARDING_HEADERS
                .iter()
                .copied()
                .filter(|name| headers.contains_key(*name))
                .collect();
            format!(
                "{} {}",
                client.map_or_else(|| "none".to_owned(), |c| c.0.0.to_string()),
                names.join(",")
            )
        }
        let router = JcRouterBuilder::new("https", "city.example")
            .trust_proxy("10.0.0.0/8".parse().unwrap())
            .trust_identity(EDGE)
            .mount(
                MountSpec {
                    module: "m".into(),
                    host: Some("city.example".into()),
                    path_prefix: None,
                },
                Router::new().route("/ip", get(seen)),
            )
            .mount(
                MountSpec {
                    module: "other".into(),
                    host: Some("other.example".into()),
                    path_prefix: None,
                },
                Router::new().route("/ip", get(|| async { "other" })),
            )
            .build()
            .unwrap();
        let ask = |identity: Option<&str>| {
            let mut b = Request::builder()
                .uri("/ip")
                .header("host", "city.example")
                .header("x-forwarded-for", "198.51.100.1")
                .header("x-forwarded-host", "other.example")
                .header("forwarded", "for=198.51.100.1")
                .header("x-real-ip", "198.51.100.1");
            if let Some(id) = identity {
                b = b.header(MESH_IDENTITY, id);
            }
            let req = b.body(Body::empty()).unwrap();
            let router = router.clone();
            async move {
                let resp = router
                    .dispatch(req, Some("10.0.0.5".parse().unwrap()))
                    .await;
                let bytes = resp.into_body().collect().await.unwrap().to_bytes();
                String::from_utf8(bytes.to_vec()).unwrap()
            }
        };
        // The edge: its X-Forwarded-Host routes and its client counts.
        assert_eq!(ask(Some(EDGE)).await, "other");
        // A shard on the same address: its own host, its peer, and no forwarding header left.
        let shard = "shard.jc-prod.serviceaccount.identity.linkerd.cluster.local";
        assert_eq!(ask(Some(shard)).await, "10.0.0.5 ");
        // No identity (an unmeshed caller): the same.
        assert_eq!(ask(None).await, "10.0.0.5 ");
    }

    /// Brief 119: the assistant reads the gateway's host on jc's own listener from loopback. A
    /// loopback request that forges the edge's identity, a client address, a forwarded host and
    /// a gateway client is not the edge even when the edge's network and identity are named:
    /// its own host routes, its peer is the client, and no forwarding header reaches a module.
    #[tokio::test]
    async fn a_loopback_request_forging_the_edge_is_not_the_edge() {
        const EDGE: &str = "apisix.jc-prod.serviceaccount.identity.linkerd.cluster.local";
        async fn seen(
            client: Option<Extension<ClientAddr>>,
            headers: axum::http::HeaderMap,
        ) -> String {
            let names: Vec<&str> = FORWARDING_HEADERS
                .iter()
                .copied()
                .filter(|name| headers.contains_key(*name))
                .collect();
            format!(
                "{} {}",
                client.map_or_else(|| "none".to_owned(), |c| c.0.0.to_string()),
                names.join(",")
            )
        }
        let router = JcRouterBuilder::new("https", "smartcity.test")
            .trust_proxy("10.42.0.0/16".parse().unwrap())
            .trust_identity(EDGE)
            .mount(
                MountSpec {
                    module: "gateway".into(),
                    host: Some("api.smartcity.test".into()),
                    path_prefix: None,
                },
                Router::new().fallback(seen),
            )
            .mount(
                MountSpec {
                    module: "other".into(),
                    host: Some("other.smartcity.test".into()),
                    path_prefix: None,
                },
                Router::new().fallback(|| async { "other" }),
            )
            .build()
            .unwrap();
        for peer in ["127.0.0.1", "::1"] {
            let req = Request::builder()
                .uri("/api/endpoint/x/ngsi-ld/v1/entities?type=T&count=true&limit=0")
                .header("host", "api.smartcity.test")
                .header(MESH_IDENTITY, EDGE)
                .header("x-forwarded-for", "203.0.113.7")
                .header("x-forwarded-host", "other.smartcity.test")
                .header("x-forwarded-proto", "https")
                .header("x-jc-client", "context-gateway")
                .body(Body::empty())
                .unwrap();
            let resp = router.dispatch(req, Some(peer.parse().unwrap())).await;
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(
                String::from_utf8(bytes.to_vec()).unwrap(),
                format!("{peer} ")
            );
        }
    }

    /// I-7: one domain, no host table: a forged Host (or X-Forwarded-Host from a peer that is not
    /// the edge) still routes, and the Mount names the base host; the edge's forwarded host counts.
    #[tokio::test]
    async fn a_forged_host_never_reaches_the_mount_without_a_host_table() {
        let jc = JcRouterBuilder::new("https", "city.example")
            .trust_proxy("10.0.0.0/8".parse().unwrap())
            .mount(
                MountSpec {
                    module: "kaisatsu".into(),
                    host: None,
                    path_prefix: Some("/login".into()),
                },
                Router::new().route("/realms/dev", get(echo_mount)),
            )
            .build()
            .unwrap();
        let issuer = |host: &str, forwarded: Option<&str>, peer: &str| {
            let mut req = Request::builder()
                .uri("/login/realms/dev")
                .header("host", host);
            if let Some(f) = forwarded {
                req = req.header("x-forwarded-host", f);
            }
            let req = req.body(Body::empty()).unwrap();
            let jc = jc.clone();
            let peer = peer.parse().unwrap();
            async move {
                let resp = jc.dispatch(req, Some(peer)).await;
                assert_eq!(resp.status(), StatusCode::OK);
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                String::from_utf8(body.to_vec()).unwrap()
            }
        };
        let base = "issuer=https://city.example/login/realms/dev";
        assert!(
            issuer("city.example", None, "127.0.0.1")
                .await
                .contains(base)
        );
        assert!(
            issuer("evil.example", None, "127.0.0.1")
                .await
                .contains(base)
        );
        assert!(
            issuer("192.168.7.9:8080", None, "192.168.7.9")
                .await
                .contains(base)
        );
        assert!(
            issuer("city.example", Some("evil.example"), "127.0.0.1")
                .await
                .contains(base)
        );
        assert!(
            issuer("city.example", Some("jc.city.example"), "10.0.0.5")
                .await
                .contains("issuer=https://jc.city.example/login/realms/dev")
        );
    }

    /// T-7184: a host the registry adds answers the modules its role names, with the routers they
    /// answer elsewhere, instead of falling to the host-less entries; a host the configuration
    /// mounts keeps its own entries; a removed host answers `421`; a set with two specs on one
    /// place changes nothing.
    #[tokio::test]
    async fn a_host_the_registry_adds_answers_its_modules_and_a_removed_one_421() {
        let answer = |name: &'static str| {
            Router::new().fallback(move |org: Option<Extension<Organization>>| async move {
                format!("{name}|{}", org.map_or_else(String::new, |o| o.0.0))
            })
        };
        let table = HostTable::new()
            .organization("bbsk")
            .unwrap()
            .bound("smartcity.bbsk.sk", "bbsk", "/")
            .unwrap()
            .shared("city.example")
            .unwrap();
        let spec = |module: &str, host: Option<&str>, prefix: Option<&str>| MountSpec {
            module: module.into(),
            host: host.map(str::to_owned),
            path_prefix: prefix.map(str::to_owned),
        };
        let jc = JcRouterBuilder::new("https", "city.example")
            .hosts(table)
            .module_router("portal", answer("portal"))
            .module_router("portal", answer("second"))
            .module_router("gateway", answer("gateway"))
            .mount(
                spec("portal", Some("smartcity.bbsk.sk"), None),
                answer("bbsk"),
            )
            .mount(spec("shared", None, None), answer("shared"))
            .build()
            .unwrap();
        let swap = jc.host_swap().expect("a host table");
        let get = |host: &'static str, path: &'static str| {
            let jc = jc.clone();
            async move {
                let req = Request::builder()
                    .uri(path)
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap();
                let resp = jc.dispatch(req, Some("127.0.0.1".parse().unwrap())).await;
                let status = resp.status();
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                (status, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        let served = |served| {
            [
                RegistryHost {
                    host: "smartcity.bbsk.sk",
                    organization: "bbsk",
                    organization_id: None,
                    role: "working",
                    served: true,
                },
                RegistryHost {
                    host: "smartcity.zilina.sk",
                    organization: "zilina",
                    organization_id: None,
                    role: "working",
                    served,
                },
            ]
        };
        swap.rebuild(&served(true)).unwrap();
        let ok = |body: &str| (StatusCode::OK, body.to_owned());
        assert_eq!(
            get("smartcity.zilina.sk", "/").await,
            ok("shared|zilina"),
            "without its mounts the host falls to the host-less entries"
        );
        let specs = [
            spec("portal", Some("Smartcity.Zilina.sk"), None),
            spec("gateway", Some("smartcity.zilina.sk"), Some("/api")),
            spec("portal", Some("smartcity.bbsk.sk:443"), Some("/x")),
            spec("kaisatsu", Some("idm.zilina.sk"), None),
            spec("portal", Some("*.apps.zilina.sk"), None),
        ];
        assert_eq!(swap.mount_hosts(&specs).unwrap(), 2);
        assert_eq!(get("smartcity.zilina.sk", "/").await, ok("portal|zilina"));
        assert_eq!(
            get("smartcity.zilina.sk", "/api/v1").await,
            ok("gateway|zilina")
        );
        assert_eq!(get("smartcity.bbsk.sk", "/x").await, ok("bbsk|bbsk"));
        // A shared host's path that names no Organization is none of the deployment's (S1-03).
        assert_eq!(
            get("city.example", "/").await,
            (StatusCode::NOT_FOUND, "unmounted path".to_owned())
        );
        let twice = [
            spec("portal", Some("smartcity.zilina.sk"), None),
            spec("gateway", Some("smartcity.zilina.sk"), Some("/")),
        ];
        assert!(swap.mount_hosts(&twice).is_err());
        assert_eq!(get("smartcity.zilina.sk", "/").await, ok("portal|zilina"));
        swap.rebuild(&served(false)).unwrap();
        assert_eq!(
            get("smartcity.zilina.sk", "/").await.0,
            StatusCode::MISDIRECTED_REQUEST
        );
        // A removed Organization of the configuration: no host and no shared path answers it.
        assert_eq!(get("city.example", "/bbsk/x").await, ok("shared|bbsk"));
        swap.rebuild_without(&served(true)[1..], &["bbsk"]).unwrap();
        assert_eq!(
            get("smartcity.bbsk.sk", "/").await.0,
            StatusCode::MISDIRECTED_REQUEST
        );
        // With jc.toml's last Organization removed only the registry's remain, and they leave the
        // shared host's paths open (T-7271): `/bbsk/x` is a path of the deployment's own.
        assert_eq!(get("city.example", "/bbsk/x").await, ok("shared|"));
        assert_eq!(get("smartcity.zilina.sk", "/").await, ok("portal|zilina"));
    }

    /// TEN-12, CHG-15, RTE-05 (T-7212): a branch answers on its own id address with the id as
    /// the module's [`IdAddress`]; its name, on a name address or in a query, gets body P
    /// whether id addresses are on or not; an expired branch's id gets the same bytes as an
    /// unknown id; a token on project A's id address naming branch B is refused alike.
    #[tokio::test]
    async fn a_branch_answers_only_on_its_own_live_id_address() {
        const BRANCH: &str = "daaaaaaaaaaaaaaaaaaaaaaaaa";
        const EXPIRED: &str = "eaaaaaaaaaaaaaaaaaaaaaaaaa";
        const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
        let api = Router::new().fallback(|req: Request<Body>| async move {
            let placed = req
                .extensions()
                .get::<IdAddress>()
                .map_or("-".to_owned(), |a| a.0.clone());
            format!("{}|{placed}", req.uri())
        });
        let table = HostTable::new()
            .shared("city.example")
            .and_then(|t| t.organization("bratislava"))
            .unwrap();
        let jc = JcRouterBuilder::new("https", "city.example")
            .hosts(table)
            .mount(
                MountSpec {
                    module: "portal".into(),
                    host: None,
                    path_prefix: Some("/api".into()),
                },
                api,
            )
            .build()
            .unwrap();
        let call = |path: String| {
            let jc = jc.clone();
            async move {
                let req = Request::builder()
                    .uri(path)
                    .header("host", "city.example")
                    .header("authorization", "Bearer ok");
                let resp = jc.dispatch(req.body(Body::empty()).unwrap(), None).await;
                let status = resp.status();
                let headers = format!("{:?}", resp.headers());
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                (status, headers, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        let by_name = format!("/bratislava/api/v1/projects/b-{BRANCH}/spaces");
        // Off: no id address, and the branch's name is still refused.
        let off = call(by_name.clone()).await;
        assert_eq!(off.0, StatusCode::NOT_FOUND);
        assert!(off.2.contains("ask the project's owner"), "{}", off.2);

        jc.host_swap().expect("a host table").replace_projects(&[
            RegistryProject {
                organization: "bratislava",
                id: A,
                name: "traffic",
                expires_at: None,
            },
            RegistryProject {
                organization: "bratislava",
                id: BRANCH,
                name: &format!("b-{BRANCH}"),
                expires_at: Some(i64::MAX),
            },
            RegistryProject {
                organization: "bratislava",
                id: EXPIRED,
                name: &format!("b-{EXPIRED}"),
                expires_at: Some(1),
            },
        ]);
        let live = call(format!("/bratislava/{BRANCH}/api/v1/spaces")).await;
        assert_eq!(live.0, StatusCode::OK);
        assert_eq!(live.2, format!("/v1/projects/b-{BRANCH}/spaces|{BRANCH}"));
        let plain = call("/bratislava/api/v1/projects/traffic/spaces".into()).await;
        assert_eq!(plain.2, "/v1/projects/traffic/spaces|-");

        let unknown = call("/bratislava/caaaaaaaaaaaaaaaaaaaaaaaaa/api/v1/spaces".into()).await;
        assert_eq!(unknown.0, StatusCode::NOT_FOUND);
        for path in [
            by_name,
            format!("/bratislava/api/v1/mcp?project=b-{BRANCH}"),
            format!("/bratislava/{EXPIRED}/api/v1/spaces"),
            format!("/bratislava/{A}/api/v1/spaces?project=b-{BRANCH}"),
            format!("/bratislava/{BRANCH}/api/v1/spaces?project=b-{EXPIRED}"),
        ] {
            assert_eq!(call(path.clone()).await, unknown, "{path}");
        }
    }

    /// RTE-05 (T-7194): with id addresses on, an unknown id, another Organization's id, a
    /// request without a session, one without a grant, an unknown object, a redirect and a
    /// misspelt id answer the same bytes and headers; a granted call reaches the project by its
    /// name. Off, an id path is routed as it is.
    #[tokio::test]
    async fn every_refusal_on_an_id_address_is_the_same_404() {
        const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
        const FOREIGN: &str = "baaaaaaaaaaaaaaaaaaaaaaaaa";
        let api = Router::new().fallback(|headers: axum::http::HeaderMap, uri: Uri| async move {
            let auth = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok());
            match (auth, uri.path()) {
                (None, _) => (
                    StatusCode::UNAUTHORIZED,
                    [
                        ("www-authenticate", "Bearer resource_metadata=\"x\""),
                        ("set-cookie", "s=1"),
                    ],
                    "sign in".to_owned(),
                )
                    .into_response(),
                (Some("Bearer none"), _) => (StatusCode::FORBIDDEN, "no grant").into_response(),
                (_, "/v1/projects/traffic/gone") => {
                    (StatusCode::NOT_FOUND, "no such space").into_response()
                }
                (_, "/v1/projects/traffic/moved") => (
                    StatusCode::PERMANENT_REDIRECT,
                    [("location", "/bratislava/api/v1/projects/traffic/spaces")],
                )
                    .into_response(),
                (_, path) => path.to_owned().into_response(),
            }
        });
        let table = HostTable::new()
            .shared("city.example")
            .and_then(|t| t.organization("bratislava"))
            .and_then(|t| t.organization("kosice"))
            .unwrap();
        let jc = JcRouterBuilder::new("https", "city.example")
            .hosts(table)
            .mount(
                MountSpec {
                    module: "portal".into(),
                    host: None,
                    path_prefix: Some("/api".into()),
                },
                api,
            )
            .build()
            .unwrap();
        let call = |path: String, auth: Option<&'static str>| {
            let jc = jc.clone();
            async move {
                let mut req = Request::builder().uri(path).header("host", "city.example");
                if let Some(auth) = auth {
                    req = req.header("authorization", auth);
                }
                let resp = jc.dispatch(req.body(Body::empty()).unwrap(), None).await;
                let status = resp.status();
                let headers = format!("{:?}", resp.headers());
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                (status, headers, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        let spaces = format!("/bratislava/{ID}/api/v1/spaces");
        assert_eq!(
            call(spaces.clone(), Some("Bearer ok")).await.0,
            StatusCode::NOT_FOUND
        );

        jc.host_swap().expect("a host table").replace_projects(&[
            RegistryProject {
                organization: "bratislava",
                id: ID,
                name: "traffic",
                expires_at: None,
            },
            RegistryProject {
                organization: "kosice",
                id: FOREIGN,
                name: "air",
                expires_at: None,
            },
        ]);
        let granted = call(spaces.clone(), Some("Bearer ok")).await;
        assert_eq!(granted.0, StatusCode::OK);
        assert_eq!(granted.2, "/v1/projects/traffic/spaces");
        let refusal = call(spaces.clone(), None).await;
        assert_eq!(refusal.0, StatusCode::NOT_FOUND);
        assert!(
            refusal.2.contains("ask the project's owner"),
            "{}",
            refusal.2
        );
        for (path, auth) in [
            (
                format!("/bratislava/{FOREIGN}/api/v1/spaces"),
                Some("Bearer ok"),
            ),
            (
                "/bratislava/caaaaaaaaaaaaaaaaaaaaaaaaa/api/v1/spaces".to_owned(),
                Some("Bearer ok"),
            ),
            (spaces.clone(), Some("Bearer none")),
            (format!("/bratislava/{ID}/api/v1/gone"), Some("Bearer ok")),
            (format!("/bratislava/{ID}/api/v1/moved"), Some("Bearer ok")),
            (format!("/bratislava/{ID}/data"), Some("Bearer ok")),
            (
                format!("/bratislava/{}/api/v1/spaces", ID.to_uppercase()),
                Some("Bearer ok"),
            ),
        ] {
            assert_eq!(call(path.clone(), auth).await, refusal, "{path} {auth:?}");
        }
        // The name address is untouched.
        let named = call("/bratislava/api/v1/projects/traffic/spaces".into(), None).await;
        assert_eq!(named.0, StatusCode::UNAUTHORIZED);
    }

    /// MCP-33 (T-7211): the project door's metadata answers without a session and names the id
    /// address as its resource, and the door challenges a caller without credentials, both the
    /// same for this Organization's id, another Organization's id and an unknown one (nothing
    /// tells them apart but the id the caller sent); a caller with credentials but no grant
    /// still gets the one 404.
    #[tokio::test]
    async fn the_project_door_metadata_answers_without_a_session_and_leaks_no_project() {
        const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaa";
        const FOREIGN: &str = "baaaaaaaaaaaaaaaaaaaaaaaaa";
        const UNKNOWN: &str = "caaaaaaaaaaaaaaaaaaaaaaaaa";
        let api = Router::new().fallback(|headers: axum::http::HeaderMap| async move {
            match headers.get(axum::http::header::AUTHORIZATION) {
                None => StatusCode::UNAUTHORIZED.into_response(),
                Some(_) => StatusCode::FORBIDDEN.into_response(),
            }
        });
        let well_known = Router::new().route(
            "/oauth-protected-resource/api/v1/mcp",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "resource": "https://city.example/api/v1/mcp",
                    "authorization_servers": ["https://city.example/id"],
                    "scopes_supported": ["mcp:portal"]
                }))
            }),
        );
        let table = HostTable::new()
            .shared("city.example")
            .and_then(|t| t.organization("bratislava"))
            .and_then(|t| t.organization("kosice"))
            .unwrap();
        let jc = JcRouterBuilder::new("https", "city.example")
            .hosts(table)
            .mount(
                MountSpec {
                    module: "portal".into(),
                    host: None,
                    path_prefix: Some("/api".into()),
                },
                api,
            )
            .mount(
                MountSpec {
                    module: "portal".into(),
                    host: None,
                    path_prefix: Some("/.well-known".into()),
                },
                well_known,
            )
            .build()
            .unwrap();
        jc.host_swap().expect("a host table").replace_projects(&[
            RegistryProject {
                organization: "bratislava",
                id: ID,
                name: "traffic",
                expires_at: None,
            },
            RegistryProject {
                organization: "kosice",
                id: FOREIGN,
                name: "air",
                expires_at: None,
            },
        ]);
        let call = |method: &'static str, path: String, auth: Option<&'static str>| {
            let jc = jc.clone();
            async move {
                let mut req = Request::builder()
                    .method(method)
                    .uri(path)
                    .header("host", "city.example");
                if let Some(auth) = auth {
                    req = req.header("authorization", auth);
                }
                let resp = jc.dispatch(req.body(Body::empty()).unwrap(), None).await;
                let status = resp.status();
                let headers = format!("{:?}", resp.headers());
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                (status, headers, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        let meta =
            |id: &str| format!("/bratislava/{id}/api/mcp/.well-known/oauth-protected-resource");
        let own = call("GET", meta(ID), None).await;
        assert_eq!(own.0, StatusCode::OK);
        let doc: serde_json::Value = serde_json::from_str(&own.2).unwrap();
        assert_eq!(
            doc["resource"],
            format!("https://city.example/bratislava/{ID}/api/mcp")
        );
        assert_eq!(doc["authorization_servers"][0], "https://city.example/id");
        assert!(!own.2.contains("traffic"), "{}", own.2);
        for other in [FOREIGN, UNKNOWN] {
            let answer = call("GET", meta(other), None).await;
            assert_eq!(answer.1, own.1, "{other}");
            assert_eq!(answer.2, own.2.replace(ID, other), "{other}");
        }

        let door = |id: &str| format!("/bratislava/{id}/api/mcp");
        let challenge = call("POST", door(ID), None).await;
        assert_eq!(challenge.0, StatusCode::UNAUTHORIZED);
        assert!(
            challenge.1.contains(&format!(
                "resource_metadata=\\\"https://city.example/bratislava/{ID}/api/mcp/.well-known/oauth-protected-resource\\\""
            )),
            "{}",
            challenge.1
        );
        for other in [FOREIGN, UNKNOWN] {
            let answer = call("POST", door(other), None).await;
            assert_eq!(
                (answer.0, answer.1, answer.2),
                (
                    challenge.0,
                    challenge.1.replace(ID, other),
                    challenge.2.clone()
                ),
                "{other}"
            );
        }
        let refused = call("POST", door(ID), Some("Bearer none")).await;
        assert_eq!(refused.0, StatusCode::NOT_FOUND);
        assert_eq!(
            call("POST", door(UNKNOWN), Some("Bearer none")).await,
            refused
        );
    }

    /// T-7136 (T-7129 §7, O1b): a swap from the host registry takes a host out (`421`) and puts
    /// a served one in without a restart, with its Organization and role; a host whose row is
    /// not `served` stays out; a rebuild that fails keeps the table that answers.
    #[tokio::test]
    async fn the_registry_swaps_hosts_in_and_out_and_a_failed_build_keeps_the_table() {
        let table = HostTable::new()
            .bound("smartcity.bbsk.sk", "bbsk", "/")
            .unwrap()
            .shared("city.example")
            .unwrap();
        let jc = JcRouterBuilder::new("https", "city.example")
            .hosts(table)
            .mount(
                MountSpec {
                    module: "portal".into(),
                    host: None,
                    path_prefix: None,
                },
                Router::new().fallback(
                    |org: Option<Extension<Organization>>,
                     headers: axum::http::HeaderMap| async move {
                        let role = headers
                            .get(HOST_ROLE_HEADER)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("-")
                            .to_owned();
                        format!("{}|{role}", org.map_or_else(String::new, |o| o.0.0))
                    },
                ),
            )
            .mount(
                MountSpec {
                    module: "gateway".into(),
                    host: None,
                    path_prefix: Some("/api".into()),
                },
                Router::new(),
            )
            .build()
            .unwrap();
        let swap = jc.host_swap().expect("a host table");
        let get = |host: &'static str| {
            let jc = jc.clone();
            async move {
                let req = Request::builder()
                    .uri("/")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap();
                let resp = jc.dispatch(req, Some("127.0.0.1".parse().unwrap())).await;
                let status = resp.status();
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                (status, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        let row = |host, organization, role, served| RegistryHost {
            host,
            organization,
            organization_id: None,
            role,
            served,
        };
        assert_eq!(get("smartcity.bbsk.sk").await.0, StatusCode::OK);
        assert_eq!(
            get("data.zilina.sk").await.0,
            StatusCode::MISDIRECTED_REQUEST
        );
        swap.rebuild(&[
            row("smartcity.bbsk.sk", "bbsk", "working", false),
            row("data.zilina.sk", "zilina", "working", true),
            row("api.zilina.sk", "zilina", "api", false),
        ])
        .unwrap();
        assert_eq!(
            get("smartcity.bbsk.sk").await.0,
            StatusCode::MISDIRECTED_REQUEST
        );
        assert_eq!(
            get("data.zilina.sk").await,
            (StatusCode::OK, "zilina|working".to_owned())
        );
        assert_eq!(
            get("api.zilina.sk").await.0,
            StatusCode::MISDIRECTED_REQUEST
        );
        // The shared host's root names no Organization (S1-03).
        assert_eq!(get("city.example").await.0, StatusCode::NOT_FOUND);
        // An Organization named like a mount's first segment, and a host that is no host name:
        // each build fails and the table of the last good build answers.
        for broken in [
            row("data.api.sk", "api", "working", true),
            row("bad_host.sk", "zilina", "working", true),
        ] {
            assert!(swap.rebuild(&[broken]).is_err(), "{broken:?}");
            assert_eq!(
                get("data.zilina.sk").await,
                (StatusCode::OK, "zilina|working".to_owned())
            );
        }
        // Every rebuild starts from the configuration: a host served again comes back.
        swap.rebuild(&[row("smartcity.bbsk.sk", "bbsk", "working", true)])
            .unwrap();
        assert_eq!(get("smartcity.bbsk.sk").await.0, StatusCode::OK);
        assert_eq!(
            get("data.zilina.sk").await.0,
            StatusCode::MISDIRECTED_REQUEST
        );
    }

    /// RTE-02: a Host (or the edge's X-Forwarded-Host) that is no plain authority is refused
    /// before it is matched, so its text never becomes the base of an issuer or a link.
    #[tokio::test]
    async fn a_host_with_userinfo_or_a_bad_port_is_refused_and_never_mounted() {
        let jc = JcRouterBuilder::new("https", "city.example")
            .trust_proxy("10.0.0.0/8".parse().unwrap())
            .mount(
                MountSpec {
                    module: "kaisatsu".into(),
                    host: Some("city.example".into()),
                    path_prefix: Some("/login".into()),
                },
                Router::new().route("/realms/dev", get(echo_mount)),
            )
            .build()
            .unwrap();
        let on = |host: &str, forwarded: Option<&str>| {
            let mut req = Request::builder()
                .uri("/login/realms/dev")
                .header("host", host);
            if let Some(f) = forwarded {
                req = req.header("x-forwarded-host", f);
            }
            let req = req.body(Body::empty()).unwrap();
            let jc = jc.clone();
            async move {
                let resp = jc.dispatch(req, Some("10.0.0.5".parse().unwrap())).await;
                let status = resp.status();
                let body = resp.into_body().collect().await.unwrap().to_bytes();
                (status, String::from_utf8(body.to_vec()).unwrap())
            }
        };
        for host in [
            "city.example:@evil.example",
            "city.example:9999@evil.example",
            "city.example:80:81",
            "city.example:",
            "city.example/evil",
            "[::1",
        ] {
            let (status, body) = on(host, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{host}");
            assert!(!body.contains("evil"), "{host}: {body}");
            let (status, _) = on("city.example", Some(host)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "forwarded {host}");
        }
        let (status, body) = on("City.Example:8443", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains("issuer=https://city.example:8443/login/realms/dev"),
            "{body}"
        );
        assert!(super::hosts::well_formed("[2001:db8::1]:443"));
        assert_eq!(
            super::hosts::without_port("[2001:db8::1]:443"),
            "[2001:db8::1]"
        );
    }

    #[tokio::test]
    async fn routes_one_domain_with_slugs_and_host_per_module_and_refuses_overlap() {
        // 1. Overlapping mount refusal
        let overlap = JcRouterBuilder::new("https", "city.example")
            .mount(
                MountSpec {
                    module: "kaisatsu".into(),
                    host: Some("city.example".into()),
                    path_prefix: Some("/login".into()),
                },
                Router::new(),
            )
            .mount(
                MountSpec {
                    module: "other".into(),
                    host: Some("city.example".into()),
                    path_prefix: Some("/login/".into()),
                },
                Router::new(),
            )
            .build();
        assert!(matches!(
            overlap,
            Err(RouterError::OverlappingMounts { .. })
        ));

        // 2. Valid one-domain slugs + dedicated host + 404 on unmounted path
        let id_router = Router::new().route("/realms/dev", get(echo_mount));
        let portal_router = Router::new().route("/home", get(echo_mount));
        let dedicated_router = Router::new().route("/realms/dev", get(echo_mount));

        let jc = JcRouterBuilder::new("https", "city.example")
            .trust_proxy("10.0.0.0/8".parse().unwrap())
            .mount(
                MountSpec {
                    module: "kaisatsu".into(),
                    host: Some("city.example".into()),
                    path_prefix: Some("/login".into()),
                },
                id_router,
            )
            .mount(
                MountSpec {
                    module: "portal".into(),
                    host: Some("city.example".into()),
                    path_prefix: Some("/portal".into()),
                },
                portal_router,
            )
            .mount(
                MountSpec {
                    module: "kaisatsu-dedicated".into(),
                    host: Some("id.city.example".into()),
                    path_prefix: None,
                },
                dedicated_router,
            )
            .build()
            .expect("router");

        // Request under `/login` on `city.example`
        let req1 = Request::builder()
            .uri("/login/realms/dev")
            .header("host", "city.example")
            .body(Body::empty())
            .unwrap();
        let res1 = jc.dispatch(req1, None).await;
        assert_eq!(res1.status(), StatusCode::OK);
        let body1 = String::from_utf8(
            res1.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            body1,
            "module=kaisatsu issuer=https://city.example/login/realms/dev cookie=__Secure-wk_sso path=/realms/dev"
        );

        // Request at root on `id.city.example`
        let req2 = Request::builder()
            .uri("/realms/dev")
            .header("host", "id.city.example")
            .body(Body::empty())
            .unwrap();
        let res2 = jc.dispatch(req2, None).await;
        assert_eq!(res2.status(), StatusCode::OK);
        let body2 = String::from_utf8(
            res2.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            body2,
            "module=kaisatsu-dedicated issuer=https://id.city.example/realms/dev cookie=__Host-wk_sso path=/realms/dev"
        );

        // Unmounted path on `city.example` -> 404
        let req3 = Request::builder()
            .uri("/unmounted/path")
            .header("host", "city.example")
            .body(Body::empty())
            .unwrap();
        let res3 = jc.dispatch(req3, None).await;
        assert_eq!(res3.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn defaults_and_mount_helpers() {
        let m = Mount::default();
        assert_eq!(m.module, "kaisatsu");
        assert_eq!(m.public_base(), "https://idm.localhost");
        assert_eq!(m.cookie_name("sso"), "__Host-wk_sso");
        let m = Mount::new("kaisatsu", "https", "city.example", "/login/").unwrap();
        assert_eq!(
            m.realm_issuer("dev"),
            "https://city.example/login/realms/dev"
        );
        assert!(Mount::new("x", "https", "h", "id").is_err());
        assert!(Mount::new("x", "https", "h", "/a?b").is_err());
        assert!(Mount::new("x", "https", "h", "/a//b").is_err());
        assert_eq!(normalize_prefix(" / ").unwrap(), "");
        let b = JcRouterBuilder::default();
        assert_eq!(b.default_scheme, "https");
        assert_eq!(b.default_host, "localhost");
    }

    #[tokio::test]
    async fn bare_prefix_maps_to_root_and_query_is_kept() {
        async fn echo(uri: Uri) -> String {
            uri.to_string()
        }
        let jc = JcRouterBuilder::new("https", "city.example")
            .mount(
                MountSpec {
                    module: "m".into(),
                    host: None,
                    path_prefix: Some("/m".into()),
                },
                Router::new().route("/", get(echo)).route("/q", get(echo)),
            )
            .mount(
                MountSpec {
                    module: "root".into(),
                    host: None,
                    path_prefix: Some("/".into()),
                },
                Router::new().route("/", get(echo)),
            )
            .build()
            .unwrap();
        for (uri, want) in [
            ("/m", "/"),
            ("/m?x=1", "/?x=1"),
            ("/m/q?a=b&c", "/q?a=b&c"),
            ("/", "/"),
        ] {
            let resp = jc
                .dispatch(
                    Request::builder()
                        .uri(uri)
                        .header("host", "city.example")
                        .body(Body::empty())
                        .unwrap(),
                    None,
                )
                .await;
            assert_eq!(resp.status(), StatusCode::OK, "{uri}");
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), want, "{uri}");
        }
        // Two root mounts on the same (any) host overlap.
        let err = JcRouterBuilder::default()
            .mount(
                MountSpec {
                    module: "a".into(),
                    host: None,
                    path_prefix: None,
                },
                Router::new(),
            )
            .mount(
                MountSpec {
                    module: "b".into(),
                    host: Some(" ".into()),
                    path_prefix: Some("/".into()),
                },
                Router::new(),
            )
            .build()
            .err()
            .expect("overlap");
        assert_eq!(
            err.to_string(),
            "overlapping mounts between module 'a' and 'b' on host '*' prefix '/'"
        );
    }

    /// Router lookup over jc's whole host-mode table (dev's rows plus the default hosts, 30
    /// entries) against today's one entry per module (11). Timing only:
    /// `cargo test -p jc-router --release -- --ignored --nocapture lookup_time`.
    #[test]
    #[ignore = "timing, run by hand"]
    fn lookup_time_over_the_full_table() {
        fn table(specs: &[(&str, &str)]) -> JcRouter {
            let mut b = JcRouterBuilder::new("https", "city.test");
            for (i, (host, prefix)) in specs.iter().enumerate() {
                b = b.mount(
                    MountSpec {
                        module: format!("m{i}"),
                        host: Some((*host).into()),
                        path_prefix: Some((*prefix).into()),
                    },
                    Router::new(),
                );
            }
            b.build().unwrap()
        }
        let hosts: Vec<String> = [
            "id",
            "opendata",
            "dashboards",
            "streams",
            "broker",
            "api",
            "portal",
            "git",
            "assistant",
            "viz",
            "tiles",
        ]
        .iter()
        .map(|l| format!("{l}.city.test"))
        .collect();
        let one_each: Vec<(&str, &str)> = hosts.iter().map(|h| (h.as_str(), "")).collect();
        let full: Vec<(&str, &str)> = vec![
            ("idm.city.test", ""),
            ("portal.city.test", ""),
            ("opendata.city.test", ""),
            ("city.test", "/grafana"),
            ("city.test", "/git"),
            ("api.city.test", ""),
            ("city.test", "/cs"),
            ("city.test", "/api/endpoint"),
            ("city.test", "/api/mcp"),
            ("city.test", "/catalog"),
            ("city.test", "/catalog.jsonld"),
            ("city.test", "/catalog.ttl"),
            ("portal.city.test", "/cs"),
            ("portal.city.test", "/api/endpoint"),
            ("portal.city.test", "/api/mcp"),
            ("streams.city.test", ""),
            ("broker.city.test", ""),
            ("assistant.city.test", ""),
            ("viz.city.test", ""),
            ("tiles.city.test", ""),
            ("status.city.test", ""),
            ("ai.city.test", ""),
            ("a1.city.test", ""),
            ("a2.city.test", ""),
            ("a3.city.test", ""),
            ("a4.city.test", ""),
            ("a5.city.test", ""),
            ("a6.city.test", ""),
            ("a7.city.test", ""),
            ("a8.city.test", ""),
        ];
        const N: u32 = 1_000_000;
        for (name, router) in [("full", table(&full)), ("one per module", table(&one_each))] {
            for (host, path) in [
                ("portal.city.test", "/api/endpoint/x/ngsi-ld/v1/entities"),
                ("nope.city.test", "/api/endpoint/x"),
            ] {
                // The fastest of five rounds: the host is shared, so slower rounds are noise.
                let mut best = u128::MAX;
                let mut hits = 0u32;
                for _ in 0..5 {
                    let start = std::time::Instant::now();
                    hits = 0;
                    for _ in 0..N {
                        let found = std::hint::black_box(&router).find(
                            std::hint::black_box(host),
                            host,
                            std::hint::black_box(path),
                        );
                        hits += u32::from(found.is_some());
                    }
                    best = best.min(start.elapsed().as_nanos() / u128::from(N));
                }
                let ns = best;
                println!(
                    "{name} ({} entries) {host}{path}: {ns} ns per lookup, {hits} hits",
                    router.entries.len()
                );
            }
        }
    }

    #[tokio::test]
    async fn a_wildcard_host_takes_one_label_after_every_exact_host() {
        let named = |module: &'static str| {
            Router::new()
                .fallback(move |uri: Uri| async move { format!("{module} {}", uri.path()) })
        };
        let spec = |module: &str, host: &str, prefix: Option<&str>| MountSpec {
            module: module.into(),
            host: Some(host.into()),
            path_prefix: prefix.map(str::to_owned),
        };
        let jc = JcRouterBuilder::new("https", "city.example")
            // Mounted first, so only the order of the table can let an exact host win.
            .mount(spec("portal", "*.apps.city.example", None), named("portal"))
            .mount(spec("shako", "git.apps.city.example", None), named("shako"))
            .mount(spec("gateway", "api.city.example", None), named("gateway"))
            .build()
            .unwrap();
        let on = |host: &str, path: &str| {
            let jc = jc.clone();
            let req = Request::builder()
                .uri(path)
                .header("host", host)
                .body(Body::empty())
                .unwrap();
            async move {
                let resp = jc.dispatch(req, None).await;
                let status = resp.status();
                let bytes = resp.into_body().collect().await.unwrap().to_bytes();
                (status, String::from_utf8(bytes.to_vec()).unwrap())
            }
        };
        assert_eq!(on("bikes.apps.city.example", "/x").await.1, "portal /x");
        assert_eq!(on("Bikes.Apps.City.Example:443", "/").await.1, "portal /");
        // An exact host beats the wildcard that also covers it.
        assert_eq!(on("git.apps.city.example", "/x").await.1, "shako /x");
        // No other host reaches the wildcard's module: not the suffix itself, not two labels,
        // not a look-alike, not a host another entry names.
        for host in [
            "apps.city.example",
            "a.b.apps.city.example",
            "bikesapps.city.example",
            "bikes.apps.city.example.evil.test",
            "portal.city.example",
        ] {
            assert_eq!(on(host, "/").await.0, StatusCode::NOT_FOUND, "{host}");
        }
        assert_eq!(on("api.city.example", "/cs").await.1, "gateway /cs");
    }
}
