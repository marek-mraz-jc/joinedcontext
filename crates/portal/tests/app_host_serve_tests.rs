//! The App hosts the Portal answers itself where no edge routes them (jc, AP-133,
//! edge-session-design S4a): `{name}.apps.{domain}` reaches App `name` alone, its own endpoints
//! through the gateway without the browser's cookies, a wasm App's own server through its shard
//! (AP-143), and no Portal route at all.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::apps::static_host::sri_sha384;
use joinedcontext_portal::config::{AppsDbSettings, Config};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

mod common;

const INDEX: &[u8] = b"<!doctype html><title>air quality</title>";
const BUNDLE_JS: &[u8] = b"console.log('air quality');";

/// One bundle per App with the integrity manifest CI writes beside it; removed on drop.
struct Bundles(std::path::PathBuf);

impl Bundles {
    fn new(case: &str, apps: &[&str]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "jc-app-host-{case}-{}-{}",
            std::process::id(),
            common::unique::unique()
        ));
        for app in apps {
            let dir = root.join(app);
            std::fs::create_dir_all(&dir).expect("bundle dir");
            let mut digests = serde_json::Map::new();
            for (name, bytes) in [("index.html", INDEX), ("bundle.js", BUNDLE_JS)] {
                std::fs::write(dir.join(name), bytes).expect("bundle file");
                digests.insert(name.into(), sri_sha384(bytes).into());
            }
            std::fs::write(
                dir.join("integrity.json"),
                serde_json::to_vec(&digests).expect("manifest"),
            )
            .expect("write manifest");
        }
        Self(root)
    }
}

impl Drop for Bundles {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn envelope(kind: &str, project: &str, name: &str, spec: serde_json::Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(project.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

fn app(kind: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": kind,
        "source": { "path": "." },
        "build": {},
        "visibility": "public",
        "lifecycle": "published",
        "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
            "types": ["AirQualityObserved"],
            "operations": ["queryEntity"]
        }]
    })
}

/// Three published Apps of project `ovzdusie` (a `ui`, a `wasm` and a `ui-rust` one) reading
/// space `air` through endpoint slug `airslug`, and another project's endpoint `foreignslug`.
fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for (name, kind) in [
        ("air-quality", "ui"),
        ("air-server", "wasm"),
        ("air-pod", "ui-rust"),
    ] {
        mirror.upsert(envelope("App", "ovzdusie", name, app(kind)));
    }
    // wasm Apps with the shard the publish recorded: on shard 1, on none, on one the host does not
    // run (2 of 2), and a `roles` App on shard 0.
    for (name, visibility, shard) in [
        ("air-shard", "public", Some(1)),
        ("air-noshard", "public", None),
        ("air-far", "public", Some(2)),
        ("air-roles", "roles", Some(0)),
    ] {
        let mut spec = app("wasm");
        spec["visibility"] = visibility.into();
        spec["roles"] = serde_json::json!([{ "name": "viewer" }]);
        let mut app = envelope("App", "ovzdusie", name, spec);
        app.status = shard.map(|shard| {
            serde_json::from_value(serde_json::json!({ "shard": shard })).expect("status")
        });
        mirror.upsert(app);
    }
    mirror.upsert(envelope(
        "Endpoint",
        "ovzdusie",
        "air",
        serde_json::json!({ "contextSpaceRef": "air", "slug": "airslug" }),
    ));
    mirror.upsert(envelope(
        "Endpoint",
        "other",
        "secret",
        serde_json::json!({ "contextSpaceRef": "secret", "slug": "foreignslug", "audience": "public" }),
    ));
    mirror
}

/// What the stub gateway saw: method, path and query, headers.
type Seen = Arc<Mutex<Vec<(String, String, HeaderMap)>>>;

/// A gateway on loopback that records each request and answers 200 with a cookie of its own.
async fn gateway() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let record = seen.clone();
    let router = axum::Router::new().fallback(move |req: Request<Body>| {
        let record = record.clone();
        async move {
            record.lock().unwrap().push((
                req.method().to_string(),
                req.uri().to_string(),
                req.headers().clone(),
            ));
            (
                [
                    (header::SET_COOKIE, "gw=1; Path=/"),
                    (header::CONTENT_TYPE, "application/ld+json"),
                ],
                "[]",
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });
    (format!("http://{addr}"), seen)
}

fn portal(bundles: &Bundles, gateway: Option<String>, serve: bool) -> axum::Router {
    let config = Config {
        apps_dir: Some(bundles.0.to_string_lossy().into_owned()),
        public_base_url: "https://portal.example.org".parse().unwrap(),
        apps_url: Some("https://example.org".parse().unwrap()),
        serve_app_hosts: serve,
        gateway_url: gateway,
        ..Config::for_tests()
    };
    server::app(AppState::new(config, None).with_mirror(mirror()))
}

/// Whether `got` is the bundle file `want`: the index carries the `#jc-config` the host writes
/// into it, so it is matched by its title.
fn served(got: &[u8], want: &[u8]) -> bool {
    if want == INDEX {
        String::from_utf8_lossy(got).contains("<title>air quality</title>")
    } else {
        got == want
    }
}

async fn send(
    portal: &axum::Router,
    host: &str,
    uri: &str,
    extra: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut req = Request::builder().uri(uri).header(header::HOST, host);
    for (key, value) in extra {
        req = req.header(*key, *value);
    }
    let response = portal
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, body.to_vec())
}

#[tokio::test]
async fn an_app_host_serves_its_app_at_the_root_and_under_the_old_path() {
    let bundles = Bundles::new("root", &["air-quality"]);
    let portal = portal(&bundles, None, true);
    for (host, uri, body) in [
        ("air-quality.apps.example.org", "/", INDEX),
        ("AIR-QUALITY.apps.Example.org:443", "/", INDEX),
        ("air-quality.apps.example.org", "/bundle.js", BUNDLE_JS),
        (
            "air-quality.apps.example.org",
            "/apps/air-quality/bundle.js",
            BUNDLE_JS,
        ),
        ("air-quality.apps.example.org", "/apps/air-quality/", INDEX),
    ] {
        let (status, headers, got) = send(&portal, host, uri, &[]).await;
        assert_eq!(status, StatusCode::OK, "{host}{uri}");
        assert!(served(&got, body), "{host}{uri}");
        // The App's own policy, framed by the Portal alone (AP-122).
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(
            csp.ends_with("frame-ancestors https://portal.example.org"),
            "{csp}"
        );
    }
}

#[tokio::test]
async fn an_app_host_reaches_no_portal_route_and_no_other_app() {
    let bundles = Bundles::new("isolated", &["air-quality", "air-server"]);
    let portal = portal(&bundles, None, true);
    // The Portal's API, its UI and another App's old path all land inside this App's bundle,
    // where no such file is: the App's own 404, never the Portal's answer.
    for uri in [
        "/api/v1/projects",
        "/api/v1/me",
        "/apps/air-server/",
        "/edge/apps/air-quality/admit",
        "/live-notify/x",
        "/mcp",
    ] {
        let (status, _, body) = send(&portal, "air-quality.apps.example.org", uri, &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(!served(&body, INDEX), "{uri}");
    }
    // A host one label under the apex naming no App is that name's 404; a host two labels under
    // it is no App host (jc's router never sends it to the Portal).
    let (status, _, _) = send(&portal, "nope.apps.example.org", "/", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, _, body) = send(&portal, "a.air-quality.apps.example.org", "/", &[]).await;
    assert!(!served(&body, INDEX));
    // The Portal's own host still sends an App to its host (T-2476).
    let (status, headers, _) = send(&portal, "portal.example.org", "/apps/air-quality/", &[]).await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        headers[header::LOCATION],
        "https://air-quality.apps.example.org/"
    );
}

#[tokio::test]
async fn an_app_host_hands_its_own_endpoints_to_the_gateway_without_cookies() {
    let bundles = Bundles::new("endpoint", &["air-quality"]);
    let (gateway, seen) = gateway().await;
    let portal = portal(&bundles, Some(gateway), true);
    let auth = [
        ("cookie", "jc_csrf=abc; __Host-portal=session"),
        ("authorization", "Bearer app-token"),
        ("x-access-token", "edge-token"),
        ("accept", "application/ld+json"),
    ];
    for uri in [
        "/api/endpoint/airslug/ngsi-ld/v1/entities?type=AirQualityObserved",
        "/apps/air-quality/api/endpoint/airslug/ngsi-ld/v1/entities?type=AirQualityObserved",
    ] {
        let (status, headers, body) =
            send(&portal, "air-quality.apps.example.org", uri, &auth).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(body, b"[]");
        assert!(
            headers.get(header::SET_COOKIE).is_none(),
            "the gateway sets no cookie on an App host"
        );
    }
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    for (method, uri, headers) in &seen {
        assert_eq!(method, "GET");
        assert_eq!(
            uri,
            "/api/endpoint/airslug/ngsi-ld/v1/entities?type=AirQualityObserved"
        );
        assert!(headers.get(header::COOKIE).is_none(), "{headers:?}");
        assert!(headers.get("x-access-token").is_none(), "{headers:?}");
        assert_eq!(headers[header::AUTHORIZATION], "Bearer app-token");
        assert_eq!(headers[header::ACCEPT], "application/ld+json");
    }
}

#[tokio::test]
async fn another_projects_slug_and_the_egress_path_never_reach_the_gateway() {
    let bundles = Bundles::new("foreign", &["air-quality"]);
    let (gateway, seen) = gateway().await;
    let portal = portal(&bundles, Some(gateway), true);
    for uri in [
        "/api/endpoint/foreignslug/ngsi-ld/v1/entities",
        "/api/endpoint/airslug/egress/x",
        "/api/endpoint/",
        "/apps/air-quality/api/endpoint/foreignslug/ngsi-ld/v1/entities",
    ] {
        let (status, _, _) = send(&portal, "air-quality.apps.example.org", uri, &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    // A host naming no App has no endpoint either.
    let (status, _, _) = send(
        &portal,
        "nope.apps.example.org",
        "/api/endpoint/airslug/ngsi-ld/v1/entities",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_half_this_portal_does_not_run_answers_a_503_naming_the_app() {
    let bundles = Bundles::new("halves", &["air-server", "air-pod"]);
    let (gateway, seen) = gateway().await;
    let portal = portal(&bundles, Some(gateway), true);
    let page = [("accept", "text/html")];
    // A wasm App's page is its static half; its own server is not here.
    let (status, _, body) = send(&portal, "air-server.apps.example.org", "/", &page).await;
    assert_eq!(status, StatusCode::OK);
    assert!(served(&body, INDEX));
    for (host, uri) in [
        ("air-server.apps.example.org", "/api/forecast"),
        ("air-server.apps.example.org", "/api/functions/x"),
        ("air-pod.apps.example.org", "/"),
        ("air-pod.apps.example.org", "/anything"),
    ] {
        let (status, headers, body) = send(&portal, host, uri, &page).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{host}{uri}");
        let text = String::from_utf8(body).unwrap();
        let name = host.split('.').next().unwrap();
        assert!(text.contains(&format!("<h1>{name}</h1>")), "{text}");
        assert!(headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html"));
        let (status, headers, _) = send(&portal, host, uri, &[]).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
    }
    // Both still read their own endpoints.
    for host in ["air-server.apps.example.org", "air-pod.apps.example.org"] {
        let (status, _, _) = send(
            &portal,
            host,
            "/api/endpoint/airslug/ngsi-ld/v1/entities",
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{host}");
    }
    assert_eq!(seen.lock().unwrap().len(), 2);
}

/// A WASM shard on loopback, reached as the HTTP proxy of every shard URL, so it sees the URL the
/// Portal built whole; it records each request and answers 200 with a cookie and a header of its
/// own.
async fn shard() -> (SocketAddr, Seen) {
    let seen: Seen = Arc::default();
    let record = seen.clone();
    let router = axum::Router::new().fallback(move |req: Request<Body>| {
        let record = record.clone();
        async move {
            record.lock().unwrap().push((
                req.method().to_string(),
                req.uri().to_string(),
                req.headers().clone(),
            ));
            (
                [
                    (header::SET_COOKIE, "shard=1; Path=/"),
                    (header::CONTENT_TYPE, "application/json"),
                    (header::HeaderName::from_static("x-shard"), "own"),
                ],
                r#"{"forecast":[]}"#,
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });
    (addr, seen)
}

/// A Portal serving the App hosts that trusts the process's realm, with the WASM host `host`
/// (shards, namespace) reached through stub `through`.
fn wasm_portal(
    bundles: &Bundles,
    host: Option<(u32, Option<&str>)>,
    through: SocketAddr,
) -> axum::Router {
    let mut config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    config.apps_dir = Some(bundles.0.to_string_lossy().into_owned());
    config.public_base_url = "https://portal.example.org".parse().unwrap();
    config.apps_url = Some("https://example.org".parse().unwrap());
    config.serve_app_hosts = true;
    config.apps_db = host.map(|(shards, namespace)| AppsDbSettings {
        url: "postgres://apps@db/apps".into(),
        shards,
        bucket: "apps".into(),
        host_namespace: namespace.map(str::to_owned),
        shards_through: Some(through),
    });
    server::app(AppState::new(config, None).with_mirror(mirror()))
}

async fn send_with(
    portal: &axum::Router,
    req: axum::http::request::Builder,
    body: &'static str,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = portal
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, body.to_vec())
}

#[tokio::test]
async fn a_wasm_apps_api_reaches_its_shard_under_its_own_prefix_without_cookies() {
    let bundles = Bundles::new("wasm-proxy", &["air-shard"]);
    let (through, seen) = shard().await;
    let portal = wasm_portal(&bundles, Some((2, Some("wasm-host"))), through);
    for (method, uri) in [
        ("GET", "/api/forecast?days=3"),
        ("POST", "/apps/air-shard/api/forecast?days=3"),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "air-shard.apps.example.org")
            .header(
                "cookie",
                "__Host-jc_app_air_shard=sid; __Host-portal=session",
            )
            .header("authorization", "Bearer app-token")
            .header("x-access-token", "edge-token")
            .header("connection", "keep-alive")
            .header("accept", "application/json");
        let (status, headers, body) = send_with(&portal, req, "{\"x\":1}").await;
        assert_eq!(status, StatusCode::OK, "{method} {uri}");
        assert_eq!(body, br#"{"forecast":[]}"#, "the shard's own answer");
        assert_eq!(headers["x-shard"], "own");
        assert!(
            headers.get(header::SET_COOKIE).is_none(),
            "the shard sets no cookie on an App host"
        );
    }
    let held = seen.lock().unwrap().clone();
    assert_eq!(held.len(), 2);
    for ((method, uri, headers), want) in held.iter().zip(["GET", "POST"]) {
        assert_eq!(method, want);
        // The shard of status.shard in the configured namespace, on the shard's apps port, under
        // the App's prefix, with the query.
        assert_eq!(
            uri,
            "http://jc-wasm-host-1.wasm-host.svc.cluster.local:8080/apps/air-shard/api/forecast?days=3"
        );
        assert_eq!(
            headers[header::HOST],
            "jc-wasm-host-1.wasm-host.svc.cluster.local:8080"
        );
        assert!(headers.get(header::COOKIE).is_none(), "{headers:?}");
        assert!(headers.get("x-access-token").is_none(), "{headers:?}");
        assert_eq!(headers[header::AUTHORIZATION], "Bearer app-token");
        assert_eq!(headers[header::ACCEPT], "application/json");
    }
    // The request names no part of the host: a dot segment that would leave the App's prefix
    // (another App on the shard, the host's own paths) is refused before anything is sent, and a
    // Host under another apex is no App host at all.
    for uri in [
        "/api/../../air-roles/api/x",
        "/api/%2e%2e/%2E%2E/healthz",
        "/api/.%2e/x",
    ] {
        let req = Request::builder()
            .uri(uri)
            .header(header::HOST, "air-shard.apps.example.org");
        let (status, _, _) = send_with(&portal, req, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    let req = Request::builder()
        .uri("/api/forecast")
        .header(header::HOST, "air-shard.apps.evil.example");
    send_with(&portal, req, "").await;
    assert_eq!(
        seen.lock().unwrap().len(),
        2,
        "nothing more reached the shard"
    );
}
#[tokio::test]
async fn a_wasm_app_without_a_shard_it_may_use_keeps_the_designed_503() {
    let bundles = Bundles::new("wasm-503", &["air-shard", "air-noshard", "air-far"]);
    let (through, seen) = shard().await;
    // No shard recorded, a shard the host does not run, no WASM host configured, and a host
    // without its namespace: the 503 naming the App, and the shard never asked.
    for (host, config) in [
        ("air-noshard", Some((2, Some("wasm-host")))),
        ("air-far", Some((2, Some("wasm-host")))),
        ("air-shard", None),
        ("air-shard", Some((2, None))),
        ("air-shard", Some((1, Some("wasm-host")))),
    ] {
        let portal = wasm_portal(&bundles, config, through);
        let req = Request::builder()
            .uri("/api/forecast")
            .header(header::HOST, format!("{host}.apps.example.org"));
        let (status, headers, body) = send_with(&portal, req, "").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{host} {config:?}");
        assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
        let text = String::from_utf8(body).unwrap();
        assert!(
            text.contains(&format!("the server of App '{host}'")),
            "{text}"
        );
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_signed_out_call_of_a_non_public_app_answers_401_before_anything_is_handed_on() {
    let bundles = Bundles::new("wasm-401", &["air-roles"]);
    let (through, seen) = shard().await;
    let portal = wasm_portal(&bundles, Some((2, Some("wasm-host"))), through);
    for (method, uri, accept) in [
        ("GET", "/api/forecast", "application/json"),
        ("GET", "/api/forecast", "text/html"),
        ("POST", "/apps/air-roles/api/forecast", "application/json"),
        (
            "GET",
            "/api/endpoint/airslug/ngsi-ld/v1/entities",
            "application/ld+json",
        ),
        ("GET", "/api/services/jobs", "application/json"),
    ] {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "air-roles.apps.example.org")
            .header("accept", accept)
            // Another App's session and the Portal's are no session of this App.
            .header(
                "cookie",
                "__Host-jc_app_air_shard=sid; __Host-portal=session",
            );
        let (status, headers, _) = send_with(&portal, req, "").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri} {accept}");
        assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_non_public_wasm_apps_shard_serves_only_whom_the_app_admits() {
    let bundles = Bundles::new("wasm-roles", &["air-roles"]);
    let (through, seen) = shard().await;
    let portal = wasm_portal(&bundles, Some((2, Some("wasm-host"))), through);
    let call = |token: String| {
        Request::builder()
            .uri("/api/forecast")
            .header(header::HOST, "air-roles.apps.example.org")
            .header("accept", "application/json")
            .header("authorization", format!("Bearer {token}"))
    };
    // Without the App's role, and on a token of another App's client: refused here.
    for token in [
        common::REALM.person_token("air-roles", "jana", &[]),
        common::REALM.person_token("air-shard", "jana", &["viewer"]),
        "not-a-token".to_owned(),
    ] {
        let (status, _, _) = send_with(&portal, call(token), "").await;
        assert!(
            status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
            "{status}"
        );
    }
    assert!(seen.lock().unwrap().is_empty(), "the shard never asked");
    // A viewer reaches shard 0 with their token.
    let token = common::REALM.person_token("air-roles", "jana", &["viewer"]);
    let (status, _, body) = send_with(&portal, call(token.clone()), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, br#"{"forecast":[]}"#);
    let held = seen.lock().unwrap().clone();
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[0].1,
        "http://jc-wasm-host-0.wasm-host.svc.cluster.local:8080/apps/air-roles/api/forecast"
    );
    assert_eq!(held[0].2[header::AUTHORIZATION], format!("Bearer {token}"));
}

#[tokio::test]
async fn a_dot_segment_never_carries_an_endpoint_call_past_the_slug_check() {
    let bundles = Bundles::new("dots", &["air-quality"]);
    let (gateway, seen) = gateway().await;
    let portal = portal(&bundles, Some(gateway), true);
    for uri in [
        "/api/endpoint/airslug/../foreignslug/ngsi-ld/v1/entities",
        "/api/endpoint/airslug/%2e%2e/foreignslug/ngsi-ld/v1/entities",
        "/api/endpoint/airslug/ngsi-ld/../../../v1/projects",
    ] {
        let (status, _, _) = send(&portal, "air-quality.apps.example.org", uri, &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_ui_rust_apps_pod_in_app_pods_answers_its_host_alone_with_the_apps_policy() {
    let bundles = Bundles::new("pods", &["air-quality"]);
    let (pod, seen) = gateway().await;
    let pod_authority = pod.trim_start_matches("http://").to_owned();
    let config = Config {
        apps_dir: Some(bundles.0.to_string_lossy().into_owned()),
        public_base_url: "https://portal.example.org".parse().unwrap(),
        apps_url: Some("https://example.org".parse().unwrap()),
        serve_app_hosts: true,
        // air-quality is a `ui` App: a pod named for it is never asked.
        app_pods: [("air-pod", &pod), ("air-quality", &pod)]
            .into_iter()
            .map(|(name, url)| (name.to_owned(), url.clone()))
            .collect(),
        ..Config::for_tests()
    };
    let portal = server::app(AppState::new(config, None).with_mirror(mirror()));
    let sent = [
        ("cookie", "__Host-jc_app_air_pod=x; __Host-portal=session"),
        ("x-access-token", "edge-token"),
        ("accept", "text/html"),
    ];
    for uri in [
        "/",
        "/api/stream?since=1",
        "/apps/air-pod/api/stream?since=1",
    ] {
        let (status, headers, body) = send(&portal, "air-pod.apps.example.org", uri, &sent).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(body, b"[]", "{uri}");
        assert!(headers.get(header::SET_COOKIE).is_none(), "{uri}");
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(
            csp.ends_with("frame-ancestors https://portal.example.org"),
            "{csp}"
        );
    }
    // Neither another App's host nor a `ui` App with a pod named reaches it.
    let (status, _, _) = send(
        &portal,
        "air-quality.apps.example.org",
        "/apps/air-pod/",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, body) = send(&portal, "air-quality.apps.example.org", "/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(served(&body, INDEX));
    let seen = seen.lock().unwrap().clone();
    let paths: Vec<&str> = seen.iter().map(|(_, uri, _)| uri.as_str()).collect();
    assert_eq!(paths, ["/", "/api/stream?since=1", "/api/stream?since=1"]);
    for (_, _, headers) in &seen {
        assert!(headers.get(header::COOKIE).is_none(), "{headers:?}");
        assert!(headers.get("x-access-token").is_none(), "{headers:?}");
        assert_eq!(headers[header::HOST], pod_authority.as_str());
    }
}

/// A non-public `ui-rust` App's pod hears only whom the App admits: a page file asked for without
/// a session, a forged token, a token without the App's role or of another App's client is
/// refused here, and a dot segment never leaves the App's path.
#[tokio::test]
async fn a_non_public_apps_pod_hears_only_whom_the_app_admits() {
    let bundles = Bundles::new("pod-roles", &[]);
    let (pod, seen) = gateway().await;
    let mut config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("config");
    config.apps_dir = Some(bundles.0.to_string_lossy().into_owned());
    config.public_base_url = "https://portal.example.org".parse().unwrap();
    config.apps_url = Some("https://example.org".parse().unwrap());
    config.serve_app_hosts = true;
    config.app_pods = [("pod-roles".to_owned(), pod)].into_iter().collect();
    let mirror = mirror();
    let mut spec = app("ui-rust");
    spec["visibility"] = "roles".into();
    spec["roles"] = serde_json::json!([{ "name": "viewer" }]);
    mirror.upsert(envelope("App", "ovzdusie", "pod-roles", spec));
    let portal = server::app(AppState::new(config, None).with_mirror(mirror));
    let call = |uri: &str, token: Option<String>| {
        let req = Request::builder()
            .uri(uri)
            .header(header::HOST, "pod-roles.apps.example.org")
            .header("accept", "*/*");
        match token {
            Some(token) => req.header("authorization", format!("Bearer {token}")),
            None => req,
        }
    };
    let (status, _, _) = send_with(&portal, call("/assets/app.js", None), "").await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "a file without a session: {status}"
    );
    for token in [
        common::REALM.person_token("pod-roles", "jana", &[]),
        common::REALM.person_token("air-shard", "jana", &["viewer"]),
        "not-a-token".to_owned(),
    ] {
        let (status, _, _) = send_with(&portal, call("/api/alerts", Some(token)), "").await;
        assert!(
            status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
            "{status}"
        );
    }
    assert!(seen.lock().unwrap().is_empty(), "the pod never asked");
    let token = common::REALM.person_token("pod-roles", "jana", &["viewer"]);
    let (status, _, _) = send_with(
        &portal,
        call("/assets/%2e%2e/secret", Some(token.clone())),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        seen.lock().unwrap().is_empty(),
        "a dot segment never reaches the pod"
    );
    let (status, _, body) = send_with(&portal, call("/api/alerts", Some(token.clone())), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"[]");
    let held = seen.lock().unwrap().clone();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].1, "/api/alerts");
    assert_eq!(held[0].2[header::AUTHORIZATION], format!("Bearer {token}"));
}

#[tokio::test]
async fn a_pod_that_does_not_answer_is_a_502_problem() {
    let bundles = Bundles::new("pod-down", &[]);
    // A port nothing listens on: bound, then released.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pod = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let config = Config {
        apps_dir: Some(bundles.0.to_string_lossy().into_owned()),
        public_base_url: "https://portal.example.org".parse().unwrap(),
        apps_url: Some("https://example.org".parse().unwrap()),
        serve_app_hosts: true,
        app_pods: [("air-pod".to_owned(), pod)].into_iter().collect(),
        ..Config::for_tests()
    };
    let portal = server::app(AppState::new(config, None).with_mirror(mirror()));
    let (status, headers, _) = send(&portal, "air-pod.apps.example.org", "/", &[]).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
}

#[tokio::test]
async fn without_serve_app_hosts_the_portal_leaves_app_hosts_to_the_edge() {
    let bundles = Bundles::new("off", &["air-quality"]);
    let portal = portal(&bundles, None, false);
    // The edge rewrites `/` to `/apps/air-quality/` there; unrewritten, `/` is the Portal's.
    let (_, _, body) = send(&portal, "air-quality.apps.example.org", "/", &[]).await;
    assert!(!served(&body, INDEX));
    let (status, _, body) = send(
        &portal,
        "air-quality.apps.example.org",
        "/apps/air-quality/",
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(served(&body, INDEX));
}
