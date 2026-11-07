//! `GET /assistant-preview/{publicId}`: the embed dialog's launcher preview under its own policy,
//! not the Portal's `script-src 'self'` that left the `srcdoc` frame blank (T-5328).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;

async fn get(domain: Option<&str>, uri: &str) -> (StatusCode, String, String) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let mut config = Config::for_tests();
    if let Some(domain) = domain {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jc-assistant-preview-{}-{n}.yaml",
            std::process::id()
        ));
        std::fs::write(&path, format!("domain: \"{domain}\"\n")).expect("branding file");
        config.branding_file = Some(path.to_string_lossy().to_string());
    }
    let response = server::app(AppState::new(config, None))
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let csp = response
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, csp, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn the_preview_carries_its_own_policy_naming_the_assistant_host() {
    let (status, csp, body) = get(
        Some("dev.example.com"),
        "/assistant-preview/helsinki-catalogue?lang=en",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        csp.contains("script-src https://assistant.dev.example.com;"),
        "{csp}"
    );
    assert!(csp.starts_with("sandbox allow-scripts;"), "{csp}");
    assert!(
        !csp.contains("script-src 'self'"),
        "the Portal's policy overrode the page's: {csp}"
    );
    assert!(
        body.contains(
            "https://assistant.dev.example.com/d/helsinki-catalogue/embed.js\" data-lang=\"en\""
        ),
        "{body}"
    );
}

#[tokio::test]
async fn an_id_that_is_no_label_or_a_portal_without_a_domain_answers_404() {
    let (status, _, _) = get(Some("dev.example.com"), "/assistant-preview/Bad%22%3E").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get(None, "/assistant-preview/helsinki-catalogue").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_language_that_is_no_code_is_left_out() {
    let (status, _, body) = get(
        Some("dev.example.com"),
        "/assistant-preview/a?lang=x%22onload",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("data-lang"), "{body}");
    assert!(body.contains("<html lang=\"sk\">"), "{body}");
}
