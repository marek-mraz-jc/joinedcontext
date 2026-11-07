//! The bundle inside the binary (T-0369, UI-01, TS-10, AP-14).
//!
//! `cargo test` runs without a UI build on purpose — build.rs creates an empty `ui/dist` so a
//! Rust-only change needs no node — but an image built that way serves the placeholder page
//! and every view of the demo is behind it. This suite is the difference between the two: it
//! is skipped where no bundle was built, and it fails where one was built badly.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::assets::{Assets, PLACEHOLDER_HTML};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;

fn embedded() -> Vec<String> {
    Assets::iter().map(|f| f.to_string()).collect()
}

/// True where the image build (or a local `pnpm build`) put a bundle in `ui/dist`.
fn bundle_was_built() -> bool {
    !embedded().is_empty()
}

#[test]
fn a_built_bundle_carries_the_shell_and_a_hashed_asset() {
    if !bundle_was_built() {
        eprintln!("no ui/dist bundle in this build — run `pnpm build` in ui/ to exercise this");
        return;
    }
    let files = embedded();
    assert!(
        files.iter().any(|f| f == "index.html"),
        "the shell is missing from the bundle: {files:?}"
    );
    // Vite emits `assets/index-<hash>.js`. A bundle with an index.html and no script is a
    // build that half ran, which would serve an empty page rather than an obvious failure.
    assert!(
        files
            .iter()
            .any(|f| f.starts_with("assets/") && f.ends_with(".js")),
        "the bundle has no hashed script: {files:?}"
    );
}

#[tokio::test]
async fn a_built_image_serves_the_portal_and_not_the_placeholder() {
    if !bundle_was_built() {
        eprintln!("no ui/dist bundle in this build — the placeholder is the correct answer here");
        return;
    }
    let app = server::app(AppState::new(Config::for_tests(), None));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/catalogue")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&body);
    assert!(
        !html.contains("UI bundle not built"),
        "the front door is the placeholder page, so every view of the demo is unreachable"
    );
    assert_ne!(html, PLACEHOLDER_HTML);
    assert!(
        html.contains("<script") || html.contains("<div id=\"root\""),
        "the shell must load the application: {html}"
    );
}
