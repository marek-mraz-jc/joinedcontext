//! T-4642 (EP-130): the Endpoint editor fills a chart template with the columns a person
//! picked, for any signed-in person; an unknown template or a missing role is a 404.

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
async fn a_template_comes_back_filled_with_the_columns_picked() {
    let (app, cookie) = portal();
    let (status, _, body) = get(
        &app,
        Some(&cookie),
        "/api/v1/visualization-templates/line?x=dateObserved&y=no2&color=name&facet=ignored&evil=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let spec: Value = serde_json::from_slice(&body).expect("JSON");
    let text = spec.to_string();
    assert!(
        text.contains("\"dateObserved\"") && text.contains("\"no2\""),
        "{text}"
    );
    assert!(
        !text.contains("{x}") && !text.contains("{y}"),
        "no placeholder stays: {text}"
    );
    assert_eq!(spec["data"]["name"], "source");
    assert!(
        jc_core::kinds::visualization::check_spec(&spec).is_ok(),
        "the spec passes the propose check"
    );

    let (status, _, body) = get(
        &app,
        Some(&cookie),
        "/api/v1/visualization-templates/bar?x=name&y=no2",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let spec: Value = serde_json::from_slice(&body).expect("JSON");
    assert!(
        spec["encoding"].get("color").is_none(),
        "a role left out drops its encoding"
    );
}

#[tokio::test]
async fn an_unknown_template_a_missing_role_or_no_session_gets_nothing() {
    let (app, cookie) = portal();
    for path in [
        "/api/v1/visualization-templates/pie?x=a&y=b",
        "/api/v1/visualization-templates/heatmap?x=a&y=b",
        "/api/v1/visualization-templates/bar?x=a&y=",
        "/api/v1/visualization-templates/..%2Fbar?x=a&y=b",
    ] {
        let (status, _, _) = get(&app, Some(&cookie), path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _, _) = get(&app, None, "/api/v1/visualization-templates/bar?x=a&y=b").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
