#![forbid(unsafe_code)]
//! Outbound HTTP guard (R3, §3.3, §4.2, §4.3 CVE-2022-29170):
//! - Per-datasource host allow-list (empty allow-list denies everything).
//! - Requires `https` by default (`allow_http_for_tests` only for local wiremock/test listeners on
//!   explicitly allow-listed hosts).
//! - Resolves DNS once, checks every resolved IPv4 and IPv6 address (including IPv4-mapped IPv6
//!   `::ffff:a.b.c.d`) against loopback (`127.0.0.0/8`, `::1`), private (`10.0.0.0/8`,
//!   `172.16.0.0/12`, `192.168.0.0/16`, `fc00::/7`), link-local (`169.254.0.0/16`, `fe80::/10`),
//!   cloud metadata (`169.254.169.254`, `fd00:ec2::254`), CGNAT (`100.64.0.0/10`), multicast,
//!   broadcast, and unspecified ranges.
//! - Pins the validated socket addresses on the HTTP client (`resolve_to_addrs`) to prevent DNS
//!   rebinding between check and connect.
//! - Disables all HTTP redirects (`redirect::Policy::none()`).
//! - Enforces wall-clock timeout and streaming response size cap (`max_response_bytes`).

use crate::DsError;
use futures_util::StreamExt;
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use reqwest::Method;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;
use url::Url;

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

    /// `raw` as written and `parsed` both start with the base (in either spelling), followed by one slug
    /// (`[A-Za-z0-9_-]+`) and the end of the path or `/`, and the path holds no encoded dot,
    /// slash or backslash.
    fn admits(&self, raw: &str, parsed: &Url) -> bool {
        let raw = raw.trim();
        let Some(rest) = raw
            .strip_prefix(&self.written)
            .or_else(|| raw.strip_prefix(&self.base))
        else {
            return false;
        };
        let Some(after) = parsed.as_str().strip_prefix(&self.base) else {
            return false;
        };
        let slug_then_end = |s: &str| {
            let end = s.find(['/', '?', '#']).unwrap_or(s.len());
            end > 0
                && s[..end]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        // The path as written and as parsed: no backslash, no encoded dot, slash or backslash,
        // no dot segment.
        let clean = |path: &str| {
            let path = path.to_ascii_lowercase();
            !path.contains('\\')
                && !["%2e", "%2f", "%5c"].iter().any(|bad| path.contains(bad))
                && !path.split('/').any(|seg| seg == ".." || seg == ".")
        };
        let raw_path = &rest[..rest.find(['?', '#']).unwrap_or(rest.len())];
        slug_then_end(rest) && slug_then_end(after) && clean(raw_path) && clean(parsed.path())
    }
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
            .find(|reach| reach.admits(raw_url, &parsed))
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
        let port = port.ok_or_else(|| DsError::GuardRefused("URL has unknown port".into()))?;

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
        let resolved: Vec<SocketAddr> = tokio::net::lookup_host((clean_host, port))
            .await
            .map_err(|e| {
                DsError::GuardRefused(format!("DNS resolution failed for '{clean_host}': {e}"))
            })?
            .collect();
        if resolved.is_empty() {
            return Err(DsError::GuardRefused(format!(
                "DNS resolution returned no addresses for '{clean_host}'"
            )));
        }
        for addr in &resolved {
            self.check_addr(addr.ip(), operator)?;
        }
        Ok(resolved)
    }

    /// Resolves DNS, checks every returned IP address, pins the resolved addresses on the HTTP
    /// client, sends the request with redirects disabled, and reads the response up to `max_response_bytes`.
    pub async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, DsError> {
        let (parsed_url, host, port, own) = self.validate(&req.url)?;

        // Resolve DNS and check every address (an own reach names its IP literal)
        let clean_host = host.trim_matches(['[', ']']);
        let addrs = match own {
            Some(addr) => vec![addr],
            None => self.resolve_checked(&host, port).await?,
        };

        let timeout = req
            .timeout
            .map_or(self.config.timeout, |t| t.min(self.config.timeout));

        // No proxy: a proxy taken from the environment would resolve the host itself and
        // bypass the address checks and the DNS pinning above.
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout);

        if clean_host.parse::<IpAddr>().is_err() {
            builder = builder.resolve_to_addrs(clean_host, &addrs);
        }

        let client = builder
            .build()
            .map_err(|e| DsError::GuardRefused(format!("failed to build HTTP client: {e}")))?;

        let mut rb = client.request(req.method, parsed_url);
        for (k, v) in req.headers {
            rb = rb.header(&k, &v);
        }
        if let Some(body) = req.body {
            rb = rb.body(body);
        }

        let resp = rb.send().await.map_err(|e| {
            // Strip any potential URL credentials or internal details
            DsError::Upstream {
                status: 502,
                message: format!("request to '{clean_host}' failed: {}", e.without_url()),
            }
        })?;

        let status = resp.status().as_u16();
        if (300..400).contains(&status) {
            return Err(DsError::GuardRefused(format!(
                "upstream attempted redirect with status {status} (redirects are disabled)"
            )));
        }

        if let Some(cl) = resp.content_length()
            && cl > self.config.max_response_bytes as u64
        {
            return Err(DsError::GuardRefused(format!(
                "response content-length {cl} exceeds limit of {} bytes",
                self.config.max_response_bytes
            )));
        }

        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|val| (k.as_str().to_owned(), val.to_owned()))
            })
            .collect();

        let mut stream = resp.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| DsError::Upstream {
                status: 502,
                message: format!("error reading response stream: {}", e.without_url()),
            })?;
            if body.len().saturating_add(chunk.len()) > self.config.max_response_bytes {
                return Err(DsError::GuardRefused(format!(
                    "response body exceeded maximum size of {} bytes",
                    self.config.max_response_bytes
                )));
            }
            body.extend_from_slice(&chunk);
        }

        Ok(OutboundResponse {
            status,
            headers,
            body,
        })
    }
}

/// The cloud metadata addresses (`169.254.169.254`, also IPv4-mapped, and AWS `fd00:ec2::254`).
#[must_use]
pub fn is_cloud_metadata(ip: IpAddr) -> bool {
    let metadata_v4 = Ipv4Addr::new(169, 254, 169, 254);
    match ip {
        IpAddr::V4(v4) => v4 == metadata_v4,
        IpAddr::V6(v6) => {
            v6.to_ipv4_mapped() == Some(metadata_v4)
                || v6 == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254)
        }
    }
}

/// Checks whether `ip` is permitted for outbound datasource connections.
/// Unwraps IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) so IPv4 restrictions cannot be bypassed via IPv6!
pub fn check_ip_allowed(ip: IpAddr, allow_loopback_for_tests: bool) -> Result<(), DsError> {
    match ip {
        IpAddr::V4(v4) => check_ipv4_allowed(v4, allow_loopback_for_tests),
        IpAddr::V6(v6) => {
            if let Some(mapped_v4) = v6.to_ipv4_mapped() {
                return check_ipv4_allowed(mapped_v4, allow_loopback_for_tests);
            }
            check_ipv6_allowed(v6, allow_loopback_for_tests)
        }
    }
}

fn check_ipv4_allowed(ip: Ipv4Addr, allow_loopback_for_tests: bool) -> Result<(), DsError> {
    // Cloud metadata endpoint 169.254.169.254 is ALWAYS blocked even in test mode
    if ip == Ipv4Addr::new(169, 254, 169, 254) {
        return Err(DsError::GuardRefused(
            "blocked cloud metadata address 169.254.169.254".into(),
        ));
    }
    if ip.is_loopback() {
        if allow_loopback_for_tests {
            return Ok(());
        }
        return Err(DsError::GuardRefused(format!(
            "blocked loopback address {ip}"
        )));
    }
    if ip.is_unspecified()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
    {
        return Err(DsError::GuardRefused(format!(
            "blocked non-public IPv4 address {ip}"
        )));
    }
    // "This network" 0/8 (0.x.y.z reaches the local host on Linux), CGNAT, IETF protocol
    // assignments, 6to4 relay anycast, benchmark and the reserved 240/4.
    for cidr in [
        "0.0.0.0/8",
        "100.64.0.0/10",
        "192.0.0.0/24",
        "192.88.99.0/24",
        "198.18.0.0/15",
        "240.0.0.0/4",
    ] {
        if let Ok(net) = Ipv4Net::from_str(cidr)
            && net.contains(&ip)
        {
            return Err(DsError::GuardRefused(format!(
                "blocked reserved IPv4 range {ip} ({cidr})"
            )));
        }
    }
    Ok(())
}

fn check_ipv6_allowed(ip: Ipv6Addr, allow_loopback_for_tests: bool) -> Result<(), DsError> {
    if ip.is_loopback() {
        if allow_loopback_for_tests {
            return Ok(());
        }
        return Err(DsError::GuardRefused(format!(
            "blocked IPv6 loopback address {ip}"
        )));
    }
    if ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
    {
        return Err(DsError::GuardRefused(format!(
            "blocked non-public IPv6 address {ip}"
        )));
    }
    let seg = ip.segments();
    // Addresses that carry an IPv4 address inside: IPv4-compatible (::a.b.c.d), NAT64
    // (64:ff9b::/96) and 6to4 (2002::/16). The embedded address gets the IPv4 rules.
    let embedded_v4 = if seg[..6] == [0, 0, 0, 0, 0, 0] || seg[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        Some(Ipv4Addr::from(
            (u32::from(seg[6]) << 16) | u32::from(seg[7]),
        ))
    } else if seg[0] == 0x2002 {
        Some(Ipv4Addr::from(
            (u32::from(seg[1]) << 16) | u32::from(seg[2]),
        ))
    } else {
        None
    };
    if let Some(v4) = embedded_v4 {
        return check_ipv4_allowed(v4, allow_loopback_for_tests);
    }
    // AWS IPv6 metadata fd00:ec2::254, documentation, site-local, discard-only, local-use NAT64
    // and Teredo (its IPv4 endpoint is hidden).
    for cidr in [
        "fd00:ec2::/32",
        "2001:db8::/32",
        "fec0::/10",
        "100::/64",
        "64:ff9b:1::/48",
        "2001::/32",
    ] {
        if let Ok(net) = Ipv6Net::from_str(cidr)
            && net.contains(&ip)
        {
            return Err(DsError::GuardRefused(format!(
                "blocked reserved IPv6 address {ip}"
            )));
        }
    }
    Ok(())
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
            "http://127.0.0.1:8080/id/realms/jc",
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
