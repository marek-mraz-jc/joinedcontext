#![forbid(unsafe_code)]
//! Outbound SSRF guard for push mirrors and webhooks (S14, S15, S21, Threat T3).
//!
//! Enforces:
//! - `https` only (except configured in-cluster hosts or explicit test loopback).
//! - Mandatory host allow-list where configured.
//! - No userinfo (`user:pass@`) in URLs.
//! - DNS resolved once and checked against private, loopback, link-local, CGNAT, ULA, multicast,
//!   IPv4-mapped/compatible IPv6, NAT64 (`64:ff9b::/96`), 6to4 (`2002::/16`), and cloud metadata
//!   (`169.254.169.254`) ranges.
//! - Validated `SocketAddr` pinned on the HTTP client (`reqwest::ClientBuilder::resolve`) so DNS
//!   rebinding cannot switch the target on connect.
//! - Redirects refused (`redirect::Policy::none()`), 5 s timeout, 1 MiB response body cap.

use crate::error::{ShakoError, ShakoResult};
use ipnet::IpNet;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub const MAX_OUTBOUND_RESPONSE_BYTES: usize = 1024 * 1024;
pub const OUTBOUND_TIMEOUT: Duration = Duration::from_secs(5);

pub type MockOutboundHandler =
    Arc<dyn Fn(&str, &str, &[(String, String)], &[u8]) -> Option<(u16, Vec<u8>)> + Send + Sync>;

#[derive(Clone)]
pub struct OutboundPolicy {
    pub in_cluster_hosts: Vec<String>,
    pub allowed_hosts: Option<Vec<String>>,
    /// Test-only allowance for `127.0.0.1` / `localhost` so local integration tests can run.
    pub allow_loopback_for_tests: bool,
    pub mock_responses: Arc<RwLock<HashMap<String, (u16, Vec<u8>)>>>,
    pub mock_handler: Option<MockOutboundHandler>,
}

impl Default for OutboundPolicy {
    fn default() -> Self {
        Self {
            // Empty by default: an in-cluster host skips the private-range check, so each one is
            // named explicitly by the operator (never a `.svc.cluster.local` wildcard, never the
            // Kubernetes API) (T3).
            in_cluster_hosts: Vec::new(),
            allowed_hosts: Some(vec!["github.com".into(), "api.github.com".into()]),
            allow_loopback_for_tests: false,
            mock_responses: Arc::new(RwLock::new(HashMap::new())),
            mock_handler: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct OutboundGuard {
    pub policy: OutboundPolicy,
}

impl OutboundGuard {
    #[must_use]
    pub fn new(policy: OutboundPolicy) -> Self {
        Self { policy }
    }

    pub fn insert_mock(&self, url: impl Into<String>, status: u16, body: impl Into<Vec<u8>>) {
        self.policy
            .mock_responses
            .write()
            .insert(url.into(), (status, body.into()));
    }

    fn is_in_cluster_host(&self, host: &str) -> bool {
        let lower = host.to_ascii_lowercase();
        self.policy.in_cluster_hosts.iter().any(|allowed| {
            if allowed.starts_with('.') {
                lower.ends_with(allowed)
            } else {
                &lower == allowed
            }
        })
    }

    /// Checks whether an IP address is private, loopback, link-local, metadata (`169.254.169.254`),
    /// unspecified, multicast, or reserved (Threat T3).
    #[must_use]
    pub fn is_forbidden_ip(ip: IpAddr, allow_loopback_for_tests: bool) -> bool {
        // Cloud metadata 169.254.169.254 is ALWAYS forbidden, even in test mode!
        if ip == IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)) {
            return true;
        }
        if allow_loopback_for_tests && ip.is_loopback() {
            return false;
        }
        const V4_RESERVED: &[&str] = &[
            "0.0.0.0/8",
            "100.64.0.0/10",
            "192.0.0.0/24",
            "198.18.0.0/15",
            "240.0.0.0/4",
        ];
        const V6_RESERVED: &[&str] = &[
            "fc00::/7",
            "fe80::/10",
            "fec0::/10",
            "100::/64",
            "2001:db8::/32",
            "2001::/32",
        ];
        let in_any = |list: &[&str]| {
            list.iter()
                .filter_map(|n| n.parse::<IpNet>().ok())
                .any(|net| net.contains(&ip))
        };
        match ip {
            IpAddr::V4(v4) => {
                v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || in_any(V4_RESERVED)
            }
            IpAddr::V6(v6) => {
                if let Some(mapped) = v6.to_ipv4_mapped() {
                    return Self::is_forbidden_ip(IpAddr::V4(mapped), allow_loopback_for_tests);
                }
                let segs = v6.segments();
                let embedded_low = Ipv4Addr::new(
                    (segs[6] >> 8) as u8,
                    segs[6] as u8,
                    (segs[7] >> 8) as u8,
                    segs[7] as u8,
                );
                if (segs[..6] == [0, 0, 0, 0, 0, 0] && !v6.is_loopback() && !v6.is_unspecified())
                    || segs[..6] == [0x64, 0xff9b, 0, 0, 0, 0]
                {
                    return Self::is_forbidden_ip(
                        IpAddr::V4(embedded_low),
                        allow_loopback_for_tests,
                    );
                }
                if segs[0] == 0x2002 {
                    let v4 = Ipv4Addr::new(
                        (segs[1] >> 8) as u8,
                        segs[1] as u8,
                        (segs[2] >> 8) as u8,
                        segs[2] as u8,
                    );
                    return Self::is_forbidden_ip(IpAddr::V4(v4), allow_loopback_for_tests);
                }
                v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || in_any(V6_RESERVED)
            }
        }
    }

    /// Validates a URL against scheme, userinfo, allow-list, and IP literal rules before DNS/connect.
    pub fn validate_url(
        &self,
        raw_url: &str,
        extra_allowed: Option<&[String]>,
    ) -> ShakoResult<Url> {
        let parsed = Url::parse(raw_url)
            .map_err(|_| ShakoError::SsrfBlocked("invalid outbound URL".into()))?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(ShakoError::SsrfBlocked(
                "outbound URL must not contain embedded credentials".into(),
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| ShakoError::SsrfBlocked("outbound URL has no host".into()))?
            .to_ascii_lowercase();

        let in_cluster = self.is_in_cluster_host(&host);
        let is_test_loopback =
            self.policy.allow_loopback_for_tests && (host == "127.0.0.1" || host == "localhost");
        if parsed.scheme() != "https"
            && !((in_cluster || is_test_loopback) && parsed.scheme() == "http")
        {
            return Err(ShakoError::SsrfBlocked(format!(
                "outbound scheme '{}' is refused; https required",
                parsed.scheme()
            )));
        }

        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
            && !in_cluster
            && Self::is_forbidden_ip(ip, self.policy.allow_loopback_for_tests)
        {
            return Err(ShakoError::SsrfBlocked(format!(
                "outbound IP {ip} is private, loopback, or metadata and refused by SSRF guard"
            )));
        }

        if let Some(allowed) = extra_allowed.or(self.policy.allowed_hosts.as_deref())
            && !in_cluster
            && !allowed.iter().any(|d| d.eq_ignore_ascii_case(&host))
        {
            return Err(ShakoError::SsrfBlocked(format!(
                "host '{host}' is not in the outbound allow-list"
            )));
        }

        Ok(parsed)
    }

    async fn read_capped(mut resp: reqwest::Response) -> ShakoResult<Vec<u8>> {
        if resp
            .content_length()
            .is_some_and(|n| n > MAX_OUTBOUND_RESPONSE_BYTES as u64)
        {
            return Err(ShakoError::SsrfBlocked(
                "upstream response exceeds 1 MiB cap".into(),
            ));
        }
        let mut out = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| ShakoError::SsrfBlocked(e.without_url().to_string()))?
        {
            if out.len().saturating_add(chunk.len()) > MAX_OUTBOUND_RESPONSE_BYTES {
                return Err(ShakoError::SsrfBlocked(
                    "upstream response exceeds 1 MiB cap".into(),
                ));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    /// Sends an HTTP request through the SSRF guard with DNS resolution pinning, no redirects,
    /// 5 s timeout, and 1 MiB response body cap.
    pub async fn request(
        &self,
        method: reqwest::Method,
        raw_url: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
        extra_allowed: Option<&[String]>,
    ) -> ShakoResult<(u16, Vec<u8>)> {
        let url = self.validate_url(raw_url, extra_allowed)?;
        if let Some(handler) = &self.policy.mock_handler
            && let Some((status, resp_body)) =
                handler(method.as_str(), url.as_str(), headers, &body)
        {
            if (300..400).contains(&status) {
                return Err(ShakoError::SsrfBlocked(format!(
                    "upstream redirect ({status}) refused by SSRF guard"
                )));
            }
            if resp_body.len() > MAX_OUTBOUND_RESPONSE_BYTES {
                return Err(ShakoError::SsrfBlocked(
                    "upstream response exceeds 1 MiB cap".into(),
                ));
            }
            return Ok((status, resp_body));
        }
        if let Some((status, resp_body)) =
            self.policy.mock_responses.read().get(url.as_str()).cloned()
        {
            if (300..400).contains(&status) {
                return Err(ShakoError::SsrfBlocked(format!(
                    "upstream redirect ({status}) refused by SSRF guard"
                )));
            }
            if resp_body.len() > MAX_OUTBOUND_RESPONSE_BYTES {
                return Err(ShakoError::SsrfBlocked(
                    "upstream response exceeds 1 MiB cap".into(),
                ));
            }
            return Ok((status, resp_body));
        }

        let host = url.host_str().unwrap_or_default().to_owned();
        let port = url.port_or_known_default().unwrap_or(443);
        let in_cluster = self.is_in_cluster_host(&host);

        let addrs: Vec<SocketAddr> = tokio::time::timeout(
            OUTBOUND_TIMEOUT,
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| ShakoError::SsrfBlocked("DNS resolution timed out".into()))?
        .map_err(|e| ShakoError::SsrfBlocked(format!("DNS resolution failed: {e}")))?
        .collect();

        let Some(&first_addr) = addrs.first() else {
            return Err(ShakoError::SsrfBlocked("DNS returned no addresses".into()));
        };
        for addr in &addrs {
            if !in_cluster && Self::is_forbidden_ip(addr.ip(), self.policy.allow_loopback_for_tests)
            {
                return Err(ShakoError::SsrfBlocked(format!(
                    "resolved address {} is private, loopback, or metadata",
                    addr.ip()
                )));
            }
        }

        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(OUTBOUND_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&host, first_addr)
            .build()
            .map_err(|e| ShakoError::SsrfBlocked(e.to_string()))?;

        let mut req = client.request(method, url);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        if !body.is_empty() {
            req = req.body(body);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ShakoError::SsrfBlocked(e.without_url().to_string()))?;
        let status = resp.status();
        if status.is_redirection() {
            return Err(ShakoError::SsrfBlocked(format!(
                "upstream redirect ({}) refused by SSRF guard",
                status.as_u16()
            )));
        }
        let bytes = Self::read_capped(resp).await?;
        Ok((status.as_u16(), bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn outbound_guard_blocks_metadata_private_ranges_redirects_and_large_bodies() {
        let guard = OutboundGuard::new(OutboundPolicy {
            allowed_hosts: Some(vec![
                "github.com".into(),
                "169.254.169.254".into(),
                "127.0.0.1".into(),
            ]),
            allow_loopback_for_tests: false,
            ..OutboundPolicy::default()
        });

        // T3: metadata IP, loopback, private IP, IPv4-mapped IPv6, NAT64, userinfo, http scheme
        for bad_url in [
            "http://169.254.169.254/latest/meta-data/",
            "https://169.254.169.254/latest/meta-data/",
            "https://127.0.0.1/hook",
            "https://10.0.0.1/hook",
            "https://[::ffff:169.254.169.254]/hook",
            "https://[64:ff9b::a9fe:a9fe]/hook",
            "http://github.com/org/repo.git",
            "https://user:secret@github.com/org/repo.git",
            "https://evil.example.com/hook",
        ] {
            assert!(
                guard.validate_url(bad_url, None).is_err(),
                "expected '{bad_url}' to be refused by SSRF guard"
            );
        }

        // No implicit in-cluster trust: the Kubernetes API and arbitrary cluster services are
        // refused unless the operator names the host.
        let default_guard = OutboundGuard::default();
        for bad_url in [
            "https://kubernetes.default.svc/api/v1/secrets",
            "http://kubernetes.default.svc/api/v1/secrets",
            "http://keycloak.auth.svc.cluster.local/admin",
            "https://apisix-admin.ingress.svc.cluster.local/apisix/admin/routes",
        ] {
            assert!(
                default_guard.validate_url(bad_url, None).is_err(),
                "expected '{bad_url}' to be refused without an explicit in-cluster host"
            );
        }
        let portal_only = OutboundGuard::new(OutboundPolicy {
            in_cluster_hosts: vec!["portal.jc.svc.cluster.local".into()],
            ..OutboundPolicy::default()
        });
        assert!(
            portal_only
                .validate_url(
                    "http://portal.jc.svc.cluster.local/api/v1/webhooks/gitea",
                    None
                )
                .is_ok()
        );
        assert!(
            portal_only
                .validate_url("http://keycloak.auth.svc.cluster.local/admin", None)
                .is_err()
        );

        // Redirects (302) and oversized responses (> 1 MiB) are refused
        guard.insert_mock("https://github.com/redirect", 302, Vec::new());
        assert!(
            guard
                .request(
                    reqwest::Method::GET,
                    "https://github.com/redirect",
                    &[],
                    Vec::new(),
                    None,
                )
                .await
                .is_err()
        );

        guard.insert_mock(
            "https://github.com/huge",
            200,
            vec![b'x'; MAX_OUTBOUND_RESPONSE_BYTES + 10],
        );
        assert!(
            guard
                .request(
                    reqwest::Method::GET,
                    "https://github.com/huge",
                    &[],
                    Vec::new(),
                    None,
                )
                .await
                .is_err()
        );
    }
}
