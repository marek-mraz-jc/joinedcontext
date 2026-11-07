//! `GET /api/v1/branding`: one image, many installations (T-0340, UI-30, OPS-46).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

const BLOCK: &str = r##"
instanceName: "Helsinki Region Context"
organisation: "City of Helsinki"
contactEmail: "opendata@hel.fi"
logo: "logo.svg"
colours:
  primary: "#0000bf"
  secondary: "#0072c6"
  accent: "#ffe977"
  background: "#ffffff"
  text: "#1a1a1a"
languages:
  default: "fi"
  offered: ["fi", "sv", "en"]
"##;

async fn branding(file: Option<&str>) -> (StatusCode, Option<String>, Value) {
    branding_with(file, None).await
}

async fn branding_with(
    file: Option<&str>,
    apps_url: Option<&str>,
) -> (StatusCode, Option<String>, Value) {
    let mut config = Config::for_tests();
    config.branding_file = file.map(str::to_owned);
    config.apps_url = apps_url.map(|url| url.parse().expect("an apps url"));
    let app = server::app(AppState::new(config, None));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    let status = response.status();
    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, cache, serde_json::from_slice(&bytes).expect("json"))
}

fn written(contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "jc-branding-api-{}-{}.yaml",
        std::process::id(),
        contents.len()
    ));
    std::fs::write(&path, contents).expect("write the branding file");
    path
}

/// The login page needs the name and the logo before anyone has signed in.
#[tokio::test]
async fn the_branding_is_public_and_cacheable() {
    let path = written(BLOCK);
    let (status, cache, body) = branding(Some(&path.display().to_string())).await;
    let _ = std::fs::remove_file(&path);

    assert_eq!(status, StatusCode::OK, "no session, no redirect: {body}");
    assert_eq!(
        cache.as_deref(),
        Some("public, max-age=300"),
        "the one API answer a browser may keep"
    );
    assert_eq!(body["instanceName"], json!("Helsinki Region Context"));
    assert_eq!(body["organisation"], json!("City of Helsinki"));
    assert_eq!(body["colours"]["primary"], json!("#0000bf"));
    assert_eq!(body["languages"]["default"], json!("fi"));
    assert_eq!(body["languages"]["offered"], json!(["fi", "sv", "en"]));
    assert_eq!(body["logo"], json!("logo.svg"));
}

/// An installation whose ConfigMap has not been rendered looks plain; it does not break.
#[tokio::test]
async fn an_installation_without_branding_serves_neutral_defaults() {
    let (status, _, body) = branding(None).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["instanceName"], json!("joinedcontext"));
    assert_eq!(body["colours"]["primary"], json!("#1d4ed8"));

    let (status, _, body) = branding(Some("/nonexistent/jc/branding.yaml")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["instanceName"], json!("joinedcontext"));
}

/// OPS-46: what reaches a CSS custom property is a colour or the default, never a value the
/// browser would evaluate as something else.
#[tokio::test]
async fn a_colour_that_is_not_a_colour_never_reaches_the_browser() {
    let path = written(
        "instanceName: \"Injected\"\ncolours:\n  primary: \"red; background: url(https://evil.example/x)\"\n",
    );
    let (status, _, body) = branding(Some(&path.display().to_string())).await;
    let _ = std::fs::remove_file(&path);

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["colours"]["primary"], json!("#1d4ed8"));
    assert!(
        !body.to_string().contains("evil.example"),
        "the rejected value must not be echoed either: {body}"
    );
}

/// The logo comes from the platform's own origin, so a page never fetches a third party.
#[tokio::test]
async fn the_logo_is_served_from_beside_the_branding_file() {
    let dir = std::env::temp_dir().join(format!("jc-branding-assets-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the asset directory");
    let file = dir.join("branding.yaml");
    std::fs::write(&file, "instanceName: \"Helsinki\"\nlogo: \"logo.svg\"\n").expect("branding");
    std::fs::write(
        dir.join("logo.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .expect("logo");

    let mut config = Config::for_tests();
    config.branding_file = Some(file.display().to_string());
    let app = server::app(AppState::new(config, None));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding/logo")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/svg+xml")
    );
    // The asset route is not the exact path the old blanket rule exempted, so every page load
    // refetched the logo; the route's own header now survives (T-2295, UI-30).
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=300"),
        "the logo may not be kept by a browser"
    );

    // Nothing is configured as a favicon, so there is nothing to serve.
    let missing = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding/favicon")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file name that would leave the ConfigMap was already dropped, so nothing can be reached
/// through this route but the two assets the branding block names.
#[tokio::test]
async fn an_asset_outside_the_branding_directory_is_not_served() {
    let dir = std::env::temp_dir().join(format!("jc-branding-escape-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the asset directory");
    let file = dir.join("branding.yaml");
    std::fs::write(&file, "logo: \"../../etc/passwd\"\n").expect("branding");

    let mut config = Config::for_tests();
    config.branding_file = Some(file.display().to_string());
    let app = server::app(AppState::new(config, None));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/branding/logo")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let _ = std::fs::remove_dir_all(&dir);
}

/// AP-67, PF-57: only an answer whose route wrote `public` may be kept. Everything else under
/// `/api/` is `no-store`, whatever the handler set or left unset — the guarantee that survives
/// T-2295, so a route carrying a person's data cannot become cacheable by omission.
#[tokio::test]
async fn an_answer_no_route_declared_public_is_never_cacheable() {
    let app = server::app(AppState::new(Config::for_tests(), None));
    for uri in [
        "/api/v1/projects",
        "/api/v1/projects/helsinki/spaces",
        "/api/v1/projects/helsinki/changes",
        "/api/v1/me",
        // The two routes that *do* write `public` — answering an error here, because nothing
        // is configured: a refusal of a public route is still not a browser's to keep.
        "/api/v1/branding/favicon",
        "/api/v1/projects/helsinki/basemap/default/style.json",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store"),
            "{uri} ({}) may be kept by a browser",
            response.status(),
        );
    }
}

/// AP-122, T-2840: the in-Portal page frames an App at the origin Apps are served from, and
/// only the Portal's configuration says which that is; a branding file cannot name one.
#[tokio::test]
async fn the_apps_origin_comes_from_the_configuration_and_never_from_the_file() {
    let path = written("instanceName: \"X\"\nappsOrigin: \"https://evil.example\"\n");
    let file = path.display().to_string();

    let (_, _, without) = branding_with(Some(&file), None).await;
    let (_, _, with) = branding_with(Some(&file), Some("https://apps.example.org")).await;
    let _ = std::fs::remove_file(&path);

    assert!(without.get("appsOrigin").is_none(), "{without}");
    assert_eq!(with["appsOrigin"], json!("https://apps.example.org"));
}

/// T-2874: the sections the UI hides come from the configuration, never from the file.
#[tokio::test]
async fn hidden_sections_come_from_the_configuration_and_never_from_the_file() {
    let path = written("instanceName: \"X\"\nhiddenSections: [\"apps\"]\n");
    let file = path.display().to_string();
    let answer = |dashboards: bool| {
        let file = file.clone();
        async move {
            let mut config = Config::for_tests();
            config.branding_file = Some(file);
            config.dashboards = dashboards;
            let app = server::app(AppState::new(config, None));
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/branding")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            serde_json::from_slice::<Value>(&bytes).expect("json")
        }
    };
    let hidden = answer(false).await;
    let shown = answer(true).await;
    let _ = std::fs::remove_file(&path);

    assert_eq!(hidden["hiddenSections"], json!(["dashboards"]), "{hidden}");
    assert_eq!(shown["hiddenSections"], json!([]), "{shown}");
}
