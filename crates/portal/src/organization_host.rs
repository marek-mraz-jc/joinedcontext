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
    /// The file holding the CKAN API token the Portal publishes with; read per run.
    pub token_file: std::path::PathBuf,
}

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
        for url in std::iter::once(&c.url).chain(c.api.as_ref()) {
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(format!("{}: '{url}' is not an http(s) URL", c.organization));
            }
        }
    }
    Ok(all)
}

/// The host the request reached, as the router that placed it saw it (jc's `Mount`: a trusted
/// proxy's `X-Forwarded-Host` already applied). Without it the `Host` header counts.
#[derive(Clone, Debug)]
pub struct RequestHost(pub String);

tokio::task_local! {
    static PINNED: OrganizationHost;
}

/// The Organization the running request is pinned to, if any.
pub fn pinned() -> Option<OrganizationHost> {
    PINNED.try_with(Clone::clone).ok()
}

/// The name of the Organization the running request is pinned to, if any.
pub fn organization() -> Option<String> {
    PINNED.try_with(|pin| pin.organization.clone()).ok()
}

/// Runs `f` pinned to `host`, as [`middleware`] does for a request; for tests and background
/// work done on an Organization's behalf.
pub async fn scope<F: std::future::Future>(host: OrganizationHost, f: F) -> F::Output {
    PINNED.scope(host, f).await
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
    let hosts = &state.config.organization_hosts;
    if hosts.is_empty() {
        return next.run(request).await;
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
    let Some(pin) = host.as_deref().and_then(|h| for_host(hosts, h)).cloned() else {
        return next.run(request).await;
    };
    PINNED
        .scope(pin.clone(), async move {
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
                        &pin.organization,
                    )
                });
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
        })
        .await
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
