//! The doors that write through a pipeline's replay, over a database that does not answer
//! (T-7813): a replay refuses what it cannot run before it reaches the store or the runner, each
//! refusal naming what to change.

#[path = "common/csrf_echo.rs"]
mod csrf_echo;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::CSRF_COOKIE;
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
/// The pool's address: nothing listens on port 1, and these are the words that must not leak.
const DEAD: &str = "postgres://nobody@127.0.0.1:1/none";

fn envelope(kind: &str, name: &str, namespace: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec,
        status: None,
    }
}

/// An administrator of the installation, so every refusal below is the store's and none the
/// caller's.
fn cookie(config: &Config) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: "f:1:ada".into(),
            username: "ada".into(),
            email: Some("ada@hel.fi".into()),
            name: None,
            roles: vec!["portal-approver".into()],
            groups: vec!["portal-approver".into()],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        stepped_up_at: None,
        id_token: "id-token-placeholder".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("store session");
    let response = (jar, StatusCode::OK).into_response();
    let mut parts: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect();
    parts.push(format!(
        "{CSRF_COOKIE}={}",
        joinedcontext_portal::auth::csrf::Token::bound(&config.cookie_key, &session).as_str()
    ));
    parts.join("; ")
}

/// A project with a space, an Endpoint and a Pipeline writing through it, over a pool that never
/// connects.
fn world() -> AppState {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(200))
        .connect_lazy(DEAD)
        .expect("a lazy pool");
    let state = AppState::new(Config::for_tests(), None).with_db(pool);
    state
        .mirror
        .upsert(envelope("ContextSpace", "air", PROJECT, json!({})));
    state.mirror.upsert(envelope(
        "Endpoint",
        "air-counters",
        PROJECT,
        json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
                "slug": "si6epqkx364lprho5uaigutk274r5grb", "audience": "project" }),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "air-intake",
        PROJECT,
        json!({ "contextSpaceRef": { "kind": "ContextSpace", "name": "air" },
                "target": { "endpointRef": { "kind": "Endpoint", "name": "air-counters" } } }),
    ));
    state.mirror.upsert(envelope(
        "Pipeline",
        "air-replay",
        PROJECT,
        json!({ "class": "resident",
                "targetEndpoint": "urn:ngsi-ld:Endpoint:hel.fi:helsinki:air-counters" }),
    ));
    state
}

async fn send_json(
    state: &AppState,
    verb: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, String) {
    use csrf_echo::EchoCsrf;
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method(verb)
                .uri(uri)
                .header(header::COOKIE, cookie(&state.config))
                .echo_csrf()
                .header(header::CONTENT_TYPE, "application/json")
                .body(body.map_or_else(Body::empty, |json| Body::from(json.to_string())))
                .expect("a request"),
        )
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A replay needs a target to write and a model to check against: a pipeline whose output names
/// no Endpoint of the project, and one whose space names no model, are conflicts that say what
/// to change; an empty or oversized id list is the caller's 400 (T-7813).
#[tokio::test]
async fn a_replay_names_why_it_cannot_run() {
    let state = world();
    let retry = |name: &str| format!("/api/v1/projects/{PROJECT}/pipelines/{name}/rejected/retry");
    let one = Some(json!({ "ids": [412] }));
    let (status, body) = send_json(&state, "POST", &retry("air-intake"), one.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("writes through no Endpoint"), "{body}");
    let (status, body) = send_json(&state, "POST", &retry("air-replay"), one).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("dataModelRef"), "{body}");
    let none = Some(json!({ "ids": [] }));
    let (status, body) = send_json(&state, "POST", &retry("air-replay"), none).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let many: Vec<i64> = (0..101).collect();
    let (status, body) = send_json(
        &state,
        "POST",
        &retry("air-replay"),
        Some(json!({ "ids": many })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
