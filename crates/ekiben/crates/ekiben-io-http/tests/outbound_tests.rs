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

/// A `name:port` entry opens a private address on that port alone: jc lists the MQTT brokers'
/// 1883 so, and a stream still cannot reach their HTTP API on the same address.
#[tokio::test]
async fn a_private_host_with_a_port_resolves_on_that_port_alone() {
    let resources = ResourceManager::new();
    let org = resources.set_org_quotas(
        &OrgId::new("o"),
        OrgQuotas {
            private_hosts: vec!["10.43.0.9:1883".to_string()],
            ..Default::default()
        },
    );
    let mqtt = url::Url::parse("http://10.43.0.9:1883/").unwrap();
    assert_eq!(resolve_checked(&mqtt, &org).await.unwrap()[0].port(), 1883);
    let api = url::Url::parse("http://10.43.0.9:6060/").unwrap();
    assert!(resolve_checked(&api, &org).await.is_err());
    let loopback = resources.set_org_quotas(
        &OrgId::new("l"),
        OrgQuotas {
            private_hosts: vec!["127.0.0.1:1883".to_string()],
            ..Default::default()
        },
    );
    let own = url::Url::parse("http://127.0.0.1:1883/").unwrap();
    assert!(resolve_checked(&own, &loopback).await.is_err());
}

/// An IPv4-compatible IPv6 address (::a.b.c.d, deprecated) is refused even for a public IPv4.
#[test]
fn an_ipv4_compatible_address_is_refused_even_when_public() {
    let public = std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0x0808, 0x0808);
    assert_eq!(
        classify(IpAddr::V6(public)),
        AddrClass::Forbidden("IPv4-compatible")
    );
}

/// A `.localhost` name is loopback without a lookup; a name that does not resolve is named.
#[tokio::test]
async fn localhost_names_are_refused_and_a_failed_lookup_is_named() {
    use ekiben_io_http::outbound::resolve_host_checked;
    let resources = ResourceManager::new();
    let org = resources.set_org_quotas(&OrgId::new("o"), OrgQuotas::default());
    for host in ["LOCALHOST", "api.Localhost"] {
        let err = resolve_host_checked(host, 80, &org)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("is loopback and is refused"), "{host}: {err}");
    }
    let err = resolve_host_checked("", 80, &org)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("failed to resolve ''"), "{err}");
}

/// A peer that never completes the handshake times out; no address at all is named too.
#[tokio::test]
async fn a_connect_that_never_completes_times_out() {
    use ekiben_io_http::outbound::connect_checked;
    use std::time::Duration;
    let err = connect_checked(&[], Duration::from_millis(10))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("connect failed: no address"),
        "{err}"
    );
    // A listener with a full accept queue drops further handshakes.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(1).unwrap();
    let addr = listener.local_addr().unwrap();
    let mut queued = Vec::new();
    for _ in 0..8 {
        if let Ok(Ok(s)) = tokio::time::timeout(
            Duration::from_millis(50),
            tokio::net::TcpStream::connect(addr),
        )
        .await
        {
            queued.push(s);
        }
    }
    let err = connect_checked(&[addr], Duration::from_millis(50))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("connect timed out"), "{err}");
    drop((queued, listener));
}
