#![forbid(unsafe_code)]
//! `jc-router` (W9): Mount table routing requests across modules by `(host?, path_prefix?)`.
//! - Supports one domain with different slugs (e.g. `city.example/id`, `/portal`, `/opendata`,
//!   `/dsp`, `/mcp`), or a host per module (`id.city.example`), or both.
//! - Refuses overlapping mounts at start.
//! - Strips the path prefix before handing the request to the module router, and injects [`Mount`]
//!   into the request extensions so every absolute URL, redirect, issuer, cookie path, CSP and
//!   CORS origin honours the mount.
//! - Trusts `X-Forwarded-Proto`, `X-Forwarded-Host` and `X-Forwarded-For` only when the peer IP is
//!   in the configured trusted proxy list, and injects the resolved [`ClientAddr`].
//! - Answers 404 for unmounted paths.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;
use tower::ServiceExt;

/// The client's address as the router resolved it: the TCP peer, or, when the peer is a trusted
/// proxy, the right-most `X-Forwarded-For` hop that is not itself a trusted proxy. Modules key
/// per-address rate limits on it; it is absent when the peer is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddr(pub IpAddr);

/// Resolved mount context passed to a module router via request extensions and at router build time (W9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    pub module: String,
    pub scheme: String,
    pub host: String,
    /// Normalized prefix: `""` for root mount, or `"/id"` (leading slash, no trailing slash).
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

    /// Public base URL including prefix (`https://city.example/id` or `https://id.city.example`).
    #[must_use]
    pub fn public_base(&self) -> String {
        format!("{}://{}{}", self.scheme, self.host, self.path_prefix)
    }

    /// Realm issuer string: `{public base}{prefix}/realms/{realm}` (W1, W9).
    #[must_use]
    pub fn realm_issuer(&self, realm: &str) -> String {
        format!("{}/realms/{realm}", self.public_base())
    }

    /// Cookie path for a realm: `{prefix}/realms/{realm}` (W9).
    #[must_use]
    pub fn realm_cookie_path(&self, realm: &str) -> String {
        format!("{}/realms/{realm}", self.path_prefix)
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
}

pub struct JcRouterBuilder {
    specs: Vec<(MountSpec, Router)>,
    default_scheme: String,
    default_host: String,
    trusted_proxies: Vec<IpNet>,
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
        }
    }

    #[must_use]
    pub fn trust_proxy(mut self, net: IpNet) -> Self {
        self.trusted_proxies.push(net);
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
        // Sort most-specific host and longest path prefix first so `/id/sub` matches before `/id` or `/`
        entries.sort_by(|a, b| {
            b.host
                .is_some()
                .cmp(&a.host.is_some())
                .then_with(|| b.prefix.len().cmp(&a.prefix.len()))
        });
        Ok(JcRouter {
            entries: Arc::new(entries),
            default_scheme: self.default_scheme,
            default_host: self.default_host,
            trusted_proxies: Arc::new(self.trusted_proxies),
        })
    }
}

impl JcRouter {
    fn is_trusted_proxy(&self, peer: Option<IpAddr>) -> bool {
        let Some(ip) = peer else {
            return false;
        };
        self.trusted_proxies.iter().any(|net| net.contains(&ip))
    }

    fn resolve_client_addr(&self, req: &Request<Body>, peer: Option<IpAddr>) -> Option<IpAddr> {
        let peer = peer?;
        if !self.is_trusted_proxy(Some(peer)) {
            return Some(peer);
        }
        let hops: Vec<IpAddr> = req
            .headers()
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .filter_map(|h| h.trim().parse::<IpAddr>().ok())
            .collect();
        hops.iter()
            .rev()
            .find(|ip| !self.is_trusted_proxy(Some(**ip)))
            .or_else(|| hops.first())
            .copied()
            .or(Some(peer))
    }

    fn resolve_scheme_and_host(
        &self,
        req: &Request<Body>,
        peer: Option<IpAddr>,
    ) -> (String, String) {
        let trust_fwd = self.is_trusted_proxy(peer);
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
            })
            .or_else(|| req.uri().host())
            .unwrap_or(&self.default_host)
            .to_ascii_lowercase();

        (scheme, host)
    }

    pub async fn dispatch(&self, mut req: Request<Body>, peer: Option<IpAddr>) -> Response<Body> {
        let peer = peer.or_else(|| {
            req.extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|c| c.0.ip())
        });
        let client = self.resolve_client_addr(&req, peer);
        let (scheme, host_with_port) = self.resolve_scheme_and_host(&req, peer);
        let host_only = host_with_port
            .split_once(':')
            .map_or(host_with_port.as_str(), |(h, _)| h);
        let path = req.uri().path().to_owned();

        let matched = self.entries.iter().find(|entry| {
            let host_ok = match &entry.host {
                Some(h) => h == &host_with_port || h == host_only,
                None => true,
            };
            if !host_ok {
                return false;
            }
            if entry.prefix.is_empty() {
                true
            } else {
                path == entry.prefix || path.starts_with(&format!("{}/", entry.prefix))
            }
        });

        let Some(entry) = matched else {
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

        let mount = Mount {
            module: entry.module.clone(),
            scheme,
            host: host_with_port,
            path_prefix: entry.prefix.clone(),
        };
        req.extensions_mut().insert(mount);
        match client {
            Some(ip) => {
                req.extensions_mut().insert(ClientAddr(ip));
            }
            None => {
                req.extensions_mut().remove::<ClientAddr>();
            }
        }

        entry
            .router
            .clone()
            .oneshot(req)
            .await
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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

    #[tokio::test]
    async fn routes_one_domain_with_slugs_and_host_per_module_and_refuses_overlap() {
        // 1. Overlapping mount refusal
        let overlap = JcRouterBuilder::new("https", "city.example")
            .mount(
                MountSpec {
                    module: "kaisatsu".into(),
                    host: Some("city.example".into()),
                    path_prefix: Some("/id".into()),
                },
                Router::new(),
            )
            .mount(
                MountSpec {
                    module: "other".into(),
                    host: Some("city.example".into()),
                    path_prefix: Some("/id/".into()),
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
                    path_prefix: Some("/id".into()),
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

        // Request under `/id` on `city.example`
        let req1 = Request::builder()
            .uri("/id/realms/dev")
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
            "module=kaisatsu issuer=https://city.example/id/realms/dev cookie=__Secure-wk_sso path=/realms/dev"
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
        assert_eq!(m.realm_cookie_path("dev"), "/realms/dev");
        assert_eq!(m.cookie_name("sso"), "__Host-wk_sso");
        let m = Mount::new("kaisatsu", "https", "city.example", "/id/").unwrap();
        assert_eq!(m.realm_cookie_path("dev"), "/id/realms/dev");
        assert_eq!(m.realm_issuer("dev"), "https://city.example/id/realms/dev");
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
}
