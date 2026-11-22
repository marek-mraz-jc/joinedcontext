#![forbid(unsafe_code)]
//! Outbound SSRF guard for push mirrors and webhooks (S14, S15, S21, Threat T3).
//!
//! Enforces:
//! - `https` only (except configured in-cluster hosts or explicit test loopback).
//! - Mandatory host allow-list where configured.
//! - No userinfo (`user:pass@`) in URLs.
//! - Resolution, the address classes, pinning, no proxy, no redirect and the caps are `jc-http`'s
//!   (SEC-04); this module keeps shako's allow-list and in-cluster admission.

use crate::error::{ShakoError, ShakoResult};
use jc_http::Method;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub const MAX_OUTBOUND_RESPONSE_BYTES: usize = 1024 * 1024;
pub const OUTBOUND_TIMEOUT: Duration = Duration::from_secs(5);
/// A mirror's pack travels within this.
pub const OUTBOUND_UPLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What an outbound request sends.
pub enum OutboundBody {
    Bytes(Vec<u8>),
    /// A spooled file, streamed from its start.
    File(std::fs::File),
}

pub type MockOutboundHandler =
    Arc<dyn Fn(&str, &str, &[(String, String)], &[u8]) -> Option<(u16, Vec<u8>)> + Send + Sync>;

/// Canned responses keyed by URL, consulted before any network call (tests only).
pub type MockResponses = Arc<RwLock<HashMap<String, (u16, Vec<u8>)>>>;

#[derive(Clone)]
pub struct OutboundPolicy {
    pub in_cluster_hosts: Vec<String>,
    pub allowed_hosts: Option<Vec<String>>,
    /// Test-only allowance for `127.0.0.1` / `localhost` so local integration tests can run.
    pub allow_loopback_for_tests: bool,
    pub mock_responses: MockResponses,
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

    /// An in-cluster host skips the address classes; nothing opens cloud metadata.
    fn admits(&self, in_cluster: bool, ip: IpAddr) -> bool {
        !jc_http::is_cloud_metadata(ip)
            && (in_cluster
                || jc_http::check_address(ip, self.policy.allow_loopback_for_tests).is_ok())
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
            && !self.admits(in_cluster, ip)
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

    /// Sends an HTTP request through the SSRF guard with DNS resolution pinning, no redirects,
    /// 5 s timeout, and 1 MiB response body cap.
    pub async fn request(
        &self,
        method: Method,
        raw_url: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
        extra_allowed: Option<&[String]>,
    ) -> ShakoResult<(u16, Vec<u8>)> {
        let body = OutboundBody::Bytes(body);
        self.send(method, raw_url, headers, body, extra_allowed)
            .await
    }

    /// [`Self::request`] with a body that may be a spooled file, which gets
    /// [`OUTBOUND_UPLOAD_TIMEOUT`].
    pub async fn send(
        &self,
        method: Method,
        raw_url: &str,
        headers: &[(String, String)],
        body: OutboundBody,
        extra_allowed: Option<&[String]>,
    ) -> ShakoResult<(u16, Vec<u8>)> {
        let blocked = |e: &dyn std::fmt::Display| ShakoError::SsrfBlocked(e.to_string());
        let url = self.validate_url(raw_url, extra_allowed)?;
        if let Some((status, resp_body)) = self.mocked(&method, &url, headers, &body)? {
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

        let in_cluster = self.is_in_cluster_host(url.host_str().unwrap_or_default());
        let (body, timeout) = match body {
            OutboundBody::Bytes(bytes) => (
                (!bytes.is_empty()).then(|| jc_http::Body::from(bytes)),
                OUTBOUND_TIMEOUT,
            ),
            OutboundBody::File(file) => {
                let mut file = tokio::fs::File::from_std(file);
                tokio::io::AsyncSeekExt::rewind(&mut file)
                    .await
                    .map_err(|e| ShakoError::Storage(format!("outbound spool: {e}")))?;
                (Some(jc_http::Body::from(file)), OUTBOUND_UPLOAD_TIMEOUT)
            }
        };
        let request = jc_http::Request {
            method,
            // validate_url let plain http through for the hosts it names alone.
            plain_http: url.scheme() == "http",
            url,
            headers: headers.to_vec(),
            body,
            timeout,
            max_response_bytes: MAX_OUTBOUND_RESPONSE_BYTES,
        };
        let response = jc_http::send(request, |ip| self.admits(in_cluster, ip))
            .await
            .map_err(|e| blocked(&e))?;
        Ok((response.status, response.body))
    }

    fn mocked(
        &self,
        method: &Method,
        url: &Url,
        headers: &[(String, String)],
        body: &OutboundBody,
    ) -> ShakoResult<Option<(u16, Vec<u8>)>> {
        if let Some(handler) = &self.policy.mock_handler {
            let mut read = Vec::new();
            let sent = match body {
                OutboundBody::Bytes(bytes) => bytes,
                OutboundBody::File(file) => {
                    use std::io::{Read as _, Seek as _};
                    let mut file = file;
                    file.rewind()
                        .and_then(|()| file.read_to_end(&mut read))
                        .map_err(|e| ShakoError::Storage(format!("outbound spool: {e}")))?;
                    &read
                }
            };
            if let Some(answer) = handler(method.as_str(), url.as_str(), headers, sent) {
                return Ok(Some(answer));
            }
        }
        Ok(self.policy.mock_responses.read().get(url.as_str()).cloned())
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
                    Method::GET,
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
                    Method::GET,
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
