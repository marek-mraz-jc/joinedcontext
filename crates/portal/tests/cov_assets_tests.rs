//! The UI door's refusals: a bundle that errs, answers without a type, or is not there (T-7823).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn ask(ui: Option<String>, verb: Method, uri: &str) -> (StatusCode, String, String) {
    let config = Config {
        ui_url: ui,
        ..Config::for_tests()
    };
    let response = server::app(AppState::new(config, None))
        .oneshot(
            Request::builder()
                .method(verb)
                .uri(uri)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let kind = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (status, kind, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn a_bundle_that_answers_an_error_status_is_unavailable() {
    let ui = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/assets/app.js"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&ui)
        .await;
    let (status, _, body) = ask(Some(ui.uri()), Method::GET, "/assets/app.js").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("the UI bundle is not reachable"), "{body}");
}

#[tokio::test]
async fn a_bundle_that_answers_404_is_a_missing_asset() {
    let ui = MockServer::start().await;
    let (status, _, body) = ask(Some(ui.uri()), Method::GET, "/assets/gone.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("assets/gone.js"), "{body}");
}

#[tokio::test]
async fn a_bundle_that_is_down_is_unavailable() {
    let (status, _, _) = ask(
        Some("http://127.0.0.1:1".to_owned()),
        Method::GET,
        "/assets/app.js",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_file_without_a_type_gets_the_type_of_its_name() {
    let ui = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/assets/site.css"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"a{}".to_vec()))
        .mount(&ui)
        .await;
    Mock::given(method("GET"))
        .and(path("/assets/blob.zzqq"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"x".to_vec()))
        .mount(&ui)
        .await;
    let (status, kind, body) = ask(Some(ui.uri()), Method::GET, "/assets/site.css").await;
    assert_eq!(
        (status, kind.as_str(), body.as_str()),
        (StatusCode::OK, "text/css", "a{}")
    );
    let (_, kind, _) = ask(Some(ui.uri()), Method::GET, "/assets/blob.zzqq").await;
    assert_eq!(kind, "application/octet-stream");
}

#[tokio::test]
async fn a_path_that_climbs_is_not_an_asset() {
    let ui = MockServer::start().await;
    let (status, _, _) = ask(Some(ui.uri()), Method::GET, "/assets/../secret.txt").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_write_on_the_ui_door_is_405() {
    for verb in [Method::POST, Method::DELETE, Method::PUT] {
        let (status, _, _) = ask(None, verb, "/internal/hook").await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    }
}

#[tokio::test]
async fn the_shell_without_a_build_is_the_placeholder_page() {
    let (status, kind, body) = ask(None, Method::GET, "/projects/demo").await;
    assert_eq!(status, StatusCode::OK);
    assert!(kind.starts_with("text/html"), "{kind}");
    assert!(!body.is_empty());
}

#[tokio::test]
async fn an_unknown_file_with_no_bundle_is_404() {
    let (status, _, body) = ask(None, Method::GET, "/nothing-here.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("nothing-here.js"), "{body}");
}

#[tokio::test]
async fn a_visitor_without_a_session_is_sent_to_the_catalogue_with_the_query() {
    let response = server::app(AppState::new(Config::for_tests(), None))
        .oneshot(
            Request::get("/?q=air")
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "/catalogue?q=air");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}
