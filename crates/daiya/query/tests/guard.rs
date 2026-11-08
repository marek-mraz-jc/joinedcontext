#![forbid(unsafe_code)]
//! The outbound guard on the wire (R3, §4.3): redirects refused, the size cap enforced on the
//! declared length and on the stream, the timeout capped by the operator's, DNS pinned to the
//! checked addresses, and every refusal before a connection is opened.

mod common;

use common::{Reply, closed_port, loopback_guard, mock};
use daiya_query::guard::{check_ip_allowed, is_cloud_metadata};
use daiya_query::{DsError, GuardConfig, OutboundGuard, OutboundRequest};
use ipnet::IpNet;
use reqwest::Method;
use std::net::IpAddr;
use std::time::Duration;

fn get(url: String) -> OutboundRequest {
    OutboundRequest {
        method: Method::GET,
        url,
        headers: vec![],
        body: None,
        timeout: None,
    }
}

fn guard_with(f: impl FnOnce(&mut GuardConfig)) -> OutboundGuard {
    let mut cfg = loopback_guard().config().clone();
    f(&mut cfg);
    OutboundGuard::new(cfg)
}

#[tokio::test]
async fn a_request_reaches_the_mock_with_its_method_headers_and_body() {
    let m = mock(vec![(
        "/post",
        Reply::raw(201, "made").header("x-upstream", "yes"),
    )])
    .await;
    let resp = loopback_guard()
        .send(OutboundRequest {
            method: Method::POST,
            url: format!("{}/post", m.base),
            headers: vec![("X-Test".into(), "1".into())],
            body: Some(b"payload".to_vec()),
            timeout: Some(Duration::from_secs(5)),
        })
        .await
        .unwrap();
    assert_eq!(resp.status, 201);
    assert_eq!(resp.body, b"made");
    assert!(
        resp.headers
            .iter()
            .any(|(k, v)| k == "x-upstream" && v == "yes")
    );
    let head = m.head(0);
    assert!(head.starts_with("POST /post"), "{head}");
    assert!(head.to_ascii_lowercase().contains("x-test: 1"), "{head}");
    assert!(head.ends_with("payload"), "{head}");
}

#[tokio::test]
async fn redirects_oversized_and_broken_answers_are_refused() {
    let m = mock(vec![
        (
            "/redirect",
            Reply::raw(302, "").header("location", "http://169.254.169.254/"),
        ),
        ("/declared", Reply::raw(200, &"x".repeat(2_000))),
        ("/streamed", Reply::raw(200, &"y".repeat(2_000)).streamed()),
        ("/short", Reply::raw(200, "only ten b").claim_length(100)),
        ("/fits", Reply::raw(200, &"z".repeat(1_000)).streamed()),
    ])
    .await;
    let g = guard_with(|c| c.max_response_bytes = 1_000);
    let err = g
        .send(get(format!("{}/redirect", m.base)))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, DsError::GuardRefused(msg) if msg.contains("redirect with status 302")),
        "{err}"
    );
    let err = g
        .send(get(format!("{}/declared", m.base)))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("content-length 2000 exceeds"),
        "{err}"
    );
    let err = g
        .send(get(format!("{}/streamed", m.base)))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("exceeded maximum size of 1000"),
        "{err}"
    );
    let err = g.send(get(format!("{}/short", m.base))).await.unwrap_err();
    assert!(
        matches!(&err, DsError::Upstream { status: 502, message } if message.contains("response stream")),
        "{err}"
    );
    assert_eq!(
        g.send(get(format!("{}/fits", m.base)))
            .await
            .unwrap()
            .body
            .len(),
        1_000
    );
    // The redirect was not followed.
    assert_eq!(m.count(), 5);
}

#[tokio::test]
async fn the_operator_timeout_caps_the_caller_timeout() {
    let m = mock(vec![(
        "/slow",
        Reply::raw(200, "late").delay(Duration::from_secs(3)),
    )])
    .await;
    let g = guard_with(|c| c.timeout = Duration::from_millis(200));
    let mut req = get(format!("{}/slow", m.base));
    req.timeout = Some(Duration::from_secs(30));
    let started = std::time::Instant::now();
    let err = g.send(req).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        matches!(&err, DsError::Upstream { status: 502, message } if message.contains("127.0.0.1")),
        "{err}"
    );
    // The caller's shorter timeout wins too.
    let mut req = get(format!("{}/slow", m.base));
    req.timeout = Some(Duration::from_millis(100));
    assert!(loopback_guard().send(req).await.is_err());

    let port = closed_port().await;
    let err = loopback_guard()
        .send(get(format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap_err();
    assert!(
        matches!(err, DsError::Upstream { status: 502, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn names_resolve_once_and_every_address_is_checked() {
    let m = mock(vec![("/", Reply::raw(200, "ok"))]).await;
    // localhost resolves to loopback: refused unless loopback is opened.
    let named = guard_with(|c| {
        c.allow_hosts = vec!["localhost".into()];
        c.allowed_networks = vec![];
    });
    let err = named
        .send(get(format!("http://localhost:{}/", m.port)))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("loopback"), "{err}");
    let opened = guard_with(|c| {
        c.allow_hosts = vec!["LOCALHOST".into()];
        c.allowed_networks = vec![];
        c.allow_loopback_for_tests = true;
    });
    let resp = opened
        .send(get(format!("http://localhost:{}/", m.port)))
        .await
        .unwrap();
    assert_eq!(resp.body, b"ok");
    let addrs = opened.resolve_checked(" localhost ", 80).await.unwrap();
    assert!(addrs.iter().all(|a| a.ip().is_loopback() && a.port() == 80));

    let missing = guard_with(|c| c.allow_hosts = vec!["daiya-test-host.invalid".into()]);
    let err = missing
        .resolve_checked("daiya-test-host.invalid", 443)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("DNS resolution failed"), "{err}");

    // An allow-list entry may pin a port; an IP literal is checked without DNS.
    let pinned = guard_with(|c| c.allow_hosts = vec![format!("127.0.0.1:{}", m.port)]);
    assert!(pinned.send(get(format!("{}/", m.base))).await.is_ok());
    let err = pinned
        .send(get(format!("http://127.0.0.1:{}/", m.port + 1)))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("allow-list"), "{err}");
    let v6 = guard_with(|c| {
        c.allow_hosts = vec!["[::1]".into()];
        c.allow_loopback_for_tests = true;
    });
    assert_eq!(
        v6.resolve_checked("[::1]", 9).await.unwrap(),
        vec!["[::1]:9".parse().unwrap()]
    );
    assert!(v6.validate_url("http://[::1]:9/x").is_ok());
}

#[test]
fn urls_are_refused_before_any_lookup() {
    let g = loopback_guard();
    for (url, needle) in [
        ("not a url", "invalid URL"),
        ("ftp://127.0.0.1/x", "scheme 'ftp'"),
        ("http://user@127.0.0.1/", "userinfo"),
        ("http://:pw@127.0.0.1/", "userinfo"),
        ("http://10.0.0.1/", "allow-list"),
    ] {
        let err = g.validate_url(url).unwrap_err();
        assert!(err.to_string().contains(needle), "{url}: {err}");
    }
    let (_, host, port) = g.validate_url("http://127.0.0.1/x").unwrap();
    assert_eq!((host.as_str(), port), ("127.0.0.1", 80));
    // An allow-listed private literal is still refused unless its network is opened.
    let private = OutboundGuard::new(GuardConfig {
        allow_hosts: vec!["10.1.2.3".into()],
        ..GuardConfig::default()
    });
    assert!(private.validate_url("https://10.1.2.3/").is_err());
    let opened = OutboundGuard::new(GuardConfig {
        allow_hosts: vec!["10.1.2.3".into(), "  ".into()],
        allowed_networks: vec!["10.0.0.0/8".parse::<IpNet>().unwrap()],
        ..GuardConfig::default()
    });
    assert!(opened.validate_url("https://10.1.2.3/").is_ok());
    assert!(opened.check_ip("::ffff:10.9.9.9".parse().unwrap()).is_ok());
    assert!(opened.check_ip("11.0.0.1".parse().unwrap()).is_ok());
    assert!(opened.check_ip("192.168.0.1".parse().unwrap()).is_err());
    assert_eq!(GuardConfig::default().max_response_bytes, 8 * 1024 * 1024);
}

#[test]
fn address_classes() {
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    for blocked in [
        "192.0.2.1",
        "198.51.100.7",
        "224.0.0.1",
        "198.18.0.1",
        "192.88.99.1",
        "::",
        "ff02::1",
        "2001:db8::1",
        "fd00:ec2::1",
    ] {
        assert!(check_ip_allowed(ip(blocked), true).is_err(), "{blocked}");
    }
    // Opened loopback lets the loopback forms through, never the metadata address.
    for ok in ["127.0.0.1", "::1", "::ffff:127.0.0.1", "::127.0.0.1"] {
        assert!(check_ip_allowed(ip(ok), true).is_ok(), "{ok}");
    }
    assert!(check_ip_allowed(ip("169.254.169.254"), true).is_err());
    assert!(is_cloud_metadata(ip("169.254.169.254")));
    assert!(is_cloud_metadata(ip("::ffff:169.254.169.254")));
    assert!(is_cloud_metadata(ip("fd00:ec2::254")));
    assert!(!is_cloud_metadata(ip("169.254.169.253")));
    assert!(!is_cloud_metadata(ip("fd00:ec2::253")));
}
