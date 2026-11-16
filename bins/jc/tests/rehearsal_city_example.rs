//! Domains H6 (T-7004, evidence/T-6991/design.md §11): a fresh `jc` at `city.example` with one
//! Organization on its own origins (working, open data, API and its own sign-in host, T-7001),
//! one on `host = "auto"` and one on a single host, asked over HTTP as the edge passes requests:
//! the H2a refusals hold on each form, a dataset IRI from before the move answers `200` or `308`,
//! and the installation's issuer is the one it was.

use jc::config::{Cli, Config};
use jc::{Module, MountMode};
use reqwest::{StatusCode, header};
use serde_json::{Value, json};
use std::net::SocketAddr;
use tokio_util::sync::CancellationToken;

const TOKEN_FIVE: &str = "five-portal-publisher-token-0123456789";
const REALM: &str = "jc";

struct Jc {
    _dir: tempfile::TempDir,
    running: jc::Running,
}

/// The installation realm and the `five` Organization's realm, as a deployment import brings them.
fn import_dir(dir: &std::path::Path) -> std::path::PathBuf {
    let import = dir.join("import");
    std::fs::create_dir_all(&import).unwrap();
    for realm in [REALM, "five"] {
        let file = json!({"realm": realm, "enabled": true});
        std::fs::write(import.join(format!("{realm}.json")), file.to_string()).unwrap();
    }
    import
}

async fn start() -> Jc {
    let dir = tempfile::tempdir().expect("tempdir");
    let token = dir.path().join("five-token");
    std::fs::write(&token, TOKEN_FIVE).unwrap();
    let text = format!(
        "[identity]\nrealm = \"{REALM}\"\n\
         [kaisatsu]\nimport_dir = \"{}\"\n\
         [[organizations]]\nname = \"five\"\n\
         catalogue_id = \"00000000-0000-7000-8000-0000000000f5\"\nportal_token_file = \"{}\"\n\
         hosts = {{ portal = \"smartcity.five.test\", opendata = \"opendata.five.test\", \
         api = \"api.five.test\", idm = \"idm.five.test\" }}\n\
         [[organizations]]\nname = \"zilina\"\n\
         catalogue_id = \"00000000-0000-7000-8000-0000000000a7\"\nhost = \"auto\"\n\
         [[organizations]]\nname = \"one\"\n\
         catalogue_id = \"00000000-0000-7000-8000-0000000000e1\"\n\
         hosts = {{ portal = \"smartcity.one.test\" }}\n",
        import_dir(dir.path()).display(),
        token.display()
    );
    let file = Config::parse_file(std::path::Path::new("jc.toml"), &text).expect("file");
    let mut cfg = Config::merge(
        Cli {
            role: Some("dataroom,kaisatsu".into()),
            listen: Some("127.0.0.1:0".parse().unwrap()),
            base_host: Some("city.example".into()),
            scheme: Some("http".into()),
            ..Cli::default()
        },
        file,
    )
    .expect("config");
    // jc-dev's form: slug mounts on the base host, the Organizations on hosts of their own.
    cfg.mode = MountMode::Slug;
    cfg.state_dir = dir.path().join("state");
    cfg.trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
    assert!(cfg.roles.contains(&Module::Kaisatsu) && cfg.roles.contains(&Module::Dataroom));
    assert_eq!(cfg.login_host, "idm.city.example");
    let running = jc::start(&cfg, CancellationToken::new())
        .await
        .expect("start");
    Jc { _dir: dir, running }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// A request as the edge passes it: `Host` the address's host.
fn on(
    addr: SocketAddr,
    method: reqwest::Method,
    host: &str,
    path: &str,
) -> reqwest::RequestBuilder {
    client()
        .request(method, format!("http://{addr}{path}"))
        .header(header::HOST, host)
}

async fn get(addr: SocketAddr, host: &str, path: &str) -> (StatusCode, String) {
    let resp = on(addr, reqwest::Method::GET, host, path)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.text().await.unwrap_or_default())
}

fn issuer(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    v["issuer"].as_str().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn each_origin_form_keeps_its_refusals_over_http() {
    let jc = start().await;
    let addr = jc.running.addr;
    // Open data hosts, the Organization's own and the shared one of `auto`: reads only, no
    // credential reaches a module, no cookie leaves.
    for (host, path) in [
        ("opendata.five.test", "/api/3/action/package_create"),
        (
            "opendata.city.example",
            "/zilina/api/3/action/package_create",
        ),
    ] {
        for method in [
            reqwest::Method::POST,
            reqwest::Method::PUT,
            reqwest::Method::PATCH,
            reqwest::Method::DELETE,
        ] {
            let resp = on(addr, method.clone(), host, path)
                .header(header::AUTHORIZATION, TOKEN_FIVE)
                .json(&json!({"name": "x"}))
                .send()
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {host}"
            );
            assert!(
                resp.headers().contains_key(header::ALLOW),
                "{method} {host}"
            );
        }
        let resp = on(
            addr,
            reqwest::Method::GET,
            host,
            path.replace("package_create", "package_list").as_str(),
        )
        .header(header::COOKIE, "ckan_lang=sk; jc_session=x")
        .send()
        .await
        .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{host}");
        assert!(resp.headers().get(header::SET_COOKIE).is_none(), "{host}");
    }
    // A host outside the registry names nothing.
    let (status, body) = get(addr, "smartcity.evil.example", "/data/dataset").await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    assert!(!body.contains("five") && !body.contains("zilina"), "{body}");
    // Every Location names a configured host, under a spoofed Host and X-Forwarded-Host.
    for (host, forwarded) in [
        ("evil.example", "smartcity.five.test"),
        ("smartcity.five.test", "evil.example"),
    ] {
        let resp = on(addr, reqwest::Method::GET, host, "/data/dataset?q=air")
            .header("x-forwarded-host", forwarded)
            .header(header::ACCEPT, "text/html")
            .send()
            .await
            .unwrap();
        if let Some(location) = resp.headers().get(header::LOCATION) {
            let location = location.to_str().unwrap();
            assert!(
                !location.contains("evil") && !location.starts_with("//"),
                "{location}"
            );
        }
    }
    // The single-host Organization answers its catalogue as a path of its working host.
    let (status, body) = get(
        addr,
        "smartcity.one.test",
        "/data/api/3/action/organization_show?id=one",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_dataset_iri_from_before_the_move_answers_200_or_308() {
    let jc = start().await;
    let addr = jc.running.addr;
    let dataset = json!({
        "name": "five-air", "title": "Air", "notes": "Air of five", "owner_org": "five",
        "license_id": "cc-by",
    });
    let resp = on(
        addr,
        reqwest::Method::POST,
        "smartcity.five.test",
        "/data/api/3/action/package_create",
    )
    .header(header::AUTHORIZATION, TOKEN_FIVE)
    .json(&dataset)
    .send()
    .await
    .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "{}",
        resp.text().await.unwrap_or_default()
    );
    // The page's IRI on the working host before the Organization had an open data host.
    let resp = on(
        addr,
        reqwest::Method::GET,
        "smartcity.five.test",
        "/data/dataset/five-air",
    )
    .header(header::ACCEPT, "text/html")
    .send()
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        resp.headers()[header::LOCATION],
        "http://opendata.five.test/dataset/five-air"
    );
    let (status, body) = get(addr, "opendata.five.test", "/dataset/five-air").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Air"), "{body}");
    // The API form of the same IRI is served in place.
    let (status, body) = get(
        addr,
        "smartcity.five.test",
        "/data/api/3/action/package_show?id=five-air",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn the_issuers_come_from_the_registry() {
    let jc = start().await;
    let addr = jc.running.addr;
    let discovery = |realm: &str| format!("/realms/{realm}/.well-known/openid-configuration");
    // The installation realm where it answered before `hosts.idm` existed: `/login` on the base
    // host (jc-dev's form).
    let path = format!("/login{}", discovery(REALM));
    let (status, body) = get(addr, "city.example", &path).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        issuer(&body),
        format!("http://city.example/login/realms/{REALM}")
    );
    // The Organization's realm on its own sign-in host, whatever a client forwards.
    for forwarded in [None, Some("city.example"), Some("evil.example")] {
        let mut req = on(
            addr,
            reqwest::Method::GET,
            "idm.five.test",
            &discovery("five"),
        );
        if let Some(f) = forwarded {
            req = req.header("x-forwarded-host", f);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if status == StatusCode::OK {
            assert_eq!(
                issuer(&body),
                "http://idm.five.test/realms/five",
                "{forwarded:?}"
            );
        } else {
            assert!(
                matches!(
                    status,
                    StatusCode::NOT_FOUND | StatusCode::MISDIRECTED_REQUEST
                ),
                "{forwarded:?}: {status} {body}"
            );
        }
    }
    // Each realm answers on its own sign-in host alone.
    for (host, realm) in [
        ("city.example", "/login/realms/five"),
        ("idm.five.test", "/realms/jc"),
        ("smartcity.five.test", "/realms/five"),
    ] {
        let (status, body) = get(
            addr,
            host,
            &format!("{realm}/.well-known/openid-configuration"),
        )
        .await;
        assert_ne!(status, StatusCode::OK, "{host} {realm}: {body}");
        assert!(!body.contains("issuer"), "{host} {realm}: {body}");
    }
}
