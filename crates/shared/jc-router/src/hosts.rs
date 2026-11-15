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

/// What one origin of an Organization serves (evidence/T-6991/design.md §1). The router
/// enforces each role's refusals before any module runs, so a module added later cannot forget
/// them, and names the role to the module in [`HOST_ROLE_HEADER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRole {
    /// Portal, assistant, dashboards: the one origin of the session cookie.
    Working,
    /// Open data for anonymous visitors: reads only, no cookie and no credential either way.
    OpenData,
    /// Endpoint data API and MCP: bearer tokens only, no cookie either way.
    Api,
    /// Sign-in: kaisatsu alone.
    SignIn,
    /// Apps and embeds: code and content written by users.
    Apps,
}

impl HostRole {
    /// The value of [`HOST_ROLE_HEADER`].
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::OpenData => "opendata",
            Self::Api => "api",
            Self::SignIn => "signin",
            Self::Apps => "apps",
        }
    }

    /// The methods the role answers, `None` for every method.
    pub(crate) fn allowed_methods(self) -> Option<&'static str> {
        (self == Self::OpenData).then_some("GET, HEAD, OPTIONS")
    }

    /// The request headers removed before any module runs.
    pub(crate) fn removed_request_headers(self) -> &'static [&'static str] {
        match self {
            Self::OpenData => &["cookie", "authorization"],
            Self::Api => &["cookie"],
            _ => &[],
        }
    }

    /// Whether `Set-Cookie` is removed from every answer.
    pub(crate) fn drops_set_cookie(self) -> bool {
        matches!(self, Self::OpenData | Self::Api)
    }
}

/// The header naming a request's [`HostRole`] to the module (the gateway's open data `404`).
/// The router removes it from every incoming request and sets it only from the table.
pub const HOST_ROLE_HEADER: &str = "x-jc-host-role";

/// Pages under `prefix` of one host belong on another origin: `308` to `to` (a configured host).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageMove {
    pub(crate) prefix: String,
    pub(crate) to: String,
    /// Only a page no mount of the host answers moves (the API host: its API calls stay).
    pub(crate) unmounted_only: bool,
}

/// The deployment's hosts and Organizations (docs/API/15-addresses-v1.md §2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostTable {
    rows: BTreeMap<String, HostRow>,
    organizations: BTreeSet<String>,
    reserved: BTreeSet<String>,
    roles: BTreeMap<String, HostRole>,
    moves: BTreeMap<String, Vec<PageMove>>,
}

/// The value of a host keyed map: exact first, then a wildcard one label under it.
fn lookup<'a, V>(
    map: &'a BTreeMap<String, V>,
    host_with_port: &str,
    host_only: &str,
) -> Option<&'a V> {
    map.get(host_only)
        .or_else(|| map.get(host_with_port))
        .or_else(|| {
            map.iter().find_map(|(h, v)| {
                let suffix = wildcard_suffix(h)?;
                (one_label_under(host_with_port, suffix) || one_label_under(host_only, suffix))
                    .then_some(v)
            })
        })
}

/// Whether `host` is an authority a URL can be built from: a lowercase name or IPv4 address, or
/// a bracketed IPv6 literal, then at most a numeric `:port` (RFC 9110 7.2). No userinfo, path or
/// second colon: a Host header becomes the base of issuers and links (RTE-02).
pub(crate) fn well_formed(host: &str) -> bool {
    let (name, port) = match host.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((literal, port))
                if !literal.is_empty()
                    && literal
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.') =>
            {
                ("v6", port)
            }
            _ => return false,
        },
        None => host.split_at(host.find(':').unwrap_or(host.len())),
    };
    let port_ok = match port.strip_prefix(':') {
        Some(digits) => {
            (1..=5).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit())
        }
        None => port.is_empty(),
    };
    port_ok
        && !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_')
        })
}

/// `host` without a `:port` suffix; an IPv6 literal keeps its brackets.
pub(crate) fn without_port(host: &str) -> &str {
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

    /// The names no Organization may take (jc passes jc-core's `RESERVED_LABELS`, T-6974):
    /// set before the Organizations, which [`HostTable::organization`] then refuses.
    #[must_use]
    pub fn reserving(mut self, names: &[&str]) -> Self {
        self.reserved.extend(names.iter().map(|n| (*n).to_owned()));
        self
    }

    /// An Organization of the deployment, selectable by path on every shared host.
    pub fn organization(mut self, name: &str) -> Result<Self, RouterError> {
        if !valid_organization(name) {
            return Err(RouterError::InvalidHostRow(format!(
                "'{name}' is not an Organization id"
            )));
        }
        if self.reserved.contains(name) {
            return Err(RouterError::InvalidHostRow(format!(
                "Organization '{name}' is reserved for the platform"
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

    /// Gives `host` (a row's host, wildcard allowed) a [`HostRole`]; one role per host.
    pub fn role(mut self, host: &str, role: HostRole) -> Result<Self, RouterError> {
        let host = without_port(host.trim()).to_ascii_lowercase();
        match self.roles.insert(host.clone(), role) {
            Some(other) if other != role => Err(RouterError::InvalidHostRow(format!(
                "host '{host}' has two roles"
            ))),
            _ => Ok(self),
        }
    }

    /// Pages asked on `host` under `prefix` answer `308` to the same path below it on `to`;
    /// with `unmounted_only`, only those no mount of `host` answers.
    pub fn move_pages(
        mut self,
        host: &str,
        prefix: &str,
        to: &str,
        unmounted_only: bool,
    ) -> Result<Self, RouterError> {
        let prefix = normalize_prefix(prefix)?;
        let to = to.trim().to_ascii_lowercase();
        if !well_formed(&to) {
            return Err(RouterError::InvalidHostRow(format!(
                "'{to}' is not a host a page can move to"
            )));
        }
        let host = without_port(host.trim()).to_ascii_lowercase();
        self.moves.entry(host).or_default().push(PageMove {
            prefix,
            to,
            unmounted_only,
        });
        Ok(self)
    }

    /// The role of a host: exact first, then a wildcard one label under it.
    pub(crate) fn role_of(&self, host_with_port: &str, host_only: &str) -> Option<HostRole> {
        lookup(&self.roles, host_with_port, host_only).copied()
    }

    /// Where a page at `path` on the host moves, with the path below the move's prefix.
    pub(crate) fn page_move(
        &self,
        host_with_port: &str,
        host_only: &str,
        path: &str,
        mounted: bool,
    ) -> Option<(&str, String)> {
        lookup(&self.moves, host_with_port, host_only)?
            .iter()
            .filter(|m| !(m.unmounted_only && mounted))
            .find_map(|m| {
                let rest = under(path, &m.prefix)?;
                Some((
                    m.to.as_str(),
                    if rest.is_empty() { "/" } else { rest }.to_owned(),
                ))
            })
    }

    /// The Organizations of the deployment.
    pub fn organizations(&self) -> impl Iterator<Item = &str> {
        self.organizations.iter().map(String::as_str)
    }

    /// The row of a host (`host_with_port`, `host_only` without its port): exact first, then a
    /// wildcard one label under it.
    fn row_of(&self, host_with_port: &str, host_only: &str) -> Option<&HostRow> {
        lookup(&self.rows, host_with_port, host_only)
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
                // `//x` or `/\x` as a Location is another site's address: such a page is served
                // in place instead of redirected.
                let canonical = under(&inner, root)
                    .filter(|c| page && !c.starts_with("//") && !c.starts_with("/\\"));
                match canonical {
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
        // A canonical path that a browser reads as another site is no redirect.
        for path in [
            "/com_example/data//evil.example/x",
            "/com_example//evil.example",
        ] {
            let (status, _) = call(&jc, "opendata.example.com", path, PAGE).await;
            assert_ne!(status, StatusCode::PERMANENT_REDIRECT, "{path}");
        }
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

    /// T-6974: a reserved name is refused as an Organization even where no module is mounted
    /// under it (an edge-only prefix such as `/grafana`, a host label).
    #[test]
    fn a_reserved_name_is_no_organization() {
        let table = HostTable::new().reserving(&["grafana", "login"]);
        for name in ["grafana", "login"] {
            let err = table.clone().organization(name).expect_err(name);
            assert!(
                err.to_string().contains("reserved for the platform"),
                "{err}"
            );
        }
        table.organization("bbsk").expect("an ordinary name");
    }

    /// What a module saw: its credentials, cookies and role header, and a cookie it tries to set.
    async fn witness(req: Request<Body>) -> axum::response::Response {
        let h = |n: &str| {
            req.headers()
                .get(n)
                .map_or("-", |v| v.to_str().unwrap_or("?"))
                .to_owned()
        };
        let seen = format!(
            "{} {} cookie={} auth={} role={}",
            req.method(),
            req.uri().path(),
            h("cookie"),
            h("authorization"),
            h(HOST_ROLE_HEADER)
        );
        axum::response::Response::builder()
            .header("set-cookie", "planted=1; Path=/")
            .body(Body::from(seen))
            .unwrap()
    }

    /// `bbsk` on five origins (T-6991 §1): Portal and the dataroom's `/data` on the working host,
    /// the dataroom on the open data host, the gateway on the API host, Apps under a wildcard.
    fn origins() -> JcRouter {
        let spec = |module: &str, host: &str, prefix: Option<&str>| MountSpec {
            module: module.into(),
            host: Some(host.into()),
            path_prefix: prefix.map(str::to_owned),
        };
        let table = HostTable::new()
            .bound("smartcity.bbsk.sk", "bbsk", "/")
            .and_then(|t| t.bound("opendata.bbsk.sk", "bbsk", "/"))
            .and_then(|t| t.bound("api.bbsk.sk", "bbsk", "/"))
            .and_then(|t| t.shared("*.apps.bbsk.sk"))
            .and_then(|t| t.role("smartcity.bbsk.sk", HostRole::Working))
            .and_then(|t| t.role("opendata.bbsk.sk", HostRole::OpenData))
            .and_then(|t| t.role("api.bbsk.sk", HostRole::Api))
            .and_then(|t| t.role("*.apps.bbsk.sk", HostRole::Apps))
            .and_then(|t| t.move_pages("smartcity.bbsk.sk", "/data", "opendata.bbsk.sk", false))
            .and_then(|t| t.move_pages("api.bbsk.sk", "/", "smartcity.bbsk.sk", true))
            .expect("rows");
        let mut b = JcRouterBuilder::new("https", "smartcity.bbsk.sk").hosts(table);
        for (module, host, prefix) in [
            ("portal", "smartcity.bbsk.sk", None),
            ("dataroom", "smartcity.bbsk.sk", Some("/data")),
            ("dataroom", "opendata.bbsk.sk", None),
            ("gateway", "api.bbsk.sk", Some("/api/endpoint")),
            ("portal", "*.apps.bbsk.sk", None),
        ] {
            b = b.mount(
                spec(module, host, prefix),
                Router::new().fallback(any(witness)),
            );
        }
        b.build().expect("router")
    }

    async fn ask(
        jc: &JcRouter,
        method: &str,
        host: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", host);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        jc.dispatch(req.body(Body::empty()).unwrap(), None).await
    }

    async fn text(resp: axum::response::Response) -> String {
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn the_open_data_host_answers_reads_only() {
        let jc = origins();
        for method in ["POST", "PUT", "PATCH", "DELETE"] {
            let resp = ask(
                &jc,
                method,
                "opendata.bbsk.sk",
                "/api/3/action/package_create",
                &[],
            )
            .await;
            assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED, "{method}");
            assert_eq!(resp.headers()["allow"], "GET, HEAD, OPTIONS", "{method}");
        }
        for method in ["GET", "HEAD", "OPTIONS"] {
            let resp = ask(&jc, method, "opendata.bbsk.sk", "/dataset/x", &[]).await;
            assert_eq!(resp.status(), StatusCode::OK, "{method}");
        }
        // The working host's own `/data` takes the Portal's writes into the catalogue.
        let resp = ask(
            &jc,
            "POST",
            "smartcity.bbsk.sk",
            "/data/api/3/action/x",
            &[],
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_cookie_or_credential_crosses_the_open_data_and_api_hosts() {
        let jc = origins();
        let sent = [
            ("cookie", "__Host-jc_session=s; jc_csrf=t"),
            ("authorization", "Bearer abc"),
            (HOST_ROLE_HEADER, "working"),
        ];
        let resp = ask(&jc, "GET", "opendata.bbsk.sk", "/dataset/x", &sent).await;
        assert!(resp.headers().get("set-cookie").is_none());
        assert_eq!(
            text(resp).await,
            "GET /dataset/x cookie=- auth=- role=opendata"
        );
        let resp = ask(&jc, "GET", "api.bbsk.sk", "/api/endpoint/x", &sent).await;
        assert!(resp.headers().get("set-cookie").is_none());
        assert_eq!(text(resp).await, "GET /x cookie=- auth=Bearer abc role=api");
        // The working host keeps its cookies; a role a client names is replaced by the table's.
        let resp = ask(&jc, "GET", "smartcity.bbsk.sk", "/projects", &sent).await;
        assert!(resp.headers().get("set-cookie").is_some());
        assert_eq!(
            text(resp).await,
            "GET /projects cookie=__Host-jc_session=s; jc_csrf=t auth=Bearer abc role=working"
        );
        let resp = ask(
            &jc,
            "GET",
            "x.apps.bbsk.sk",
            "/",
            &[(HOST_ROLE_HEADER, "working")],
        )
        .await;
        assert_eq!(text(resp).await, "GET / cookie=- auth=- role=apps");
    }

    #[tokio::test]
    async fn a_host_role_header_from_a_client_never_reaches_a_module() {
        // Without a host table, too: no role is ever set there.
        let mut b = JcRouterBuilder::new("https", "city.example");
        b = b.mount(
            MountSpec {
                module: "portal".into(),
                host: None,
                path_prefix: None,
            },
            Router::new().fallback(any(witness)),
        );
        let jc = b.build().unwrap();
        let resp = ask(
            &jc,
            "GET",
            "city.example",
            "/",
            &[(HOST_ROLE_HEADER, "opendata")],
        )
        .await;
        assert_eq!(text(resp).await, "GET / cookie=- auth=- role=-");
    }

    #[tokio::test]
    async fn a_page_on_the_wrong_group_moves_to_a_configured_host_only() {
        let jc = origins();
        let page = ("accept", "text/html");
        let spoofed = [
            page,
            ("x-forwarded-host", "evil.example"),
            ("x-forwarded-proto", "https"),
        ];
        let cases = [
            (
                "smartcity.bbsk.sk",
                "/data/dataset/x?q=1",
                "https://opendata.bbsk.sk/dataset/x?q=1",
            ),
            ("smartcity.bbsk.sk", "/data", "https://opendata.bbsk.sk/"),
            (
                "smartcity.bbsk.sk",
                "/data//evil.example",
                "https://opendata.bbsk.sk//evil.example",
            ),
            (
                "api.bbsk.sk",
                "/projects/p",
                "https://smartcity.bbsk.sk/projects/p",
            ),
        ];
        for (host, path, to) in cases {
            for headers in [&[page][..], &spoofed[..]] {
                let resp = ask(&jc, "GET", host, path, headers).await;
                assert_eq!(
                    resp.status(),
                    StatusCode::PERMANENT_REDIRECT,
                    "{host}{path}"
                );
                let location = resp.headers()["location"].to_str().unwrap();
                assert_eq!(location, to, "{host}{path}");
                assert!(!location.starts_with("//"), "{location}");
            }
        }
        // An API call stays where it answers; a page the API host serves stays there.
        let resp = ask(&jc, "GET", "smartcity.bbsk.sk", "/data/api/3/action/x", &[]).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = ask(&jc, "GET", "api.bbsk.sk", "/api/endpoint/x", &[page]).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = ask(&jc, "GET", "api.bbsk.sk", "/projects/p", &[]).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_host_takes_one_role_and_moves_to_a_plain_host() {
        let t = HostTable::new().role("a.example", HostRole::Api).unwrap();
        assert!(t.clone().role("a.example", HostRole::Api).is_ok());
        assert!(t.role("A.example:443", HostRole::OpenData).is_err());
        for to in ["evil.example/x", "user@evil.example", "", "a b"] {
            assert!(
                HostTable::new()
                    .move_pages("a.example", "/", to, false)
                    .is_err(),
                "{to}"
            );
        }
    }
}
