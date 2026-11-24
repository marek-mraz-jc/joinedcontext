//! Input: an outbound URL (a CIMD client_id, an upstream IdP endpoint, a JWKS URI), then NUL,
//! then an `outbound_private_hosts` entry, then NUL, then allowed domains (one per line). An
//! accepted URL never carries userinfo and never names a forbidden IP literal.
#![no_main]

use kaisatsu_oidc::{OutboundGuard, OutboundPolicy, PrivateHost};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut parts = text.split('\0');
    let raw_url = parts.next().unwrap_or_default();
    let private = parts.next().unwrap_or_default();
    let allowed: Vec<String> = parts
        .next()
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let mut policy = OutboundPolicy::default();
    if let Ok(host) = PrivateHost::parse(private) {
        policy.private_hosts.push(host);
    }
    let guard = OutboundGuard::new(policy);
    for domains in [None, Some(allowed.as_slice())] {
        if let Ok(url) = guard.validate_url(raw_url, domains) {
            assert!(
                url.username().is_empty() && url.password().is_none(),
                "userinfo accepted"
            );
            if let Some(url::Host::Ipv4(ip)) = url.host() {
                assert!(
                    !OutboundGuard::is_always_forbidden_ip(ip.into()),
                    "forbidden {ip}"
                );
            }
            if let Some(url::Host::Ipv6(ip)) = url.host() {
                assert!(
                    !OutboundGuard::is_always_forbidden_ip(ip.into()),
                    "forbidden {ip}"
                );
            }
        }
    }
});
