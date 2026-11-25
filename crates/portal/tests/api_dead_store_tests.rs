//! The read doors over a database that does not answer (T-7813).
//!
//! Every route here reads a store the Portal keeps in Postgres: a pipeline's rejected records,
//! its runs and their log, its source ledger, and the access requests of a project. With the
//! pool unreachable each answers `503` with a sentence a person can act on ("try again"), and
//! never the driver's text, which names the database host and user.

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
    state
}

async fn get(state: &AppState, uri: &str) -> (StatusCode, String) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::COOKIE, cookie(&state.config))
                .body(Body::empty())
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

/// Each read of a pipeline's records, runs, log and ledger, and each list of access requests,
/// answers 503 with "try again" and keeps the driver's words to the log.
#[tokio::test]
async fn every_store_read_is_a_503_that_names_no_database() {
    let state = world();
    let pipeline = format!("/api/v1/projects/{PROJECT}/pipelines/air-intake");
    for uri in [
        format!("{pipeline}/rejected"),
        format!("{pipeline}/rejected/groups"),
        format!("{pipeline}/rejected/export"),
        format!("{pipeline}/runs"),
        format!("{pipeline}/runs/run-1/log"),
        format!("{pipeline}/source-deletes"),
        format!("/api/v1/projects/{PROJECT}/pipeline-runs"),
        format!("/api/v1/projects/{PROJECT}/pipeline-health"),
        format!("{pipeline}/access-requests"),
        format!("/api/v1/projects/{PROJECT}/access-requests"),
        format!("/api/v1/projects/{PROJECT}/endpoints/air-counters/access-requests"),
    ] {
        let (status, body) = get(&state, &uri).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}: {body}");
        assert!(body.contains("try again"), "{uri}: {body}");
        assert!(
            !body.contains("127.0.0.1") && !body.contains("nobody") && !body.contains("pool"),
            "{uri}: {body}"
        );
    }
}
