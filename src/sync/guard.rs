//! The address rule every outbound fetch of the Portal obeys (MF-28, MF-32, DM-49).
//!
//! Two modules below open sockets to a URL somebody typed into a manifest: [`super::remote`]
//! for a `SyncSource`, [`super::schema_api`] for a peer's schema surface. Both run inside the
//! cluster, where the metadata service, the node's kubelet, every `*.svc.cluster.local` and
//! the Portal's own admin port are one hop away and nothing on the outside can reach them.
//! A fetcher without this rule is a probe for whoever can get a manifest approved, and its
//! answer comes back in `status.lastError` where they can read it (T-0802, T-1705).
//!
//! The rule is three checks, and a fetch needs all three because each one alone is bypassable:
//!
//! - the URL's own host, when it is written as a literal address;
//! - what a name resolves to, checked in the resolver so that a public name answering with an
//!   internal address — the whole of DNS rebinding — is refused at the point of connecting;
//! - the target of every redirect, which is a second URL the origin chooses and the first
//!   check never saw.
//!
//! The address list and the resolver are the platform's one guard, [`jc_core::outbound`]
//! (T-4776, T-5335); this module keeps the Portal's stricter `https`-only redirect rule.

use std::net::IpAddr;

use jc_core::outbound::is_public;
use reqwest::Url;

/// The internal address a URL names outright, if it names one.
///
/// A host that is a name answers `None` here and is judged by [`jc_core::outbound::PublicOnly`] instead, once it
/// has resolved. A host written in any other notation — `0177.0.0.1`, `2852039166` — is not an
/// address as far as the URL parser is concerned, so it too arrives at the resolver, which is
/// the check that sees what the C library made of it.
pub(crate) fn internal_host(url: &str) -> Option<IpAddr> {
    let parsed = Url::parse(url).ok()?;
    let ip = match parsed.host()? {
        url::Host::Ipv4(v4) => IpAddr::V4(v4),
        url::Host::Ipv6(v6) => IpAddr::V6(v6),
        url::Host::Domain(_) => return None,
    };
    (!is_public(ip)).then_some(ip)
}

/// Whether one redirect hop may be followed.
///
/// An origin answers `302` with a URL of its own choosing, so the hop is judged exactly as the
/// first URL was: `https`, a public address, and not past the budget. Written as a predicate
/// rather than inside the closure that reqwest takes, because a closure a test cannot call is
/// a defence nobody proves.
pub(crate) fn follows(next: &Url, hops: usize, max: usize) -> bool {
    next.scheme() == "https" && hops < max && jc_core::outbound::check(next).is_ok()
}

/// The redirect rule of every https-only fetcher: each hop judged by [`follows`], and a hop it
/// refuses stopped rather than failed with a reason, so the caller says the URL could not be
/// read, never what the redirect pointed at.
pub(crate) fn https_redirects(max: usize) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if follows(attempt.url(), attempt.previous().len(), max) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// The address rule of MF-28 and DM-49, played as the attack of T-1705: the metadata service, a
/// cluster Service, a scheme that is not HTTP, a redirect chain and a rebinding name.
#[cfg(test)]
mod tests {
    use super::*;

    /// MF-28, DM-49 (T-1705): the addresses that are inside the cluster, and the ones that are
    /// not. The counterpart matters as much as the list: a rule that refuses everything is a
    /// fetcher that fetches nothing.
    #[test]
    fn the_cluster_s_own_addresses_are_internal_and_a_public_one_is_not() {
        for ip in [
            "169.254.169.254", // the cloud metadata service
            "10.42.0.1",       // the pod network
            "172.31.255.254",
            "192.168.0.7",
            "127.0.0.53",
            "100.100.100.100",
            "0.0.0.0",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::abcd",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public(ip.parse().expect(ip)), "{ip} passed as public");
        }
        for ip in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(is_public(ip.parse().expect(ip)), "{ip} was refused");
        }
    }

    /// MF-28, DM-49 (T-1705): a URL that names an internal address outright is caught before a
    /// socket is opened, brackets and credentials and all.
    #[test]
    fn a_url_naming_an_address_inside_the_cluster_is_read_as_internal() {
        for url in [
            "https://169.254.169.254/latest/meta-data/",
            "https://127.0.0.1/bundle.zip",
            "https://10.0.0.1/bundle.zip",
            "https://user:token@192.168.1.1/bundle.zip",
            "https://[::1]/bundle.zip",
            "https://[fd00::1]/bundle.zip",
            "https://[::ffff:169.254.169.254]/latest/meta-data/",
        ] {
            assert!(internal_host(url).is_some(), "{url} passed as public");
        }
        for url in [
            "https://93.184.216.34/bundle.zip",
            "https://git.example/udp/models.git",
            "https://keycloak.identity.svc.cluster.local/realms/jc",
        ] {
            assert!(
                internal_host(url).is_none(),
                "{url} is judged by the resolver, not by its spelling"
            );
        }
    }

    /// MF-28, DM-49 (T-1705): a redirect is a second URL the origin chose, and it is judged as
    /// the first one was.
    #[test]
    fn a_redirect_is_followed_only_to_a_public_https_address_and_only_so_far() {
        let url = |text: &str| Url::parse(text).expect(text);
        assert!(follows(&url("https://models.example/v2/index.json"), 0, 3));
        assert!(follows(&url("https://93.184.216.34/v2/index.json"), 2, 3));

        assert!(
            !follows(&url("http://models.example/v2/index.json"), 0, 3),
            "a hop out of TLS is a source that can be rewritten in transit"
        );
        assert!(
            !follows(&url("https://169.254.169.254/latest/meta-data/"), 0, 3),
            "the metadata service is the whole reason this check exists"
        );
        assert!(
            !follows(&url("https://[fd00::1]/v2/index.json"), 0, 3),
            "a unique-local address is the cluster's own network"
        );
        assert!(
            !follows(&url("https://models.example/v2/index.json"), 3, 3),
            "a chain longer than the budget is a loop or a laundering of the first URL"
        );
        assert!(
            !follows(&url("file:///etc/passwd"), 0, 3),
            "a redirect into a scheme with no host reads a file off the node"
        );
    }
}
