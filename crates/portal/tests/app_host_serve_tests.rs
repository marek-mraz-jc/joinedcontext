//! The App hosts the Portal answers itself where no edge routes them (jc, AP-133,
//! edge-session-design S4a): `{name}.apps.{domain}` reaches App `name` alone, its own endpoints
//! through the gateway without the browser's cookies, and no Portal route at all.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::apps::static_host::sri_sha384;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;
use tower::ServiceExt;

#[path = "common/unique.rs"]
mod unique;

const INDEX: &[u8] = b"<!doctype html><title>air quality</title>";
const BUNDLE_JS: &[u8] = b"console.log('air quality');";

/// One bundle per App with the integrity manifest CI writes beside it; removed on drop.
struct Bundles(std::path::PathBuf);

impl Bundles {
    fn new(case: &str, apps: &[&str]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "jc-app-host-{case}-{}-{}",
            std::process::id(),
            crate::unique::unique()
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
