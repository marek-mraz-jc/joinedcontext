//! Two Organizations of one `jc` on their own hosts (`[[organizations]]`): each open data portal
//! is its own dataroom catalogue, the Portal's publisher token of one writes only its own
//! catalogue and only under its Organization's CKAN organisation, and nothing of one shows on the
//! other's host in any listing, count, search, feed or page.

use jc::config::{Cli, Config};
use jc::{Module, MountMode};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::net::SocketAddr;
use tokio_util::sync::CancellationToken;

const TOKEN_ALPHA: &str = "alpha-portal-publisher-token-0123456789";
const TOKEN_BETA: &str = "beta-portal-publisher-token-01234567890";

struct Jc {
    _dir: tempfile::TempDir,
    running: jc::Running,
}

/// jc with dataroom alone, in host mode on `city.test` over plain http, both Organizations of
/// `tests/fixtures/two-orgs.toml` with a Portal token each; loopback is a trusted proxy, so the
/// test client can forward a host.
async fn start() -> Jc {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut text = String::new();
    for (org, token) in [("alpha", TOKEN_ALPHA), ("beta", TOKEN_BETA)] {
        let path = dir.path().join(format!("{org}-token"));
        std::fs::write(&path, token).unwrap();
        text.push_str(&format!("\n[[organizations]]\nname = \"{org}\"\n"));
        let id = if org == "alpha" { "a1" } else { "b1" };
        text.push_str(&format!(
            "catalogue_id = \"00000000-0000-7000-8000-0000000000{id}\"\nportal_token_file = \"{}\"\n",
            path.display()
        ));
        text.push_str(&format!(
            "hosts = {{ portal = \"portal.{org}.test\", opendata = \"opendata.{org}.test\", api = \"api.{org}.test\", mcp = \"mcp.{org}.test\" }}\n"
        ));
    }
    let file = Config::parse_file(std::path::Path::new("jc.toml"), &text).expect("file");
    let mut cfg = Config::merge(
        Cli {
            role: Some("dataroom".into()),
            listen: Some("127.0.0.1:0".parse().unwrap()),
            base_host: Some("city.test".into()),
            scheme: Some("http".into()),
            ..Cli::default()
        },
        file,
    )
    .expect("config");
    cfg.mode = MountMode::Host;
    cfg.state_dir = dir.path().join("state");
    cfg.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    assert_eq!(cfg.roles, vec![Module::Dataroom]);
    let running = jc::start(&cfg, CancellationToken::new())
        .await
        .expect("start");
    Jc { _dir: dir, running }
}

struct Call<'a> {
    host: &'a str,
    forwarded: Option<(&'a str, &'a str)>,
    token: Option<&'a str>,
    body: Option<Value>,
}

impl<'a> Call<'a> {
    fn on(host: &'a str) -> Self {
        Self {
            host,
            forwarded: None,
            token: None,
            body: None,
        }
    }
}

async fn send(addr: SocketAddr, call: Call<'_>, path: &str) -> (StatusCode, String) {
    let method = if call.body.is_some() {
        Method::POST
    } else {
        Method::GET
    };
    let mut req = reqwest::Client::new()
        .request(method, format!("http://{addr}{path}"))
        .header("host", call.host);
    if let Some((host, proto)) = call.forwarded {
        req = req
            .header("x-forwarded-host", host)
            .header("x-forwarded-proto", proto);
    }
    if let Some(token) = call.token {
        req = req.header("authorization", token);
    }
    if let Some(body) = call.body {
        req = req.json(&body);
    }
    let resp = req.send().await.expect("request");
    let status = resp.status();
    (status, resp.text().await.unwrap_or_default())
}

async fn get(addr: SocketAddr, host: &str, path: &str) -> (StatusCode, String) {
    send(addr, Call::on(host), path).await
}

fn package(name: &str, org: &str) -> Value {
    json!({
        "name": name,
        "title": format!("Title of {name}"),
        "notes": format!("Notes of {name}"),
        "owner_org": org,
        "license_id": "cc-by",
    })
}

async fn publish(
    addr: SocketAddr,
    host: &str,
    token: &str,
    body: Value,
    action: &str,
) -> (StatusCode, String) {
    let call = Call {
        token: Some(token),
        body: Some(body),
        ..Call::on(host)
    };
    send(addr, call, &format!("/api/3/action/{action}")).await
}

#[tokio::test]
async fn two_organizations_publish_and_read_only_their_own_catalogue() {
    let jc = start().await;
    let addr = jc.running.addr;
    let (alpha, beta) = ("opendata.alpha.test", "opendata.beta.test");

    // Each Portal publisher publishes under its own Organization's CKAN organisation, which jc
    // made from the configuration.
    for (host, token, name, org) in [
        (alpha, TOKEN_ALPHA, "alpha-air", "alpha"),
        (beta, TOKEN_BETA, "beta-bikes", "beta"),
    ] {
        let (status, body) = get(
            addr,
            host,
            &format!("/api/3/action/organization_show?id={org}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{host}: {body}");
        let (status, body) = publish(addr, host, token, package(name, org), "package_create").await;
        assert_eq!(status, StatusCode::OK, "{host} {name}: {body}");
    }

    // The grant of one catalogue writes nothing in the other, under either organisation.
    for (host, token, org) in [
        (beta, TOKEN_ALPHA, "beta"),
        (beta, TOKEN_ALPHA, "alpha"),
        (alpha, TOKEN_BETA, "alpha"),
        (alpha, TOKEN_BETA, "beta"),
    ] {
        let (status, body) =
            publish(addr, host, token, package("stray", org), "package_create").await;
        assert!(
            status.is_client_error(),
            "{host} {org}: a foreign token wrote: {status} {body}"
        );
    }
    // Inside its own catalogue it publishes under no other CKAN organisation (gap 6), and makes
    // none.
    let (status, body) = publish(
        addr,
        alpha,
        TOKEN_ALPHA,
        package("stray", "hel-fi"),
        "package_create",
    )
    .await;
    assert!(status.is_client_error(), "{status} {body}");
    let (status, body) = publish(
        addr,
        alpha,
        TOKEN_ALPHA,
        json!({"name": "hel-fi", "title": "Helsinki"}),
        "organization_create",
    )
    .await;
    assert!(status.is_client_error(), "{status} {body}");

    // Nothing of the other Organization on either host: lists, counts, search, pages, feeds.
    for (host, own, other) in [(alpha, "alpha", "beta"), (beta, "beta", "alpha")] {
        let own_ds = format!("{own}-");
        let other_ds = format!("{other}-");
        let (status, body) = get(addr, host, "/api/3/action/package_search?q=").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let found: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(found["result"]["count"], 1, "{host}: {body}");
        for path in [
            "/api/3/action/package_search?q=",
            "/api/3/action/package_search?q=bikes",
            "/api/3/action/package_search?q=air",
            "/api/3/action/package_list",
            "/api/3/action/organization_list?all_fields=true",
            "/api/v1/catalogue",
            "/catalog.jsonld",
            "/catalog.ttl",
            "/feeds/dataset.atom",
            "/sitemap.xml",
            "/dataset",
            "/organization",
            "/",
        ] {
            let (status, body) = get(addr, host, path).await;
            assert_eq!(status, StatusCode::OK, "{host}{path}: {body}");
            assert!(
                !body.contains(&other_ds) && !body.contains(&format!("opendata.{other}.test")),
                "{host}{path} shows {other}: {body}"
            );
            assert!(
                !body.contains(&format!("\"{other}\"")),
                "{host}{path} names {other}: {body}"
            );
        }
        let (_, body) = get(addr, host, "/api/3/action/package_list").await;
        assert!(body.contains(&own_ds), "{host}: {body}");
        // The other's addresses answer as a name nobody holds does, and say nothing of it.
        let other_pkg = format!("{other}-{}", if other == "beta" { "bikes" } else { "air" });
        for template in [
            "/api/3/action/package_show?id=NAME",
            "/api/3/action/organization_show?id=NAME",
            "/dataset/NAME",
            "/organization/NAME",
            "/feeds/organization/NAME.atom",
            "/publisher/NAME/catalog.jsonld",
        ] {
            let name = if template.contains("organization") || template.contains("publisher") {
                other.to_owned()
            } else {
                other_pkg.clone()
            };
            let (status, body) = get(addr, host, &template.replace("NAME", &name)).await;
            let (unknown, _) = get(addr, host, &template.replace("NAME", "nobody-here")).await;
            assert_eq!(status, unknown, "{host}{template} {name}: {body}");
            // An organisation's Atom feed of a name nobody holds is an empty feed.
            if !template.ends_with(".atom") {
                assert_ne!(status, StatusCode::OK, "{host}{template} {name}: {body}");
            }
            // Only the name the request itself sent comes back.
            let rest = body.replace(&name, "");
            assert!(
                !rest.contains(other) && !rest.contains("Title of"),
                "{host}{template}: {body}"
            );
        }
    }

    // Links name the catalogue's canonical base: plain http here (gap 7).
    let (_, body) = get(addr, alpha, "/catalog.jsonld").await;
    assert!(body.contains("http://opendata.alpha.test/"), "{body}");
    assert!(!body.contains("https://opendata.alpha.test"), "{body}");
    let (_, body) = get(addr, alpha, "/api/3/action/package_search?q=").await;
    assert!(
        body.contains("\"help\":\"http://opendata.alpha.test/api/3/action/help_show"),
        "{body}"
    );

    // Each catalogue frames its own Organization's Portal and API.
    for org in ["alpha", "beta"] {
        let (status, body) = get(addr, &format!("opendata.{org}.test"), "/api/v1/branding").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let branding: Value = serde_json::from_str(&body).unwrap();
        let addresses = &branding["addresses"];
        assert_eq!(addresses["api"], format!("http://api.{org}.test"), "{body}");
        assert_eq!(
            addresses["portal"],
            format!("http://portal.{org}.test"),
            "{body}"
        );
    }

    // Behind a trusted proxy the forwarded host names the catalogue, not the raw Host: the
    // router routed on alpha's, so dataroom answers alpha's.
    let call = Call {
        forwarded: Some((alpha, "https")),
        ..Call::on(beta)
    };
    let (status, body) = send(addr, call, "/api/3/action/package_list").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("alpha-air") && !body.contains("beta-bikes"),
        "{body}"
    );
    let call = Call {
        forwarded: Some((alpha, "https")),
        ..Call::on(beta)
    };
    // Its ids stay the catalogue's canonical ones (T-6797): the forwarded scheme moves none.
    let (_, body) = send(addr, call, "/catalog.jsonld").await;
    assert!(body.contains("http://opendata.alpha.test/"), "{body}");
    assert!(!body.contains("https://opendata.alpha.test"), "{body}");
    assert!(!body.contains("opendata.beta.test"), "{body}");

    // A host no Organization holds is outside the registry: 421 before any catalogue.
    let (status, _) = get(addr, "opendata.gamma.test", "/api/3/action/package_list").await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);

    jc.running.stop().await.expect("clean stop");
}
