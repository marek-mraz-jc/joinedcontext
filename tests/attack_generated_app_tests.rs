//! T-1706 attack vector: a generated app attacks the person, the platform or another app
//! (AP-19, AP-63).
//!
//! **The attack.** A published app, running on the apex beside the Portal's host, reads the
//! Portal's cookies, calls the Portal API with the viewer's session, or frames the Portal; its
//! dependencies phone home.
//!
//! **The defence.**
//! - Apps run on the apex and never on `portal.{domain}`.
//! - The Portal's cookies are host-only, so the apex never sees them in `document.cookie`.
//! - The cookie-authenticated API opens no CORS door.
//! - Every mutation needs the double-submit CSRF token, which an app on another origin cannot read.
//!
//! Played elsewhere and not repeated here:
//! - An app asked for on the Portal host: `static_host_tests.rs::an_app_asked_for_on_the_portal_host_is_sent_to_the_apps_origin` (T-2476).
//! - The run preview opened in a tab of its own: `agents::preview::tests::the_preview_runs_sandboxed_without_the_portal_origin` (T-2476).
//! - Framing the Portal: `security_headers_tests.rs::the_portal_cannot_be_framed_by_another_origin`.
//! - An app's own policy, which talks only to its own origin:
//!   - `static_host_tests.rs::every_app_response_carries_its_own_policy_and_not_the_portals`
//!   - `apps::static_host::tests::a_plain_app_may_not_be_framed_and_talks_only_to_the_platform`
//! - Session cookies that the page cannot read: `auth::session::tests` (the `HttpOnly` flags).
//!
//! Still open, owner's decision: T-2477 (apps share one origin with each other and pod egress is open).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use joinedcontext_portal::auth::csrf::{self, CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use tower::ServiceExt;
use wiremock::MockServer;

/// Where the apps live: the apex of the Portal's `portal.example.org`.
const APPS_ORIGIN: &str = "https://example.org";

/// Every `Set-Cookie` line the Portal writes when it stores a session and hands out the CSRF
/// token, the `Cookie` header a browser sends back with them, and the token itself.
fn portal_cookies(config: &Config) -> (Vec<String>, String, String) {
    let now = session::now_unix();
    let s = Session {
        identity: Identity {
            client: None,
            subject: "f:1:demo.steward".into(),
            username: "demo.steward".into(),
            email: Some("demo.steward@banskabystrica.sk".into()),
            name: Some("Demo Steward".into()),
            roles: Vec::new(),
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token".into(),
        access_expires_at: now + 3600,
        refresh_token: Some("refresh-token".into()),
    };
    let jar = PrivateCookieJar::new(config.cookie_key.clone());
    let jar = session::store(jar, &s).expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut lines: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().expect("cookie header").to_owned())
        .collect();
    let token = csrf::Token::mint();
    lines.push(csrf::cookie(&token).to_string());
    let sent = lines
        .iter()
        .map(|line| line.split(';').next().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("; ");
    (lines, sent, token.as_str().to_owned())
}

/// AP-63: a cookie with `Domain=` would be sent to, and for the CSRF token readable on, every
/// host under it, the apex that serves the apps included. Every Portal cookie stays host-only.
#[test]
fn no_portal_cookie_reaches_the_apex_the_apps_run_on() {
    let (lines, _, _) = portal_cookies(&Config::for_tests());
    assert_eq!(
        lines.len(),
        3,
        "session, refresh and CSRF cookies: {lines:?}"
    );
    for line in &lines {
        assert!(
            !line.to_ascii_lowercase().contains("domain="),
            "a Portal cookie widened to a sibling host: {line}"
        );
    }
    assert!(lines
        .iter()
        .any(|l| l.starts_with(&format!("{CSRF_COOKIE}="))));
}

/// AP-19, AP-63: a request an app sends to the Portal carries the viewer's session cookie (the
/// apex and `portal.{domain}` are one site, so `SameSite=Lax` does not hold it back). It is
/// still refused: the app cannot read the CSRF cookie to echo it, a guess is a mismatch, and
/// nothing reaches the forge.
#[tokio::test]
async fn an_app_sending_the_viewers_session_to_the_portal_api_writes_nothing() {
    let forge = MockServer::start().await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("mock url"),
        "owner",
        "config",
        "forge-token",
    )
    .expect("client");
    let config = Config {
        public_base_url: "https://portal.example.org".parse().unwrap(),
        ..Config::for_tests()
    };
    let (_, cookies, token) = portal_cookies(&config);
    let app = server::app(AppState::new(config, None).with_gitea(Arc::new(client)));

    let body = serde_json::json!({
        "apiVersion": "joinedcontext.com/v1alpha1",
        "kind": "ContextSpace",
        "metadata": { "name": "exfil", "namespace": "ovzdusie" },
        "spec": { "isSandbox": true }
    });
    for guess in [None, Some("guessed-token")] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/projects/ovzdusie/spaces")
            .header(header::ORIGIN, APPS_ORIGIN)
            .header(header::COOKIE, &cookies)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(guess) = guess {
            request = request.header(CSRF_HEADER, guess);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{guess:?}");
        assert!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "the answer is never readable by the app"
        );
    }

    // The header the app would need is not a CORS-safelisted one, so a browser asks first; the
    // Portal grants no origin, so the browser never sends it.
    let preflight = app
        .clone()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/projects/ovzdusie/spaces")
                .header(header::ORIGIN, APPS_ORIGIN)
                .header("access-control-request-method", "POST")
                .header("access-control-request-headers", CSRF_HEADER)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    for granted in [
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
    ] {
        assert!(preflight.headers().get(&granted).is_none(), "{granted}");
    }

    // The control: the same session with the token the Portal page reads is let through (a dry
    // run, which writes nothing), so the refusals above are the CSRF gate and not a bad session.
    let control = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects/ovzdusie/spaces?dryRun=All")
                .header(header::COOKIE, &cookies)
                .header(CSRF_HEADER, &token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(control.status(), StatusCode::OK);

    let calls = forge.received_requests().await.unwrap_or_default();
    assert!(calls.is_empty(), "the forge was reached: {calls:?}");
}
