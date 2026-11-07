#![forbid(unsafe_code)]
//! Outbound SSRF guard, host allow-list, and IP range validation (C9, Security).

use dataroom_core::{DataroomError, Result};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use url::Url;

/// Maximum response body size read by a harvester (`32 MiB`, matching `ours/deployment/harvest-hub.py`).
pub const MAX_HARVEST_BYTES: usize = 32 * 1024 * 1024;

/// Maximum redirect hops followed during an outbound harvest fetch (A2).
pub const MAX_REDIRECT_HOPS: usize = 5;

///Validated outbound target with its pinned socket addresses so the HTTP connection uses the
/// exact DNS answer that passed the SSRF check (A2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTarget {
    pub url: Url,
    pub host: String,
    pub pinned_addrs: Vec< std::net::SocketAddr>,
}

/// Returns `true` when `ip` is loopback, private RFC 1918, link-local, cloud metadata, CGNAT,
/// unique local, multicast, broadcast, or unspecified.
pub fn is_private_or_internal_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_internal_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_internal_v4(mapped);
            }
            let seg = v6.segments();
            // Addresses that carry an IPv4 address: NAT64 64:ff9b::/96, 6to4 2002::/16 and the
            // deprecated IPv4-compatible ::/96 reach that IPv4 host, so they are judged by it.
            let embedded_v4 = |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
            if seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
                return is_internal_v4(embedded_v4(seg[6], seg[7]));
            }
            if seg[0] == 0x2002 {
                return is_internal_v4(embedded_v4(seg[1], seg[2]));
            }
            if seg[0..6] == [0, 0, 0, 0, 0, 0] && !v6.is_loopback() && !v6.is_unspecified() {
                return is_internal_v4(embedded_v4(seg[6], seg[7]));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local fc00::/7
                || (seg[0] & 0xfe00) == 0xfc00
                // Link-local fe80::/10 and the deprecated site-local fec0::/10
                || (seg[0] & 0xffc0) == 0xfe80
                || (seg[0] & 0xffc0) == 0xfec0
                // Documentation 2001:db8::/32
                || (seg[0] == 0x2001 && seg[1] == 0x0db8)
        }
    }
}

fn is_internal_v4(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || v4.is_multicast()
        // 100.64.0.0/10 (Carrier-Grade NAT)
        || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64)
        // 169.254.169.254 metadata & link-local
        || (octets[0] == 169 && octets[1] == 254)
        // 192.0.0.0/24
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        // 0.0.0.0/8 ("this network", reaches the local host on Linux)
        || octets[0] == 0
        // 198.18.0.0/15 (benchmarking)
        || (octets[0] == 198 && (octets[1] & 0xfe) == 18)
        // 240.0.0.0/4 (reserved)
        || octets[0] >= 240
}

/// Validates an outbound harvest URL:
/// 1. Must be `https://` without embedded credentials.
/// 2. Host must not be `localhost`, `.local`, `.internal`, `.cluster.local`, or an internal IP literal.
/// 3. Host must be in `allowed_hosts` (and in `catalogue_allowed_hosts` when non-empty).
/// 4. If `resolve_dns` is `true`, every resolved socket address must be a public IP.
pub fn validate_outbound_url(
    raw_url: &str,
    allowed_hosts: &[String],
    catalogue_allowed_hosts: &[String],
    resolve_dns: bool,
) -> Result<Url> {
    let parsed = Url::parse(raw_url).map_err(|e| {
        DataroomError::OutboundRefused(format!("'{raw_url}' is not a valid URL: {e}"))
    })?;

    if parsed.scheme() != "https" {
        return Err(DataroomError::OutboundRefused(format!(
            "scheme '{}' is refused; only https:// is allowed",
            parsed.scheme()
        )));
    }

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(DataroomError::OutboundRefused(
            "URL must not contain credentials".into(),
        ));
    }

    let Some(host_str) = parsed.host_str() else {
        return Err(DataroomError::OutboundRefused("URL has no host".into()));
    };
    let host_lower = host_str.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();

    if host_lower == "localhost"
        || host_lower.ends_with(".localhost")
        || host_lower.ends_with(".local")
        || host_lower.ends_with(".internal")
        || host_lower.ends_with(".cluster.local")
        || host_lower.ends_with(".svc")
    {
        return Err(DataroomError::OutboundRefused(format!(
            "internal host '{host_str}' is refused"
        )));
    }

    if let Ok(ip) = host_lower.parse::<IpAddr>() {
        if is_private_or_internal_ip(ip) {
            return Err(DataroomError::OutboundRefused(format!(
                "internal IP address {ip} is refused"
            )));
        }
    } else if let Ok(v6) = host_lower.parse::<Ipv6Addr>() {
        if is_private_or_internal_ip(IpAddr::V6(v6)) {
            return Err(DataroomError::OutboundRefused(format!(
                "internal IPv6 address {v6} is refused"
            )));
        }
    }

    let in_source_allowlist = allowed_hosts
        .iter()
        .any(|h| h.eq_ignore_ascii_case(&host_lower));
    if !in_source_allowlist {
        return Err(DataroomError::OutboundRefused(format!(
            "host '{host_str}' is not in the harvest source's allowedHosts"
        )));
    }

    if !catalogue_allowed_hosts.is_empty()
        && !catalogue_allowed_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&host_lower))
    {
        return Err(DataroomError::OutboundRefused(format!(
            "host '{host_str}' is not in the catalogue's allowed_outbound_hosts"
        )));
    }

    if resolve_dns {
        let _ = resolve_and_validate_host(&host_lower, parsed.port_or_known_default().unwrap_or(443))?;
    }

    Ok(parsed)
}

/// Resolves a host name via the system resolver and verifies that at least one address is returned
/// and every returned address is a public IP (not private, loopback, link-local, CGNAT, or metadata).
pub fn resolve_and_validate_host(
    host: &str,
    port: u16,
) -> Result<Vec<std::net::SocketAddr>> {
    let addrs: Vec<std::net::SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| {
            DataroomError::OutboundRefused(format!("DNS resolution failed for '{host}': {e}"))
        })?
        .collect();
    if addrs.is_empty() {
        return Err(DataroomError::OutboundRefused(format!(
            "DNS resolution for '{host}' returned no addresses"
        )));
    }
    for sock in &addrs {
        if is_private_or_internal_ip(sock.ip()) {
            return Err(DataroomError::OutboundRefused(format!(
                "host '{host}' resolved to internal address {}",
                sock.ip()
            )));
        }
    }
    Ok(addrs)
}

/// A name resolver used instead of the system one (tests, a pinned resolver).
pub type DnsResolver = dyn Fn(&str, u16) -> Result<Vec<std::net::SocketAddr>>;

/// Validates an outbound URL and returns a [`ValidatedTarget`] with pinned DNS addresses (A2).
pub fn validate_and_pin_url(
    raw_url: &str,
    allowed_hosts: &[String],
    catalogue_allowed_hosts: &[String],
    resolve_dns: bool,
    custom_resolver: Option<&DnsResolver>,
) -> Result<ValidatedTarget> {
    let parsed = validate_outbound_url(raw_url, allowed_hosts, catalogue_allowed_hosts, false)?;
    let host = parsed
        .host_str()
        .unwrap_or("")
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    let port = parsed.port_or_known_default().unwrap_or(443);

    let pinned_addrs = if let Some(resolver) = custom_resolver {
        let addrs = resolver(&host, port)?;
        if addrs.is_empty() {
            return Err(DataroomError::OutboundRefused(format!(
                "DNS resolution for '{host}' returned no addresses"
            )));
        }
        for sock in &addrs {
            if is_private_or_internal_ip(sock.ip()) {
                return Err(DataroomError::OutboundRefused(format!(
                    "host '{host}' resolved to internal address {}",
                    sock.ip()
                )));
            }
        }
        addrs
    } else if resolve_dns {
        resolve_and_validate_host(&host, port)?
    } else {
        Vec::new()
    };

    Ok(ValidatedTarget {
        url: parsed,
        host,
        pinned_addrs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2_internal_ranges_include_embedded_ipv4_and_reserved_blocks() {
        for internal in [
            "0.1.2.3",
            "198.18.0.1",
            "240.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::ffff:127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "2002:7f00:1::",
            "::10.0.0.1",
            "fec0::1",
            "2001:db8::1",
        ] {
            let ip: IpAddr = internal.parse().unwrap();
            assert!(is_private_or_internal_ip(ip), "{internal} must be internal");
        }
        for public in ["93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946", "64:ff9b::5db8:d822"] {
            let ip: IpAddr = public.parse().unwrap();
            assert!(!is_private_or_internal_ip(ip), "{public} is public");
        }
    }
}
