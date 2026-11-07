//! The Portal reads its pages from the `portal-ui` bundle when `JC_PORTAL_UI_URL` names one, and
//! `GET /api/v1/version` says whether that bundle was built against this binary's API (T-2485).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use sha2::Digest;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn get(config: &Config, uri: &str) -> (StatusCode, String, String, String) {
    let response = server::app(AppState::new(config.clone(), None))
        .oneshot(Request::get(uri).body(Body::empty()).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let header_of = |name| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let (kind, cache) = (
        header_of(header::CONTENT_TYPE),
        header_of(header::CACHE_CONTROL),
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        kind,
        cache,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

fn committed_spec_sha() -> String {
    let spec = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/ui/openapi.json"))
        .expect("ui/openapi.json is committed");
    format!("{:x}", sha2::Sha256::digest(&spec))
}

/// A bundle serving the shell, one asset and the `version.json` given.
async fn bundle(version: Value) -> MockServer {
    let ui = MockServer::start().await;
    let file = |at: &str, body: &str, kind: &str| {
        Mock::given(method("GET"))
            .and(path(at.to_owned()))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body.to_owned(), kind))
    };
    file(
        "/index.html",
        "<!doctype html><title>from the bundle</title>",
        "text/html",
    )
    .mount(&ui)
    .await;
    file("/assets/app-1a2b.js", "console.log(1)", "text/javascript")
        .mount(&ui)
        .await;
    file("/version.json", &version.to_string(), "application/json")
        .mount(&ui)
        .await;
    ui
}

fn reading(ui: &MockServer) -> Config {
    Config {
        ui_url: Some(ui.uri()),
        ..Config::for_tests()
    }
}

#[tokio::test]
async fn pages_and_assets_come_from_the_bundle_with_the_portals_headers() {
    let ui = bundle(json!({"commit": "abc", "openapi": committed_spec_sha()})).await;
    let config = reading(&ui);

    for page in ["/catalogue", "/index.html", "/projects/demo/explore"] {
        let (status, kind, cache, body) = get(&config, page).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert!(kind.starts_with("text/html"), "{page}: {kind}");
        assert_eq!(cache, "no-cache", "{page}");
        assert!(body.contains("from the bundle"), "{page}: {body}");
    }
    let (status, kind, cache, body) = get(&config, "/assets/app-1a2b.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(kind.contains("javascript"), "{kind}");
    assert_eq!(cache, "public, max-age=31536000, immutable");
    assert_eq!(body, "console.log(1)");

    // A file the bundle lacks, or a path that climbs, is a 404 and never the shell.
    for missing in ["/assets/gone.js", "/assets/../version.json"] {
        let (status, ..) = get(&config, missing).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    }
}

#[tokio::test]
async fn the_version_says_whether_the_bundle_speaks_this_api() {
    let ui = bundle(json!({"commit": "abc", "openapi": committed_spec_sha()})).await;
    let (status, _, _, body) = get(&reading(&ui), "/api/v1/version").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let version: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(version["ui"]["commit"], "abc");
    assert_eq!(version["portal"]["openapi"], json!(committed_spec_sha()));
    assert_eq!(version["contract"], "match", "{version}");

    let other = bundle(json!({"commit": "def", "openapi": "0".repeat(64)})).await;
    let (_, _, _, body) = get(&reading(&other), "/api/v1/version").await;
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json")["contract"],
        "mismatch"
    );

    let unnamed = bundle(json!({"commit": "ghi"})).await;
    let (_, _, _, body) = get(&reading(&unnamed), "/api/v1/version").await;
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json")["contract"],
        "unknown"
    );
}

#[tokio::test]
async fn an_unreachable_bundle_is_a_503_and_an_unknown_contract() {
    // A port that was free a moment ago: nothing listens there (wiremock pools its servers).
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port();
    let config = Config {
        ui_url: Some(format!("http://127.0.0.1:{port}")),
        ..Config::for_tests()
    };
    let (status, ..) = get(&config, "/catalogue").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _, _, body) = get(&config, "/api/v1/version").await;
    assert_eq!(status, StatusCode::OK);
    let version: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(version["ui"], Value::Null);
    assert_eq!(version["contract"], "unknown");
}
