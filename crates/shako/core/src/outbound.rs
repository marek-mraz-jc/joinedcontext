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

    /// Serves one HTTP request on a loopback port: answers `status` with `body`, then hands the
    /// request it read (head and body) back.
    fn serve_once(status: u16, body: &'static [u8]) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!(
            "http://127.0.0.1:{}/hook",
            listener.local_addr().expect("addr").port()
        );
        let served = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("timeout");
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            let mut done = false;
            while !done {
                let n = stream.read(&mut buf).unwrap_or(0);
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen).to_ascii_lowercase();
                let complete = text.find("\r\n\r\n").is_some_and(|end| {
                    let length = text
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let chunked = text.contains("transfer-encoding: chunked");
                    (chunked && text.ends_with("0\r\n\r\n"))
                        || (!chunked && seen.len() >= end + 4 + length)
                });
                done = complete || n == 0;
            }
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).expect("answer");
            stream.write_all(body).expect("answer body");
            seen
        });
        (url, served)
    }

    /// An unnamed read-write file, as a mirror push spools its pack.
    fn spool() -> std::fs::File {
        let path = std::env::temp_dir().join(format!("shako-spool-{}", uuid::Uuid::now_v7()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("spool");
        std::fs::remove_file(&path).expect("unlink");
        file
    }

    fn loopback() -> OutboundGuard {
        OutboundGuard::new(OutboundPolicy {
            allowed_hosts: Some(vec!["127.0.0.1".into()]),
            allow_loopback_for_tests: true,
            ..OutboundPolicy::default()
        })
    }

    #[tokio::test]
    async fn a_request_reaches_an_allowed_loopback_server_with_its_body_and_headers() {
        let (url, served) = serve_once(201, b"made");
        let headers = [("X-Test".to_owned(), "yes".to_owned())];
        let (status, body) = loopback()
            .request(Method::POST, &url, &headers, b"payload".to_vec(), None)
            .await
            .expect("sent");
        assert_eq!((status, body.as_slice()), (201, &b"made"[..]));
        let seen = String::from_utf8_lossy(&served.join().expect("server")).into_owned();
        assert!(seen.starts_with("POST /hook HTTP/1.1"), "{seen}");
        assert!(seen.to_ascii_lowercase().contains("x-test: yes"), "{seen}");
        assert!(seen.ends_with("payload"), "{seen}");

        // A spooled file goes from its start, whatever was read of it before.
        let (url, served) = serve_once(200, b"");
        let mut file = spool();
        std::io::Write::write_all(&mut file, b"spooled body").expect("write");
        let (status, _) = loopback()
            .send(Method::PUT, &url, &[], OutboundBody::File(file), None)
            .await
            .expect("sent");
        assert_eq!(status, 200);
        assert!(String::from_utf8_lossy(&served.join().expect("server")).contains("spooled body"));
    }

    #[tokio::test]
    async fn a_refused_connection_is_reported_as_blocked() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let err = loopback()
            .request(
                Method::GET,
                &format!("http://127.0.0.1:{port}/x"),
                &[],
                Vec::new(),
                None,
            )
            .await
            .expect_err("nothing listens");
        assert!(matches!(err, ShakoError::SsrfBlocked(_)), "{err}");
    }

    /// A spool that cannot rewind (a pipe) is a storage error, sent nowhere and handed to no mock.
    #[tokio::test]
    async fn a_spool_that_cannot_rewind_is_a_storage_error() {
        fn pipe_spool() -> std::fs::File {
            let (reader, _writer) = std::io::pipe().expect("pipe");
            std::fs::File::from(std::os::fd::OwnedFd::from(reader))
        }
        let handler: MockOutboundHandler =
            Arc::new(|_: &str, _: &str, _: &[(String, String)], _: &[u8]| Some((200, Vec::new())));
        let mocked = OutboundGuard::new(OutboundPolicy {
            mock_handler: Some(handler),
            ..OutboundPolicy::default()
        });
        let guards = [
            (loopback(), "http://127.0.0.1:9/x".to_owned()),
            (mocked, "https://github.com/echo".to_owned()),
        ];
        for (guard, url) in guards {
            let err = guard
                .send(
                    Method::PUT,
                    &url,
                    &[],
                    OutboundBody::File(pipe_spool()),
                    None,
                )
                .await
                .expect_err("a pipe does not rewind");
            assert!(
                matches!(&err, ShakoError::Storage(m) if m.contains("outbound spool")),
                "{err}"
            );
        }
    }

    #[tokio::test]
    async fn a_mock_handler_reads_a_spooled_body_and_falls_back_to_canned_answers() {
        let handler: MockOutboundHandler = Arc::new(
            |method: &str, url: &str, _: &[(String, String)], body: &[u8]| {
                url.ends_with("/echo")
                    .then(|| (200, [method.as_bytes(), b" ", body].concat()))
            },
        );
        let guard = OutboundGuard::new(OutboundPolicy {
            mock_handler: Some(handler),
            ..OutboundPolicy::default()
        });
        let mut file = spool();
        std::io::Write::write_all(&mut file, b"from disk").expect("write");
        let (_, echoed) = guard
            .send(
                Method::POST,
                "https://github.com/echo",
                &[],
                OutboundBody::File(file),
                None,
            )
            .await
            .expect("echo");
        assert_eq!(echoed, b"POST from disk");
        guard.insert_mock("https://github.com/canned", 204, b"".to_vec());
        let (status, _) = guard
            .request(
                Method::GET,
                "https://github.com/canned",
                &[],
                Vec::new(),
                None,
            )
            .await
            .expect("canned");
        assert_eq!(status, 204);
    }

    #[test]
    fn a_url_is_refused_when_unparseable_hostless_or_outside_a_given_list() {
        let guard = OutboundGuard::default();
        let says = |url: &str, extra: Option<&[String]>| {
            guard
                .validate_url(url, extra)
                .expect_err(url)
                .public_detail()
        };
        assert_eq!(says("not a url", None), "invalid outbound URL");
        assert_eq!(says("data:text/plain,hi", None), "outbound URL has no host");
        let extra = ["mirror.example".to_owned()];
        assert!(
            guard
                .validate_url("https://mirror.example/r", Some(&extra))
                .is_ok()
        );
        assert!(
            says("https://github.com/r", Some(&extra)).contains("not in the outbound allow-list")
        );
        // A dotted in-cluster entry names every host under it.
        let cluster = OutboundGuard::new(OutboundPolicy {
            in_cluster_hosts: vec![".jc.svc.cluster.local".into()],
            ..OutboundPolicy::default()
        });
        assert!(
            cluster
                .validate_url("http://Portal.JC.svc.cluster.local/x", None)
                .is_ok()
        );
        assert!(
            cluster
                .validate_url("http://portal.other.svc.cluster.local/x", None)
                .is_err()
        );
        assert!(
            cluster
                .validate_url("http://169.254.169.254.jc.svc.cluster.local/x", None)
                .is_ok()
        );
    }
}
