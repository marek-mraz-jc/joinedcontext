//! The outbound rule of connections ekiben opens itself (E3 websocket).

use std::net::IpAddr;

use ekiben_core::{OrgId, OrgQuotas, ResourceManager};
use ekiben_io_http::outbound::{check_ip, classify, resolve_checked, AddrClass};

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn addresses_are_classified_by_what_they_reach() {
    for s in [
        "127.0.0.1",
        "0.0.0.0",
        "169.254.169.254",
        "100.100.100.200",
        "224.0.0.1",
        "255.255.255.255",
        "::1",
        "::",
        "fe80::1",
        "ff02::1",
        "fec0::1",
        "fd00:ec2::254",
        "::ffff:127.0.0.1",
        "64:ff9b::7f00:1",
        "2002:7f00:1::",
        "2001:0:4136:e378::1",
        "::7f00:1",
    ] {
        assert!(matches!(classify(ip(s)), AddrClass::Forbidden(_)), "{s}");
    }
    for s in [
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "100.64.0.1",
        "198.18.0.1",
        "fd12::1",
        "::ffff:10.0.0.1",
    ] {
        assert_eq!(classify(ip(s)), AddrClass::Private, "{s}");
    }
    for s in ["1.1.1.1", "2606:4700:4700::1111", "::ffff:8.8.8.8"] {
        assert_eq!(classify(ip(s)), AddrClass::Public, "{s}");
    }
}

#[test]
fn private_hosts_open_private_ranges_but_never_loopback_or_metadata() {
    assert!(check_ip(ip("10.0.0.5"), false, true).is_ok());
    assert!(check_ip(ip("10.0.0.5"), false, false).is_err());
    assert!(check_ip(ip("127.0.0.1"), false, true).is_err());
    assert!(check_ip(ip("169.254.169.254"), false, true).is_err());
    assert!(check_ip(ip("127.0.0.1"), true, false).is_ok());
}

#[tokio::test]
async fn resolution_checks_scheme_host_and_every_address() {
    let resources = ResourceManager::new();
    let org = resources.set_org_quotas(
        &OrgId::new("o"),
        OrgQuotas {
            private_hosts: vec!["gateway.internal".to_string()],
            ..Default::default()
        },
    );
    for url in [
        "ws://127.0.0.1:9/x",
        "ws://[::1]:9/x",
        "ws://169.254.169.254/x",
        "ws://localhost/x",
        "ftp://example.com/x",
    ] {
        let u = url::Url::parse(url).unwrap();
        assert!(resolve_checked(&u, &org).await.is_err(), "{url}");
    }
    let public = url::Url::parse("wss://1.1.1.1/x").unwrap();
    let addrs = resolve_checked(&public, &org).await.unwrap();
    assert_eq!(addrs[0].port(), 443);
    assert!(org.is_private_host_allowed("GATEWAY.internal"));
    assert!(!org.is_private_host_allowed("other.internal"));
}
