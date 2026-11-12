//! The host registry of docs/API/15-addresses-v1.md §2: every host a deployment answers on is
//! *shared* (the first path segment selects the Organization) or *bound* to one Organization,
//! whose root may map to a path prefix of it. The router decides a request's Organization here,
//! once, and hands it to the module as [`Organization`]; no module reads `Host` to find it.
//!
//! - A host not in the registry answers `421`.
//! - An unknown Organization id, and a known one on a host bound to another, answer the `404`
//!   of an unknown path.
//! - On a bound host the prefix-less path is canonical; the prefixed form (`/{org}/...`) still
//!   answers: an API call in place, a page `308` to the prefix-less form.

use crate::{RouterError, normalize_prefix, one_label_under, wildcard_suffix};
use std::collections::{BTreeMap, BTreeSet};

/// The Organization a request belongs to, as the router decided it from the registry: the
/// first path segment on a shared host, or the row of a bound host. Never a header, URN or token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Organization(pub String);

/// One host of the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostRow {
    /// The first path segment selects the Organization; a path that names none is the
    /// deployment's own (sign-in chooser, assets, discovery, one-Organization paths).
    Shared,
    /// Every request belongs to `organization`; the host root maps to `root` below it (`""` for
    /// `/{org}/`, `/data` for `/{org}/data/`).
    Bound { organization: String, root: String },
}

/// How a request's path was placed: what it routes to and where its public addresses live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placed {
    /// Route `path` (the Organization's segment removed) for `organization`. `via` says how a
    /// mount prefix reads publicly.
    Route {
        path: String,
        organization: Option<String>,
        via: Via,
    },
    /// A page asked in the prefixed form of a bound host: its canonical path.
    Canonical(String),
    /// The host is not in the registry (`421`).
    Misdirected,
    /// An unknown Organization, or another one than the host's (`404`).
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Via {
    /// No Organization: the mount prefix as it is.
    Plain,
    /// `/{org}` in front of the mount prefix (shared host, or a bound host's prefixed form).
    Prefixed(String),
    /// A bound host: the mount prefix with its root removed, else the prefixed form.
    Bound { organization: String, root: String },
}

impl Via {
    /// The public prefix of a module mounted at `prefix` (`""` or `/x`) for this request.
    pub(crate) fn public_prefix(&self, prefix: &str) -> String {
        match self {
            Self::Plain => prefix.to_owned(),
            Self::Prefixed(org) => format!("/{org}{prefix}"),
            Self::Bound { organization, root } => match under(prefix, root) {
                Some(rest) => rest.to_owned(),
                None => format!("/{organization}{prefix}"),
            },
        }
    }
}

/// `path` without `root` when it is `root` or continues it with a whole segment.
fn under<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    path.strip_prefix(root)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The deployment's hosts and Organizations (docs/API/15-addresses-v1.md §2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostTable {
    rows: BTreeMap<String, HostRow>,
    organizations: BTreeSet<String>,
}

/// `host` without a `:port` suffix; an IPv6 literal keeps its brackets.
fn without_port(host: &str) -> &str {
    host.rsplit_once(':')
        .filter(|(name, port)| {
            !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && !name.ends_with(':')
        })
        .map_or(host, |(name, _)| name)
}

/// Whether `segment` has the shape of an Organization id (`[a-z][a-z0-9_-]{1,62}` with a `_`):
/// no route segment has one, so an unknown one is an unknown Organization, not a path.
fn id_shaped(segment: &str) -> bool {
    let b = segment.as_bytes();
    (2..=63).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.contains(&b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
}

/// A plain Organization name or id: lowercase letters, digits, `-` and `_`, 1-63 long.
fn valid_organization(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=63).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
}

impl HostTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An Organization of the deployment, selectable by path on every shared host.
    pub fn organization(mut self, name: &str) -> Result<Self, RouterError> {
        if !valid_organization(name) {
            return Err(RouterError::InvalidHostRow(format!(
                "'{name}' is not an Organization id"
            )));
        }
        self.organizations.insert(name.to_owned());
        Ok(self)
    }

    /// A shared host (`city.example`, `localhost:42301`, `*.apps.city.example`).
    pub fn shared(self, host: &str) -> Result<Self, RouterError> {
        self.row(host, HostRow::Shared)
    }

    /// A host bound to `organization`, its root mapped to `root` below it (`/` or `/data`).
    pub fn bound(self, host: &str, organization: &str, root: &str) -> Result<Self, RouterError> {
        let root = normalize_prefix(root)?;
        let this = self.organization(organization)?;
        this.row(
            host,
            HostRow::Bound {
                organization: organization.to_owned(),
                root,
            },
        )
    }

    fn row(mut self, host: &str, row: HostRow) -> Result<Self, RouterError> {
        let host = without_port(host.trim()).to_ascii_lowercase();
        if host.is_empty() || host.contains('/') || host.contains('_') {
            return Err(RouterError::InvalidHostRow(format!(
                "'{host}' is not a host name"
            )));
        }
        match self.rows.get(&host) {
            Some(existing) if *existing != row => Err(RouterError::InvalidHostRow(format!(
                "host '{host}' has two rows"
            ))),
            _ => {
                self.rows.insert(host, row);
                Ok(self)
            }
        }
    }

    /// The Organizations of the deployment.
    pub fn organizations(&self) -> impl Iterator<Item = &str> {
        self.organizations.iter().map(String::as_str)
    }

    /// The row of a host (`host_with_port`, `host_only` without its port): exact first, then a
    /// wildcard one label under it.
    fn row_of(&self, host_with_port: &str, host_only: &str) -> Option<&HostRow> {
        self.rows
            .get(host_only)
            .or_else(|| self.rows.get(host_with_port))
            .or_else(|| {
                self.rows.iter().find_map(|(h, row)| {
                    let suffix = wildcard_suffix(h)?;
                    (one_label_under(host_with_port, suffix) || one_label_under(host_only, suffix))
                        .then_some(row)
                })
            })
    }

    /// Places a request for `path` on a host; `page` says a browser asked for a page.
    pub(crate) fn place(
        &self,
        host_with_port: &str,
        host_only: &str,
        path: &str,
        page: bool,
    ) -> Placed {
        let Some(row) = self.row_of(host_with_port, host_only) else {
            return Placed::Misdirected;
        };
        let (first, rest) = match path.strip_prefix('/').unwrap_or(path).split_once('/') {
            Some((first, rest)) => (first, format!("/{rest}")),
            None => (path.trim_start_matches('/'), String::new()),
        };
        let known = self.organizations.contains(first);
        match row {
            HostRow::Shared if known => Placed::Route {
                path: if rest.is_empty() { "/".into() } else { rest },
                organization: Some(first.to_owned()),
                via: Via::Prefixed(first.to_owned()),
            },
            HostRow::Shared if id_shaped(first) => Placed::NotFound,
            HostRow::Shared => Placed::Route {
                path: path.to_owned(),
                organization: None,
                via: Via::Plain,
            },
            HostRow::Bound { organization, root } if known && first == organization => {
                let inner = if rest.is_empty() {
                    "/".to_owned()
                } else {
                    rest
                };
                match under(&inner, root).filter(|_| page) {
                    Some(canonical) => Placed::Canonical(if canonical.is_empty() {
                        "/".into()
                    } else {
                        canonical.to_owned()
                    }),
                    None => Placed::Route {
                        path: inner,
                        organization: Some(organization.clone()),
                        via: Via::Prefixed(organization.clone()),
                    },
                }
            }
            HostRow::Bound { .. } if known || id_shaped(first) => Placed::NotFound,
            HostRow::Bound { organization, root } => Placed::Route {
                path: format!("{root}{path}"),
                organization: Some(organization.clone()),
                via: Via::Bound {
                    organization: organization.clone(),
                    root: root.clone(),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JcRouter, JcRouterBuilder, Mount, MountSpec};
    use axum::Router;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::{Request, StatusCode, Uri};
    use axum::routing::any;
    use http_body_util::BodyExt;

    async fn echo(
        Extension(mount): Extension<Mount>,
        org: Option<Extension<Organization>>,
        uri: Uri,
    ) -> String {
        let org = org.map_or_else(|| "-".to_owned(), |o| o.0.0);
        format!(
            "{} org={org} base={} path={}",
            mount.module,
            mount.public_base(),
            uri.path()
        )
    }

    /// The deployment of the contract's examples: a shared host, `data.bratislava.sk` bound at
    /// the Organization's root, `opendata.example.com` bound at its catalogue.
    fn jc() -> JcRouter {
        let module = |name: &str| {
            (
                MountSpec {
                    module: name.into(),
                    host: None,
                    path_prefix: Some(format!("/{name}")),
                },
                Router::new().fallback(any(echo)),
            )
        };
        let table = HostTable::new()
            .shared("city.example")
            .and_then(|t| t.shared("*.apps.city.example"))
            .and_then(|t| t.organization("com_example"))
            .and_then(|t| t.bound("data.bratislava.sk", "sk_bratislava", "/"))
            .and_then(|t| t.bound("opendata.example.com", "com_example", "/data"))
            .expect("rows");
        let mut b = JcRouterBuilder::new("https", "city.example").hosts(table);
        for name in ["data", "api"] {
            let (spec, router) = module(name);
            b = b.mount(spec, router);
        }
        b.build().expect("router")
    }

    async fn call(jc: &JcRouter, host: &str, path: &str, accept: &str) -> (StatusCode, String) {
        let req = Request::builder()
            .uri(path)
            .header("host", host)
            .header("accept", accept)
            .body(Body::empty())
            .unwrap();
        let resp = jc.dispatch(req, None).await;
        let status = resp.status();
        let location = resp
            .headers()
            .get("location")
            .map(|l| l.to_str().unwrap().to_owned());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        (status, location.unwrap_or(body))
    }

    const PAGE: &str = "text/html,application/xhtml+xml";
    const API: &str = "application/json";

    #[tokio::test]
    async fn a_shared_host_takes_the_organization_from_the_first_segment() {
        let jc = jc();
        assert_eq!(
            call(&jc, "city.example", "/com_example/data/dataset/x", API).await,
            (
                StatusCode::OK,
                "data org=com_example base=https://city.example/com_example/data path=/dataset/x"
                    .into()
            )
        );
        // A path that names no Organization is the deployment's own.
        assert_eq!(
            call(&jc, "city.example", "/data/dataset/x", API).await,
            (
                StatusCode::OK,
                "data org=- base=https://city.example/data path=/dataset/x".into()
            )
        );
        // A wildcard row covers one label under it.
        assert_eq!(
            call(&jc, "a.apps.city.example", "/api/x", API).await.0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_bound_host_at_the_organization_root_is_that_organization() {
        let jc = jc();
        assert_eq!(
            call(&jc, "data.bratislava.sk", "/data/dataset/x", API).await,
            (
                StatusCode::OK,
                "data org=sk_bratislava base=https://data.bratislava.sk/data path=/dataset/x"
                    .into()
            )
        );
    }

    #[tokio::test]
    async fn a_bound_host_with_a_root_prefix_repeats_neither_organization_nor_role() {
        let jc = jc();
        assert_eq!(
            call(&jc, "opendata.example.com", "/dataset/x", API).await,
            (
                StatusCode::OK,
                "data org=com_example base=https://opendata.example.com path=/dataset/x".into()
            )
        );
        // The Organization's other addresses answer in their prefixed form on that host.
        assert_eq!(
            call(&jc, "opendata.example.com", "/com_example/api/v1/x", API).await,
            (
                StatusCode::OK,
                "api org=com_example base=https://opendata.example.com/com_example/api path=/v1/x"
                    .into()
            )
        );
    }

    #[tokio::test]
    async fn a_host_outside_the_registry_is_421_and_names_nothing() {
        let jc = jc();
        let (status, body) = call(&jc, "evil.example", "/com_example/data/", API).await;
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
        assert!(
            !body.contains("com_example") && !body.contains("bratislava"),
            "{body}"
        );
        // A port does not make a registered host another one; a deeper label is not a wildcard's.
        assert_eq!(
            call(&jc, "city.example:8443", "/data/", API).await.0,
            StatusCode::OK
        );
        assert_eq!(
            call(&jc, "a.b.apps.city.example", "/api/", API).await.0,
            StatusCode::MISDIRECTED_REQUEST
        );
    }

    #[tokio::test]
    async fn a_foreign_or_unknown_organization_is_the_404_of_an_unknown_path() {
        let jc = jc();
        let unknown_path = call(&jc, "city.example", "/nothing/here", API).await;
        assert_eq!(unknown_path.0, StatusCode::NOT_FOUND);
        for (host, path) in [
            ("opendata.example.com", "/sk_bratislava/data/dataset/x"),
            ("data.bratislava.sk", "/com_example/data/dataset/x"),
            ("city.example", "/sk_nowhere/data/dataset/x"),
            ("data.bratislava.sk", "/sk_nowhere/data/"),
        ] {
            assert_eq!(
                call(&jc, host, path, API).await,
                unknown_path,
                "{host}{path}"
            );
            assert_eq!(
                call(&jc, host, path, PAGE).await,
                unknown_path,
                "{host}{path}"
            );
        }
    }

    #[tokio::test]
    async fn the_prefixed_form_on_a_bound_host_is_308_for_a_page_and_in_place_for_the_api() {
        let jc = jc();
        assert_eq!(
            call(
                &jc,
                "opendata.example.com",
                "/com_example/data/dataset/x?lang=sk",
                PAGE
            )
            .await,
            (StatusCode::PERMANENT_REDIRECT, "/dataset/x?lang=sk".into())
        );
        assert_eq!(
            call(
                &jc,
                "data.bratislava.sk",
                "/sk_bratislava/data/dataset/x",
                PAGE
            )
            .await,
            (StatusCode::PERMANENT_REDIRECT, "/data/dataset/x".into())
        );
        assert_eq!(
            call(&jc, "opendata.example.com", "/com_example/data/dataset/x", API).await,
            (
                StatusCode::OK,
                "data org=com_example base=https://opendata.example.com/com_example/data path=/dataset/x"
                    .into()
            )
        );
        // A write is never redirected, whatever it accepts.
        let post = Request::builder()
            .method("POST")
            .uri("/com_example/data/x")
            .header("host", "opendata.example.com")
            .header("accept", PAGE)
            .body(Body::empty())
            .unwrap();
        assert_eq!(jc.dispatch(post, None).await.status(), StatusCode::OK);
    }

    #[test]
    fn rows_and_organizations_are_checked() {
        assert!(HostTable::new().shared("bad/host").is_err());
        assert!(HostTable::new().shared("a_b.example").is_err());
        assert!(HostTable::new().bound("x.example", "Upper", "/").is_err());
        assert!(
            HostTable::new()
                .bound("x.example", "com_x", "/../a")
                .is_err()
        );
        let twice = HostTable::new()
            .shared("x.example")
            .and_then(|t| t.bound("X.example", "com_x", "/"));
        assert!(twice.is_err());
        // An Organization named like a mount's first segment would hide the module.
        let shadow = JcRouterBuilder::new("https", "city.example")
            .hosts(HostTable::new().organization("data").expect("org"))
            .mount(
                MountSpec {
                    module: "dataroom".into(),
                    host: None,
                    path_prefix: Some("/data".into()),
                },
                Router::new(),
            )
            .build();
        assert!(matches!(
            shadow,
            Err(crate::RouterError::OrganizationShadowsMount { .. })
        ));
    }
}
