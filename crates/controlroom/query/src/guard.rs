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
use ipnet::{Ipv4Net, Ipv6Net};
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
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            allow_hosts: Vec::new(),
            max_response_bytes: 8 * 1024 * 1024,
            timeout: Duration::from_secs(10),
            allow_http: false,
            allow_loopback_for_tests: false,
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
        let parsed = Url::parse(raw_url)
            .map_err(|e| DsError::GuardRefused(format!("invalid URL '{raw_url}': {e}")))?;

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(DsError::GuardRefused(
                "URL must not contain embedded userinfo credentials".into(),
            ));
        }

        let scheme = parsed.scheme();
        if scheme != "https" && !(self.config.allow_http && scheme == "http") {
            return Err(DsError::GuardRefused(format!(
                "disallowed URL scheme '{scheme}' (https required)"
            )));
        }

        let host = parsed
            .host_str()
            .ok_or_else(|| DsError::GuardRefused("URL is missing host".into()))?
            .to_ascii_lowercase();

        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| DsError::GuardRefused("URL has unknown port".into()))?;

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
        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
            check_ip_allowed(ip, self.config.allow_loopback_for_tests)?;
        }

        Ok((parsed, host, port))
    }

    /// Resolves DNS, checks every returned IP address, pins the resolved addresses on the HTTP
    /// client, sends the request with redirects disabled, and reads the response up to `max_response_bytes`.
    pub async fn send(&self, req: OutboundRequest) -> Result<OutboundResponse, DsError> {
        let (parsed_url, host, port) = self.validate_url(&req.url)?;

        // Resolve DNS and check every address
        let clean_host = host.trim_matches(['[', ']']);
        let addrs: Vec<SocketAddr> = if let Ok(ip) = clean_host.parse::<IpAddr>() {
            check_ip_allowed(ip, self.config.allow_loopback_for_tests)?;
            vec![SocketAddr::new(ip, port)]
        } else {
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
                check_ip_allowed(addr.ip(), self.config.allow_loopback_for_tests)?;
            }
            resolved
        };

        let timeout = req
            .timeout
            .map_or(self.config.timeout, |t| t.min(self.config.timeout));

        let mut builder = reqwest::Client::builder()
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
    // CGNAT 100.64.0.0/10 and benchmark 198.18.0.0/15
    for cidr in ["100.64.0.0/10", "198.18.0.0/15"] {
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
    // AWS IPv6 metadata fd00:ec2::254 and documentation 2001:db8::/32
    for cidr in ["fd00:ec2::/32", "2001:db8::/32"] {
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
}
