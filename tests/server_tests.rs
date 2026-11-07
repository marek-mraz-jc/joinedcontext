use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;

mod common;

/// T-5627: a visitor without a session who opens the installation's name gets the public
/// catalogue, never a login; the query (`?lang=en`) travels along and no cache keeps the answer.
#[tokio::test]
async fn root_sends_a_visitor_without_a_session_to_the_catalogue() {
    for (uri, location) in [("/", "/catalogue"), ("/?lang=en", "/catalogue?lang=en")] {
        let app = server::app(AppState::new(Config::for_tests(), None));
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND, "{uri}");
        assert_eq!(response.headers()[header::LOCATION], location, "{uri}");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-store",
            "{uri}"
        );
    }
}

/// T-5627: a token the Portal cannot verify is no session either; the edge header is believed
/// only when the configuration trusts an edge, and then only once verified.
#[tokio::test]
async fn root_sends_an_unverified_token_to_the_catalogue_too() {
    let mut config = Config::for_tests();
    config.trust_edge_token = true;
    for (name, value) in [
        ("x-access-token", "forged"),
        ("authorization", "Bearer forged"),
    ] {
        let app = server::app(AppState::new(config.clone(), None));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(name, value)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND, "{name}");
        assert_eq!(response.headers()[header::LOCATION], "/catalogue", "{name}");
    }
}

/// T-5627: a signed-in steward still lands on the home page at `/`.
#[tokio::test]
async fn root_serves_a_signed_in_person_the_home_page() {
    let config = Config::for_tests();
    let cookie = common::cookie(&config, common::person("steward"));
    let app = server::app(AppState::new(config, None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .contains("text/html"));
}

#[tokio::test]
async fn a_page_serves_the_html_shell() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/html"));

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    // `ui/dist` is empty in CI (build.rs only creates the folder) and full after a local
    // `pnpm build`; both must answer a page with the SPA shell.
    assert!(
        body_str.contains("UI bundle not built") || body_str.contains("id=\"root\""),
        "unexpected shell: {body_str}"
    );
}

#[tokio::test]
async fn api_health_returns_status_and_cache_control() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let cache_control = response
        .headers()
        .get(header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(cache_control, "no-store");

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let health: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(health["status"], "ok");
    assert_eq!(health["name"], "joinedcontext-portal");
}

#[tokio::test]
async fn spa_route_falls_back_to_index() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/ovzdusie/spaces")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(content_type.contains("text/html"));

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    // `ui/dist` is empty in CI (build.rs only creates the folder) and full after a local
    // `pnpm build`; both must answer a page with the SPA shell.
    assert!(
        body_str.contains("UI bundle not built") || body_str.contains("id=\"root\""),
        "unexpected shell: {body_str}"
    );
}

#[tokio::test]
async fn missing_asset_returns_404_problem_json() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/assets/does-not-exist.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "application/problem+json");

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(problem["status"], 404);
    assert_eq!(
        problem["type"],
        "https://joinedcontext.com/errors/resource-not-found"
    );
}

#[tokio::test]
async fn unknown_api_endpoint_returns_404_problem_json() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "application/problem+json");

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let problem: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(problem["status"], 404);
    assert_eq!(
        problem["type"],
        "https://joinedcontext.com/errors/resource-not-found"
    );

    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(!body_str.contains("<!doctype html>"));
}

#[tokio::test]
async fn security_headers_present_on_endpoints() {
    for path in ["/", "/api/v1/health"] {
        let app = server::app(AppState::new(Config::for_tests(), None));
        let response = app
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let headers = response.headers();
        assert_eq!(
            headers.get("x-content-type-options").unwrap(),
            "nosniff",
            "failed on {path}"
        );
        assert_eq!(
            headers.get("x-frame-options").unwrap(),
            "SAMEORIGIN",
            "failed on {path}"
        );
        assert_eq!(
            headers.get("referrer-policy").unwrap(),
            "no-referrer",
            "failed on {path}"
        );
        assert_eq!(
            headers.get("cross-origin-opener-policy").unwrap(),
            "same-origin",
            "failed on {path}"
        );
        assert_eq!(
            headers.get("permissions-policy").unwrap(),
            "geolocation=(self), camera=(), microphone=()",
            "failed on {path}"
        );
        assert!(
            headers.get("content-security-policy").is_some(),
            "missing CSP on {path}"
        );
    }
}

/// UI-16, T-2747: every answer carries the request's reference, the edge's when it is a plain
/// token, a fresh one when there is none or when the header could forge a log line.
#[tokio::test]
async fn every_answer_names_its_request() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    let id_of = |response: &axum::response::Response| {
        response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };

    let edge = "3f2c1a9e-6b1d-4c3e-9a8f-0d1e2f3a4b5c";
    let request = Request::get("/api/v1/health")
        .header("x-request-id", edge)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(id_of(&response).as_deref(), Some(edge));

    for sent in [None, Some("x\tlevel=error forged"), Some("short")] {
        let mut request = Request::get("/api/v1/nothing-here");
        if let Some(sent) = sent {
            request = request.header("x-request-id", sent);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let id = id_of(&response).expect("an id on every answer, a 404 included");
        assert_eq!(id.len(), 32, "{sent:?} → {id}");
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
