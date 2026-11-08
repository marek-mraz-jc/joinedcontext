#![forbid(unsafe_code)]
//! The outbound SSRF guard against a real local HTTP server (W12): schemes, userinfo, the
//! allow-list, forbidden addresses (literal and resolved), upstream errors, redirects, the 1 MiB
//! cap (declared and streamed), invalid JSON, bearer tokens and form posts.

use kaisatsu_oidc::guard::{MAX_OUTBOUND_BYTES, OutboundGuard, OutboundPolicy};
use serde_json::Value;
use std::net::IpAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves one canned answer per path; records the last request head it saw.
async fn server() -> (u16, std::sync::Arc<parking_lot::Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(parking_lot::Mutex::new(String::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let seen = seen2.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                *seen.lock() = head.clone();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let big = "x".repeat(MAX_OUTBOUND_BYTES + 10);
                let resp: Vec<u8> = match path.as_str() {
                    "/ok" => b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"keys\":[]}".to_vec(),
                    "/fail" => b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                    "/redirect" => b"HTTP/1.1 302 Found\r\nlocation: http://169.254.169.254/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                    "/declared-big" => format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", MAX_OUTBOUND_BYTES + 10).into_bytes(),
                    "/streamed-big" => {
                        let mut r = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n".to_vec();
                        for chunk in big.as_bytes().chunks(64 * 1024) {
                            r.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                            r.extend_from_slice(chunk);
                            r.extend_from_slice(b"\r\n");
                        }
                        r.extend_from_slice(b"0\r\n\r\n");
                        r
                    }
                    "/not-json" => b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello".to_vec(),
                    "/broken" => b"HTTP/1.1 200 OK\r\ncontent-length: 50\r\nconnection: close\r\n\r\nshort".to_vec(),
                    _ => b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                };
                let _ = sock.write_all(&resp).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (port, seen)
}

fn local_guard() -> OutboundGuard {
    OutboundGuard::new(OutboundPolicy {
        in_cluster_hosts: vec!["127.0.0.1".into(), ".svc.cluster.local".into()],
        ..OutboundPolicy::default()
    })
}

#[tokio::test]
async fn real_fetches_through_the_guard() {
    let (port, seen) = server().await;
    let g = local_guard();
    let base = format!("http://127.0.0.1:{port}");
    let v: Value = g
        .get_json(&format!("{base}/ok"), None, Some("tok-1"))
        .await
        .unwrap();
    assert_eq!(v["keys"], serde_json::json!([]));
    assert!(
        seen.lock()
            .to_ascii_lowercase()
            .contains("authorization: bearer tok-1")
    );
    for (path, want) in [
        ("/fail", "upstream answered 500"),
        ("/redirect", "upstream answered 302"),
        ("/declared-big", "1 MiB"),
        ("/streamed-big", "1 MiB"),
        ("/not-json", "invalid JSON"),
    ] {
        let e = g
            .get_json::<Value>(&format!("{base}{path}"), None, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains(want), "{path}: {e}");
    }
    assert!(
        g.get_json::<Value>(&format!("{base}/broken"), None, None)
            .await
            .is_err()
    );

    let (st, body) = g
        .post_form(&format!("{base}/ok"), &[("logout_token", "t")])
        .await
        .unwrap();
    assert_eq!((st, body.as_slice()), (200, b"{\"keys\":[]}".as_slice()));
    assert!(seen.lock().contains("logout_token=t"));
    let (st, _) = g.post_form(&format!("{base}/fail"), &[]).await.unwrap();
    assert_eq!(st, 500);
    let e = g
        .post_form(&format!("{base}/streamed-big"), &[])
        .await
        .unwrap_err();
    assert!(e.to_string().contains("1 MiB"));
    // A closed port: the connection error carries no URL.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    for r in [
        g.get_json::<Value>(&format!("http://127.0.0.1:{closed}/x"), None, None)
            .await
            .map(|_| ()),
        g.post_form(&format!("http://127.0.0.1:{closed}/x"), &[])
            .await
            .map(|_| ()),
    ] {
        assert!(r.is_err());
    }
}

#[tokio::test]
async fn refusals_before_any_connection() {
    let g = OutboundGuard::default();
    for (url, want) in [
        ("not a url", "invalid outbound URL"),
        ("https://user:pw@example.org/x", "userinfo"),
        ("data:text/plain,hi", "no host"),
        ("http://example.org/x", "https required"),
        ("ftp://example.org/x", "https required"),
        ("https://127.0.0.1/x", "refused by SSRF guard"),
        ("https://[::1]/x", "refused by SSRF guard"),
        ("https://169.254.169.254/latest", "refused by SSRF guard"),
        ("https://[::ffff:10.0.0.1]/x", "refused by SSRF guard"),
    ] {
        let e = g.validate_url(url, None).unwrap_err().to_string();
        assert!(e.contains(want), "{url}: {e}");
        assert!(g.get_json::<Value>(url, None, None).await.is_err());
        assert!(g.post_form(url, &[]).await.is_err());
    }
    let allow = vec!["claude.ai".to_owned()];
    assert!(g.validate_url("https://CLAUDE.ai/x", Some(&allow)).is_ok());
    assert!(
        g.validate_url("https://evil.example/x", Some(&allow))
            .unwrap_err()
            .to_string()
            .contains("allow-list")
    );
    let listed = OutboundGuard::new(OutboundPolicy {
        allowed_hosts: Some(allow),
        ..OutboundPolicy::default()
    });
    assert!(listed.validate_url("https://vscode.dev/x", None).is_err());
    // In-cluster hosts may use http and private addresses.
    assert!(
        g.validate_url("http://jwks.kube-system.svc.cluster.local/x", None)
            .is_ok()
    );
    assert!(
        g.validate_url("http://kubernetes.default.svc/x", None)
            .is_ok()
    );
    assert!(
        g.validate_url("http://kubernetes.default.svc.evil.example/x", None)
            .is_err()
    );
    // A public name that resolves to loopback, and a name that does not resolve.
    let e = g
        .get_json::<Value>("https://localhost:9/x", None, None)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("private/loopback"), "{e}");
    let e = g.post_form("https://localhost:9/x", &[]).await.unwrap_err();
    assert!(e.to_string().contains("private/loopback"), "{e}");
    assert!(
        g.get_json::<Value>("https://no-such-host.invalid/x", None, None)
            .await
            .is_err()
    );
    assert!(
        g.post_form("https://no-such-host.invalid/x", &[])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn mocks_answer_offline_with_the_same_rules() {
    let g = OutboundGuard::default();
    g.insert_mock("https://idp.example/jwks", 200, br#"{"keys":[1]}"#.to_vec());
    g.insert_mock("https://idp.example/down", 503, Vec::new());
    g.insert_mock(
        "https://idp.example/big",
        200,
        vec![b' '; MAX_OUTBOUND_BYTES + 1],
    );
    g.insert_mock("https://idp.example/text", 200, b"nope".to_vec());
    let v: Value = g
        .get_json("https://idp.example/jwks", None, None)
        .await
        .unwrap();
    assert_eq!(v["keys"][0], 1);
    for (u, want) in [
        ("https://idp.example/down", "503"),
        ("https://idp.example/big", "1 MiB"),
        ("https://idp.example/text", "invalid JSON"),
    ] {
        let e = g
            .get_json::<Value>(u, None, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains(want), "{u}: {e}");
    }
    assert_eq!(
        g.post_form("https://idp.example/down", &[])
            .await
            .unwrap()
            .0,
        503
    );
}

#[test]
fn forbidden_address_ranges() {
    let bad = [
        "10.1.2.3",
        "172.16.0.1",
        "192.168.1.1",
        "127.0.0.1",
        "169.254.169.254",
        "0.1.2.3",
        "100.64.0.1",
        "192.0.0.8",
        "198.18.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "224.0.0.1",
        "192.0.2.1",
        "::1",
        "::",
        "fc00::1",
        "fe80::1",
        "fec0::1",
        "100::1",
        "2001:db8::1",
        "2001::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::10.0.0.1",
        "64:ff9b::a9fe:a9fe",
        "2002:0a00:0001::1",
    ];
    for ip in bad {
        assert!(
            OutboundGuard::is_forbidden_ip(ip.parse::<IpAddr>().unwrap()),
            "{ip}"
        );
    }
    for ip in [
        "8.8.8.8",
        "2606:4700::1111",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
        "2002:0808:0808::1",
    ] {
        assert!(
            !OutboundGuard::is_forbidden_ip(ip.parse::<IpAddr>().unwrap()),
            "{ip}"
        );
    }
}
