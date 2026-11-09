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
use kaisatsu_core::{KaisatsuError, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use url::Url;

pub const MAX_OUTBOUND_BYTES: usize = 1024 * 1024;
pub const OUTBOUND_TIMEOUT: Duration = Duration::from_secs(5);
/// The most cluster JWKS URLs the cache holds.
const CLUSTER_JWKS_ENTRIES: usize = 16;

/// The pod's own credentials for the Kubernetes API server, read at each fetch (a projected
/// ServiceAccount token rotates): its token, sent only to an in-cluster host, and the cluster CA,
/// the one root such a fetch trusts.
#[derive(Debug, Clone)]
pub struct ClusterCredentials {
    pub token: PathBuf,
    pub ca: PathBuf,
    /// How long a cluster JWKS answers from the cache before it is read again.
    pub jwks_ttl: Duration,
    /// The least time between two reads of one cluster JWKS: a token naming a `kid` the cached
    /// set lacks reads it again (key rotation), a stream of made-up `kid`s one read per window.
    pub jwks_min_refetch: Duration,
}

impl Default for ClusterCredentials {
    fn default() -> Self {
        let dir = PathBuf::from("/var/run/secrets/kubernetes.io/serviceaccount");
        Self {
            token: dir.join("token"),
            ca: dir.join("ca.crt"),
            jwks_ttl: Duration::from_secs(300),
            jwks_min_refetch: Duration::from_secs(10),
        }
    }
}

/// A cluster JWKS read at `read`.
#[derive(Debug, Clone)]
struct CachedJwks {
    read: Instant,
    jwks: serde_json::Value,
}

/// Test-only canned answers by URL: `(status, body)`.
pub type MockResponses = Arc<RwLock<HashMap<String, (u16, Vec<u8>)>>>;

#[derive(Debug, Clone)]
pub struct OutboundPolicy {
    /// Allowed in-cluster HTTP/HTTPS host suffixes or exact hosts (e.g. `.svc.cluster.local`,
    /// `kubernetes.default.svc.cluster.local`).
    pub in_cluster_hosts: Vec<String>,
    /// Optional allow-list of public hosts for CIMD fetches (`claude.ai`, `vscode.dev`).
    pub allowed_hosts: Option<Vec<String>>,
    /// Test-only mock responses keyed by URL so unit/e2e tests can run offline.
    pub mock_responses: MockResponses,
    /// What a cluster JWKS fetch ([`OutboundGuard::cluster_jwks`]) authenticates and trusts with.
    pub cluster: ClusterCredentials,
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
            cluster: ClusterCredentials::default(),
        }
    }
}

/// Test-only record of the forms posted to a mocked URL, by URL.
pub type MockPosts = Arc<RwLock<HashMap<String, Vec<Vec<(String, String)>>>>>;

#[derive(Clone, Default)]
pub struct OutboundGuard {
    pub policy: OutboundPolicy,
    mock_posts: MockPosts,
    cluster_jwks: Arc<RwLock<HashMap<String, CachedJwks>>>,
}

impl OutboundGuard {
    #[must_use]
    pub fn new(policy: OutboundPolicy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    pub fn insert_mock(&self, url: impl Into<String>, status: u16, body: impl Into<Vec<u8>>) {
        self.policy
            .mock_responses
            .write()
            .insert(url.into(), (status, body.into()));
    }

    /// The forms [`Self::post_form`] sent to the mocked `url`, oldest first (tests read what a
    /// back-channel logout carried).
    pub fn posted_forms(&self, url: &str) -> Vec<Vec<(String, String)>> {
        self.mock_posts.read().get(url).cloned().unwrap_or_default()
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
    /// unspecified, or documentation/reserved (W12). IPv6 forms that embed an IPv4 address
    /// (mapped, compatible, NAT64 `64:ff9b::/96`, 6to4 `2002::/16`) are judged by that address.
    #[must_use]
    pub fn is_forbidden_ip(ip: IpAddr) -> bool {
        const V4_RESERVED: &[&str] = &[
            "0.0.0.0/8",     // "this network"
            "100.64.0.0/10", // CGNAT
            "192.0.0.0/24",  // IETF protocol assignments
            "198.18.0.0/15", // benchmarking
            "240.0.0.0/4",   // reserved, includes broadcast
        ];
        const V6_RESERVED: &[&str] = &[
            "fc00::/7",      // unique local
            "fe80::/10",     // link-local
            "fec0::/10",     // site-local (deprecated, still routed by some stacks)
            "100::/64",      // discard
            "2001:db8::/32", // documentation
            "2001::/32",     // Teredo
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
                    return Self::is_forbidden_ip(IpAddr::V4(mapped));
                }
                let segs = v6.segments();
                let embedded_low = std::net::Ipv4Addr::new(
                    (segs[6] >> 8) as u8,
                    segs[6] as u8,
                    (segs[7] >> 8) as u8,
                    segs[7] as u8,
                );
                // ::a.b.c.d (IPv4-compatible) and 64:ff9b::a.b.c.d (NAT64)
                if segs[..6] == [0, 0, 0, 0, 0, 0] && !v6.is_loopback() && !v6.is_unspecified()
                    || segs[..6] == [0x64, 0xff9b, 0, 0, 0, 0]
                {
                    return Self::is_forbidden_ip(IpAddr::V4(embedded_low));
                }
                // 2002:AABB:CCDD::/48 (6to4)
                if segs[0] == 0x2002 {
                    let v4 = std::net::Ipv4Addr::new(
                        (segs[1] >> 8) as u8,
                        segs[1] as u8,
                        (segs[2] >> 8) as u8,
                        segs[2] as u8,
                    );
                    return Self::is_forbidden_ip(IpAddr::V4(v4));
                }
                v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || in_any(V6_RESERVED)
            }
        }
    }

    /// Validates a URL against W12's scheme, host allow-list, and IP literal rules before any
    /// network activity.
    pub fn validate_url(&self, raw_url: &str, allowed_domains: Option<&[String]>) -> Result<Url> {
        let parsed = Url::parse(raw_url)
            .map_err(|_| KaisatsuError::OutboundRefused("invalid outbound URL".into()))?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(KaisatsuError::OutboundRefused(
                "outbound URL must not contain userinfo".into(),
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| KaisatsuError::OutboundRefused("outbound URL has no host".into()))?
            .to_ascii_lowercase();

        let in_cluster = self.is_in_cluster_host(&host);
        if parsed.scheme() != "https" && !(in_cluster && parsed.scheme() == "http") {
            return Err(KaisatsuError::OutboundRefused(format!(
                "outbound scheme '{}' is refused; https required",
                parsed.scheme()
            )));
        }

        if let Some(domains) = allowed_domains.or(self.policy.allowed_hosts.as_deref())
            && !domains.iter().any(|d| d.eq_ignore_ascii_case(&host))
        {
            return Err(KaisatsuError::OutboundRefused(format!(
                "host '{host}' is not in the outbound allow-list"
            )));
        }

        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
            && !in_cluster
            && Self::is_forbidden_ip(ip)
        {
            return Err(KaisatsuError::OutboundRefused(format!(
                "outbound IP {ip} is private/loopback/metadata and refused by SSRF guard"
            )));
        }
        Ok(parsed)
    }

    /// Reads at most [`MAX_OUTBOUND_BYTES`] of `resp`, refusing before buffering more (a declared
    /// `Content-Length` over the cap is refused without reading the body at all).
    async fn read_capped(mut resp: reqwest::Response) -> Result<Vec<u8>> {
        let too_big =
            || KaisatsuError::OutboundRefused("upstream response exceeds 1 MiB cap".into());
        if resp
            .content_length()
            .is_some_and(|n| n > MAX_OUTBOUND_BYTES as u64)
        {
            return Err(too_big());
        }
        let mut out = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| KaisatsuError::OutboundRefused(e.without_url().to_string()))?
        {
            if out.len() + chunk.len() > MAX_OUTBOUND_BYTES {
                return Err(too_big());
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    /// Resolves DNS, checks every resolved address against `is_forbidden_ip`, pins the address on
    /// `reqwest`, enforces the 5 s timeout, refuses redirects, and caps the body at 1 MiB (W12).
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        raw_url: &str,
        allowed_domains: Option<&[String]>,
        bearer_token: Option<&str>,
    ) -> Result<T> {
        self.fetch_json(raw_url, allowed_domains, bearer_token, None)
            .await
    }

    /// The JWKS at `raw_url` on an in-cluster host (the Kubernetes API server's
    /// `/openid/v1/jwks`), read as Keycloak's kubernetes identity provider reads it: with the
    /// pod's ServiceAccount token as bearer and the cluster CA as the only root, each file used
    /// when it exists. Cached for [`ClusterCredentials::jwks_ttl`]; `refresh` (a token whose
    /// `kid` the cached set lacks) reads it again, at most once per
    /// [`ClusterCredentials::jwks_min_refetch`]. A URL on any other host is refused before a
    /// request: the token never leaves the cluster.
    pub async fn cluster_jwks(&self, raw_url: &str, refresh: bool) -> Result<serde_json::Value> {
        let url = self.validate_url(raw_url, None)?;
        let host = url.host_str().unwrap_or_default();
        if !self.is_in_cluster_host(host) {
            return Err(KaisatsuError::OutboundRefused(format!(
                "host '{host}' is not an in-cluster host for a cluster JWKS"
            )));
        }
        if let Some(hit) = self.cluster_jwks.read().get(url.as_str()) {
            let age = hit.read.elapsed();
            let cluster = &self.policy.cluster;
            if age < cluster.jwks_ttl && (!refresh || age < cluster.jwks_min_refetch) {
                return Ok(hit.jwks.clone());
            }
        }
        let read = |path: &std::path::Path| std::fs::read(path).ok();
        let token = read(&self.policy.cluster.token)
            .map(|t| String::from_utf8_lossy(&t).trim().to_owned())
            .filter(|t| !t.is_empty());
        let ca = read(&self.policy.cluster.ca);
        let jwks: serde_json::Value = self
            .fetch_json(url.as_str(), None, token.as_deref(), ca.as_deref())
            .await?;
        // Only a set with keys is kept: an empty or broken answer is read again next time.
        if !jwks["keys"].as_array().is_some_and(|keys| !keys.is_empty()) {
            return Ok(jwks);
        }
        let mut cache = self.cluster_jwks.write();
        if cache.len() >= CLUSTER_JWKS_ENTRIES && !cache.contains_key(url.as_str()) {
            cache.clear();
        }
        cache.insert(
            url.as_str().to_owned(),
            CachedJwks {
                read: Instant::now(),
                jwks: jwks.clone(),
            },
        );
        Ok(jwks)
    }

    /// [`Self::get_json`], trusting only the PEM roots of `ca_pem` when given.
    async fn fetch_json<T: serde::de::DeserializeOwned>(
        &self,
        raw_url: &str,
        allowed_domains: Option<&[String]>,
        bearer_token: Option<&str>,
        ca_pem: Option<&[u8]>,
    ) -> Result<T> {
        let url = self.validate_url(raw_url, allowed_domains)?;
        if let Some((status, body)) = self.policy.mock_responses.read().get(url.as_str()).cloned() {
            if !(200..300).contains(&status) {
                return Err(KaisatsuError::OutboundRefused(format!(
                    "upstream answered {status}"
                )));
            }
            if body.len() > MAX_OUTBOUND_BYTES {
                return Err(KaisatsuError::OutboundRefused(
                    "upstream response exceeds 1 MiB cap".into(),
                ));
            }
            return serde_json::from_slice(&body)
                .map_err(|_| KaisatsuError::OutboundRefused("invalid JSON from upstream".into()));
        }

        let host = url.host_str().unwrap_or_default().to_owned();
        let port = url.port_or_known_default().unwrap_or(443);
        let in_cluster = self.is_in_cluster_host(&host);

        let addrs: Vec<SocketAddr> = tokio::time::timeout(
            OUTBOUND_TIMEOUT,
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| KaisatsuError::OutboundRefused("DNS resolution timed out".into()))?
        .map_err(|e| KaisatsuError::OutboundRefused(format!("DNS resolution failed: {e}")))?
        .collect();

        if addrs.is_empty() {
            return Err(KaisatsuError::OutboundRefused(
                "DNS returned no addresses".into(),
            ));
        }
        for addr in &addrs {
            if !in_cluster && Self::is_forbidden_ip(addr.ip()) {
                return Err(KaisatsuError::OutboundRefused(format!(
                    "resolved address {} is private/loopback/metadata",
                    addr.ip()
                )));
            }
        }

        // Pin the resolved address so DNS rebinding cannot switch the target on connect (W12).
        let mut builder = reqwest::Client::builder()
            .timeout(OUTBOUND_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .resolve(&host, addrs[0]);
        if let Some(pem) = ca_pem {
            let roots = reqwest::Certificate::from_pem_bundle(pem)
                .map_err(|_| KaisatsuError::OutboundRefused("unreadable cluster CA".into()))?;
            builder = builder.tls_built_in_root_certs(false);
            for root in roots {
                builder = builder.add_root_certificate(root);
            }
        }
        let client = builder
            .build()
            .map_err(|e| KaisatsuError::OutboundRefused(e.to_string()))?;

        let mut req = client.get(url);
        if let Some(token) = bearer_token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| KaisatsuError::OutboundRefused(e.without_url().to_string()))?;
        if !resp.status().is_success() {
            return Err(KaisatsuError::OutboundRefused(format!(
                "upstream answered {}",
                resp.status()
            )));
        }
        let bytes = Self::read_capped(resp).await?;
        serde_json::from_slice(&bytes)
            .map_err(|_| KaisatsuError::OutboundRefused("invalid JSON from upstream".into()))
    }

    /// Sends a form POST (used for back-channel logout and upstream OIDC token exchange) through
    /// the W12 outbound guard.
    pub async fn post_form(&self, raw_url: &str, form: &[(&str, &str)]) -> Result<(u16, Vec<u8>)> {
        let url = self.validate_url(raw_url, None)?;
        if let Some((status, body)) = self.policy.mock_responses.read().get(url.as_str()).cloned() {
            self.mock_posts
                .write()
                .entry(url.as_str().to_owned())
                .or_default()
                .push(
                    form.iter()
                        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                        .collect(),
                );
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
        .map_err(|_| KaisatsuError::OutboundRefused("DNS resolution timed out".into()))?
        .map_err(|e| KaisatsuError::OutboundRefused(format!("DNS resolution failed: {e}")))?
        .collect();

        if addrs.is_empty() {
            return Err(KaisatsuError::OutboundRefused(
                "DNS returned no addresses".into(),
            ));
        }
        for addr in &addrs {
            if !in_cluster && Self::is_forbidden_ip(addr.ip()) {
                return Err(KaisatsuError::OutboundRefused(format!(
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
            .map_err(|e| KaisatsuError::OutboundRefused(e.to_string()))?;

        let resp = client
            .post(url)
            .form(form)
            .send()
            .await
            .map_err(|e| KaisatsuError::OutboundRefused(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let bytes = Self::read_capped(resp).await?;
        Ok((status, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pod's ServiceAccount token goes to an in-cluster host only: any other JWKS URL is
    /// refused before a lookup, with a public host or a cluster-looking name alike.
    #[tokio::test]
    async fn a_cluster_jwks_is_read_from_an_in_cluster_host_only() {
        let guard = OutboundGuard::default();
        for url in [
            "https://jwks.example.com/openid/v1/jwks",
            "https://kubernetes.default.svc.cluster.local.example.com/openid/v1/jwks",
        ] {
            let why = guard
                .cluster_jwks(url, false)
                .await
                .unwrap_err()
                .to_string();
            assert!(why.contains("not an in-cluster host"), "{url}: {why}");
        }
    }

    #[test]
    fn public_addresses_stay_allowed() {
        for ok in [
            "1.1.1.1",
            "2606:4700:4700::1111",
            "64:ff9b::808:808",
            "2002:808:808::1",
        ] {
            assert!(
                !OutboundGuard::is_forbidden_ip(ok.parse().unwrap()),
                "{ok} is public and must stay reachable"
            );
        }
    }

    #[tokio::test]
    async fn mocked_upstream_errors_and_oversized_bodies_are_refused() {
        let guard = OutboundGuard::default();
        guard.insert_mock("https://idp.example/jwks", 500, b"{}".to_vec());
        assert!(
            guard
                .get_json::<serde_json::Value>("https://idp.example/jwks", None, None)
                .await
                .is_err()
        );
        guard.insert_mock(
            "https://idp.example/big",
            200,
            vec![b' '; MAX_OUTBOUND_BYTES + 1],
        );
        assert!(
            guard
                .get_json::<serde_json::Value>("https://idp.example/big", None, None)
                .await
                .is_err()
        );
        guard.insert_mock("https://idp.example/ok", 200, br#"{"keys":[]}"#.to_vec());
        let v: serde_json::Value = guard
            .get_json("https://idp.example/ok", None, None)
            .await
            .unwrap();
        assert_eq!(v["keys"], serde_json::json!([]));
        let allow = vec!["claude.ai".to_owned()];
        assert!(
            guard
                .validate_url("https://evil.example/x", Some(&allow))
                .is_err()
        );
    }

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
            "https://0.0.0.0/",
            "https://0.1.2.3/",
            "https://198.18.0.1/",
            "https://240.0.0.1/",
            "https://100.64.0.1/",
            "https://[::ffff:169.254.169.254]/",
            "https://[::169.254.169.254]/",
            "https://[64:ff9b::a9fe:a9fe]/",
            "https://[2002:a9fe:a9fe::1]/",
            "https://[fec0::1]/",
            "https://[fd00::1]/",
            "https://[fe80::1]/",
        ] {
            assert!(
                guard.validate_url(bad, None).is_err(),
                "expected {bad} to be refused by OutboundGuard"
            );
        }
        // In-cluster HTTP callback for Portal back-channel logout (RI line 790) is permitted
        assert!(
            guard
                .validate_url(
                    "http://portal.portal.svc.cluster.local:8080/api/v1/auth/backchannel-logout",
                    None
                )
                .is_ok()
        );
    }
}
