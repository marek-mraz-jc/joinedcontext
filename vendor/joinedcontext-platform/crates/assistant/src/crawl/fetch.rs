//! Guarded HTTP fetching with SSRF protection, manual redirects, and size caps (T-3052).

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, IF_MODIFIED_SINCE, IF_NONE_MATCH, LOCATION};
use reqwest::{Client, StatusCode};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use url::Url;

use super::CrawlPolicy;

/// Maximum number of manual redirect hops followed.
pub const MAX_REDIRECT_HOPS: usize = 5;

/// Outcome of a guarded GET request.
#[derive(Debug)]
pub enum FetchOutcome {
    /// Request succeeded and returned a complete response body.
    Success {
        /// The final URL reached after redirect hops.
        final_url: Url,
        /// Response HTTP status code.
        status: StatusCode,
        /// Response headers.
        headers: HeaderMap,
        /// Body bytes.
        body: Vec<u8>,
    },
    /// The resource has not been modified (HTTP 304).
    NotModified {
        /// The final URL reached.
        final_url: Url,
    },
    /// A redirect hop left the site's host.
    RedirectExternal {
        /// Target URL of the external redirect.
        target_url: Url,
    },
    /// The URL was refused by SSRF validation or egress DNS check.
    Refused,
    /// The response exceeded the maximum permitted byte cap.
    TooLarge,
    /// The server answered with a status that is neither success, redirect nor 304.
    Status(StatusCode),
    /// A network error occurred.
    Failed(String),
}

/// Public intermediates some sites do not send with their certificate (T-3298): a browser fetches
/// a missing one from the leaf's AIA address, rustls does not. Each is offered to the chain
/// building beside what the server sent; trust still ends at a native root, and an expired or
/// unrelated intermediate builds no chain. Downloaded from cacerts.digicert.com and pinned by
/// SHA-256 in `the_shipped_intermediates_are_the_pinned_ones`; both expire 2027-11-02.
pub const INTERMEDIATES: &[(&str, &[u8])] = &[
    // www.bbsk.sk
    (
        "GeoTrust TLS RSA CA G1",
        include_bytes!("intermediates/GeoTrustTLSRSACAG1.crt"),
    ),
    // www.praha.eu
    (
        "Thawte TLS RSA CA G1",
        include_bytes!("intermediates/ThawteTLSRSACAG1.crt"),
    ),
];

/// A webpki check that also offers `extra` intermediates when it builds the chain.
#[derive(Debug)]
struct WithIntermediates {
    inner: Arc<WebPkiServerVerifier>,
    extra: Vec<CertificateDer<'static>>,
}

impl ServerCertVerifier for WithIntermediates {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let mut offered = intermediates.to_vec();
        offered.extend(self.extra.iter().cloned());
        self.inner
            .verify_server_cert(end_entity, &offered, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// The crawler's TLS: verified to `roots`, with `intermediates` offered beside the server's own.
pub fn tls_config(
    roots: RootCertStore,
    intermediates: Vec<CertificateDer<'static>>,
) -> Result<ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|err| format!("no TLS verifier: {err}"))?;
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("no TLS versions: {err}"))?
        .dangerous() // a custom verifier; it only adds intermediates to webpki's own check
        .with_custom_certificate_verifier(Arc::new(WithIntermediates {
            inner,
            extra: intermediates,
        }))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// The production TLS: the native roots and [`INTERMEDIATES`].
fn public_tls() -> Result<ClientConfig, String> {
    native_tls(rustls_native_certs::load_native_certs())
}

/// [`public_tls`] of the certificates the platform store gave.
fn native_tls(found: rustls_native_certs::CertificateResult) -> Result<ClientConfig, String> {
    let mut roots = RootCertStore::empty();
    let (added, _) = roots.add_parsable_certificates(found.certs);
    if added == 0 {
        return Err(format!(
            "no native root certificate could be read: {:?}",
            found.errors
        ));
    }
    let intermediates = INTERMEDIATES
        .iter()
        .map(|(_, der)| CertificateDer::from(der.to_vec()))
        .collect();
    tls_config(roots, intermediates)
}

/// Builds the crawl client: every name through `resolver` (production: `PublicOnly`), no
/// redirect of its own (each hop passes [`is_url_allowed`] in [`fetch_url`]), no cookies, the
/// platform user agent, and TLS verified to the native roots with [`INTERMEDIATES`] offered. A
/// client that cannot be built is an error, never a default client, which would carry no
/// resolver and so no guard.
pub fn client<R: reqwest::dns::Resolve + 'static>(resolver: Arc<R>) -> Result<Client, String> {
    client_with_tls(resolver, public_tls()?)
}

/// [`client`] with the TLS of `config`, for a test's own roots.
pub fn client_with_tls<R: reqwest::dns::Resolve + 'static>(
    resolver: Arc<R>,
    config: ClientConfig,
) -> Result<Client, String> {
    Client::builder()
        .dns_resolver(resolver)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent("jc-assistant/0.1 (+https://joinedcontext.com/assistant)")
        .use_preconfigured_tls(config)
        .build()
        .map_err(|err| format!("the HTTP client could not be built: {err}"))
}

/// Checks whether a URL satisfies the crawler's SSRF guard rules.
///
/// Refuses user credentials, non-https schemes (unless `allow_plain_http` is true),
/// and IP literals that do not satisfy [`agent_proxy::public_dns::is_public`].
pub fn is_url_allowed(url: &Url, allow_plain_http: bool) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => {}
        "http" if allow_plain_http => {}
        _ => return false,
    }
    match url.host() {
        Some(url::Host::Ipv4(v4)) => {
            if !agent_proxy::public_dns::is_public(std::net::IpAddr::V4(v4)) {
                return false;
            }
        }
        Some(url::Host::Ipv6(v6)) => {
            if !agent_proxy::public_dns::is_public(std::net::IpAddr::V6(v6)) {
                return false;
            }
        }
        Some(url::Host::Domain(_)) => {}
        None => return false,
    }
    true
}

/// Performs a guarded GET request following at most 5 hops.
pub async fn fetch_url(
    client: &Client,
    policy: &CrawlPolicy,
    site_host: &str,
    initial_url: &Url,
    max_bytes: u64,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> FetchOutcome {
    let mut current_url = initial_url.clone();
    let mut hops = 0;

    loop {
        if !is_url_allowed(&current_url, policy.allow_plain_http) {
            return FetchOutcome::Refused;
        }

        let mut req = client.get(current_url.clone());
        if hops == 0 {
            if let Some(et) = etag {
                if let Ok(v) = HeaderValue::from_str(et) {
                    req = req.header(IF_NONE_MATCH, v);
                }
            }
            if let Some(lm) = last_modified {
                if let Ok(v) = HeaderValue::from_str(lm) {
                    req = req.header(IF_MODIFIED_SINCE, v);
                }
            }
        }

        let resp = match req.send().await {
            Ok(r) => r,
            Err(err) => {
                if is_refused_error(&err) {
                    return FetchOutcome::Refused;
                }
                // The whole chain, so a TLS refusal names the certificate (T-3298).
                let mut why = err.to_string();
                let mut source = std::error::Error::source(&err);
                while let Some(cause) = source {
                    why.push_str(": ");
                    why.push_str(&cause.to_string());
                    source = cause.source();
                }
                tracing::warn!(url = %current_url, %why, "read failed");
                return FetchOutcome::Failed(why);
            }
        };

        if resp.status() == StatusCode::NOT_MODIFIED {
            return FetchOutcome::NotModified {
                final_url: current_url,
            };
        }

        if resp.status().is_redirection() {
            if hops >= MAX_REDIRECT_HOPS {
                return FetchOutcome::Failed("too many redirects (> 5 hops)".to_string());
            }
            let loc_header = match resp.headers().get(LOCATION) {
                Some(h) => h,
                None => {
                    return FetchOutcome::Failed("redirect missing location header".to_string())
                }
            };
            let loc_str = match loc_header.to_str() {
                Ok(s) => s,
                Err(_) => {
                    return FetchOutcome::Failed("invalid location header encoding".to_string())
                }
            };
            let target_url = match current_url.join(loc_str) {
                Ok(u) => u,
                Err(_) => return FetchOutcome::Failed(format!("invalid redirect URL: {loc_str}")),
            };

            // The same host on another port is another service (T-3059).
            if target_url.host_str() != Some(site_host)
                || target_url.port_or_known_default() != current_url.port_or_known_default()
            {
                return FetchOutcome::RedirectExternal { target_url };
            }

            current_url = target_url;
            hops += 1;
            continue;
        }

        if !resp.status().is_success() {
            return FetchOutcome::Status(resp.status());
        }

        if let Some(cl) = resp.content_length() {
            if cl > max_bytes {
                return FetchOutcome::TooLarge;
            }
        }

        let headers = resp.headers().clone();
        let status = resp.status();
        let mut stream = resp.bytes_stream();
        let mut body = Vec::new();

        while let Some(chunk_res) = stream.next().await {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(err) => return FetchOutcome::Failed(err.to_string()),
            };
            if (body.len() + chunk.len()) as u64 > max_bytes {
                return FetchOutcome::TooLarge;
            }
            body.extend_from_slice(&chunk);
        }

        return FetchOutcome::Success {
            final_url: current_url,
            status,
            headers,
            body,
        };
    }
}

fn is_refused_error(err: &reqwest::Error) -> bool {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(current) = cause {
        if agent_proxy::public_dns::refused(current).is_some() {
            return true;
        }
        cause = current.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A platform store that gives no root is an error naming it, never a client trusting
    /// nothing; a verifier of no roots is refused the same way.
    #[test]
    fn no_root_certificate_is_a_tls_error_that_says_so() {
        let none = native_tls(rustls_native_certs::CertificateResult::default())
            .expect_err("no roots");
        assert!(none.starts_with("no native root certificate"), "{none}");
        let empty = tls_config(RootCertStore::empty(), Vec::new()).expect_err("no roots");
        assert!(empty.starts_with("no TLS verifier"), "{empty}");
    }

    fn allowed(url: &str) -> bool {
        is_url_allowed(&Url::parse(url).expect("a URL"), false)
    }

    /// Public address literals pass; an http(s) URL always has a host, so the guard's no-host
    /// arm is a defence no parsed crawl URL reaches.
    #[test]
    fn a_public_address_literal_is_allowed_and_every_web_url_has_a_host() {
        assert!(allowed("https://93.184.216.34/"));
        assert!(allowed("https://[2606:4700:4700::1111]/"));
        for url in ["https:///x", "http:x", "https://a/../b"] {
            assert!(Url::parse(url).map_or(true, |u| u.host().is_some()), "{url}");
        }
        assert!(!is_url_allowed(&Url::parse("data:text/plain,x").expect("a URL"), true));
    }

    /// A body the server cuts short is a failed read naming the error, never a short page.
    #[tokio::test]
    async fn a_body_cut_mid_stream_is_a_failed_read() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let address = listener.local_addr().expect("an address");
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 50\r\n\r\nshort")
                    .await;
            }
        });
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .resolve(HOST, address)
            .build()
            .expect("a client");
        let policy = CrawlPolicy {
            allow_plain_http: true,
            max_page_bytes: 64,
        };
        let url = Url::parse(&format!("http://{HOST}:{}/", address.port())).expect("a URL");
        let outcome = fetch_url(&client, &policy, HOST, &url, 64, None, None).await;
        assert!(matches!(outcome, FetchOutcome::Failed(_)), "{outcome:?}");
    }

    /// T-3227: every address a crawled link or a start URL could aim inside the cluster or at the
    /// node is refused before a request exists; a public https page is not.
    #[test]
    fn no_link_reaches_inside_the_cluster_the_node_or_another_scheme() {
        assert!(allowed("https://www.banskabystrica.sk/odpad"));
        for refused in [
            // Cloud metadata, the node, the cluster's own ranges.
            "https://169.254.169.254/latest/meta-data/",
            "https://127.0.0.1/",
            "https://10.43.0.10/",
            "https://172.16.0.1/",
            "https://192.168.1.1/",
            "https://100.64.0.1/",
            // The same addresses in the forms a parser normalises: decimal, octal, IPv4-mapped.
            "https://2130706433/",
            "https://0177.0.0.1/",
            "https://[::ffff:127.0.0.1]/",
            "https://[::1]/",
            "https://[fd00::1]/",
            // Other schemes, plain http without the policy, and credentials in the URL.
            "file:///etc/passwd",
            "ftp://www.banskabystrica.sk/",
            "gopher://www.banskabystrica.sk/",
            "http://www.banskabystrica.sk/",
            "https://user:secret@www.banskabystrica.sk/",
        ] {
            assert!(!allowed(refused), "{refused} must be refused");
        }
    }

    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const HOST: &str = "www.city.test";

    /// A client that reaches `server` under [`HOST`] over plain http, as a test fixture may.
    fn fixture(server: &MockServer) -> (Client, CrawlPolicy, Url) {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .resolve(HOST, *server.address())
            .build()
            .expect("a client");
        let policy = CrawlPolicy {
            allow_plain_http: true,
            max_page_bytes: 64,
        };
        let base =
            Url::parse(&format!("http://{HOST}:{}/", server.address().port())).expect("a URL");
        (client, policy, base)
    }

    async fn get(server: &MockServer, at: &str) -> FetchOutcome {
        let (client, policy, base) = fixture(server);
        let url = base.join(at).expect("joins");
        fetch_url(
            &client,
            &policy,
            HOST,
            &url,
            policy.max_page_bytes,
            None,
            None,
        )
        .await
    }

    #[tokio::test]
    async fn a_known_page_sends_its_validators_and_an_unchanged_one_is_not_modified() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .and(header("if-none-match", "\"v1\""))
            // `header` would split the date at its comma.
            .and(wiremock::matchers::header_exists("if-modified-since"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;
        let (client, policy, base) = fixture(&server);
        let url = base.join("page").expect("joins");
        let outcome = fetch_url(
            &client,
            &policy,
            HOST,
            &url,
            64,
            Some("\"v1\""),
            Some("Tue, 01 Sep 2026 08:00:00 GMT"),
        )
        .await;
        assert!(matches!(outcome, FetchOutcome::NotModified { final_url } if final_url == url));
        // A validator that is no header value is left out, never sent broken.
        let outcome = fetch_url(
            &client,
            &policy,
            HOST,
            &url,
            64,
            Some("bad\nvalue"),
            Some("x\ny"),
        )
        .await;
        assert!(matches!(outcome, FetchOutcome::Status(s) if s == StatusCode::NOT_FOUND));
    }

    #[tokio::test]
    async fn a_redirect_on_the_site_is_followed_and_one_off_it_is_reported() {
        let server = MockServer::start().await;
        Mock::given(path("/old"))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/new"))
            .mount(&server)
            .await;
        Mock::given(path("/new"))
            .respond_with(ResponseTemplate::new(200).set_body_string("moved"))
            .mount(&server)
            .await;
        Mock::given(path("/away"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://elsewhere.test/x"),
            )
            .mount(&server)
            .await;
        Mock::given(path("/port"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", format!("http://{HOST}:1/x")),
            )
            .mount(&server)
            .await;
        let followed = match get(&server, "old").await {
            FetchOutcome::Success {
                final_url,
                body,
                status,
                ..
            } => Some((final_url.path().to_owned(), status, body)),
            _ => None,
        };
        assert_eq!(
            followed.expect("followed"),
            ("/new".to_owned(), StatusCode::OK, b"moved".to_vec())
        );
        assert!(matches!(
            get(&server, "away").await,
            FetchOutcome::RedirectExternal { target_url } if target_url.host_str() == Some("elsewhere.test")
        ));
        assert!(matches!(
            get(&server, "port").await,
            FetchOutcome::RedirectExternal { target_url } if target_url.port() == Some(1)
        ));
    }

    #[tokio::test]
    async fn a_broken_redirect_fails_with_its_reason() {
        let server = MockServer::start().await;
        Mock::given(path("/loop"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
            .mount(&server)
            .await;
        Mock::given(path("/bare"))
            .respond_with(ResponseTemplate::new(302))
            .mount(&server)
            .await;
        Mock::given(path("/bytes"))
            .respond_with(ResponseTemplate::new(302).append_header(
                "location",
                reqwest::header::HeaderValue::from_bytes(b"/\xff").expect("a value"),
            ))
            .mount(&server)
            .await;
        Mock::given(path("/creds"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("http://u:p@{HOST}/x")),
            )
            .mount(&server)
            .await;
        Mock::given(path("/badurl"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "http://[::1"))
            .mount(&server)
            .await;
        let failed = |outcome: FetchOutcome| {
            match outcome {
                FetchOutcome::Failed(why) => Some(why),
                _ => None,
            }
            .expect("failed")
        };
        assert_eq!(
            failed(get(&server, "loop").await),
            "too many redirects (> 5 hops)"
        );
        assert_eq!(
            failed(get(&server, "bare").await),
            "redirect missing location header"
        );
        assert_eq!(
            failed(get(&server, "bytes").await),
            "invalid location header encoding"
        );
        assert!(failed(get(&server, "badurl").await).starts_with("invalid redirect URL"));
        // A hop that carries credentials is refused before it is sent.
        assert!(matches!(
            get(&server, "creds").await,
            FetchOutcome::RedirectExternal { .. } | FetchOutcome::Refused
        ));
    }

    #[tokio::test]
    async fn an_error_status_and_an_oversized_body_are_named() {
        let server = MockServer::start().await;
        Mock::given(path("/gone"))
            .respond_with(ResponseTemplate::new(410))
            .mount(&server)
            .await;
        Mock::given(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(65)))
            .mount(&server)
            .await;
        assert!(
            matches!(get(&server, "gone").await, FetchOutcome::Status(s) if s == StatusCode::GONE)
        );
        assert!(matches!(get(&server, "big").await, FetchOutcome::TooLarge));
    }

    #[tokio::test]
    async fn a_body_longer_than_its_stream_allows_is_too_large_without_a_length() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("a connection");
            let mut request = [0_u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut request).await;
            let chunk = "x".repeat(40);
            let answer = format!(
                "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n28\r\n{chunk}\r\n28\r\n{chunk}\r\n0\r\n\r\n"
            );
            let _ = socket.write_all(answer.as_bytes()).await;
        });
        let client = Client::builder()
            .resolve(HOST, addr)
            .build()
            .expect("a client");
        let policy = CrawlPolicy {
            allow_plain_http: true,
            max_page_bytes: 64,
        };
        let url = Url::parse(&format!("http://{HOST}:{}/", addr.port())).expect("a URL");
        assert!(matches!(
            fetch_url(&client, &policy, HOST, &url, 64, None, None).await,
            FetchOutcome::TooLarge
        ));
    }

    #[tokio::test]
    async fn an_unreachable_site_fails_with_the_whole_chain_and_a_refused_one_is_refused() {
        let client = Client::builder().build().expect("a client");
        let policy = CrawlPolicy {
            allow_plain_http: true,
            max_page_bytes: 64,
        };
        let url = Url::parse("http://nothing.invalid/").expect("a URL");
        assert!(matches!(
            fetch_url(&client, &policy, "nothing.invalid", &url, 64, None, None).await,
            FetchOutcome::Failed(why) if why.contains(": ")
        ));
        let guarded = client_with_tls(
            Arc::new(agent_proxy::public_dns::PublicOnly),
            public_tls().expect("native roots"),
        )
        .expect("a client");
        let local = Url::parse("http://localhost/").expect("a URL");
        assert!(matches!(
            fetch_url(&guarded, &policy, "localhost", &local, 64, None, None).await,
            FetchOutcome::Refused
        ));
        let https = CrawlPolicy::default();
        assert!(matches!(
            fetch_url(&guarded, &https, "localhost", &local, 64, None, None).await,
            FetchOutcome::Refused
        ));
    }
}
