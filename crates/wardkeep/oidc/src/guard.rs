#![forbid(unsafe_code)]
//! Outbound SSRF guard (W12):
//! - All outbound fetches (CIMD metadata, upstream OIDC JWKS/token, back-channel logout) go through
//!   this guard.
//! - `https` only (except configured in-cluster addresses such as `*.svc.cluster.local`).
//! - Host allow-list where configured.
//! - Resolved IP address checked against private, loopback, link-local, multicast, CGNAT, and cloud
//!   metadata (`169.254.169.254`) ranges unless the target is a configured in-cluster address or
//!   explicit test loopback.
//! - DNS answer pinned on the HTTP client, 5 s timeout, 1 MiB response body cap, no redirects followed.

use ipnet::IpNet;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use url::Url;
use wardkeep_core::{Result, WardkeepError};

pub const MAX_OUTBOUND_BYTES: usize = 1024 * 1024;
pub const OUTBOUND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct OutboundPolicy {
    /// Allowed in-cluster HTTP/HTTPS host suffixes or exact hosts (e.g. `.svc.cluster.local`,
    /// `kubernetes.default.svc.cluster.local`).
    pub in_cluster_hosts: Vec<String>,
    /// Optional allow-list of public hosts for CIMD fetches (`claude.ai`, `vscode.dev`).
    pub allowed_hosts: Option<Vec<String>>,
    /// Test-only mock responses keyed by URL so unit/e2e tests can run offline.
    pub mock_responses: Arc<RwLock<HashMap<String, (u16, Vec<u8>)>>>,
}

impl Default for OutboundPolicy {
    fn default() -> Self {
        Self {
            in_cluster_hosts: vec![
                ".svc.cluster.local".into(),
                "kubernetes.default.svc".into(),
                "kubernetes.default.svc.cluster.local".into(),
            ],
            allowed_hosts: None,
            mock_responses: Arc::new(RwLock::new(HashMap::new())),
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
    /// unspecified, or documentation/reserved (W12).
    #[must_use]
    pub fn is_forbidden_ip(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                    || v4.octets()[0] == 100 && (v4.octets()[1] & 0b1100_0000) == 64 // 100.64.0.0/10 CGNAT
                    || v4.octets() == [169, 254, 169, 254] // Cloud metadata service
            }
            IpAddr::V6(v6) => {
                if let Some(mapped) = v6.to_ipv4_mapped() {
                    return Self::is_forbidden_ip(IpAddr::V4(mapped));
                }
                let segs = v6.segments();
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (segs[0] & 0xfe00) == 0xfc00 // fc00::/7 Unique Local
                    || (segs[0] & 0xffc0) == 0xfe80 // fe80::/10 Link-Local
            }
        }
    }

    /// Validates a URL against W12's scheme, host allow-list, and IP literal rules before any
    /// network activity.
    pub fn validate_url(&self, raw_url: &str, allowed_domains: Option<&[String]>) -> Result<Url> {
        let parsed = Url::parse(raw_url)
            .map_err(|_| WardkeepError::OutboundRefused("invalid outbound URL".into()))?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(WardkeepError::OutboundRefused(
                "outbound URL must not contain userinfo".into(),
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| WardkeepError::OutboundRefused("outbound URL has no host".into()))?
            .to_ascii_lowercase();

        let in_cluster = self.is_in_cluster_host(&host);
        if parsed.scheme() != "https" && !(in_cluster && parsed.scheme() == "http") {
            return Err(WardkeepError::OutboundRefused(format!(
                "outbound scheme '{}' is refused; https required",
                parsed.scheme()
            )));
        }

        if let Some(domains) = allowed_domains.or(self.policy.allowed_hosts.as_deref()) {
            if !domains.iter().any(|d| d.eq_ignore_ascii_case(&host)) {
                return Err(WardkeepError::OutboundRefused(format!(
                    "host '{host}' is not in the outbound allow-list"
                )));
            }
        }

        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
            if !in_cluster && Self::is_forbidden_ip(ip) {
                return Err(WardkeepError::OutboundRefused(format!(
                    "outbound IP {ip} is private/loopback/metadata and refused by SSRF guard"
                )));
            }
        }
        let _ = IpNet::default();
        Ok(parsed)
    }

    /// Resolves DNS, checks every resolved address against `is_forbidden_ip`, pins the address on
    /// `reqwest`, enforces the 5 s timeout, refuses redirects, and caps the body at 1 MiB (W12).
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        raw_url: &str,
        allowed_domains: Option<&[String]>,
        bearer_token: Option<&str>,
    ) -> Result<T> {
        let url = self.validate_url(raw_url, allowed_domains)?;
        if let Some((status, body)) = self.policy.mock_responses.read().get(url.as_str()).cloned() {
            if !(200..300).contains(&status) {
                return Err(WardkeepError::OutboundRefused(format!(
                    "upstream answered {status}"
                )));
            }
            if body.len() > MAX_OUTBOUND_BYTES {
                return Err(WardkeepError::OutboundRefused(
                    "upstream response exceeds 1 MiB cap".into(),
                ));
            }
            return serde_json::from_slice(&body)
                .map_err(|_| WardkeepError::OutboundRefused("invalid JSON from upstream".into()));
        }

        let host = url.host_str().unwrap_or_default().to_owned();
        let port = url.port_or_known_default().unwrap_or(443);
        let in_cluster = self.is_in_cluster_host(&host);

        let addrs: Vec<SocketAddr> = tokio::time::timeout(
            OUTBOUND_TIMEOUT,
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| WardkeepError::OutboundRefused("DNS resolution timed out".into()))?
        .map_err(|e| WardkeepError::OutboundRefused(format!("DNS resolution failed: {e}")))?
        .collect();

        if addrs.is_empty() {
            return Err(WardkeepError::OutboundRefused("DNS returned no addresses".into()));
        }
        for addr in &addrs {
            if !in_cluster && Self::is_forbidden_ip(addr.ip()) {
                return Err(WardkeepError::OutboundRefused(format!(
                    "resolved address {} is private/loopback/metadata",
                    addr.ip()
                )));
            }
        }

        // Pin the resolved address so DNS rebinding cannot switch the target on connect (W12).
        let client = reqwest::Client::builder()
            .timeout(OUTBOUND_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&host, addrs[0])
            .build()
            .map_err(|e| WardkeepError::OutboundRefused(e.to_string()))?;

        let mut req = client.get(url);
        if let Some(token) = bearer_token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| WardkeepError::OutboundRefused(e.without_url().to_string()))?;
        if !resp.status().is_success() {
            return Err(WardkeepError::OutboundRefused(format!(
                "upstream answered {}",
                resp.status()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| WardkeepError::OutboundRefused(e.without_url().to_string()))?;
        if bytes.len() > MAX_OUTBOUND_BYTES {
            return Err(WardkeepError::OutboundRefused(
                "upstream response exceeds 1 MiB cap".into(),
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| WardkeepError::OutboundRefused("invalid JSON from upstream".into()))
    }

    /// Sends a form POST (used for back-channel logout and upstream OIDC token exchange) through
    /// the W12 outbound guard.
    pub async fn post_form(
        &self,
        raw_url: &str,
        form: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>)> {
        let url = self.validate_url(raw_url, None)?;
        if let Some((status, body)) = self.policy.mock_responses.read().get(url.as_str()).cloned() {
            return Ok((status, body));
        }
        let host = url.host_str().unwrap_or_default().to_owned();
        let port = url.port_or_known_default().unwrap_or(443);
        let in_cluster = self.is_in_cluster_host(&host);

        let addrs: Vec<SocketAddr> = tokio::time::timeout(
            OUTBOUND_TIMEOUT,
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| WardkeepError::OutboundRefused("DNS resolution timed out".into()))?
        .map_err(|e| WardkeepError::OutboundRefused(format!("DNS resolution failed: {e}")))?
        .collect();

        if addrs.is_empty() {
            return Err(WardkeepError::OutboundRefused("DNS returned no addresses".into()));
        }
        for addr in &addrs {
            if !in_cluster && Self::is_forbidden_ip(addr.ip()) {
                return Err(WardkeepError::OutboundRefused(format!(
                    "resolved address {} is private/loopback/metadata",
                    addr.ip()
                )));
            }
        }

        let client = reqwest::Client::builder()
            .timeout(OUTBOUND_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&host, addrs[0])
            .build()
            .map_err(|e| WardkeepError::OutboundRefused(e.to_string()))?;

        let resp = client
            .post(url)
            .form(form)
            .send()
            .await
            .map_err(|e| WardkeepError::OutboundRefused(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| WardkeepError::OutboundRefused(e.without_url().to_string()))?;
        if bytes.len() > MAX_OUTBOUND_BYTES {
            return Err(WardkeepError::OutboundRefused(
                "upstream response exceeds 1 MiB cap".into(),
            ));
        }
        Ok((status, bytes.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_metadata_private_loopback_and_http_urls() {
        let guard = OutboundGuard::default();
        for bad in [
            "https://169.254.169.254/latest/meta-data/",
            "https://127.0.0.1/oauth/metadata",
            "https://10.0.0.1/metadata",
            "https://192.168.1.1/metadata",
            "https://[::1]/metadata",
            "http://claude.ai/oauth/mcp-oauth-client-metadata",
            "https://user:pass@claude.ai/oauth/mcp-oauth-client-metadata",
        ] {
            assert!(
                guard.validate_url(bad, None).is_err(),
                "expected {bad} to be refused by OutboundGuard"
            );
        }
        // In-cluster HTTP callback for Portal back-channel logout (RI line 790) is permitted
        assert!(guard
            .validate_url(
                "http://portal.portal.svc.cluster.local:8080/api/v1/auth/backchannel-logout",
                None
            )
            .is_ok());
    }
}
