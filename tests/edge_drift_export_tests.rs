//! Edge cases of the drift list, its two resolutions and the project export (T-2009, T-2010, T-2011,
//! T-2012; CC-19, MF-17, MF-18, PF-59, R20, UI-26).
//!
//! **The contract, in one sentence:** drift is a member's view of one project and its resolutions are
//! writes held to `propose` on `Entity`, and an export is a read of one project at one revision — so a
//! caller no binding covers gets the answer of a project that is not there, and nothing a caller sends
//! as a revision reaches the forge unless it could be one.
//!
//! The happy paths live in `drift_api_tests.rs` (what a scan found, when it ran, the two buttons, the
//! reader who resolves nothing) and `export_api_tests.rs` (the YAML stream, the JSON list, the zip and
//! its index). This file is the other side: a stranger, a project name that is not one, a format
//! nobody serves, a revision that is not a revision.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::MockServer;

use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::git::GiteaClient;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::reconciler::drift::{Difference, Drifted, Found, Kind, Store};
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const SPACE: &str = "helsinki";
const ENTITY: &str = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-1";
const CSRF: &str = "csrf-token-edge-drift";
const MEMBER: &str = "peter.member@hel.fi";
const STRANGER: &str = "nobody@hel.fi";

fn org(kind: &str, name: &str, spec: Value) -> ResourceEnvelope {
    ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta::new(name, ORG_NAMESPACE),
        spec,
        status: None,
    }
}

fn cookie(config: &Config, email: &str) -> String {
    use axum::response::IntoResponse;
    let now = session::now_unix();
    let username = email.split('@').next().unwrap_or(email).to_owned();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("f:1:{username}"),
            username,
            email: Some(email.to_owned()),
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

/// One scan of `helsinki`: an entity the space answers differently, and one it does not hold.
fn scanned() -> Arc<Store> {
    let store = Arc::new(Store::default());
    let mut found = BTreeMap::new();
    found.insert(
        PROJECT.to_owned(),
        Found {
            observed_at: "2026-09-18T09:12:03Z".parse().expect("an instant"),
            entities: vec![
                Drifted {
                    space: SPACE.to_owned(),
                    id: ENTITY.to_owned(),
                    drift: Kind::Modified,
                    diff: vec![Difference {
                        path: "airQualityIndex.value".to_owned(),
                        declared: json!(42),
                        live: Some(json!(7)),
                    }],
                    resolutions: vec!["revert", "adopt"],
                    source: "projects/helsinki/spaces/helsinki/entities/seed/stations.json"
                        .to_owned(),
                    declared: json!({ "id": ENTITY, "type": "AirQualityObserved" }),
                },
                Drifted {
                    space: SPACE.to_owned(),
                    id: "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-2".to_owned(),
                    drift: Kind::Missing,
                    diff: Vec::new(),
                    resolutions: vec!["revert"],
                    source: "projects/helsinki/spaces/helsinki/entities/seed/stations.json"
                        .to_owned(),
                    declared: json!({ "id": "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-2" }),
                },
            ],
        },
    );
    store.replace_all(found);
    store
}

/// A Portal that knows what one scan found, with a member of the project who may propose entities and
/// a person no binding covers. No space surface is configured, so a resolution that got past every
/// check answers 503 — and that is how a case tells "refused" from "would have written".
fn drift_state() -> AppState {
    let mut state = AppState::new(Config::for_tests(), None);
    state.drift = scanned();
    state.mirror.upsert(org(
        "Role",
        "entity-author",
        json!({ "rules": [{ "kinds": ["Entity"], "verbs": ["read", "propose"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "entity-authors",
        json!({ "subjects": [{ "user": MEMBER }], "role": "entity-author",
                "scope": { "project": PROJECT } }),
    ));
    state
}

async fn call(state: &AppState, email: &str, verb: Method, uri: &str) -> (StatusCode, Value) {
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .method(verb)
                .uri(uri)
                .header(header::COOKIE, cookie(&state.config, email))
                .header(CSRF_HEADER, CSRF)
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

// -------------------------------------------------------------------------------------------------
// T-2009 `list_drift`
// -------------------------------------------------------------------------------------------------

/// PF-59, R20: the drift of a project is a member's view. A caller no binding covers is told the
/// project is not there — never `403`, which would say which projects exist — and the refusal carries
/// nothing the scan found: not an entity id, not a file of the repository.
#[tokio::test]
async fn the_drift_of_a_project_is_not_there_for_a_caller_no_binding_covers() {
    let state = drift_state();

    let (status, body) = call(
        &state,
        STRANGER,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/drift"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let text = body.to_string();
    for leaked in ["station-1", "station-2", "entities/seed", "airQualityIndex"] {
        assert!(!text.contains(leaked), "the 404 carried {leaked:?}: {text}");
    }

    // The member sees what the scan found, and when it ran.
    let (status, list) = call(
        &state,
        MEMBER,
        Method::GET,
        &format!("/api/v1/projects/{PROJECT}/drift"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"].as_array().map(Vec::len), Some(2), "{list}");
    assert!(
        list["metadata"]["observedAt"].is_string(),
        "the page cannot say when the scan ran: {list}",
    );

    // A name that could not be a project, and a project nobody scanned: the member's own binding is
    // scoped to `helsinki`, so both are the same 404.
    for project in ["Helsinki", "helsinki_1", "%2e%2e", "doprava"] {
        let (status, body) = call(
            &state,
            MEMBER,
            Method::GET,
            &format!("/api/v1/projects/{project}/drift"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{project}: {body}");
    }
}

// -------------------------------------------------------------------------------------------------
// T-2010 `revert_drift` and T-2011 `adopt_drift`
// -------------------------------------------------------------------------------------------------

/// CC-19, UI-26: both resolutions are writes. They act only on what the last scan reported, and only
/// for a caller who may propose an `Entity` here — so a stranger, a member without the verb, an entity
/// of another space and an id nobody reported are each refused before anything is written, and an
/// entity the space does not hold cannot be adopted at all.
#[tokio::test]
async fn a_resolution_acts_only_on_what_the_scan_reported_and_only_for_a_caller_who_may_write() {
    let state = drift_state();
    let encoded = ENTITY.replace(':', "%3A");
    let missing = "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-2".replace(':', "%3A");

    // A caller no binding covers: not there, for both buttons.
    for verb in ["revert", "adopt"] {
        let (status, body) = call(
            &state,
            STRANGER,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{encoded}/{verb}"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{verb}: {body}");
    }

    // A member who reads the project but may not propose entities: refused as a write.
    let reader_state = {
        let state = drift_state();
        state.mirror.upsert(org(
            "Role",
            "entity-reader",
            json!({ "rules": [{ "kinds": ["Entity"], "verbs": ["read"] }] }),
        ));
        state.mirror.upsert(org(
            "RoleBinding",
            "entity-readers",
            json!({ "subjects": [{ "user": "eva.reader@hel.fi" }], "role": "entity-reader",
                    "scope": { "project": PROJECT } }),
        ));
        state
    };
    for verb in ["revert", "adopt"] {
        let (status, body) = call(
            &reader_state,
            "eva.reader@hel.fi",
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{encoded}/{verb}"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{verb}: {body}");
    }

    // The member who may write: an id the scan did not report, and the right id under the wrong
    // space, are both "no drift was reported for this" — the list is what the buttons act on.
    for (space, id) in [
        (
            SPACE,
            "urn%3Angsi-ld%3AAirQualityObserved%3Ahel.fi%3Ahelsinki%3Astation-9",
        ),
        ("doprava", encoded.as_str()),
        ("", encoded.as_str()),
        (SPACE, "not-a-urn"),
    ] {
        let (status, body) = call(
            &state,
            MEMBER,
            Method::POST,
            &format!("/api/v1/projects/{PROJECT}/drift/{space}/{id}/revert"),
        )
        .await;
        assert!(
            status == StatusCode::NOT_FOUND,
            "{space}/{id} answered {status}: {body}",
        );
    }

    // An entity the space does not hold: adopting "it is gone" would be a deletion, which is never a
    // button (CC-19). Reverting it is the way back, and this installation has no space surface, so it
    // gets as far as saying there is nowhere to write.
    let (status, body) = call(
        &state,
        MEMBER,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{missing}/adopt"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = call(
        &state,
        MEMBER,
        Method::POST,
        &format!("/api/v1/projects/{PROJECT}/drift/{SPACE}/{missing}/revert"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
}

// -------------------------------------------------------------------------------------------------
// T-2012 `export`
// -------------------------------------------------------------------------------------------------

/// MF-17, MF-18: an export is a read of one project at one revision. A format this Portal does not
/// serve and a revision that could not be one are refused here, before the forge is asked — a caller
/// does not get to put their own text where a git reference goes.
#[tokio::test]
async fn a_format_or_a_revision_that_is_not_one_never_reaches_the_forge() {
    let forge = MockServer::start().await;
    let client = GiteaClient::new(
        forge.uri().parse().expect("a url"),
        "bb",
        "org",
        "token-xyz",
    )
    .expect("a client");
    let state = AppState::new(Config::for_tests(), None);
    state.mirror.upsert(org(
        "Role",
        "project-reader",
        json!({ "rules": [{ "kinds": ["Endpoint", "ContextSpace"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(org(
        "RoleBinding",
        "project-readers",
        json!({ "subjects": [{ "user": MEMBER }], "role": "project-reader",
                "scope": { "project": PROJECT } }),
    ));
    let state = state.with_gitea(Arc::new(client));

    // A format nobody serves. The check is exact: `YAML` is not `yaml`.
    for format in ["tar", "YAML", "yml", "zip.gz", "", "json,yaml"] {
        let (status, body) = call(
            &state,
            MEMBER,
            Method::GET,
            &format!("/api/v1/projects/{PROJECT}/export?format={format}"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{format:?}: {body}");
    }

    // A revision that could not be a commit or a branch name: a flag, a traversal, a shell, a space,
    // something longer than any reference.
    let too_long = "a".repeat(129);
    for revision in [
        "--upload-pack=x",
        "..",
        "main..other",
        "main;rm%20-rf",
        "main%20other",
        "refs/heads/main%00",
        "",
        too_long.as_str(),
    ] {
        let (status, body) = call(
            &state,
            MEMBER,
            Method::GET,
            // Named, so the check is the revision's and not UI-87's administrator gate.
            &format!("/api/v1/projects/{PROJECT}/export?names=x&revision={revision}"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{revision:?}: {body}");
    }

    // A caller no binding covers, and a name that could not be a project: the same 404, and neither
    // asks the forge for anything.
    for (who, project) in [
        (STRANGER, PROJECT),
        (MEMBER, "Helsinki"),
        (MEMBER, "helsinki_1"),
        (MEMBER, "doprava"),
    ] {
        let (status, body) = call(
            &state,
            who,
            Method::GET,
            &format!("/api/v1/projects/{project}/export"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} {project}: {body}");
    }

    assert!(
        forge
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "a refused export reached the forge",
    );
}
