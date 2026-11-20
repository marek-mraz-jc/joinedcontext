//! One Portal serving several Organizations, each on its own host (ADR-N-058, jc
//! `[[organizations]]`, `JC_PORTAL_ORGANIZATION_HOSTS`).
//!
//! A request on an Organization's Portal host is pinned to that Organization for its whole run:
//! [`middleware`] puts the pin in a task-local, so every check below it asks [`pinned`] without
//! a parameter. With the pin:
//!
//! - permissions reach that Organization's projects alone (`permissions::effective`), so another
//!   Organization's project reads as one that is not there, whoever asks;
//! - a person who belongs to no project of the Organization is refused every API call (403);
//! - the session and the CSRF token are sealed to the Organization, so a cookie minted on another
//!   host is no session here (`auth::session`, `auth::csrf`);
//! - redirect URIs and absolute links use the host's own base ([`public_base`]).
//!
//! A request on any other host (the deployment's own Portal address, the internal listener, a
//! background task) carries no pin and behaves as before.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use url::Url;

use crate::state::AppState;

/// One Organization's Portal address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrganizationHost {
    /// The `Organization` manifest's name, as `[[organizations]] name` gives it.
    pub organization: String,
    /// The Portal's public base on that host, e.g. `https://portal.city.example`.
    pub base: Url,
}

/// A set jc's Organization registry replaces while the Portal serves (T-7183, T-7185): every
/// clone holds the same set; a request reads the set current when it arrives.
#[derive(Clone, Debug, Default)]
pub struct Swappable<T>(std::sync::Arc<std::sync::RwLock<std::sync::Arc<T>>>);

impl<T> Swappable<T> {
    pub fn new(set: T) -> Self {
        Self(std::sync::Arc::new(std::sync::RwLock::new(
            std::sync::Arc::new(set),
        )))
    }

    /// The set held now.
    pub fn current(&self) -> std::sync::Arc<T> {
        match self.0.read() {
            Ok(held) => std::sync::Arc::clone(&held),
            Err(poisoned) => std::sync::Arc::clone(&poisoned.into_inner()),
        }
    }

    pub(crate) fn store(&self, next: T) {
        let next = std::sync::Arc::new(next);
        match self.0.write() {
            Ok(mut held) => *held = next,
            Err(poisoned) => *poisoned.into_inner() = next,
        }
    }
}

impl<T: PartialEq> PartialEq for Swappable<T> {
    fn eq(&self, other: &Self) -> bool {
        *self.current() == *other.current()
    }
}
impl<T: Eq> Eq for Swappable<T> {}

/// The Organizations' Portal hosts (T-7183): `JC_PORTAL_ORGANIZATION_HOSTS` at start, replaced
/// whenever jc's Organization registry runs another set
/// ([`AppState::replace_organization_hosts`]).
pub type OrganizationHosts = Swappable<Vec<OrganizationHost>>;

impl AppState {
    /// Makes `raw` (a `JC_PORTAL_ORGANIZATION_HOSTS` list, checked as at start) the
    /// Organizations' Portal hosts (T-7183). An Organization left out is pinned on no host from
    /// the next request on, and its own realm goes with it (a realm needs its Organization's
    /// host); one added is pinned on its host at once and its realm is taken by the next
    /// realm set. A list that fails the check changes nothing. Returns how many hosts are held.
    pub fn replace_organization_hosts(&self, raw: &str) -> Result<usize, String> {
        let next = parse(raw)?;
        let count = next.len();
        let kept: Vec<String> = next.iter().map(|h| h.organization.clone()).collect();
        self.organization_hosts.store(next);
        self.organization_realms.retain(&kept);
        Ok(count)
    }

    /// jc's run-time Organizations (T-7185): the start-time catalogues and addresses
    /// (`JC_PORTAL_ORGANIZATION_CATALOGUES`, `..._ADDRESSES`) with `catalogues` and `addresses`
    /// of the Organizations those do not name; the set before is replaced whole, and a start-time
    /// entry wins over one of the same name. A `removed` Organization lists no address and keeps
    /// its catalogue entry without any token, so its projects publish nowhere (a run fails and
    /// says so) instead of falling back to the catalogue a project's `CkanInstance` names.
    pub fn replace_run_time_organizations(
        &self,
        catalogues: Vec<OrganizationCatalogue>,
        addresses: Vec<OrganizationAddresses>,
        removed: &[String],
    ) {
        let kept = |name: &String| !removed.contains(name);
        let held = self.organization_catalogues.current();
        let mut next = self.config.organization_catalogues.clone();
        for c in catalogues {
            if !next.iter().any(|o| o.organization == c.organization) {
                next.push(c);
            }
        }
        for c in held.iter() {
            if !kept(&c.organization) && !next.iter().any(|o| o.organization == c.organization) {
                next.push(c.clone());
            }
        }
        for c in &mut next {
            if !kept(&c.organization) {
                c.token = None;
                c.token_file = None;
            }
        }
        self.organization_catalogues.store(next);
        let mut next: Vec<OrganizationAddresses> = self
            .config
            .organization_addresses
            .iter()
            .filter(|a| kept(&a.organization))
            .cloned()
            .collect();
        for a in addresses {
            if !next.iter().any(|o| o.organization == a.organization) {
                next.push(a);
            }
        }
        self.organization_addresses.store(next);
    }
}

/// One Organization's own catalogue (`JC_PORTAL_ORGANIZATION_CATALOGUES`): every Endpoint of its
/// projects publishes there, into the CKAN organisation of its name, with its token, whatever
/// the project's `CkanInstance` says; the dataroom of a jc refuses any other organisation.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizationCatalogue {
    /// The `Organization` manifest's name, also the CKAN organisation's.
    pub organization: String,
    /// The catalogue's public URL, e.g. `https://opendata.city.example`.
    pub url: Url,
    /// Where its API is called; its public URL when unset.
    #[serde(default)]
    pub api: Option<Url>,
    /// The address `api`'s host is reached at instead of through DNS (jc's own listener).
    #[serde(default)]
    pub reach: Option<std::net::SocketAddr>,
    /// Where the Organization's Endpoints answer, as a dataset links them: a host, or a whole
    /// origin off https; the Portal's data host when unset.
    #[serde(default)]
    pub data_host: Option<String>,
    /// The file holding the CKAN API token the Portal publishes with; read per run. Required in
    /// `JC_PORTAL_ORGANIZATION_CATALOGUES`; jc's run-time entries carry [`Self::token`] instead.
    #[serde(default)]
    pub token_file: Option<std::path::PathBuf>,
    /// The token jc minted in this process for a run-time Organization's catalogue (T-7185),
    /// never read from the environment.
    #[serde(skip)]
    pub token: Option<CatalogueToken>,
}

/// A catalogue token held in memory (see [`OrganizationCatalogue::token`]); `Debug` prints a
/// placeholder, two are equal only when they are the same holder.
#[derive(Clone)]
pub struct CatalogueToken(pub std::sync::Arc<jcctl::secrets::SecretValue>);

impl PartialEq for CatalogueToken {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for CatalogueToken {}

impl std::fmt::Debug for CatalogueToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CatalogueToken(redacted)")
    }
}

/// The Organizations' own catalogues (T-7185): `JC_PORTAL_ORGANIZATION_CATALOGUES` at start,
/// with jc's run-time Organizations added ([`AppState::replace_run_time_organizations`]).
pub type OrganizationCatalogues = Swappable<Vec<OrganizationCatalogue>>;

/// Every address of each Organization: `JC_PORTAL_ORGANIZATION_ADDRESSES` at start, with jc's
/// run-time Organizations added.
pub type OrganizationAddressList = Swappable<Vec<OrganizationAddresses>>;

/// `JC_PORTAL_ORGANIZATION_CATALOGUES`: a JSON array of [`OrganizationCatalogue`], each
/// Organization once.
pub fn parse_catalogues(raw: &str) -> Result<Vec<OrganizationCatalogue>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let all: Vec<OrganizationCatalogue> =
        serde_json::from_str(raw).map_err(|e| format!("not a list of catalogues: {e}"))?;
    for (i, c) in all.iter().enumerate() {
        if all[..i].iter().any(|o| o.organization == c.organization) {
            return Err(format!("{}: named twice", c.organization));
        }
        if c.token_file.is_none() {
            return Err(format!(
                "{}: tokenFile names the file of its CKAN API token, e.g. /run/secrets/catalogue-token",
                c.organization
            ));
        }
        for url in std::iter::once(&c.url).chain(c.api.as_ref()) {
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(format!("{}: '{url}' is not an http(s) URL", c.organization));
            }
        }
    }
    Ok(all)
}

/// What answers at one address of an Organization (API/15 §2, T-7013).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum AddressKind {
    /// Its Portal on a host of its own.
    Portal,
    /// Its open data portal.
    Opendata,
    /// Its APIs (the gateway).
    Api,
    /// Its MCP servers, each at `{url}{project}/{server}`.
    Mcp,
    /// A further portal host the operator listed for it (ODP-08).
    PortalSite,
    /// A host of its own bound at a root (`[[hosts]]`).
    Host,
    /// Its path on a host Organizations share: `https://{base}/{id}/`.
    Shared,
    /// Where its people sign in: the Portal's identity provider.
    SignIn,
}

/// One address: what answers and where.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizationAddress {
    pub kind: AddressKind,
    /// The address, e.g. `https://portal.bbsk.sk/` or `https://smartcity.sk/sk_bbsk/`.
    #[schema(value_type = String)]
    pub url: Url,
}

/// Every address of one Organization, as jc's host registry routes it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizationAddresses {
    /// The `Organization` manifest's name.
    pub organization: String,
    /// Its id (`sk_bbsk`, EP-186), once jc sets one.
    #[serde(default)]
    pub organization_id: Option<String>,
    pub addresses: Vec<OrganizationAddress>,
}

/// `JC_PORTAL_ORGANIZATION_ADDRESSES`: a JSON array of [`OrganizationAddresses`], each
/// Organization once, every address an http(s) URL with a host.
pub fn parse_addresses(raw: &str) -> Result<Vec<OrganizationAddresses>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let all: Vec<OrganizationAddresses> =
        serde_json::from_str(raw).map_err(|e| format!("not a list of addresses: {e}"))?;
    for (i, org) in all.iter().enumerate() {
        if all[..i].iter().any(|o| o.organization == org.organization) {
            return Err(format!("{}: named twice", org.organization));
        }
        if let Some(id) = &org.organization_id {
            jc_core::names::OrganizationId::parse(id)
                .map_err(|e| format!("{}: {e}", org.organization))?;
        }
        for a in &org.addresses {
            if !matches!(a.url.scheme(), "http" | "https") || a.url.host_str().is_none() {
                return Err(format!(
                    "{}: '{}' is not an http(s) URL",
                    org.organization, a.url
                ));
            }
        }
    }
    Ok(all)
}

/// The host the request reached, as the router that placed it saw it (jc's `Mount`: a trusted
/// proxy's `X-Forwarded-Host` already applied). Without it the `Host` header counts.
#[derive(Clone, Debug)]
pub struct RequestHost(pub String);

/// The Organization the router that placed the request decided for it (jc's host registry: a
/// bound host's row or a shared host's first path segment). It wins over [`RequestHost`].
#[derive(Clone, Debug)]
pub struct RequestOrganization(pub String);

tokio::task_local! {
    static PINNED: OrganizationHost;
    static REPOSITORY: String;
}

/// The Organization the running request is pinned to, if any.
pub fn pinned() -> Option<OrganizationHost> {
    PINNED.try_with(Clone::clone).ok()
}

/// The name of the Organization the running request is pinned to, if any.
pub fn organization() -> Option<String> {
    PINNED.try_with(|pin| pin.organization.clone()).ok()
}

/// The name the repository gives the Organization the running request is pinned to
/// ([`repository_name`]), if any: what the pin is compared with in the mirror's manifests.
pub fn repository_organization() -> Option<String> {
    REPOSITORY.try_with(Clone::clone).ok().or_else(organization)
}

/// Runs `f` pinned to `host`, as [`middleware`] does for a request; for tests and background
/// work done on an Organization's behalf.
pub async fn scope<F: std::future::Future>(host: OrganizationHost, f: F) -> F::Output {
    PINNED.scope(host, f).await
}

/// [`scope`] with the pin's [`repository_name`] read from `state`, as [`middleware`] pins a request.
pub async fn scope_in<F: std::future::Future>(
    state: &AppState,
    host: OrganizationHost,
    f: F,
) -> F::Output {
    let repository = repository_name(
        &state.mirror,
        &state.organization_hosts.current(),
        &host.organization,
    );
    PINNED.scope(host, REPOSITORY.scope(repository, f)).await
}

/// The name the repository (the forge) gives the registry's Organization `organization` (T-7414).
/// A forge can keep the installation Organization's manifest under another name (jc-prod: `hel`)
/// than the registry and `JC_PORTAL_INSTALLATION_ORGANIZATION` give it (`smartcity`); its
/// bindings, and the projects it owns, are then the configured installation's. Only while the
/// repository holds no Organization of the configured name and its installation claim is no
/// Organization this Portal serves a host of, so one Organization never reads as another.
pub fn repository_name(
    mirror: &crate::store::Mirror,
    hosts: &[OrganizationHost],
    organization: &str,
) -> String {
    let unnamed = mirror.configured_installation() == Some(organization)
        && mirror
            .get(
                crate::permissions::ORG_NAMESPACE,
                "Organization",
                organization,
            )
            .is_none();
    unnamed
        .then(|| crate::permissions::installation_organization(mirror))
        .flatten()
        .filter(|claim| !hosts.iter().any(|h| &h.organization == claim))
        .unwrap_or_else(|| organization.to_owned())
}

/// The registry's name for the Organization the repository names `name`: the inverse of
/// [`repository_name`], so the installation Organization's forge name (jc-prod: `hel`) reads as
/// the configured one (`smartcity`) and every other name stays as given (T-7436).
pub fn registry_name(
    mirror: &crate::store::Mirror,
    hosts: &[OrganizationHost],
    name: &str,
) -> String {
    mirror
        .configured_installation()
        .filter(|configured| {
            *configured != name && repository_name(mirror, hosts, configured) == name
        })
        .unwrap_or(name)
        .to_owned()
}

/// The public base of the running request: its Organization host's, else the configured one.
pub fn public_base(config: &crate::config::Config) -> Url {
    pinned().map_or_else(|| config.public_base_url.clone(), |pin| pin.base)
}

/// A host name without its port, lower-cased.
fn bare(host: &str) -> String {
    host.rsplit_once(':')
        .filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()))
        .map_or(host, |(name, _)| name)
        .to_ascii_lowercase()
}

/// `JC_PORTAL_ORGANIZATION_HOSTS`: `name=url` pairs separated by commas, each url an http(s)
/// origin, each name and each host once.
pub fn parse(raw: &str) -> Result<Vec<OrganizationHost>, String> {
    let mut out: Vec<OrganizationHost> = Vec::new();
    for pair in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, url) = pair
            .split_once('=')
            .ok_or_else(|| format!("'{pair}' is not name=url"))?;
        let name = name.trim();
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(format!("'{name}' is not an Organization name"));
        }
        let base: Url = url
            .trim()
            .parse()
            .map_err(|e| format!("{name}: '{url}': {e}"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || base.path() != "/"
            || base.query().is_some()
        {
            return Err(format!("{name}: '{url}' is not an http(s) origin"));
        }
        let host = bare(base.host_str().unwrap_or_default());
        if out.iter().any(|o| o.organization == name) {
            return Err(format!("{name}: named twice"));
        }
        if out
            .iter()
            .any(|o| bare(o.base.host_str().unwrap_or_default()) == host)
        {
            return Err(format!("{name}: host {host} is another Organization's"));
        }
        out.push(OrganizationHost {
            organization: name.to_owned(),
            base,
        });
    }
    Ok(out)
}

/// The Organization host `host` names, if any.
pub fn for_host<'a>(hosts: &'a [OrganizationHost], host: &str) -> Option<&'a OrganizationHost> {
    let host = bare(host);
    hosts
        .iter()
        .find(|o| o.base.host_str().is_some_and(|h| bare(h) == host))
}

/// Pins every request on an Organization's host to it, and refuses the API to a signed-in
/// caller who belongs to no project of that Organization: an SSO session of another
/// Organization's person signs them in here, and finds nothing but this 403.
pub async fn middleware(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let hosts = state.organization_hosts.current();
    if hosts.is_empty() {
        return next.run(request).await;
    }
    let placed = request.extensions().get::<RequestOrganization>();
    let decided = placed
        .and_then(|org| hosts.iter().find(|h| h.organization == org.0))
        .cloned();
    // An Organization the router placed but this Portal holds no host for (one the registry
    // added or removed a moment ago) is refused, as the gateway refuses it: unpinned, the request
    // would be served across every Organization.
    if placed.is_some() && decided.is_none() {
        return (
            axum::http::StatusCode::MISDIRECTED_REQUEST,
            "this host is not served here",
        )
            .into_response();
    }
    let host = request
        .extensions()
        .get::<RequestHost>()
        .map(|h| h.0.clone())
        .or_else(|| {
            request
                .headers()
                .get(axum::http::header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        });
    let Some(pin) = decided.or_else(|| host.as_deref().and_then(|h| for_host(&hosts, h)).cloned())
    else {
        return next.run(request).await;
    };
    let repository = repository_name(&state.mirror, &hosts, &pin.organization);
    PINNED
        .scope(
            pin.clone(),
            REPOSITORY.scope(repository.clone(), async move {
                if gated(request.uri().path()) {
                    let (mut parts, body) = request.into_parts();
                    let caller =
                        <crate::auth::session::CurrentUser as axum::extract::FromRequestParts<
                            AppState,
                        >>::from_request_parts(&mut parts, &state)
                        .await;
                    let foreign = caller.is_ok_and(|user| {
                        !crate::permissions::belongs_to_organization(
                            &state.mirror,
                            &state.config.bootstrap_admins,
                            &user.0.identity,
                            &repository,
                        )
                    });
                    // Before the first run's mirror nobody belongs anywhere: that is no refusal.
                    if foreign && state.mirror.is_empty() {
                        return starting(
                            crate::error::ApiError::Unavailable(STARTING.into()).into_response(),
                        );
                    }
                    if foreign {
                        return crate::error::ApiError::Denied(format!(
                            "this Portal is {}'s; you belong to none of its projects",
                            pin.organization
                        ))
                        .into_response();
                    }
                    return next.run(Request::from_parts(parts, body)).await;
                }
                next.run(request).await
            }),
        )
        .await
}

/// What a request answers while this process has no mirror to serve: no completed run of an
/// earlier process left one, and its own first run has not ended (T-7401).
pub const STARTING: &str =
    "this Portal is starting and has not loaded its projects yet; try again in a minute";

/// `response` with the `Retry-After` of [`STARTING`].
pub fn starting(mut response: Response) -> Response {
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("60"),
    );
    response
}

/// The API a signed-in caller of another Organization is refused on this host: everything under
/// `/api/v1` (the Portal's MCP server included) but signing in and out, and the personal
/// connectors' MCP doors.
fn gated(path: &str) -> bool {
    (path.starts_with("/api/v1/") && !path.starts_with("/api/v1/auth/")) || path.starts_with("/c/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_parse_and_each_bad_one_is_refused() {
        let hosts = parse("hel=https://portal.hel.example, ba=http://portal.ba.localhost:8080")
            .expect("parses");
        assert_eq!(hosts.len(), 2);
        assert_eq!(
            for_host(&hosts, "PORTAL.BA.localhost:8080").map(|h| h.organization.as_str()),
            Some("ba")
        );
        assert_eq!(
            for_host(&hosts, "portal.hel.example").map(|h| h.base.as_str()),
            Some("https://portal.hel.example/")
        );
        assert!(for_host(&hosts, "portal.example").is_none());
        assert!(parse("").expect("empty").is_empty());
        for bad in [
            "hel",
            "Hel=https://a.example",
            "hel=ftp://a.example",
            "hel=https://a.example/portal",
            "hel=https://a.example,hel=https://b.example",
            "hel=https://a.example,ba=https://A.example:8443",
        ] {
            assert!(parse(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn catalogues_parse_and_a_bad_one_is_refused() {
        let all = parse_catalogues(
            r#"[{"organization":"hel","url":"http://opendata.hel.localhost:42301",
                 "reach":"127.0.0.1:42301","tokenFile":"/run/hel-token"}]"#,
        )
        .expect("parses");
        assert_eq!(all[0].reach, Some("127.0.0.1:42301".parse().unwrap()));
        assert!(parse_catalogues(" ").expect("empty").is_empty());
        for bad in [
            r#"[{"organization":"hel","url":"ftp://x.example","tokenFile":"t"}]"#,
            r#"[{"organization":"hel","url":"https://x.example"}]"#,
            r#"[{"organization":"hel","url":"https://x.example","tokenFile":"t","token":"s"}]"#,
            r#"[{"organization":"hel","url":"https://x.example","tokenFile":"t"},
                {"organization":"hel","url":"https://y.example","tokenFile":"t"}]"#,
            "hel=https://x.example",
        ] {
            assert!(parse_catalogues(bad).is_err(), "{bad} was accepted");
        }
    }

    #[tokio::test]
    async fn the_pin_holds_inside_its_scope_alone() {
        let host = parse("hel=https://portal.hel.example").expect("parses")[0].clone();
        assert_eq!(organization(), None);
        let inside = scope(host, async { organization() }).await;
        assert_eq!(inside.as_deref(), Some("hel"));
        assert_eq!(organization(), None);
    }

    #[test]
    fn the_api_is_gated_and_signing_in_is_not() {
        assert!(gated("/api/v1/projects"));
        assert!(gated("/api/v1/mcp"));
        assert!(gated("/c/abc"));
        assert!(!gated("/api/v1/auth/login"));
        assert!(!gated("/api/v1/auth/callback"));
        assert!(!gated("/"));
        assert!(!gated("/assets/index.js"));
    }
}
