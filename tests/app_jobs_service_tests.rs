//! `GET /apps/{name}/api/services/jobs`: an App's scheduled jobs with their next and last run,
//! for a signed-in person who may open the App, within the three layers (AP-154, AP-162, AP-164,
//! API/06 §3, T-3584).

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

use common::envelope;
use joinedcontext_portal::config::Config;
use joinedcontext_portal::state::AppState;

const APP: &str = "street-lights";
const PROJECT: &str = "lights";

/// A Portal holding the organization (the default service list: jobs on) and App `street-lights`
/// with two jobs, one of which ran and failed.
fn state(services: Value) -> AppState {
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
    .expect("config");
    let state = AppState::new(config, None);
    state.mirror.upsert(envelope(
        "Organization",
        "helsinki",
        "org",
        json!({ "domain": "hel.fi", "locales": ["en"], "defaultLocale": "en" }),
    ));
    let mut app = envelope(
        "App",
        APP,
        PROJECT,
        json!({
            "kind": "wasm",
            "source": { "path": "." },
            "build": {},
            "visibility": "project",
            "lifecycle": "published",
            "dataNeeds": [],
            "services": services,
            "server": { "jobs": [
                { "name": "dim-at-dawn", "schedule": "0 6 * * *", "export": "dim" },
                { "name": "report", "schedule": "*/15 * * * *", "export": "report" },
            ] },
        }),
    );
    app.status = Some(
        serde_json::from_value(json!({ "jobs": [{
            "name": "report", "lastRun": "2026-10-10T21:15:00Z", "outcome": "failed",
            "message": "the endpoint answered 503", "durationMs": 120, "failuresInARow": 1,
        }] }))
        .expect("a status"),
    );
    state.mirror.upsert(app);
    common::bind(&state.mirror, "jana", PROJECT);
    state
}

async fn jobs(state: &AppState, signed_in: bool) -> (StatusCode, Value, String) {
    let mut request = Request::builder().uri(format!("/apps/{APP}/api/services/jobs"));
    if signed_in {
        let token = common::REALM.person_token(APP, "jana", &[]);
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = joinedcontext_portal::server::app(state.clone())
        .oneshot(request.body(Body::empty()).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("a body");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

#[tokio::test]
async fn each_job_answers_its_schedule_its_next_run_and_its_last_run() {
    let (status, body, text) = jobs(&state(json!(["jobs"])), true).await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let list = body.as_array().expect("a list");
    assert_eq!(list.len(), 2, "{text}");
    assert_eq!(list[0]["name"], "dim-at-dawn");
    assert!(
        list[0]["nextRun"]
            .as_str()
            .is_some_and(|at| at.ends_with("T06:00:00Z")),
        "{text}"
    );
    assert!(list[0].get("lastRun").is_none(), "never ran: {text}");
    assert_eq!(
        list[1]["lastRun"],
        json!({ "at": "2026-10-10T21:15:00Z", "ok": false, "message": "the endpoint answered 503" })
    );
}

#[tokio::test]
async fn an_app_without_the_service_or_a_caller_without_a_session_gets_no_list() {
    let (status, body, text) = jobs(&state(json!([])), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert_eq!(
        (body["service"].as_str(), body["layer"].as_str()),
        (Some("jobs"), Some("app"))
    );
    let (status, _, text) = jobs(&state(json!(["jobs"])), false).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a project App is not there for an anonymous caller: {text}"
    );
}
