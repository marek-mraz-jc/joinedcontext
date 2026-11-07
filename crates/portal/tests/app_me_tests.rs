//! A published App's backend learns the caller's roles from the Portal (AP-109, AP-92,
//! ADR-N-030, Architecture/16 §13).
//!
//! The backend forwards the edge's token of the App's own client `app-{name}` as
//! `Authorization: Bearer`; the roles are that token's `resource_access.app-{name}.roles` that
//! the manifest declares, and nothing else the token or the caller carries.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use common::envelope;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;

const ME: &str = "/api/v1/projects/helsinki/apps/air-quality/me";

fn app(lifecycle: &str) -> Value {
    json!({
        "kind": "fullstack",
        "source": { "git": {
            "url": "https://forge.example/joinedcontext/helsinki_air-quality.git",
            "ref": "8c56954a1f0e3d2c1b0a99887766554433221100",
        }},
        "build": { "rust": "1.90", "node": "22" },
        "visibility": "project",
        "lifecycle": lifecycle,
        "dataNeeds": [{
            "contextSpaceRef": { "kind": "ContextSpace", "name": "helsinki" },
            "types": ["AirQualityObserved"],
            "operations": ["queryEntity"],
        }],
        "roles": [
            { "name": "viewer", "title": { "en": "Viewer" } },
            { "name": "steward", "title": { "en": "Steward" } },
        ],
        "access": [
            { "role": "viewer", "subjects": [{ "group": "air-quality-viewer" }] },
            { "role": "steward", "subjects": [{ "group": "air-quality-steward" }] },
        ],
    })
}

/// A Portal that trusts the process's realm, with `air-quality` in the given lifecycle.
fn state_with(lifecycle: &str) -> AppState {
    let config = Config::from_vars(|key| {
        match key {
            "JC_OIDC_ISSUER" => Some(common::REALM.issuer.as_str()),
            "JC_PORTAL_COOKIE_KEY" => {
                Some("cookie-key-of-the-portal-tests-it-seals-no-session-anybody-uses-0001")
            }
            "JC_OIDC_CLIENT_ID" => Some("portal-api"),
            "JC_OIDC_CLIENT_SECRET" => Some("secret"),
            _ => None,
        }
        .map(str::to_owned)
    })
    .expect("a realm");
    let state = AppState::new(config, None);
    state
        .mirror
        .upsert(envelope("App", "air-quality", "helsinki", app(lifecycle)));
    for who in ["jana", "vera", "otto"] {
        common::bind(&state.mirror, who, "helsinki");
    }
    state
}

async fn ask(state: &AppState, path: &str, token: Option<&str>) -> (StatusCode, String, String) {
    let mut request = Request::get(path);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request.body(Body::empty()).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    (status, String::from_utf8_lossy(&bytes).into_owned(), cache)
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).expect("a JSON answer")
}

/// AP-92, AP-109: the roles are the token's roles of the App's own client, in the order
/// `spec.roles` declares them; a person holding none gets an empty list, the answer names nobody
/// else, and nobody needs a rule in the project to ask about themselves.
#[tokio::test]
async fn the_answer_is_the_callers_roles_from_the_apps_own_token() {
    let state = state_with("published");
    let realm = &common::REALM;

    let jana = realm.person_token("air-quality", "jana", &["steward", "viewer"]);
    let (status, text, cache) = ask(&state, ME, Some(&jana)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        body(&text),
        json!({ "id": "sub-jana", "name": "jana@hel.fi", "email": "jana@hel.fi", "roles": ["viewer", "steward"] })
    );
    assert!(cache.contains("no-store"), "{cache}");

    // A role the manifest does not declare is not one.
    let stale = realm.person_token("air-quality", "vera", &["viewer", "auditor"]);
    assert_eq!(
        body(&ask(&state, ME, Some(&stale)).await.1)["roles"],
        json!(["viewer"])
    );

    let otto = realm.person_token("air-quality", "otto", &[]);
    let (status, text, _) = ask(&state, ME, Some(&otto)).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(body(&text)["roles"], json!([]));
    assert!(
        !text.contains("jana"),
        "the answer names nobody but the caller: {text}"
    );
}

/// AP-92, ADR-N-030 §3.4: a token of another client, even one naming this App in its audience,
/// or of another App, is refused; the Portal's own login is not this App's.
#[tokio::test]
async fn a_token_that_is_not_the_apps_own_is_refused() {
    let state = state_with("published");
    let realm = &common::REALM;
    for token in [
        realm.person_token_of("app-air-quality", "edge", "jana", &["steward"]),
        realm.person_token("other", "jana", &["steward"]),
        realm.person_token_of("portal-api", "portal-api", "jana", &["steward"]),
    ] {
        let (status, text, _) = ask(&state, ME, Some(&token)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{text}");
    }
}

/// AP-109: an App that is not published, or not in this project, is a `404`; no credential is a
/// `401`, answered before the App is looked up.
#[tokio::test]
async fn an_unpublished_app_is_not_found_and_an_anonymous_caller_is_refused() {
    let token = common::REALM.person_token("air-quality", "jana", &["viewer"]);
    for lifecycle in ["draft", "preview", "retired"] {
        let (status, text, _) = ask(&state_with(lifecycle), ME, Some(&token)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{lifecycle}: {text}");
    }
    let state = state_with("published");
    let (status, _, _) = ask(
        &state,
        "/api/v1/projects/espoo/apps/air-quality/me",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = ask(&state, ME, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = ask(&state, "/api/v1/projects/helsinki/apps/nothing/me", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "nothing learnt without a token"
    );
}

/// T-4910: a person the project App keeps out, with a valid token of its client, gets the `404` of an
/// App that is not there, the same answer as a draft.
#[tokio::test]
async fn a_person_outside_the_project_learns_nothing_of_a_project_app() {
    let pavol = common::REALM.person_token("air-quality", "pavol", &["viewer"]);
    let (status, text, _) = ask(&state_with("published"), ME, Some(&pavol)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    let (_, draft, _) = ask(&state_with("draft"), ME, Some(&pavol)).await;
    assert_eq!(text, draft);
}
