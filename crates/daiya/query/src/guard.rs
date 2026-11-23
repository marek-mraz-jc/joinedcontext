#![forbid(unsafe_code)]
//! Outbound HTTP guard (R3, §3.3, §4.2, §4.3 CVE-2022-29170):
//! - Per-datasource host allow-list (empty allow-list denies everything).
//! - Requires `https` by default (`allow_http_for_tests` only for local wiremock/test listeners on
//!   explicitly allow-listed hosts).
//! - Sends through [`jc_http`], which resolves DNS once and checks every IPv4 and IPv6 address
//!   (including IPv4-mapped IPv6 `::ffff:a.b.c.d`) against loopback (`127.0.0.0/8`, `::1`), private (`10.0.0.0/8`,
//!   `172.16.0.0/12`, `192.168.0.0/16`, `fc00::/7`), link-local (`169.254.0.0/16`, `fe80::/10`),
//!   cloud metadata (`169.254.169.254`, `fd00:ec2::254`), CGNAT (`100.64.0.0/10`), multicast,
//!   broadcast, and unspecified ranges.
//! - Pins the validated socket addresses on the HTTP client (`resolve_to_addrs`) to prevent DNS
//!   rebinding between check and connect.
//! - Disables all HTTP redirects (`redirect::Policy::none()`).
//! - Enforces wall-clock timeout and streaming response size cap (`max_response_bytes`).

use crate::DsError;
use ipnet::IpNet;
use reqwest::Method;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use url::Url;

pub use jc_http::is_cloud_metadata;

/// Configuration for [`OutboundGuard`].
#[derive(Debug, Clone)]
pub struct GuardConfig {
    /// Allowed hostnames (e.g. `["prometheus-operated.monitoring.svc.cluster.local"]`).
    pub allow_hosts: Vec<String>,
    /// Maximum response body size in bytes (default 8 MiB, §3.3).
    pub max_response_bytes: usize,
    /// Request timeout (default 10s).
    pub timeout: Duration,
    /// Allow `http://` scheme (only for in-cluster or local test mock servers).
    pub allow_http: bool,
    /// Allow loopback addresses `127.0.0.1` / `::1` ONLY when explicitly enabled in unit/e2e tests
    /// for local mock servers; even then, `169.254.169.254` (metadata), private RFC 1918 ranges,
    /// and unlisted hosts remain strictly blocked.
    pub allow_loopback_for_tests: bool,
    /// Non-public networks the operator opens on purpose (an in-cluster Prometheus or broker
    /// on a service network). Only an allow-listed host may still be reached, and the cloud
    /// metadata addresses stay blocked even inside these networks.
    pub allowed_networks: Vec<IpNet>,
    /// Endpoint bases on the host's own listener the guard lets through, over `http` too,
    /// whatever their address class ([`OwnReach`]). Anything else on that listener stays
    /// refused like any loopback or private address.
    pub own_reach: Vec<OwnReach>,
    /// The host's gateway Endpoints at an address of their own ([`EndpointBase`]): with
    /// [`Self::own_reach`], the only URLs a caller's token is sent to.
    pub endpoint_bases: Vec<EndpointBase>,
    /// One `host:port` the operator opens on purpose ([`OperatorReach`]): a URL naming exactly
    /// that host and port may use `http` and reach a non-public address. The allow-list still
    /// applies and the cloud metadata addresses stay blocked.
    pub operator_reach: Option<OperatorReach>,
}

/// One address the operator opens to a datasource kind, written `http://<host>:<port>` (the
/// label-enforcing Prometheus proxy of an instance on a shared cluster): one host and one port,
/// never a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorReach {
    host: String,
    port: u16,
}

impl OperatorReach {
    /// `raw` is `http[s]://<host>:<port>[/]`: the port written, no path, query, fragment or
    /// credentials, and not a cloud metadata address.
    pub fn parse(raw: &str) -> Result<Self, DsError> {
        let refused = |why: &str| DsError::GuardRefused(format!("operator reach '{raw}': {why}"));
        let url = Url::parse(raw.trim()).map_err(|_| refused("not a URL"))?;
        let host = url
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| refused("no host"))?
            .to_ascii_lowercase();
        let port = url.port().ok_or_else(|| refused("no port written"))?;
        let metadata = host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(is_cloud_metadata);
        if !matches!(url.scheme(), "http" | "https")
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || metadata
        {
            return Err(refused("want http://<host>:<port>"));
        }
        Ok(Self { host, port })
    }

    fn admits(&self, host: &str, port: u16) -> bool {
        self.host == host && self.port == port
    }
}

/// The NGSI-LD and OData Endpoints of the host's own gateway, as one base URL
/// (`http://127.0.0.1:8080/api/endpoint/`): a request URL passes only when it names the base's
/// IP literal and port exactly as written and goes on with one Endpoint slug, so the host's other
/// mounts on that listener (its identity provider, the Portal, the forge, the assistant) stay out
/// of reach. Every request URL is judged again, after parsing and dot-segment removal; encoded
/// dots, slashes and backslashes in the path are refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnReach {
    /// The base as the URL parser writes it (no `:80`).
    base: String,
    /// The base with its port always written.
    written: String,
    addr: SocketAddr,
}

impl OwnReach {
    /// `base` is `http://<ip>[:<port>]/<path>/`: an IP literal and a path ending in `/`, with no
    /// query, fragment or credentials.
    pub fn endpoints(base: &str) -> Result<Self, DsError> {
        let refused = |why: &str| DsError::GuardRefused(format!("own reach '{base}': {why}"));
        let url = Url::parse(base).map_err(|_| refused("not a URL"))?;
        let ip = match url.host() {
            Some(url::Host::Ipv4(v4)) => IpAddr::V4(v4),
            Some(url::Host::Ipv6(v6)) => IpAddr::V6(v6),
            _ => return Err(refused("the host is no IP literal")),
        };
        let port = url
            .port_or_known_default()
            .ok_or_else(|| refused("no port"))?;
        if url.scheme() != "http"
            || !url.path().ends_with('/')
            || ["%", "\\", "/."].iter().any(|bad| base.contains(bad))
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || is_cloud_metadata(ip)
        {
            return Err(refused("want http://<ip>:<port>/<path>/"));
        }
        let addr = SocketAddr::new(ip, port);
        Ok(Self {
            base: url.as_str().to_owned(),
            written: format!("http://{addr}{}", url.path()),
            addr,
        })
    }

    /// The listener address this reach opens.
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The Endpoint slug `raw` names under this base ([`slug_under`]).
    fn slug<'a>(&self, raw: &str, parsed: &'a Url) -> Option<&'a str> {
        slug_under(&self.base, &self.written, raw, parsed)
    }
}

/// The base under which the host's gateway serves its Endpoints at an address of its own, as the
/// Portal names them in the datasources it makes (`https://api.city.example/api/endpoint/`). A
/// caller's token goes out only to a URL under such a base or an [`OwnReach`], and only for the
/// one Endpoint slug the URL names ([`OutboundGuard::token_endpoint`]). The base opens no
/// address, network or scheme: the request still passes every other check of the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointBase {
    /// The base as the URL parser writes it.
    base: String,
}

impl EndpointBase {
    /// `base` is `http[s]://<host>[:<port>]/<path>/`: a path ending in `/`, with no query,
    /// fragment, credentials, encoded character or dot segment.
    pub fn parse(base: &str) -> Result<Self, DsError> {
        let refused = |why: &str| DsError::GuardRefused(format!("endpoint base '{base}': {why}"));
        let url = Url::parse(base.trim()).map_err(|_| refused("not a URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none_or(str::is_empty)
            || !url.path().ends_with('/')
            || ["%", "\\", "/."].iter().any(|bad| base.contains(bad))
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(refused("want http[s]://<host>[:<port>]/<path>/"));
        }
        Ok(Self {
            base: url.as_str().to_owned(),
        })
    }

    fn slug<'a>(&self, raw: &str, parsed: &'a Url) -> Option<&'a str> {
        slug_under(&self.base, &self.base, raw, parsed)
    }
}

/// The Endpoint slug of `raw`: `raw` as written and `parsed` both start with `base` (as the URL
/// parser writes it; `raw` may use the spelling `written`), followed by one slug
/// (`[A-Za-z0-9_-]+`) and the end of the path or `/`, and neither path holds a backslash, an
/// encoded dot, slash or backslash, or a dot segment.
fn slug_under<'a>(base: &str, written: &str, raw: &str, parsed: &'a Url) -> Option<&'a str> {
    let raw = raw.trim();
    let rest = raw
        .strip_prefix(written)
        .or_else(|| raw.strip_prefix(base))?;
    let after = parsed.as_str().strip_prefix(base)?;
    let slug_of = |s: &str| {
        let end = s.find(['/', '?', '#']).unwrap_or(s.len());
        (end > 0
            && s[..end]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        .then_some(end)
    };
    let clean = |path: &str| {
        let path = path.to_ascii_lowercase();
        !path.contains('\\')
            && !["%2e", "%2f", "%5c"].iter().any(|bad| path.contains(bad))
            && !path.split('/').any(|seg| seg == ".." || seg == ".")
    };
    let raw_path = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
    slug_of(rest)?;
    let end = slug_of(after)?;
    (clean(raw_path) && clean(parsed.path())).then(|| &after[..end])
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            allow_hosts: Vec::new(),
            max_response_bytes: 8 * 1024 * 1024,
            timeout: Duration::from_secs(10),
            allow_http: false,
            allow_loopback_for_tests: false,
            allowed_networks: Vec::new(),
            own_reach: Vec::new(),
            endpoint_bases: Vec::new(),
            operator_reach: None,
        }
    }
}

/// Outbound HTTP request passed to [`OutboundGuard::send`].
#[derive(Debug, Clone)]
pub struct OutboundRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
}

/// Validated, size-capped response from [`OutboundGuard::send`].
#[derive(Debug, Clone)]
pub struct OutboundResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Outbound guard enforcing SSRF, allow-list, DNS pinning, redirect refusal, and size limits (R3, R12).
#[derive(Debug, Clone)]
pub struct OutboundGuard {
    config: GuardConfig,
}

impl OutboundGuard {
    #[must_use]
    pub const fn new(config: GuardConfig) -> Self {
        Self { config }
    }

    #[must_use]
    pub const fn config(&self) -> &GuardConfig {
        &self.config
    }

    /// The Endpoint slug `raw_url` names when the URL is one of the host's own gateway Endpoints
    /// (an [`OwnReach`] or an [`EndpointBase`]); `None` for any other URL. A driver sends a
    /// caller's token only to such a URL, exchanged for that slug: a host or slug the
    /// datasource's author wrote elsewhere never receives it.
    #[must_use]
    pub fn token_endpoint(&self, raw_url: &str) -> Option<String> {
        let parsed = Url::parse(raw_url.trim()).ok()?;
        self.config
            .own_reach
            .iter()
            .find_map(|reach| reach.slug(raw_url, &parsed))
            .or_else(|| {
                self.config
                    .endpoint_bases
                    .iter()
                    .find_map(|base| base.slug(raw_url, &parsed))
            })
            .map(str::to_owned)
    }

    /// Validates a URL and its host against the guard's scheme and allow-list rules (before DNS).
    pub fn validate_url(&self, raw_url: &str) -> Result<(Url, String, u16), DsError> {
        self.validate(raw_url)
            .map(|(url, host, port, _)| (url, host, port))
    }

    /// [`Self::validate_url`], plus the own listener address when one [`OwnReach`] admits the URL.
    fn validate(&self, raw_url: &str) -> Result<(Url, String, u16, Option<SocketAddr>), DsError> {
        let parsed = Url::parse(raw_url)
            .map_err(|e| DsError::GuardRefused(format!("invalid URL '{raw_url}': {e}")))?;

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(DsError::GuardRefused(
                "URL must not contain embedded userinfo credentials".into(),
            ));
        }

        let scheme = parsed.scheme();
        let own = self
            .config
            .own_reach
            .iter()
            .find(|reach| reach.slug(raw_url, &parsed).is_some())
            .map(OwnReach::addr);
        let host = parsed
            .host_str()
            .ok_or_else(|| DsError::GuardRefused("URL is missing host".into()))?
            .to_ascii_lowercase();
        let port = parsed.port_or_known_default();
        let operator = port.is_some_and(|port| self.operator_admits(&host, port));
        let http_ok = self.config.allow_http || own.is_some() || operator;
        if scheme != "https" && !(http_ok && scheme == "http") {
            return Err(DsError::GuardRefused(format!(
                "disallowed URL scheme '{scheme}' (https required)"
            )));
        }
        // Only http and https pass the scheme check above, and both have a known port.
        let Some(port) = port else {
            return Err(DsError::GuardRefused("URL has unknown port".into()));
        };

        let host_with_port = format!("{host}:{port}");
        let allowed = self.config.allow_hosts.iter().any(|entry| {
            let e = entry.trim().to_ascii_lowercase();
            !e.is_empty() && (e == host || e == host_with_port)
        });

        if !allowed {
            return Err(DsError::GuardRefused(format!(
                "host '{host}' is not in the datasource allow-list"
            )));
        }

        // If host is an IP literal, check it immediately
        if own.is_none()
            && let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
        {
            self.check_addr(ip, operator)?;
        }

        Ok((parsed, host, port, own))
    }

    fn operator_admits(&self, host: &str, port: u16) -> bool {
        self.config
            .operator_reach
            .as_ref()
            .is_some_and(|reach| reach.admits(host.trim_matches(['[', ']']), port))
    }

    /// [`Self::check_ip`], or for the operator's one opened address only the metadata check.
    fn check_addr(&self, ip: IpAddr, operator: bool) -> Result<(), DsError> {
        if operator && !is_cloud_metadata(ip) {
            return Ok(());
        }
        self.check_ip(ip)
    }

    /// Checks one resolved address: the metadata addresses never pass, an operator-opened
    /// network passes, everything else follows [`check_ip_allowed`].
    pub fn check_ip(&self, ip: IpAddr) -> Result<(), DsError> {
        if is_cloud_metadata(ip) {
            return Err(DsError::GuardRefused(format!(
                "blocked cloud metadata address {ip}"
            )));
        }
        let unmapped = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            IpAddr::V4(_) => ip,
        };
        if self
            .config
            .allowed_networks
            .iter()
            .any(|net| net.contains(&unmapped))
        {
            return Ok(());
        }
        check_ip_allowed(ip, self.config.allow_loopback_for_tests)
    }

    /// Checks `host` against the allow-list, resolves it once and checks every address. The
    /// answer is the address set a caller must connect to (DNS pinning), for HTTP and for
    /// the Postgres driver's TCP connection alike.
    pub async fn resolve_checked(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, DsError> {
        let host = host.trim().to_ascii_lowercase();
        let clean_host = host.trim_matches(['[', ']']);
        let host_with_port = format!("{clean_host}:{port}");
        let allowed = self.config.allow_hosts.iter().any(|entry| {
            let e = entry.trim().to_ascii_lowercase();
            !e.is_empty() && (e == clean_host || e == host || e == host_with_port)
        });
        if !allowed {
            return Err(DsError::GuardRefused(format!(
                "host '{clean_host}' is not in the datasource allow-list"
            )));
        }
        let operator = self.operator_admits(clean_host, port);
        if let Ok(ip) = clean_host.parse::<IpAddr>() {
            self.check_addr(ip, operator)?;
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        jc_http::resolve(clean_host, port, self.config.timeout, |ip| {
            self.check_addr(ip, operator).is_ok()
        })
        .await
        .map_err(outbound_error)
    }

    /// Resolves DNS, checks every returned IP address, pins the resolved addresses on the HTTP
    /// client, sends the request with redirects disabled, and reads the response up to `max_response_bytes`.
    pub async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, DsError> {
        let (parsed_url, host, port, own) = self.validate(&req.url)?;

        let operator = self.operator_admits(&host, port);
        let timeout = req
            .timeout
            .map_or(self.config.timeout, |t| t.min(self.config.timeout));
        let answer = jc_http::send(
            jc_http::Request {
                method: req.method,
                // `validate` let `http` through only where this guard admits it.
                plain_http: parsed_url.scheme() == "http",
                url: parsed_url,
                headers: req.headers,
                body: req.body.map(Into::into),
                timeout,
                max_response_bytes: self.config.max_response_bytes,
            },
            // An own reach names its IP literal and opens that address alone.
            |ip| match own {
                Some(addr) => ip == addr.ip(),
                None => self.check_addr(ip, operator).is_ok(),
            },
        )
        .await
        .map_err(outbound_error)?;

        let headers: Vec<(String, String)> = answer
            .headers
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|val| (k.as_str().to_owned(), val.to_owned()))
            })
            .collect();

        Ok(OutboundResponse {
            status: answer.status,
            headers,
            body: answer.body,
        })
    }
}

fn outbound_error(error: jc_http::Error) -> DsError {
    match error {
        jc_http::Error::Transport { .. } | jc_http::Error::Body(_) => DsError::Upstream {
            status: 502,
            message: error.to_string(),
        },
        refused => DsError::GuardRefused(refused.to_string()),
    }
}

/// Checks whether `ip` is permitted for outbound datasource connections ([`jc_http::check_address`]).
pub fn check_ip_allowed(ip: IpAddr, allow_loopback_for_tests: bool) -> Result<(), DsError> {
    jc_http::check_address(ip, allow_loopback_for_tests)
        .map_err(|blocked| DsError::GuardRefused(blocked.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_blocks_private_loopback_link_local_metadata_and_ipv4_mapped_ipv6() {
        for blocked in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "169.254.169.254",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fc00::1",
            "fd00:ec2::254",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:10.0.0.1",
        ] {
            let ip: IpAddr = blocked.parse().unwrap();
            assert!(
                check_ip_allowed(ip, false).is_err(),
                "expected {blocked} to be blocked"
            );
        }

        let public_ip: IpAddr = "93.184.216.34".parse().unwrap();
        assert!(check_ip_allowed(public_ip, false).is_ok());
    }

    #[test]
    fn guard_blocks_ipv4_hidden_in_ipv6_and_reserved_ranges() {
        for blocked in [
            // IPv4-compatible, NAT64 and 6to4 forms of loopback, metadata and private addresses.
            "::127.0.0.1",
            "::169.254.169.254",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b::10.0.0.1",
            "2002:a9fe:a9fe::1",
            "2002:0a00:0001::1",
            "64:ff9b:1::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "fec0::1",
            "100::1",
            "0.1.2.3",
            "192.0.0.170",
            "240.0.0.1",
            "255.255.255.255",
        ] {
            let ip: IpAddr = blocked.parse().unwrap();
            assert!(
                check_ip_allowed(ip, false).is_err(),
                "expected {blocked} to be blocked"
            );
        }
        for public in [
            "2606:4700:4700::1111",
            "64:ff9b::5db8:d822",
            "2002:5db8:d822::1",
        ] {
            let ip: IpAddr = public.parse().unwrap();
            assert!(check_ip_allowed(ip, false).is_ok(), "{public}");
        }
    }

    #[tokio::test]
    async fn guard_refuses_metadata_address_literals_even_when_allow_listed() {
        let guard = OutboundGuard::new(GuardConfig {
            allow_hosts: vec!["[::ffff:a9fe:a9fe]".into(), "169.254.169.254".into()],
            allow_http: true,
            ..GuardConfig::default()
        });
        for url in [
            "http://169.254.169.254/latest/meta-data/",
            "http://[::ffff:a9fe:a9fe]/latest/meta-data/",
        ] {
            let err = guard
                .send(OutboundRequest {
                    method: Method::GET,
                    url: url.into(),
                    headers: vec![],
                    body: None,
                    timeout: None,
                })
                .await;
            assert!(matches!(err, Err(DsError::GuardRefused(_))), "{url}");
        }
    }

    #[test]
    fn guard_enforces_https_and_host_allow_list() {
        let guard = OutboundGuard::new(GuardConfig {
            allow_hosts: vec!["metrics.example.com".into()],
            ..GuardConfig::default()
        });
        assert!(
            guard
                .validate_url("https://metrics.example.com/api/v1/query")
                .is_ok()
        );
        assert!(
            guard
                .validate_url("http://metrics.example.com/api/v1/query")
                .is_err()
        );
        assert!(
            guard
                .validate_url("https://evil.example.com/api/v1/query")
                .is_err()
        );
        assert!(
            guard
                .validate_url("https://user:pass@metrics.example.com/api")
                .is_err()
        );
    }

    fn own_guard() -> OutboundGuard {
        OutboundGuard::new(GuardConfig {
            allow_hosts: vec!["127.0.0.1".into(), "localhost".into(), "[::1]".into()],
            own_reach: vec![
                OwnReach::endpoints("http://127.0.0.1:8080/api/endpoint/").unwrap(),
                OwnReach::endpoints("http://[::1]:80/api/endpoint/").unwrap(),
            ],
            ..GuardConfig::default()
        })
    }

    #[test]
    fn a_token_goes_only_to_an_endpoint_of_the_hosts_gateway() {
        let mut guard = own_guard();
        guard.config.endpoint_bases =
            vec![EndpointBase::parse("https://api.city.example/api/endpoint/").unwrap()];
        for (url, slug) in [
            ("http://127.0.0.1:8080/api/endpoint/air", Some("air")),
            (
                "http://127.0.0.1:8080/api/endpoint/air/ngsi-ld/v1",
                Some("air"),
            ),
            ("https://api.city.example/api/endpoint/air", Some("air")),
            (
                "https://api.city.example/api/endpoint/air/odata/v4",
                Some("air"),
            ),
            ("https://evil.example/api/endpoint/air", None),
            (
                "https://api.city.example.evil.example/api/endpoint/air",
                None,
            ),
            ("https://api.city.example/x/api/endpoint/air", None),
            ("https://api.city.example/api/endpoint/", None),
            ("https://api.city.example/api/endpoint/%61ir", None),
            (
                "https://api.city.example/api/endpoint/air/../../portal",
                None,
            ),
            ("http://api.city.example/api/endpoint/air", None),
            ("http://127.0.0.1:8081/api/endpoint/air", None),
            ("not a url", None),
        ] {
            assert_eq!(guard.token_endpoint(url).as_deref(), slug, "{url}");
        }
        for bad in [
            "https://api.city.example/api/endpoint",
            "ftp://api.city.example/api/endpoint/",
            "https://u:p@api.city.example/api/endpoint/",
            "https://api.city.example/api/endpoint/?x=1",
            "https://api.city.example/api/%65ndpoint/",
        ] {
            assert!(EndpointBase::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn own_reach_admits_one_endpoint_and_nothing_else_on_the_listener() {
        let guard = own_guard();
        for url in [
            "http://127.0.0.1:8080/api/endpoint/kallio",
            "http://127.0.0.1:8080/api/endpoint/kallio/ngsi-ld/v1/entities?type=A&q=a%2Fb",
            "http://127.0.0.1:8080/api/endpoint/k_2-x/odata/Things",
            "http://[::1]:80/api/endpoint/kallio/ngsi-ld/v1/types",
            "http://[::1]/api/endpoint/kallio/ngsi-ld/v1/types",
        ] {
            assert!(guard.validate(url).unwrap().3.is_some(), "{url}");
        }
        for url in [
            "http://127.0.0.1:8080/login/realms/jc",
            "http://127.0.0.1:8080/portal/api/v1/me",
            "http://127.0.0.1:8080/git/api/v1/user",
            "http://127.0.0.1:8080/api/v1/d/x",
            "http://127.0.0.1:8080/api/endpoint/",
            "http://127.0.0.1:8080/api/endpointx/y",
            "http://127.0.0.1:8080/api/endpoint/x/../../id",
            "http://127.0.0.1:8080/api/endpoint/x/%2e%2e/%2E%2E/id",
            "http://127.0.0.1:8080/api/endpoint/x/.%2e/v1/d",
            "http://127.0.0.1:8080/api/endpoint/x%2f..%2fid",
            "http://127.0.0.1:8080/api/endpoint/x/a%2Fb",
            "http://127.0.0.1:8080/api/endpoint/x%5c..%5cid",
            "http://127.0.0.1:8080/api/endpoint/x\\..\\..\\id",
            "http://127.0.0.1:8080/api/endpoint/x/./y",
            "http://127.0.0.1:8081/api/endpoint/kallio",
            "http://localhost:8080/api/endpoint/kallio",
            "http://127.1:8080/api/endpoint/kallio",
            "http://2130706433:8080/api/endpoint/kallio",
            "http://127.0.0.1:08080/api/endpoint/kallio",
            "https://127.0.0.1:8080/api/endpoint/kallio",
            "http://user@127.0.0.1:8080/api/endpoint/kallio",
            "http://[::ffff:127.0.0.1]:8080/api/endpoint/kallio",
        ] {
            assert!(guard.validate(url).is_err(), "{url} must be refused");
        }
    }

    #[tokio::test]
    async fn own_reach_opens_no_raw_socket_and_no_metadata_base() {
        // The Postgres driver resolves through resolve_checked: the own address stays refused.
        assert!(
            own_guard()
                .resolve_checked("127.0.0.1", 8080)
                .await
                .is_err()
        );
        for base in [
            "http://169.254.169.254:80/api/endpoint/",
            "http://localhost:8080/api/endpoint/",
            "https://127.0.0.1:8080/api/endpoint/",
            "http://127.0.0.1:8080/api/endpoint",
            "http://127.0.0.1:8080/api/%2e%2e/",
        ] {
            assert!(OwnReach::endpoints(base).is_err(), "{base}");
        }
    }
}
