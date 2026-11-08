//! The outbound guard: scheme, credentials, internal names and addresses, both allow-lists and
//! the DNS answer, for the first URL and for the pinned one.

use dataroom_harvest::{resolve_and_validate_host, validate_and_pin_url, validate_outbound_url};
use std::net::SocketAddr;

fn hosts(h: &[&str]) -> Vec<String> {
    h.iter().map(|s| s.to_string()).collect()
}

fn refused(url: &str, allowed: &[&str], catalogue: &[&str]) -> String {
    validate_outbound_url(url, &hosts(allowed), &hosts(catalogue), false)
        .expect_err(url)
        .to_string()
}

#[test]
fn only_https_without_credentials_is_fetched() {
    let ok = ["data.example"];
    assert!(refused("not a url", &ok, &[]).contains("is not a valid URL"));
    assert!(refused("http://data.example/x", &ok, &[]).contains("scheme 'http' is refused"));
    assert!(refused("file:///etc/passwd", &ok, &[]).contains("scheme 'file' is refused"));
    assert!(refused("https://user:pw@data.example/", &ok, &[]).contains("credentials"));
    assert!(refused("https://user@data.example/", &ok, &[]).contains("credentials"));
    let url = validate_outbound_url("https://DATA.example/feed", &hosts(&ok), &[], false).unwrap();
    assert_eq!(url.as_str(), "https://data.example/feed");
}

#[test]
fn internal_names_are_refused_even_when_a_source_lists_them() {
    for host in [
        "localhost",
        "LOCALHOST",
        "api.localhost",
        "printer.local",
        "metadata.google.internal",
        "kube-dns.kube-system.svc",
        "postgres.dev.svc.cluster.local",
        // A trailing root dot names the same host.
        "localhost.",
        "metadata.google.internal.",
        "postgres.dev.svc.cluster.local.",
    ] {
        let err = refused(&format!("https://{host}/"), &[host], &[]);
        assert!(err.contains("internal host"), "{host}: {err}");
    }
}

#[test]
fn internal_address_literals_are_refused_in_every_spelling() {
    for host in [
        "127.0.0.1",
        "2130706433",
        "0x7f000001",
        "0177.0.0.1",
        "127.1",
        "10.0.0.1",
        "169.254.169.254",
        "[::1]",
        "[::ffff:10.0.0.1]",
        "[fd00::1]",
        "[fe80::1]",
        "0.0.0.0",
    ] {
        let err = refused(&format!("https://{host}/"), &[host], &[]);
        assert!(err.contains("internal IP address"), "{host}: {err}");
    }
}

#[test]
fn a_host_must_be_on_the_source_list_and_on_a_non_empty_catalogue_list() {
    let err = refused("https://evil.example/", &["data.example"], &[]);
    assert!(
        err.contains("not in the harvest source's allowedHosts"),
        "{err}"
    );
    let err = refused(
        "https://data.example/",
        &["data.example"],
        &["other.example"],
    );
    assert!(
        err.contains("not in the catalogue's allowed_outbound_hosts"),
        "{err}"
    );
    validate_outbound_url(
        "https://data.example./x",
        &hosts(&["Data.Example"]),
        &hosts(&["other.example", "DATA.example"]),
        false,
    )
    .expect("both lists name the host");
}

#[test]
fn the_dns_answer_must_be_public() {
    // An address literal resolves to itself without a name server.
    let url = validate_outbound_url(
        "https://93.184.216.34:8443/x",
        &hosts(&["93.184.216.34"]),
        &[],
        true,
    )
    .unwrap();
    assert_eq!(url.port(), Some(8443));
    let addrs = resolve_and_validate_host("93.184.216.34", 443).unwrap();
    assert_eq!(addrs, vec![SocketAddr::from(([93, 184, 216, 34], 443))]);

    let err = resolve_and_validate_host("localhost", 443)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("host 'localhost' resolved to internal address"),
        "{err}"
    );
    let err = resolve_and_validate_host("no-such-host.invalid", 443)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("DNS resolution failed for 'no-such-host.invalid'"),
        "{err}"
    );
}

#[test]
fn a_pinned_target_carries_only_a_checked_answer() {
    let allowed = hosts(&["data.example"]);
    let public = |_: &str, port: u16| Ok(vec![SocketAddr::from(([93, 184, 216, 34], port))]);
    let t =
        validate_and_pin_url("https://data.example/a", &allowed, &[], true, Some(&public)).unwrap();
    assert_eq!(t.host, "data.example");
    assert_eq!(
        t.pinned_addrs,
        vec![SocketAddr::from(([93, 184, 216, 34], 443))]
    );

    // One internal address among public ones refuses the whole answer (DNS rebinding).
    let mixed = |_: &str, port: u16| {
        Ok(vec![
            SocketAddr::from(([93, 184, 216, 34], port)),
            SocketAddr::from(([10, 0, 0, 7], port)),
        ])
    };
    let err = validate_and_pin_url("https://data.example/a", &allowed, &[], true, Some(&mixed))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("resolved to internal address 10.0.0.7"),
        "{err}"
    );

    let empty = |_: &str, _: u16| Ok(Vec::new());
    let err = validate_and_pin_url("https://data.example/a", &allowed, &[], true, Some(&empty))
        .unwrap_err()
        .to_string();
    assert!(err.contains("returned no addresses"), "{err}");

    let failing = |h: &str, _: u16| {
        Err(dataroom_core::DataroomError::OutboundRefused(format!(
            "no answer for {h}"
        )))
    };
    let err = validate_and_pin_url(
        "https://data.example/a",
        &allowed,
        &[],
        true,
        Some(&failing),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("no answer for data.example"), "{err}");

    // The guard runs before any resolver is asked.
    let err = validate_and_pin_url("https://evil.example/", &allowed, &[], true, Some(&public))
        .unwrap_err()
        .to_string();
    assert!(err.contains("allowedHosts"), "{err}");

    // Without a resolver the system one answers; without DNS nothing is pinned.
    let lit = hosts(&["93.184.216.34"]);
    let t = validate_and_pin_url("https://93.184.216.34/", &lit, &[], true, None).unwrap();
    assert_eq!(
        t.pinned_addrs,
        vec![SocketAddr::from(([93, 184, 216, 34], 443))]
    );
    let t = validate_and_pin_url("https://93.184.216.34/", &lit, &[], false, None).unwrap();
    assert!(t.pinned_addrs.is_empty());
}
