//! The outbound rule for connections ekiben opens itself (the `websocket` input): one DNS
//! resolution, every resolved address checked, and the connection made to a checked address, so
//! a name cannot be re-resolved (rebound) to another address between the check and the connect.
//!
//! - Schemes: `http`, `https`, `ws`, `wss` only.
//! - Refused unless the organization's `allow_private_ips` quota is on: loopback, unspecified,
//!   link-local (with the cloud metadata address), multicast, broadcast and reserved ranges,
//!   RFC 1918, CGNAT (100.64/10), benchmarking (198.18/15), IPv6 unique-local, site-local and
//!   Teredo, and every IPv4 address embedded in IPv6 (mapped, compatible, NAT64, 6to4) by the
//!   rule of the address it embeds.
//! - An organization's `private_hosts` (exact names) may resolve to private ranges (RFC 1918,
//!   CGNAT, ULA) but never to loopback, link-local, metadata, unspecified or multicast.
//! - A name resolving to several addresses is refused when any one of them is.
//! - The organization's `allowed_http_hosts` allow-list applies as for `http_client`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use ekiben_core::{redact_url_str, CoreError, OrgContext};
use tokio::net::TcpStream;
use url::Url;

/// How an address may be reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrClass {
    /// A public address.
    Public,
    /// A private range an organization may open with `private_hosts`.
    Private,
    /// Never reachable through `private_hosts` (loopback, link-local, metadata, multicast...).
    Forbidden(&'static str),
}

fn classify_v4(ip: Ipv4Addr) -> AddrClass {
    let o = ip.octets();
    if ip.is_unspecified() || o[0] == 0 {
        return AddrClass::Forbidden("unspecified");
    }
    if ip.is_loopback() {
        return AddrClass::Forbidden("loopback");
    }
    if ip.is_link_local() {
        return AddrClass::Forbidden("link-local or cloud metadata");
    }
    if ip == Ipv4Addr::new(100, 100, 100, 200) {
        return AddrClass::Forbidden("cloud metadata");
    }
    if ip.is_multicast() {
        return AddrClass::Forbidden("multicast");
    }
    if ip.is_broadcast() || o[0] >= 240 {
        return AddrClass::Forbidden("reserved");
    }
    if ip.is_private()
        || (o[0] == 100 && (o[1] & 0b1100_0000) == 64)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
    {
        return AddrClass::Private;
    }
    AddrClass::Public
}

/// The class of `ip` under the outbound rule.
pub fn classify(ip: IpAddr) -> AddrClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddrClass {
    if ip.is_unspecified() {
        return AddrClass::Forbidden("unspecified");
    }
    if ip.is_loopback() {
        return AddrClass::Forbidden("loopback");
    }
    let seg = ip.segments();
    // IPv4-mapped ::ffff:a.b.c.d and IPv4-compatible ::a.b.c.d follow the embedded address.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    if seg[..6].iter().all(|s| *s == 0) {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        );
        return match classify_v4(v4) {
            AddrClass::Public => AddrClass::Forbidden("IPv4-compatible"),
            other => other,
        };
    }
    // NAT64 64:ff9b::/96 and 6to4 2002::/16 reach the IPv4 address they embed.
    if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6].iter().all(|s| *s == 0) {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        );
        return classify_v4(v4);
    }
    if seg[0] == 0x2002 {
        let v4 = Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            seg[1] as u8,
            (seg[2] >> 8) as u8,
            seg[2] as u8,
        );
        return classify_v4(v4);
    }
    if seg[0] == 0x2001 && seg[1] == 0 {
        return AddrClass::Forbidden("Teredo");
    }
    if (seg[0] & 0xffc0) == 0xfe80 {
        return AddrClass::Forbidden("link-local");
    }
    if (seg[0] & 0xffc0) == 0xfec0 {
        return AddrClass::Forbidden("site-local");
    }
    if (seg[0] & 0xff00) == 0xff00 {
        return AddrClass::Forbidden("multicast");
    }
    // AWS's IPv6 instance metadata address lives in the unique-local range.
    if ip == Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254) {
        return AddrClass::Forbidden("cloud metadata");
    }
    if (seg[0] & 0xfe00) == 0xfc00 {
        return AddrClass::Private;
    }
    AddrClass::Public
}

/// Whether `ip` may be reached: always when `allow_private_ips`; a private range when the host
/// is one of the organization's `private_hosts`; otherwise only a public address.
pub fn check_ip(
    ip: IpAddr,
    allow_private_ips: bool,
    private_host: bool,
) -> Result<(), &'static str> {
    if allow_private_ips {
        return Ok(());
    }
    match classify(ip) {
        AddrClass::Public => Ok(()),
        AddrClass::Private if private_host => Ok(()),
        AddrClass::Private => Err("private"),
        AddrClass::Forbidden(why) => Err(why),
    }
}

fn denied(org: &OrgContext, reason: String) -> CoreError {
    CoreError::AccessDenied {
        org: org.org_id.0.clone(),
        reason,
    }
}

/// The checked addresses `url` may be connected to, resolved once.
pub async fn resolve_checked(url: &Url, org: &OrgContext) -> Result<Vec<SocketAddr>, CoreError> {
    let scheme = url.scheme();
    let default_port = match scheme {
        "http" | "ws" => 80,
        "https" | "wss" => 443,
        other => {
            return Err(denied(
                org,
                format!("refused URL scheme '{other}': only http, https, ws and wss are allowed"),
            ))
        }
    };
    let host = url.host_str().ok_or_else(|| {
        CoreError::config(format!(
            "URL '{}' has no host",
            redact_url_str(url.as_str())
        ))
    })?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    org.check_http_host(bare)?;
    let port = url.port().unwrap_or(default_port);
    let quotas = org.get_quotas();
    let private_host = org.is_private_host_allowed(bare);

    let addrs: Vec<SocketAddr> = match bare.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => {
            if (bare.eq_ignore_ascii_case("localhost")
                || bare.to_ascii_lowercase().ends_with(".localhost"))
                && !quotas.allow_private_ips
            {
                return Err(denied(
                    org,
                    format!("outbound rule: host '{bare}' is loopback and is refused"),
                ));
            }
            tokio::net::lookup_host((bare, port))
                .await
                .map_err(|e| CoreError::other(format!("failed to resolve '{bare}': {e}")))?
                .collect()
        }
    };
    if addrs.is_empty() {
        return Err(CoreError::other(format!("'{bare}' resolved to no address")));
    }
    for a in &addrs {
        if let Err(why) = check_ip(a.ip(), quotas.allow_private_ips, private_host) {
            return Err(denied(
                org,
                format!("outbound rule: '{bare}' resolves to a {why} address, which is refused"),
            ));
        }
    }
    Ok(addrs)
}

/// Connects to the first reachable checked address.
pub async fn connect_checked(
    addrs: &[SocketAddr],
    timeout: Duration,
) -> Result<TcpStream, CoreError> {
    let mut last = String::from("no address");
    for a in addrs {
        match tokio::time::timeout(timeout, TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => last = e.to_string(),
            Err(_) => last = "connect timed out".to_string(),
        }
    }
    Err(CoreError::other(format!("connect failed: {last}")))
}
