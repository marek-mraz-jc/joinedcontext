#![forbid(unsafe_code)]
//! Outbound SSRF guard, host allow-list, and IP range validation (C9, Security).

use dataroom_core::{DataroomError, Result};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use url::Url;

/// Maximum response body size read by a harvester (`32 MiB`, matching `ours/deployment/harvest-hub.py`).
pub const MAX_HARVEST_BYTES: usize = 32 * 1024 * 1024;

/// Returns `true` when `ip` is loopback, private RFC 1918, link-local, cloud metadata, CGNAT,
/// unique local, multicast, broadcast, or unspecified.
pub fn is_private_or_internal_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_internal_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_internal_v4(mapped);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local fc00::/7
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local fe80::/10
                || (v6.segments()[0] & 0xffc0) == 0xfe80
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
        let port = parsed.port_or_known_default().unwrap_or(443);
        if let Ok(addrs) = (host_lower.as_str(), port).to_socket_addrs() {
            for sock in addrs {
                if is_private_or_internal_ip(sock.ip()) {
                    return Err(DataroomError::OutboundRefused(format!(
                        "host '{host_str}' resolved to internal address {}",
                        sock.ip()
                    )));
                }
            }
        }
    }

    Ok(parsed)
}
