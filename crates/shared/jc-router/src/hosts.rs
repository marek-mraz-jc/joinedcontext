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
    /// Organization id -> name, for the Organizations whose path segment is their id (EP-186).
    ids: BTreeMap<String, String>,
    reserved: BTreeSet<String>,
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
        if self.ids.contains_key(name) {
            return Err(RouterError::InvalidHostRow(format!(
                "'{name}' is another Organization's id"
            )));
        }
        self.organizations.insert(name.to_owned());
        Ok(self)
    }

    /// An Organization whose path segment is its id (`sk_bbsk`, EP-186), its `name`
    /// (`metadata.name`, no `_`) what the router hands the modules. On a shared host `/{id}/...`
    /// is the Organization; the old `/{name}/...` still answers: an API call in place, a page
    /// `308` to `/{id}/...`. A name with `_`, an id without one, or a name or id another
    /// Organization holds is refused, so no segment can name two Organizations.
    pub fn organization_id(self, name: &str, id: &str) -> Result<Self, RouterError> {
        if name.contains('_') || !id_shaped(id) {
            return Err(RouterError::InvalidHostRow(format!(
                "Organization '{name}' with id '{id}': the name holds no `_`, the id holds one"
            )));
        }
        let taken = |s: &str| self.organizations.contains(s) || self.ids.contains_key(s);
        if self.ids.values().any(|n| n == name) || taken(id) || self.reserved.contains(id) {
            return Err(RouterError::InvalidHostRow(format!(
                "Organization '{name}' with id '{id}': the name or the id is taken"
            )));
        }
        let mut this = self.organization(name)?;
        this.ids.insert(id.to_owned(), name.to_owned());
        Ok(this)
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

    /// The Organization a first path segment names, as `(name, its id when the segment is its
    /// old name)`: its id, or its name.
    fn named(&self, segment: &str) -> Option<(&str, Option<&str>)> {
        if let Some(name) = self.ids.get(segment) {
            return Some((name, None));
        }
        self.organizations.get(segment).map(|name| {
            let id = self
                .ids
                .iter()
                .find(|(_, n)| *n == name)
                .map(|(id, _)| id.as_str());
            (name.as_str(), id)
        })
    }

    /// The path segment of Organization `name`: its id, else the name.
    fn segment(&self, name: &str) -> String {
        self.ids
            .iter()
            .find(|(_, n)| *n == name)
            .map_or_else(|| name.to_owned(), |(id, _)| id.clone())
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
        let named = self.named(first);
        let known = named.is_some();
        match row {
            // The old name of an Organization with an id: a page moves to the id (`rest` keeps
            // the raw path, so the Location stays below `/{id}/`), an API call answers in place.
            HostRow::Shared if page && named.is_some_and(|(_, id)| id.is_some()) => {
                let id = named.and_then(|(_, id)| id).unwrap_or_default();
                Placed::Canonical(format!("/{id}{rest}"))
            }
            HostRow::Shared if known => Placed::Route {
                path: if rest.is_empty() { "/".into() } else { rest },
                organization: named.map(|(name, _)| name.to_owned()),
                via: Via::Prefixed(first.to_owned()),
            },
            HostRow::Shared if id_shaped(first) => Placed::NotFound,
            HostRow::Shared => Placed::Route {
                path: path.to_owned(),
                organization: None,
                via: Via::Plain,
            },
            HostRow::Bound { organization, root }
                if named.is_some_and(|(name, _)| name == organization) =>
            {
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
                        via: Via::Prefixed(first.to_owned()),
                    },
                }
            }
            HostRow::Bound { .. } if known || id_shaped(first) => Placed::NotFound,
            HostRow::Bound { organization, root } => Placed::Route {
                path: format!("{root}{path}"),
                organization: Some(organization.clone()),
                via: Via::Bound {
                    organization: self.segment(organization),
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

    /// T-7007 (EP-186): Organizations whose segment is their id, one also on a bound host.
    fn jc_ids() -> JcRouter {
        let table = HostTable::new()
            .shared("city.example")
            .and_then(|t| t.organization_id("bbsk", "sk_bbsk"))
            .and_then(|t| t.organization_id("hel", "fi_hel"))
            .and_then(|t| t.bound("data.bbsk.sk", "bbsk", "/"))
            .expect("rows");
        let mut b = JcRouterBuilder::new("https", "city.example").hosts(table);
        for name in ["data", "api"] {
            b = b.mount(
                MountSpec {
                    module: name.into(),
                    host: None,
                    path_prefix: Some(format!("/{name}")),
                },
                Router::new().fallback(any(echo)),
            );
        }
        b.build().expect("router")
    }

    #[tokio::test]
    async fn the_id_is_the_segment_and_the_old_name_moves_to_it() {
        let jc = jc_ids();
        // Modules learn the Organization's name; its public addresses carry the id.
        assert_eq!(
            call(&jc, "city.example", "/sk_bbsk/data/dataset/x", API).await,
            (
                StatusCode::OK,
                "data org=bbsk base=https://city.example/sk_bbsk/data path=/dataset/x".into()
            )
        );
        // A page under the old name moves to the id with its path and query; the Location is
        // host-relative, so a forwarded host from an untrusted peer cannot reach it.
        let req = Request::builder()
            .uri("/bbsk/data/x?y=1")
            .header("host", "city.example")
            .header("x-forwarded-host", "evil.example")
            .header("accept", PAGE)
            .body(Body::empty())
            .unwrap();
        let resp = jc.dispatch(req, None).await;
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(resp.headers()["location"], "/sk_bbsk/data/x?y=1");
        for (path, location) in [
            ("/bbsk", "/sk_bbsk"),
            ("/bbsk//evil.example/x", "/sk_bbsk//evil.example/x"),
            ("/bbsk/%2f%2fevil.example", "/sk_bbsk/%2f%2fevil.example"),
        ] {
            assert_eq!(
                call(&jc, "city.example", path, PAGE).await,
                (StatusCode::PERMANENT_REDIRECT, location.into()),
                "{path}"
            );
        }
        // An API call under the old name answers in place, during the move.
        assert_eq!(
            call(&jc, "city.example", "/bbsk/data/x", API).await,
            (
                StatusCode::OK,
                "data org=bbsk base=https://city.example/bbsk/data path=/x".into()
            )
        );
    }

    #[tokio::test]
    async fn an_unknown_id_is_the_404_of_a_hidden_organization() {
        let jc = jc_ids();
        let unknown = call(&jc, "city.example", "/sk_other/data/x", API).await;
        assert_eq!(unknown.0, StatusCode::NOT_FOUND);
        // On bbsk's own host, hel's id and name are as absent as an id nobody holds.
        for path in ["/fi_hel/data/x", "/hel/data/x", "/sk_other/data/x"] {
            assert_eq!(
                call(&jc, "data.bbsk.sk", path, API).await,
                unknown,
                "{path}"
            );
        }
        // The bound host takes its own id (and name) in the prefixed form.
        assert_eq!(
            call(&jc, "data.bbsk.sk", "/sk_bbsk/data/x", PAGE).await,
            (StatusCode::PERMANENT_REDIRECT, "/data/x".into())
        );
        assert_eq!(
            call(&jc, "data.bbsk.sk", "/bbsk/data/x", API).await,
            (
                StatusCode::OK,
                "data org=bbsk base=https://data.bbsk.sk/bbsk/data path=/x".into()
            )
        );
        assert_eq!(
            call(&jc, "data.bbsk.sk", "/data/x", API).await,
            (
                StatusCode::OK,
                "data org=bbsk base=https://data.bbsk.sk/data path=/x".into()
            )
        );
    }

    #[test]
    fn a_name_and_an_id_never_name_two_organizations() {
        let base = || HostTable::new().organization_id("bbsk", "sk_bbsk").unwrap();
        for (name, id) in [
            ("sk_x", "sk_y"),     // a name with `_`
            ("x", "x-y"),         // an id without one
            ("other", "sk_bbsk"), // the id taken
            ("bbsk", "sk_other"), // the name taken
        ] {
            assert!(base().organization_id(name, id).is_err(), "{name} {id}");
        }
        assert!(base().organization("sk_bbsk").is_err());
        assert!(
            HostTable::new()
                .organization("sk_bbsk")
                .unwrap()
                .organization_id("x", "sk_bbsk")
                .is_err()
        );
        assert!(
            HostTable::new()
                .reserving(&["sk_admin"])
                .organization_id("x", "sk_admin")
                .is_err()
        );
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
}
