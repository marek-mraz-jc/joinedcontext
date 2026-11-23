#![forbid(unsafe_code)]
//! Outbound HTTP of `jc` (ARCH/SEC-04, NET-07): the one address table and the one client.
//!
//! - [`check_address`]: loopback, private, link-local, metadata, CGNAT, multicast and reserved
//!   addresses are refused, also when an IPv6 address carries them.
//! - [`resolve`]: resolves once and every address must pass. A module's own admission (the
//!   listener beside it, a network or host the operator opened) is an input and never opens
//!   a metadata address.
//! - [`send`]: resolves on every request, connects to those addresses only, follows no redirect
//!   and no proxy of the environment, and reads at most the cap.
//!
//! Which hosts a module may name at all (its allow-list) stays with the module.

use ipnet::{Ipv4Net, Ipv6Net};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

pub use reqwest::header::HeaderMap;
pub use reqwest::{Body, Method, Url};

const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

// "This network" 0/8 (0.x.y.z reaches the local host on Linux), CGNAT, IETF protocol
// assignments, 6to4 relay anycast, benchmark and the reserved 240/4.
const RESERVED_V4: [(Ipv4Net, &str); 6] = [
    (
        Ipv4Net::new_assert(Ipv4Addr::new(0, 0, 0, 0), 8),
        "this network",
    ),
    (
        Ipv4Net::new_assert(Ipv4Addr::new(100, 64, 0, 0), 10),
        "shared address space",
    ),
    (
        Ipv4Net::new_assert(Ipv4Addr::new(192, 0, 0, 0), 24),
        "protocol assignments",
    ),
    (
        Ipv4Net::new_assert(Ipv4Addr::new(192, 88, 99, 0), 24),
        "6to4 relay",
    ),
    (
        Ipv4Net::new_assert(Ipv4Addr::new(198, 18, 0, 0), 15),
        "benchmark",
    ),
    (
        Ipv4Net::new_assert(Ipv4Addr::new(240, 0, 0, 0), 4),
        "reserved",
    ),
];

// AWS metadata's network, documentation, site-local, discard-only, local-use NAT64 and Teredo
// (its IPv4 endpoint is hidden).
const RESERVED_V6: [(Ipv6Net, &str); 6] = [
    (
        Ipv6Net::new_assert(Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0), 32),
        "metadata",
    ),
    (
        Ipv6Net::new_assert(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
        "documentation",
    ),
    (
        Ipv6Net::new_assert(Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10),
        "site-local",
    ),
    (
        Ipv6Net::new_assert(Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64),
        "discard-only",
    ),
    (
        Ipv6Net::new_assert(Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
        "local NAT64",
    ),
    (
        Ipv6Net::new_assert(Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 32),
        "Teredo",
    ),
];

/// An address outside the policy and the class it is refused for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("blocked {class} address {ip}")]
pub struct Blocked {
    pub ip: IpAddr,
    pub class: &'static str,
}

/// Why an outbound request was not made or not answered.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("outbound URL refused: {0}")]
    Url(&'static str),
    #[error("host '{0}' does not resolve to an address the outbound policy allows")]
    Unreachable(String),
    #[error("request to '{host}' failed: {reason}")]
    Transport { host: String, reason: String },
    #[error("error reading response stream: {0}")]
    Body(String),
    #[error("upstream attempted redirect with status {0} (redirects are disabled)")]
    Redirect(u16),
    #[error("response content-length {declared} exceeds limit of {max} bytes")]
    DeclaredTooLarge { declared: u64, max: usize },
    #[error("response body exceeded maximum size of {0} bytes")]
    TooLarge(usize),
}

/// The IPv4 address an IPv6 address carries: mapped, IPv4-compatible, NAT64 or 6to4.
fn carried_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return Some(mapped);
    }
    if ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    let s = ip.segments();
    let pair = |high: u16, low: u16| Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
    if s[..6] == [0, 0, 0, 0, 0, 0] || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        Some(pair(s[6], s[7]))
    } else if s[0] == 0x2002 {
        Some(pair(s[1], s[2]))
    } else {
        None
    }
}

/// The cloud metadata addresses (`169.254.169.254`, also carried inside IPv6, and AWS
/// `fd00:ec2::254`). No admission opens them.
#[must_use]
pub fn is_cloud_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4 == METADATA_V4,
        IpAddr::V6(v6) => v6 == METADATA_V6 || carried_v4(v6) == Some(METADATA_V4),
    }
}

/// Whether `ip` is an address an outbound request may reach without an admission.
/// `allow_loopback` is for a module's local test servers and opens nothing else.
pub fn check_address(ip: IpAddr, allow_loopback: bool) -> Result<(), Blocked> {
    let refused = |class| Err(Blocked { ip, class });
    if is_cloud_metadata(ip) {
        return refused("cloud metadata");
    }
    let v4 = match ip {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(v6) => match carried_v4(v6) {
            Some(v4) => v4,
            None => {
                if v6.is_loopback() {
                    return if allow_loopback {
                        Ok(())
                    } else {
                        refused("loopback")
                    };
                }
                if v6.is_unspecified()
                    || v6.is_multicast()
                    || v6.is_unique_local()
                    || v6.is_unicast_link_local()
                {
                    return refused("non-public IPv6");
                }
                return match RESERVED_V6.iter().find(|(net, _)| net.contains(&v6)) {
                    Some((_, class)) => refused(class),
                    None => Ok(()),
                };
            }
        },
    };
    if v4.is_loopback() {
        return if allow_loopback {
            Ok(())
        } else {
            refused("loopback")
        };
    }
    if v4.is_unspecified()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_documentation()
    {
        return refused("non-public IPv4");
    }
    match RESERVED_V4.iter().find(|(net, _)| net.contains(&v4)) {
        Some((_, class)) => refused(class),
        None => Ok(()),
    }
}

/// Resolves `host` once within `timeout`. Every address must pass `admit`, and none may be a
/// metadata address. A name that does not resolve and a name that resolves to a refused address
/// answer the same [`Error::Unreachable`], so a refusal tells nothing about internal names.
pub async fn resolve(
    host: &str,
    port: u16,
    timeout: Duration,
    admit: impl Fn(IpAddr) -> bool,
) -> Result<Vec<SocketAddr>, Error> {
    let host = host.trim_matches(['[', ']']);
    let unreachable = || Error::Unreachable(host.to_owned());
    let resolved: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        // A lookup that times out and one that fails are the same refusal.
        Err(_) => tokio::time::timeout(timeout, tokio::net::lookup_host((host, port)))
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or_else(unreachable)?
            .collect(),
    };
    let allowed = |addr: &SocketAddr| !is_cloud_metadata(addr.ip()) && admit(addr.ip());
    if resolved.is_empty() || !resolved.iter().all(allowed) {
        return Err(unreachable());
    }
    Ok(resolved)
}

/// One request and what it may take.
#[derive(Debug)]
pub struct Request {
    pub method: Method,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
    /// `http` passes only when the module admitted it for this URL.
    pub plain_http: bool,
    /// Resolution and the request each end within this.
    pub timeout: Duration,
    pub max_response_bytes: usize,
}

/// The whole answer, at most `max_response_bytes` of body.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Sends `request` to the addresses [`resolve`] admits now. Each send gets a client of its own,
/// so a connection admitted for one caller's rules never serves another's.
pub async fn send(request: Request, admit: impl Fn(IpAddr) -> bool) -> Result<Response, Error> {
    let url = request.url;
    match url.scheme() {
        "https" => {}
        "http" if request.plain_http => {}
        _ => return Err(Error::Url("https is required")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Url("no credentials in the URL"));
    }
    let host = url
        .host_str()
        .map(|h| h.trim_matches(['[', ']']).to_owned())
        .ok_or(Error::Url("no host"))?;
    let port = url.port_or_known_default().ok_or(Error::Url("no port"))?;
    let addrs = resolve(&host, port, request.timeout, admit).await?;
    let failed = |e: reqwest::Error| Error::Transport {
        host: host.clone(),
        reason: e.without_url().to_string(),
    };
    // No proxy: a proxy taken from the environment would resolve the host itself and bypass
    // the address checks and the pinning.
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(request.timeout);
    if host.parse::<IpAddr>().is_err() {
        builder = builder.resolve_to_addrs(&host, &addrs);
    }
    let client = builder.build().map_err(failed)?;
    let mut call = client.request(request.method, url);
    for (name, value) in request.headers {
        call = call.header(name, value);
    }
    if let Some(body) = request.body {
        call = call.body(body);
    }
    let mut response = call.send().await.map_err(failed)?;
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(Error::Redirect(status));
    }
    let max = request.max_response_bytes;
    if let Some(declared) = response.content_length().filter(|n| *n > max as u64) {
        return Err(Error::DeclaredTooLarge { declared, max });
    }
    let headers = response.headers().clone();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| Error::Body(e.without_url().to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > max {
            return Err(Error::TooLarge(max));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Response {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn private_reserved_and_carried_addresses_are_refused() {
        for blocked in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "0.1.2.3",
            "192.0.0.170",
            "198.18.0.1",
            "203.0.113.9",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fe80::1",
            "fc00::1",
            "fec0::1",
            "ff02::1",
            "100::1",
            "2001:db8::1",
            "64:ff9b:1::1",
            "2001::1",
            "::127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::10.0.0.1",
            "2002:a00:1::1",
        ] {
            assert!(check_address(ip(blocked), false).is_err(), "{blocked}");
        }
        // A public IPv4 address, built from its octets, plain and mapped into IPv6.
        let public_v4 = Ipv4Addr::from([93, 184, 216, 34]);
        for public in [
            IpAddr::V4(public_v4),
            ip("2606:2800:220:1::1"),
            IpAddr::V6(public_v4.to_ipv6_mapped()),
        ] {
            assert_eq!(check_address(public, false), Ok(()), "{public}");
        }
        assert_eq!(
            check_address(ip("fec0::1"), false).unwrap_err().class,
            "site-local"
        );
        for (address, class) in [
            ("224.0.0.1", "non-public IPv4"),
            ("255.255.255.255", "non-public IPv4"),
        ] {
            assert_eq!(check_address(ip(address), false).unwrap_err().class, class);
        }
        assert_eq!(
            check_address(ip("100.64.0.1"), false)
                .unwrap_err()
                .to_string(),
            "blocked shared address space address 100.64.0.1"
        );
        assert!(check_address(ip("127.0.0.1"), true).is_ok());
        assert!(check_address(ip("::1"), true).is_ok());
        assert!(check_address(ip("10.0.0.1"), true).is_err());
    }

    #[tokio::test]
    async fn no_admission_opens_a_metadata_address() {
        for metadata in [
            "169.254.169.254",
            "fd00:ec2::254",
            "::ffff:169.254.169.254",
            "::169.254.169.254",
            "64:ff9b::a9fe:a9fe",
            "2002:a9fe:a9fe::1",
        ] {
            assert!(is_cloud_metadata(ip(metadata)), "{metadata}");
            assert!(check_address(ip(metadata), true).is_err(), "{metadata}");
            let reached = resolve(metadata, 80, Duration::from_secs(1), |_| true).await;
            assert!(matches!(reached, Err(Error::Unreachable(_))), "{metadata}");
        }
    }

    /// One connection on loopback: answers `answer` and hands back what the request sent.
    async fn serve_one(answer: &'static str) -> (Url, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = [0u8; 2048];
            let n = socket.read(&mut head).await.unwrap_or(0);
            let _ = socket.write_all(answer.as_bytes()).await;
            String::from_utf8_lossy(&head[..n]).into_owned()
        });
        (format!("http://{addr}/").parse().unwrap(), seen)
    }

    async fn serve(answer: &'static str) -> Url {
        serve_one(answer).await.0
    }

    fn get(url: Url, plain_http: bool) -> Request {
        Request {
            method: Method::GET,
            url,
            headers: Vec::new(),
            body: None,
            plain_http,
            timeout: Duration::from_secs(5),
            max_response_bytes: 8,
        }
    }

    /// A name resolves within the timeout and every address it gives must pass; one that does
    /// not resolve, does not resolve in time or gives a refused address is the same refusal.
    #[tokio::test]
    async fn a_name_resolves_once_and_every_address_must_pass() {
        let wait = Duration::from_secs(5);
        let local = resolve("localhost", 80, wait, |_| true).await.unwrap();
        assert!(!local.is_empty() && local.iter().all(|a| a.port() == 80 && a.ip().is_loopback()));
        assert_eq!(
            resolve("[::1]", 8, wait, |_| true).await.unwrap(),
            vec![SocketAddr::new(ip("::1"), 8)]
        );
        for (host, timeout, strict) in [("localhost", wait, true), ("nothing.invalid", wait, false)]
        {
            let got = resolve(host, 80, timeout, |a| {
                !strict || check_address(a, false).is_ok()
            });
            match got.await {
                Err(Error::Unreachable(named)) => assert_eq!(named, host),
                other => panic!("{host} {timeout:?}: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_send_needs_admission_follows_no_redirect_and_caps_the_body() {
        let ok = serve("HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok").await;
        let strict = |ip| check_address(ip, false).is_ok();
        assert!(matches!(
            send(get(ok.clone(), true), strict).await,
            Err(Error::Unreachable(_))
        ));
        assert!(matches!(
            send(get(ok.clone(), false), |_| true).await,
            Err(Error::Url(_))
        ));
        let answer = send(get(ok, true), |_| true).await.unwrap();
        assert_eq!((answer.status, answer.body.as_slice()), (200, &b"ok"[..]));

        let away = serve(
            "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        )
        .await;
        assert!(matches!(
            send(get(away, true), |_| true).await,
            Err(Error::Redirect(302))
        ));
        let long = serve("HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n0123456789abcdef").await;
        assert!(matches!(
            send(get(long, true), |_| true).await,
            Err(Error::TooLarge(8))
        ));
    }

    /// A name is pinned to the addresses resolved for it, the caller's headers and body go out,
    /// and a declared length over the cap, a body cut short, a credential in the URL and a TLS
    /// handshake that fails each refuse.
    #[tokio::test]
    async fn a_send_carries_headers_and_body_and_refuses_what_it_cannot_trust() {
        let (url, seen) = serve_one("HTTP/1.1 201 Created\r\ncontent-length: 2\r\n\r\nok").await;
        let mut named = url.clone();
        named.set_host(Some("localhost")).unwrap();
        let mut post = get(named, true);
        post.method = Method::POST;
        post.headers = vec![("x-probe".into(), "1".into())];
        post.body = Some(Body::from("payload"));
        let answer = send(post, |_| true).await.unwrap();
        assert_eq!((answer.status, answer.body.as_slice()), (201, &b"ok"[..]));
        let head = seen.await.unwrap();
        assert!(head.starts_with("POST / HTTP/1.1"), "{head}");
        assert!(
            head.contains("x-probe: 1") && head.ends_with("payload"),
            "{head}"
        );

        // Exactly the cap passes, declared or not.
        for full in [
            "HTTP/1.1 200 OK\r\ncontent-length: 8\r\n\r\n12345678",
            "HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n12345678",
        ] {
            let answer = send(get(serve(full).await, true), |_| true).await.unwrap();
            assert_eq!(answer.body, b"12345678");
        }
        let big = serve("HTTP/1.1 200 OK\r\ncontent-length: 9\r\n\r\n123456789").await;
        assert!(matches!(
            send(get(big, true), |_| true).await,
            Err(Error::DeclaredTooLarge {
                declared: 9,
                max: 8
            })
        ));
        let cut =
            serve("HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nab").await;
        assert!(matches!(
            send(get(cut, true), |_| true).await,
            Err(Error::Body(_))
        ));
        let mut secret = url.clone();
        secret.set_username("user").unwrap();
        assert!(matches!(
            send(get(secret, true), |_| true).await,
            Err(Error::Url("no credentials in the URL"))
        ));
        let plain = serve("HTTP/1.1 200 OK\r\n\r\n").await;
        let mut tls = plain.clone();
        tls.set_scheme("https").unwrap();
        match send(get(tls, false), |_| true).await {
            Err(Error::Transport { host, .. }) => assert_eq!(host, "127.0.0.1"),
            other => panic!("{other:?}"),
        }
    }
}
