//! T-3263, T-3306 (AP-141): the App templates and their screenshots, for any signed-in person
//! and nobody else; a name the gallery does not hold, or another width, is a 404.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use serde_json::Value;
use tower::ServiceExt;

const CSRF: &str = "csrf-token-value";

fn session_cookie(config: &Config, subject: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: subject.into(),
            username: "demo.steward".into(),
            email: None,
            name: None,
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_string())
        .collect();
    parts.push(format!("__Host-jc_csrf={CSRF}"));
    parts.join("; ")
}

/// A Portal and a session it signed, one key for both.
fn portal() -> (axum::Router, String) {
    let config = Config::for_tests();
    let cookie = session_cookie(&config, "f:1:demo.steward");
    (server::app(AppState::new(config, None)), cookie)
}

async fn get(
    app: &axum::Router,
    cookie: Option<&str>,
    path: &str,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let app = app.clone();
    let mut request = Request::builder().uri(path);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let kind = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, kind, body)
}

#[tokio::test]
async fn every_template_is_listed_with_a_screenshot_at_each_width() {
    let (app, cookie) = portal();
    let (status, _, body) = get(&app, Some(&cookie), "/api/v1/app-templates").await;
    assert_eq!(status, StatusCode::OK);
    let listed: Value = serde_json::from_slice(&body).expect("JSON");
    let names: Vec<String> = listed["templates"]
        .as_array()
        .expect("templates")
        .iter()
        .map(|t| t["name"].as_str().expect("name").to_owned())
        .collect();
    assert!(names.len() >= 7, "{names:?}");
    for name in &names {
        for width in [1440, 375] {
            let (status, kind, png) = get(
                &app,
                Some(&cookie),
                &format!("/api/v1/app-templates/{name}/screenshot/{width}"),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{name} at {width}");
            assert_eq!(kind.as_deref(), Some("image/png"));
            assert_eq!(&png[1..4], b"PNG");
        }
    }
}

#[tokio::test]
async fn another_width_an_unknown_template_or_no_session_reads_nothing() {
    let (app, cookie) = portal();
    for path in [
        "/api/v1/app-templates/kpi-dashboard/screenshot/800",
        "/api/v1/app-templates/no-such/screenshot/1440",
        "/api/v1/app-templates/..%2F..%2Fsecret/screenshot/1440",
    ] {
        let (status, _, _) = get(&app, Some(&cookie), path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _, _) = get(
        &app,
        None,
        "/api/v1/app-templates/kpi-dashboard/screenshot/1440",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = get(&app, None, "/templates/no-such/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
