//! The outbound rule of the `ngsi_ld` components (task section 4): `http`/`https` only, every
//! address a name resolves to judged where the connection is made (the client connects to the
//! addresses the guard kept, so no second lookup can slip in), a literal address judged before
//! the first request, no redirects, connect and request timeouts, and a body cap.
//!
//! An address passes when it is public, or when the URL's host is one of the organization's
//! `private_hosts` and the address lies in a private range (RFC 1918, CGNAT, ULA). Loopback,
//! link-local, multicast, unspecified, reserved and the cloud metadata addresses never pass. An
//! organization with `allow_private_ips` (tests, a person's own `ekiben run`) skips the check.
//!
//! ponytail: one implementation for the NGSI-LD pair; the task wants it in ekiben-io-http for
//! E3/E8 as well, which is that worker's file (see crates/ekiben-b-STATUS.md).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ekiben_core::{redact_url_str, CoreError, OrgContext};
use url::Url;

/// The AWS/Azure/GCP IPv6 metadata address, inside the ULA range and never reachable.
const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

/// Whether `ip` is an address on the public internet.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match embedded_v4(v6) {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    let private = a == 0
        || a == 10
        || a == 127
        || (a == 100 && (b & 0xc0) == 64)
        || (a == 169 && b == 254)
        || (a == 172 && (b & 0xf0) == 16)
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 198 && (b & 0xfe) == 18)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224;
    !private
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    let private = ip.is_unspecified()
        || ip.is_loopback()
        || (s[0] & 0xff00) == 0xff00
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xffc0) == 0xfec0
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        || (s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0);
    !private
}

/// The IPv4 address an IPv6 address carries: mapped `::ffff:a.b.c.d`, compatible `::a.b.c.d`,
/// NAT64 `64:ff9b::a.b.c.d` and 6to4 `2002:aabb:ccdd::`.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let v4 = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    if s[..5] == [0; 5] && (s[5] == 0xffff || (s[5] == 0 && (s[6] != 0 || s[7] > 1))) {
        return Some(v4(s[6], s[7]));
    }
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0; 4] {
        return Some(v4(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return Some(v4(s[1], s[2]));
    }
    None
}

/// Whether `ip` lies in a range one of the organization's `private_hosts` may resolve to.
pub fn is_private_range(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => match embedded_v4(v6) {
            Some(v4) => private_v4(v4),
            None => (v6.segments()[0] & 0xfe00) == 0xfc00 && v6 != METADATA_V6,
        },
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    a == 10
        || (a == 172 && (b & 0xf0) == 16)
        || (a == 192 && b == 168)
        || (a == 100 && (b & 0xc0) == 64)
}

/// Whether `org` may connect to `ip` for a URL naming `host`.
pub fn admits(org: &OrgContext, host: &str, ip: IpAddr) -> bool {
    if org.get_quotas().allow_private_ips {
        return true;
    }
    is_public(ip) || (org.is_private_host_allowed(host) && is_private_range(ip))
}

/// Judges a URL before it is called: `http`/`https` with a host, the organization's host
/// allow-list, and an address written into the URL (a name is judged when it is resolved).
pub fn check_url(url: &Url, org: &OrgContext) -> Result<(), CoreError> {
    let denied = |reason: String| CoreError::AccessDenied {
        org: org.org_id.0.clone(),
        reason,
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Err(denied(format!(
            "refused URL scheme '{}': only 'http' and 'https' are allowed",
            url.scheme()
        )));
    }
    let Some(host) = url.host_str() else {
        return Err(denied(format!(
            "URL '{}' has no host",
            redact_url_str(url.as_str())
        )));
    };
    org.check_http_host(host)?;
    let bare = host.trim_matches(|c| c == '[' || c == ']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        if !admits(org, bare, ip) {
            return Err(denied(format!(
                "SSRF protection: an outbound call to the private address '{ip}' is refused"
            )));
        }
    } else {
        let lower = bare.to_ascii_lowercase();
        if (lower == "localhost" || lower.ends_with(".localhost"))
            && !org.get_quotas().allow_private_ips
        {
            return Err(denied(format!(
                "SSRF protection: an outbound call to '{bare}' is refused"
            )));
        }
    }
    Ok(())
}

/// The system resolver keeping only the addresses [`admits`] lets the organization reach.
struct GuardedResolver {
    org: Arc<OrgContext>,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let org = self.org.clone();
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let kept: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| admits(&org, &host, a.ip()))
                .collect();
            if kept.is_empty() {
                return Err(format!(
                    "SSRF protection: '{host}' resolves to no address this organization may reach"
                )
                .into());
            }
            Ok(Box::new(kept.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// A client for one component of `org`: the guarded resolver, no proxy from the environment,
/// no redirects (an Endpoint never redirects; a hop could leave it), and the timeouts.
pub fn build_client(org: Arc<OrgContext>, timeout: Duration) -> Result<reqwest::Client, CoreError> {
    let connect = timeout
        .min(Duration::from_secs(5))
        .max(Duration::from_millis(200));
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(GuardedResolver { org }))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(connect);
    if !timeout.is_zero() {
        builder = builder.timeout(timeout);
    }
    builder
        .build()
        .map_err(|e| CoreError::config(format!("failed to build the HTTP client: {e}")))
}

/// The body of `resp`, refused as soon as it passes `max` bytes.
pub async fn read_capped(mut resp: reqwest::Response, max: usize) -> Result<Bytes, String> {
    if resp
        .content_length()
        .is_some_and(|n| n > u64::try_from(max).unwrap_or(u64::MAX))
    {
        return Err(format!("the answer is longer than {max} bytes"));
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("the answer could not be read: {}", e.without_url()))?
    {
        if buf.len().saturating_add(chunk.len()) > max {
            return Err(format!("the answer is longer than {max} bytes"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(buf))
}

/// reqwest's Display stops at "error sending request": the causes (DNS refusal, TLS) follow.
pub(crate) fn error_chain(err: &dyn std::error::Error) -> String {
    let mut out = err.to_string();
    let mut src = err.source();
    while let Some(e) = src {
        out.push_str(": ");
        out.push_str(&e.to_string());
        src = e.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::{OrgId, OrgQuotas};

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("ip")
    }

    #[test]
    fn public_and_private_addresses() {
        for public in [
            "8.8.8.8",
            "1.1.1.1",
            "2a00:1450:4001::1",
            "100.128.0.1",
            "172.32.0.1",
        ] {
            assert!(is_public(ip(public)), "{public}");
        }
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd00:ec2::254",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::127.0.0.1",
            "64:ff9b::10.0.0.1",
            "2002:7f00:1::",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip(private)), "{private}");
        }
    }

    #[test]
    fn private_ranges_never_include_loopback_link_local_or_metadata() {
        for ok in [
            "10.0.0.5",
            "172.20.1.1",
            "192.168.0.10",
            "100.64.1.1",
            "fd12::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_private_range(ip(ok)), "{ok}");
        }
        for no in [
            "127.0.0.1",
            "169.254.169.254",
            "fd00:ec2::254",
            "fe80::1",
            "::1",
            "0.0.0.0",
            "8.8.8.8",
        ] {
            assert!(!is_private_range(ip(no)), "{no}");
        }
    }

    #[test]
    fn private_hosts_admit_private_ranges_only_for_the_listed_name() {
        let org = OrgContext::new(
            OrgId::new("o"),
            OrgQuotas {
                private_hosts: vec!["context-gateway.jc.svc.cluster.local".into()],
                ..OrgQuotas::default()
            },
        );
        let gw = "context-gateway.jc.svc.cluster.local";
        assert!(admits(&org, gw, ip("10.96.0.12")));
        assert!(admits(&org, gw, ip("8.8.8.8")));
        assert!(!admits(&org, gw, ip("127.0.0.1")));
        assert!(!admits(&org, gw, ip("169.254.169.254")));
        assert!(!admits(&org, gw, ip("fd00:ec2::254")));
        assert!(!admits(&org, "other.svc.cluster.local", ip("10.96.0.12")));
        assert!(admits(&org, "other.example", ip("8.8.8.8")));
    }

    #[test]
    fn literal_private_urls_are_refused_before_any_request() {
        let org = OrgContext::new(OrgId::new("o"), OrgQuotas::default());
        for url in [
            "http://127.0.0.1:1/ngsi-ld/v1",
            "http://169.254.169.254/latest/meta-data",
            "http://[::ffff:127.0.0.1]/x",
            "http://[fd00:ec2::254]/x",
            "http://localhost/x",
            "http://a.localhost/x",
            "ftp://8.8.8.8/x",
        ] {
            let parsed = Url::parse(url).expect("url");
            assert!(check_url(&parsed, &org).is_err(), "{url}");
        }
        assert!(check_url(&Url::parse("https://8.8.8.8/x").expect("url"), &org).is_ok());
        let open = OrgContext::new(
            OrgId::new("o"),
            OrgQuotas {
                allow_private_ips: true,
                ..OrgQuotas::default()
            },
        );
        assert!(check_url(&Url::parse("http://127.0.0.1:1/x").expect("url"), &open).is_ok());
    }
}
