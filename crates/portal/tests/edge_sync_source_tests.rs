//! Edge cases of the three sync-source routes (T-2036 … T-2042; PF-59, R20, T-1361).
//!
//! **The contract, in one sentence:** a sync source is read by a member while running it and pausing
//! it need `propose` — so a refusal never drives anything.
//!
//! The happy paths live in `sync_source_routes_tests.rs` (the pause, the detach, the bindings). This
//! file is the other side: a caller who may not drive, a source the repository does not hold, a driver
//! that is not running. (The api-key cases beside them went with the key routes, T-4616.)
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const PROJECT: &str = "banskabystrica";
const ELSEWHERE: &str = "doprava";
const SOURCE: &str = "shmu-open-data";
const CSRF: &str = "csrf-token-edge-sync";
const STEWARD: &str = "eva.steward";
const READER: &str = "peter.reader";
const STRANGER: &str = "nobody";

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

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    envelope(kind, name, ORG_NAMESPACE, spec)
}

/// One SyncSource, and the bindings that tell a steward who may propose from a reader who may only
/// read.
fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    mirror.upsert(envelope(
        "SyncSource",
        SOURCE,
        PROJECT,
        json!({
            "url": "https://opendata.shmu.sk/catalog.json",
            "kind": "ckan",
            "schedule": "PT1H",
        }),
    ));
    mirror.upsert(org(
        "Role",
        "source-steward",
        json!({ "rules": [{ "kinds": ["SyncSource"], "verbs": ["read", "propose"] }] }),
    ));
    mirror.upsert(org(
        "Role",
        "source-reader",
        json!({ "rules": [{ "kinds": ["SyncSource"], "verbs": ["read"] }] }),
    ));
    for (name, user, role) in [
        ("stewards", STEWARD, "source-steward"),
        ("readers", READER, "source-reader"),
    ] {
        mirror.upsert(org(
            "RoleBinding",
            name,
            json!({ "subjects": [{ "user": format!("{user}@banskabystrica.sk") }],
                    "role": role, "scope": { "project": PROJECT } }),
        ));
    }
    mirror
}

fn cookie(config: &Config, username: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username: username.into(),
            email: Some(format!("{username}@banskabystrica.sk")),
            name: None,
            roles: Vec::new(),
            groups: Vec::new(),
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
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
    parts.push(format!("{CSRF_COOKIE}={CSRF}"));
    parts.join("; ")
}

fn world() -> AppState {
    AppState::new(Config::for_tests(), None).with_mirror(mirror())
}

async fn send(
    state: &AppState,
    who: &str,
    verb: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(verb)
        .uri(uri)
        .header(header::COOKIE, cookie(&state.config, who))
        .header(CSRF_HEADER, CSRF);
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = server::app(state.clone())
        .oneshot(request.body(body).expect("a request"))
        .await
        .expect("a response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// PF-59, T-1361: a sync source's status is a read of the project's sources, while running one and
/// pausing one drive the loop and need `propose`. A reader may look and not drive, a caller no binding
/// covers is told the source is not there, and a source the repository does not hold is not driven for
/// anybody.
#[tokio::test]
async fn a_sync_source_is_read_by_a_member_and_driven_only_by_one_who_may_propose() {
    let state = world();
    let source = format!("/api/v1/projects/{PROJECT}/syncsources/{SOURCE}");
    let status_uri = format!("{source}/status");
    let now_uri = format!("{source}/sync");
    let pause_uri = format!("{source}/pause");

    // A caller no binding covers: the source is not there, on the read and on both writes.
    let (status, body) = send(&state, STRANGER, Method::GET, &status_uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    for uri in [&now_uri, &pause_uri] {
        let (status, body) = send(
            &state,
            STRANGER,
            Method::POST,
            uri,
            Some(json!({ "paused": true })),
        )
        .await;
        // The refusal grammar of the two sides is not the same, and deliberately so: `status` is a
        // read, so it hides the source (PF-59, R20), while `sync` and `pause` are writes, so they
        // name the verb the caller has not got. Both are refusals before the loop is looked for, so
        // neither tells a stranger whether this project syncs anything.
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert!(
            !body.to_string().contains(SOURCE),
            "{uri} named the source to a stranger: {body}",
        );
    }

    // A reader of the project's sources may look — this installation runs no loop, so the answer is
    // that there is none rather than an invented status — and may drive neither.
    let (status, body) = send(&state, READER, Method::GET, &status_uri, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(
        body["detail"].as_str().unwrap_or_default().contains("sync"),
        "{body}",
    );
    for uri in [&now_uri, &pause_uri] {
        let (status, body) = send(
            &state,
            READER,
            Method::POST,
            uri,
            Some(json!({ "paused": true })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
    }

    // The steward may drive, and gets the same "no loop is running" rather than a silent nothing.
    for (verb, uri, body) in [
        (Method::POST, &now_uri, None),
        (Method::POST, &pause_uri, Some(json!({ "paused": true }))),
    ] {
        let (status, answer) = send(&state, STEWARD, verb, uri, body).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}: {answer}");
    }

    // A pause that does not say which way: refused as a body, before the loop is looked for.
    for body in [
        json!({}),
        json!({ "paused": "yes" }),
        json!(true),
        Value::Null,
    ] {
        let (status, answer) = send(
            &state,
            STEWARD,
            Method::POST,
            &pause_uri,
            Some(body.clone()),
        )
        .await;
        assert!(
            status.is_client_error(),
            "{body} answered {status}: {answer}",
        );
    }

    // A source the repository does not hold, and names that could not be one: not found for the
    // steward who may drive, which is the order that matters — the verb first, then the source.
    for (project, name) in [
        (PROJECT, "nothing-like-this"),
        (PROJECT, "SHMU-Open-Data"),
        (PROJECT, "%2e%2e"),
        (ELSEWHERE, SOURCE),
    ] {
        let (status, body) = send(
            &state,
            STEWARD,
            Method::GET,
            &format!("/api/v1/projects/{project}/syncsources/{name}/status"),
            None,
        )
        .await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::SERVICE_UNAVAILABLE,
            "{project}/{name} answered {status}: {body}",
        );
    }
}
