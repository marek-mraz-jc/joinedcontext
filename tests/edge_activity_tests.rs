//! Edge cases of the activity trail (T-1963, T-1966, T-1967, T-1968; PF-51, PF-59, OPS-48, OPS-49).
//!
//! **The contract, in one sentence:** what a project's activity says is read by that project's
//! members only, appended by the collector's own client only, and never mixed with another project's
//! — not by a filter, not by a cursor, not by a live tail, not by a record that names a project it
//! was not sent for.
//!
//! The trail is the audit record: who was refused, which pipeline failed, what an agent was allowed
//! to do. A reader who should not have it learns what a department is building; a writer who should
//! not have it writes the audit trail itself (T-1402). So the cases below aim at the two ends —
//! `member_of` on the reading side, the collector's client on the writing side — and at the filter in
//! between, which is the one piece of code the list, the stream and the operation all share.
//!
//! Tests only (the family's rule). Every case here is green; a red one becomes its own task.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::PrivateCookieJar;
use chrono::{DateTime, Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method as http_method, path as http_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::activity::{ActivityEvent, ActivityFilter, DEFAULT_LIMIT, MAX_LIMIT};
use joinedcontext_portal::api::activity::COLLECTOR_CLIENT;
use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;
use joinedcontext_portal::store::Mirror;

const PROJECT: &str = "helsinki";
const ELSEWHERE: &str = "espoo";
const CSRF: &str = "test-csrf-activity-edge";
const REALM_PATH: &str = "/realms/banskabystrica";
const PORTAL_AUDIENCE: &str = "portal-api";

/// Both projects exist, so nothing below is a 404 for want of a namespace.
fn mirror() -> Arc<Mirror> {
    let mirror = Arc::new(Mirror::new());
    for project in [PROJECT, ELSEWHERE] {
        mirror.upsert(ResourceEnvelope {
            api_version: API_VERSION.into(),
            kind: "ContextSpace".into(),
            metadata: ObjectMeta {
                name: "air-quality".into(),
                namespace: Some(project.into()),
                ..Default::default()
            },
            spec: json!({}),
            status: None,
        });
    }
    mirror
}

/// A session cookie. `roles` decides membership: the bootstrap role is a member of every project,
/// a person with none is a member of nowhere until a binding says so.
fn cookie(config: &Config, username: &str, roles: Vec<&str>) -> String {
    let now = session::now_unix();
    let session = Session {
        identity: Identity {
            client: None,
            subject: format!("sub-{username}"),
            username: username.to_owned(),
            email: None,
            name: Some(username.to_owned()),
            roles: roles.into_iter().map(str::to_owned).collect(),
            groups: vec![],
            second_factor: false,
        },
        expires_at: now + 3600,
        issued_at: now,
        signed_in_at: None,
        id_token: "id-token".into(),
        access_expires_at: now + 3600,
        refresh_token: None,
    };
    let jar = session::store(PrivateCookieJar::new(config.cookie_key.clone()), &session)
        .expect("a session");
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

fn event_at(
    project: &str,
    kind: &str,
    severity: &str,
    object: &str,
    time: DateTime<Utc>,
) -> ActivityEvent {
    ActivityEvent {
        time,
        project: project.into(),
        space: Some("air-quality".into()),
        kind: kind.into(),
        source: "gateway".into(),
        summary: format!("{kind} in {project}"),
        severity: severity.into(),
        correlation_id: None,
        details: json!({ "object": object }),
    }
}

async fn get(app: &axum::Router, cookie: Option<&str>, uri: &str) -> (StatusCode, Value) {
    let mut request = Request::builder().method(Method::GET).uri(uri);
    if let Some(cookie) = cookie {
        request = request
            .header(header::COOKIE, cookie)
            .header(CSRF_HEADER, CSRF);
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::empty()).expect("a request"))
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

fn filter_with(limit: i64) -> ActivityFilter {
    ActivityFilter {
        limit,
        ..Default::default()
    }
}

// -------------------------------------------------------------------------------------------------
// T-1966 `list_activity`: who may read it
// -------------------------------------------------------------------------------------------------

/// PF-59, R20: a caller who is a member of no project reads no activity, and the answer is the same
/// whether the project is there or not — an activity route that told the two apart would say which
/// projects exist. A caller with no session at all is asked to sign in.
#[tokio::test]
async fn a_caller_who_is_no_member_learns_neither_the_activity_nor_that_the_project_is_there() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    state
        .activity
        .append(&[event_at(
            PROJECT,
            "access.denied",
            "error",
            "endpoints/public-air",
            Utc::now(),
        )])
        .await
        .expect("appended");
    let app = server::app(state);
    let stranger = cookie(&config, "nobody", vec![]);

    let (real, body) = get(
        &app,
        Some(&stranger),
        &format!("/api/v1/projects/{PROJECT}/activity"),
    )
    .await;
    let (invented, _) = get(
        &app,
        Some(&stranger),
        "/api/v1/projects/nosuchproject/activity",
    )
    .await;
    assert_eq!(real, StatusCode::NOT_FOUND);
    assert_eq!(invented, StatusCode::NOT_FOUND, "the two answers differ");
    let text = body.to_string();
    for leaked in ["access.denied", "public-air", "refused"] {
        assert!(!text.contains(leaked), "the 404 carried {leaked:?}: {text}");
    }

    // No session: the door asks who this is before it asks anything else.
    let (unauthenticated, _) =
        get(&app, None, &format!("/api/v1/projects/{PROJECT}/activity")).await;
    assert_eq!(unauthenticated, StatusCode::UNAUTHORIZED);

    // A project name that is not a name is not found either, and never a path.
    for forged in [
        "Not_A_Project",
        "..",
        "%2e%2e",
        "a%2Fb",
        "-x",
        &"a".repeat(64),
    ] {
        let (status, _) = get(
            &app,
            Some(&stranger),
            &format!("/api/v1/projects/{forged}/activity"),
        )
        .await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
            "{forged:?} answered {status}",
        );
    }
}

/// PF-51: every value the list reads is one of this route's or a bad request, and the page size is
/// clamped rather than refused — a browser that asks for a million gets the ceiling, not an error.
#[tokio::test]
async fn every_value_the_list_reads_is_this_routes_own_and_the_page_size_is_clamped() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let events: Vec<ActivityEvent> = (0..5)
        .map(|n| {
            event_at(
                PROJECT,
                "access.denied",
                "warning",
                "endpoints/public-air",
                Utc::now() - Duration::seconds(n),
            )
        })
        .collect();
    state.activity.append(&events).await.expect("appended");
    let app = server::app(state);
    let jana = cookie(&config, "jana", vec!["portal-approver"]);
    let uri = |query: &str| format!("/api/v1/projects/{PROJECT}/activity{query}");

    // Refused, by name, before anything is read.
    for bad in [
        "?severity=loud",
        "?severity=INFO",
        "?severity=",
        "?since=yesterday",
        "?since=2026-13-01T00:00:00Z",
        "?since=",
        "?cursor=not-base64!",
        "?cursor=YWJj",
        "?limit=many",
        "?limit=1.5",
    ] {
        let (status, _) = get(&app, Some(&jana), &uri(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} was accepted");
    }

    // Clamped, not refused: `limit.clamp(1, MAX_LIMIT)`, so nothing and less than nothing are one
    // event, and more than the ceiling is the ceiling.
    for (query, expected) in [
        ("?limit=0", 1),
        ("?limit=-1", 1),
        ("?limit=1", 1),
        ("?limit=9223372036854775807", 5),
        ("", 5),
    ] {
        let (status, list) = get(&app, Some(&jana), &uri(query)).await;
        assert_eq!(status, StatusCode::OK, "{query}: {list}");
        let items = list["items"].as_array().map(Vec::len).unwrap_or_default();
        assert_eq!(items, expected, "{query}");
    }
    // The default page is inside the ceiling, or a caller who asks for nothing gets more than the
    // most a caller may ask for.
    assert_eq!(DEFAULT_LIMIT.min(MAX_LIMIT), DEFAULT_LIMIT);

    // An empty or repeated kind is no filter at all rather than a filter nothing matches.
    for query in [
        "?kind=",
        "?kind=,",
        "?kind=access.denied,access.denied",
        "?kind=%20access.denied%20",
    ] {
        let (status, list) = get(&app, Some(&jana), &uri(query)).await;
        assert_eq!(status, StatusCode::OK, "{query}: {list}");
        assert_eq!(list["items"].as_array().map(Vec::len), Some(5), "{query}");
    }
}

/// PF-59: a cursor is a position, not a permission. One handed out for a page of this project is used
/// on another and pages nothing of the first; and a forged one is refused or reads nothing, never
/// somebody else's events.
#[tokio::test]
async fn a_cursor_pages_only_the_project_it_is_used_on() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let now = Utc::now();
    let mut events = Vec::new();
    for (project, kind) in [(PROJECT, "access.denied"), (ELSEWHERE, "pipeline.error")] {
        for n in 0..4 {
            events.push(event_at(
                project,
                kind,
                "warning",
                "endpoints/public-air",
                now - Duration::seconds(n),
            ));
        }
    }
    state.activity.append(&events).await.expect("appended");
    let app = server::app(state);
    // The bootstrap role is a member of both projects, which is what makes this case about the
    // cursor rather than about the binding.
    let jana = cookie(&config, "jana", vec!["portal-approver"]);

    let (status, first) = get(
        &app,
        Some(&jana),
        &format!("/api/v1/projects/{PROJECT}/activity?limit=2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let cursor = first["next"].as_str().expect("a cursor").to_owned();

    // The same cursor on the other project: its own events, and never this one's kind.
    let (status, crossed) = get(
        &app,
        Some(&jana),
        &format!("/api/v1/projects/{ELSEWHERE}/activity?cursor={cursor}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{crossed}");
    for item in crossed["items"].as_array().expect("items") {
        assert_eq!(item["project"], json!(ELSEWHERE), "{item}");
    }

    // A cursor of a moment nobody wrote in, and one from the far future: no crash, nothing foreign.
    for forged in ["MHwx", "LTF8LTE", "OTk5OTk5OTk5OTk5OTk5OTk5fDE"] {
        let (status, list) = get(
            &app,
            Some(&jana),
            &format!("/api/v1/projects/{PROJECT}/activity?cursor={forged}"),
        )
        .await;
        assert!(
            status == StatusCode::OK || status == StatusCode::BAD_REQUEST,
            "{forged} answered {status}: {list}",
        );
        for item in list["items"].as_array().into_iter().flatten() {
            assert_eq!(item["project"], json!(PROJECT), "{forged} read {item}");
        }
    }
}

/// UI-31: paging is a walk, not a sample. Seven events read three at a time are the same seven, in
/// the same order, each once — a boundary that skipped one would lose an audit record from the view.
#[tokio::test]
async fn a_walk_through_the_pages_repeats_nothing_and_skips_nothing() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let now = Utc::now();
    let written: Vec<ActivityEvent> = (0..7)
        .map(|n| {
            event_at(
                PROJECT,
                "access.denied",
                "warning",
                &format!("endpoints/e{n}"),
                now - Duration::seconds(n),
            )
        })
        .collect();
    state.activity.append(&written).await.expect("appended");
    let app = server::app(state);
    let jana = cookie(&config, "jana", vec!["portal-approver"]);

    let mut seen: Vec<String> = Vec::new();
    let mut query = "?limit=3".to_owned();
    for _ in 0..10 {
        let (status, page) = get(
            &app,
            Some(&jana),
            &format!("/api/v1/projects/{PROJECT}/activity{query}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        for item in page["items"].as_array().expect("items") {
            seen.push(
                item["details"]["object"]
                    .as_str()
                    .expect("an object")
                    .to_owned(),
            );
        }
        match page["next"].as_str() {
            Some(cursor) => query = format!("?limit=3&cursor={cursor}"),
            None => break,
        }
    }
    let expected: Vec<String> = (0..7).map(|n| format!("endpoints/e{n}")).collect();
    assert_eq!(
        seen, expected,
        "the walk did not read every event once, newest first"
    );
}

// -------------------------------------------------------------------------------------------------
// T-1963 `ActivityStore::list`: the filter itself
// -------------------------------------------------------------------------------------------------

/// PF-59: the store is asked for one project's events and answers that project's, whatever the filter
/// says. Every filter below matches the other project's events exactly, and none of them crosses.
#[tokio::test]
async fn a_list_is_one_projects_events_whatever_the_filter_asks_for() {
    let state = AppState::new(Config::for_tests(), None).with_mirror(mirror());
    let now = Utc::now();
    state
        .activity
        .append(&[
            event_at(PROJECT, "access.denied", "error", "endpoints/mine", now),
            event_at(ELSEWHERE, "access.denied", "error", "endpoints/theirs", now),
        ])
        .await
        .expect("appended");

    for filter in [
        ActivityFilter::default(),
        ActivityFilter {
            space: Some("air-quality".into()),
            ..filter_with(50)
        },
        ActivityFilter {
            kinds: vec!["access.denied".into()],
            ..filter_with(50)
        },
        ActivityFilter {
            source: Some("gateway".into()),
            ..filter_with(50)
        },
        ActivityFilter {
            severity: Some("info".into()),
            ..filter_with(50)
        },
        ActivityFilter {
            object: Some("endpoints/theirs".into()),
            ..filter_with(50)
        },
        ActivityFilter {
            since: Some(now - Duration::days(1)),
            ..filter_with(50)
        },
    ] {
        let page = state.activity.list(PROJECT, &filter).await.expect("a page");
        for event in &page.items {
            assert_eq!(
                event.project, PROJECT,
                "{filter:?} answered another project's event"
            );
        }
        // And the object of the other project's event is never matched here.
        if filter.object.is_some() {
            assert!(page.items.is_empty(), "{filter:?} matched across projects");
        }
    }
}

/// UI-31: a severity is a floor, a kind and a space and a source are themselves, and `since` includes
/// the instant it names. The live tail runs the same predicate over arriving events, so what this
/// case pins is what both halves do.
#[tokio::test]
async fn a_severity_is_a_floor_and_every_other_condition_is_exact() {
    let state = AppState::new(Config::for_tests(), None).with_mirror(mirror());
    let now = Utc::now();
    let older = now - Duration::seconds(10);
    state
        .activity
        .append(&[
            event_at(PROJECT, "access.denied", "info", "endpoints/a", now),
            event_at(PROJECT, "pipeline.error", "warning", "endpoints/b", now),
            event_at(PROJECT, "endpoint.traffic", "error", "endpoints/c", older),
        ])
        .await
        .expect("appended");

    let count = |filter: ActivityFilter| {
        let state = state.clone();
        async move {
            state
                .activity
                .list(PROJECT, &filter)
                .await
                .expect("a page")
                .items
                .len()
        }
    };

    // A floor, not an equality.
    assert_eq!(
        count(ActivityFilter {
            severity: Some("info".into()),
            ..filter_with(50)
        })
        .await,
        3
    );
    assert_eq!(
        count(ActivityFilter {
            severity: Some("warning".into()),
            ..filter_with(50)
        })
        .await,
        2
    );
    assert_eq!(
        count(ActivityFilter {
            severity: Some("error".into()),
            ..filter_with(50)
        })
        .await,
        1
    );

    // Exact, each of them: a prefix, a suffix and another case are not the value.
    for (filter, expected) in [
        (
            ActivityFilter {
                kinds: vec!["access.denied".into()],
                ..filter_with(50)
            },
            1,
        ),
        (
            ActivityFilter {
                kinds: vec!["access".into()],
                ..filter_with(50)
            },
            0,
        ),
        (
            ActivityFilter {
                kinds: vec!["ACCESS.DENIED".into()],
                ..filter_with(50)
            },
            0,
        ),
        (
            ActivityFilter {
                kinds: vec!["access.denied".into(), "pipeline.error".into()],
                ..filter_with(50)
            },
            2,
        ),
        (
            ActivityFilter {
                space: Some("air".into()),
                ..filter_with(50)
            },
            0,
        ),
        (
            ActivityFilter {
                source: Some("Gateway".into()),
                ..filter_with(50)
            },
            0,
        ),
        (
            ActivityFilter {
                object: Some("endpoints/a".into()),
                ..filter_with(50)
            },
            1,
        ),
        (
            ActivityFilter {
                object: Some("endpoints".into()),
                ..filter_with(50)
            },
            0,
        ),
        (
            ActivityFilter {
                object: Some("endpoints/../a".into()),
                ..filter_with(50)
            },
            0,
        ),
    ] {
        assert_eq!(count(filter.clone()).await, expected, "{filter:?}");
    }

    // `since` includes its own instant and excludes what is a nanosecond older.
    assert_eq!(
        count(ActivityFilter {
            since: Some(older),
            ..filter_with(50)
        })
        .await,
        3
    );
    assert_eq!(
        count(ActivityFilter {
            since: Some(older + Duration::nanoseconds(1)),
            ..filter_with(50)
        })
        .await,
        2,
    );
    assert_eq!(
        count(ActivityFilter {
            since: Some(now + Duration::days(1)),
            ..filter_with(50)
        })
        .await,
        0
    );
}

// -------------------------------------------------------------------------------------------------
// T-1967 `stream_activity`
// -------------------------------------------------------------------------------------------------

/// OPS-49, PF-59: the live tail is the same door as the list — a non-member is told the project is not
/// there, a filter that is not one of this route's is refused before a stream is opened — and what it
/// does open is not cached or buffered by anything in front of it.
#[tokio::test]
async fn the_live_tail_is_the_same_door_as_the_list_and_is_never_cached() {
    let config = Config::for_tests();
    let state = AppState::new(config.clone(), None).with_mirror(mirror());
    let app = server::app(state);
    let stranger = cookie(&config, "nobody", vec![]);
    let jana = cookie(&config, "jana", vec!["portal-approver"]);
    let uri = format!("/api/v1/projects/{PROJECT}/activity/stream");

    let (status, _) = get(&app, Some(&stranger), &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&app, None, &uri).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = get(&app, Some(&jana), &format!("{uri}?severity=loud")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The stream itself: opened, its headers read, then dropped. Reading the body would wait for an
    // event, and what this case is about is the answer's own terms.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(&uri)
                .header(header::COOKIE, &jana)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
    );
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache"),
        "a cached tail is a tail that stops",
    );
    assert_eq!(
        headers
            .get("x-accel-buffering")
            .and_then(|value| value.to_str().ok()),
        Some("no"),
        "a buffered tail arrives in blocks or not at all",
    );
}

/// PF-59: an event of another project never reaches this project's tail. The hub is asked for one
/// project and the handler checks the project of every event again
/// (`src/api/activity.rs`, `event.project != project_of_stream`), so this holds twice over.
#[tokio::test]
async fn an_event_of_another_project_never_reaches_this_projects_tail() {
    let state = AppState::new(Config::for_tests(), None).with_mirror(mirror());
    let mut tail = state.activity_events.subscribe(PROJECT).await;
    let now = Utc::now();
    state
        .activity
        .append(&[
            event_at(ELSEWHERE, "access.denied", "error", "endpoints/theirs", now),
            event_at(PROJECT, "pipeline.error", "error", "endpoints/mine", now),
            event_at(
                ELSEWHERE,
                "endpoint.traffic",
                "info",
                "endpoints/theirs",
                now,
            ),
        ])
        .await
        .expect("appended");

    // Whatever arrives, it is this project's — and this project's own event does arrive, so the tail
    // is not simply empty.
    let mut seen = Vec::new();
    while let Ok(event) = tail.try_recv() {
        assert_eq!(
            event.project, PROJECT,
            "another project's event reached the tail"
        );
        seen.push(event.kind);
    }
    assert_eq!(seen, vec!["pipeline.error".to_owned()]);
}

// -------------------------------------------------------------------------------------------------
// T-1968 `ingest_activity`
// -------------------------------------------------------------------------------------------------

/// A Portal that knows the realm, and the key that realm signs with.
async fn portal_with_realm() -> (axum::Router, AppState, jsonwebtoken::EncodingKey, String) {
    use p256::pkcs8::EncodePrivateKey;

    let realm: &'static MockServer = Box::leak(Box::new(MockServer::start().await));
    let issuer = format!("{}{REALM_PATH}", realm.uri().trim_end_matches('/'));
    let secret = p256::SecretKey::random(&mut rand_core::OsRng);
    let der = secret.to_pkcs8_der().expect("der");
    let signer = jsonwebtoken::EncodingKey::from_ec_der(der.as_bytes());
    let mut jwk: Value = serde_json::from_str(&secret.public_key().to_jwk_string()).expect("jwk");
    jwk["kid"] = json!("key-activity-test");
    jwk["alg"] = json!("ES256");
    jwk["use"] = json!("sig");
    Mock::given(http_method("GET"))
        .and(http_path(format!(
            "{REALM_PATH}/protocol/openid-connect/certs"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "keys": [jwk] })))
        .mount(realm)
        .await;

    let issuer_for_config = issuer.clone();
    let config = Config::from_vars(move |key| match key {
        "JC_OIDC_ISSUER" => Some(issuer_for_config.clone()),
        "JC_OIDC_CLIENT_ID" => Some(PORTAL_AUDIENCE.to_owned()),
        "JC_OIDC_CLIENT_SECRET" => Some("secret".to_owned()),
        "JC_PORTAL_COOKIE_KEY" => Some("k".repeat(64)),
        _ => None,
    })
    .expect("config");
    let state = AppState::new(config, None).with_mirror(mirror());
    state
        .bearer
        .as_ref()
        .expect("a realm")
        .refresh()
        .await
        .expect("jwks");
    (server::app(state.clone()), state, signer, issuer)
}

fn workload_token(signer: &jsonwebtoken::EncodingKey, issuer: &str, client: &str) -> String {
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    header.kid = Some("key-activity-test".to_owned());
    let now = session::now_unix();
    let claims = json!({
        "iss": issuer,
        "typ": "Bearer",
        "aud": PORTAL_AUDIENCE,
        "sub": format!("service-account-{client}"),
        "azp": client,
        "exp": now + 300,
        "iat": now,
    });
    jsonwebtoken::encode(&header, &claims, signer).expect("sign")
}

fn otlp(records: Vec<Value>) -> Value {
    json!({ "resourceLogs": [{ "scopeLogs": [{ "logRecords": records }] }] })
}

fn record(project: &str, kind: &str) -> Value {
    json!({
        "timeUnixNano": "1789314063000000000",
        "attributes": [
            { "key": "project", "value": { "stringValue": project } },
            { "key": "kind", "value": { "stringValue": kind } },
            { "key": "source", "value": { "stringValue": "gateway" } },
            { "key": "severity", "value": { "stringValue": "warning" } },
            { "key": "summary", "value": { "stringValue": "An anonymous caller was refused." } },
        ],
    })
}

async fn ingest(app: &axum::Router, bearer: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/activity")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(body.to_string()))
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

/// T-6431: the wire shape otelcol-contrib 0.162.0 exports after the allow-list and the gateway's
/// `mcp.tool` record, taken from a real export: an empty scope, an emptied body, `severityText`
/// and an `intValue` as a string. On dev every batch was gzip, the exporter's default, which this
/// route refuses as a 400 before it reads a record.
#[tokio::test]
async fn the_collectors_real_export_lands_and_a_gzip_batch_does_not() {
    let (app, state, signer, issuer) = portal_with_realm().await;
    let token = workload_token(&signer, &issuer, COLLECTOR_CLIENT);
    let text = |key: &str, value: &str| json!({ "key": key, "value": { "stringValue": value } });
    let batch = json!({ "resourceLogs": [{
        "resource": { "attributes": [text("service.name", "context-gateway")] },
        "scopeLogs": [{ "scope": {}, "logRecords": [{
            "timeUnixNano": "1788696000000000000",
            "severityText": "INFO",
            "body": { "stringValue": "" },
            "attributes": [
                text("project", PROJECT),
                text("space", "air-quality"),
                text("kind", "mcp.tool"),
                text("source", "gateway"),
                text("summary", "user:ana called query_entities on walk-read."),
                text("severity", "info"),
                text("endpoint", "walk-read"),
                text("tool", "query_entities"),
                text("outcome", "ok"),
                { "key": "count", "value": { "intValue": "3" } },
            ],
        }] }],
    }] });

    let gzip = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/activity")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::CONTENT_ENCODING, "gzip")
                .body(Body::from(&b"\x1f\x8b\x08\x00\x00\x00\x00\x00"[..]))
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(gzip.status(), StatusCode::BAD_REQUEST);

    let (status, body) = ingest(&app, Some(&token), batch).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["partialSuccess"].is_null(), "{body}");
    let page = state
        .activity
        .list(PROJECT, &filter_with(50))
        .await
        .expect("a page");
    assert_eq!(page.items.len(), 1, "only the plain batch was stored");
    let event = &page.items[0];
    assert_eq!(event.kind, "mcp.tool");
    assert_eq!(event.space.as_deref(), Some("air-quality"));
    assert_eq!(
        event.details,
        json!({ "endpoint": "walk-read", "tool": "query_entities", "outcome": "ok", "count": 3 })
    );
}

/// OPS-48, T-1402: the audit trail is appended by the collector's own Keycloak client and by nobody
/// else. Not a person's session, whatever roles it carries; not another workload's token, though it
/// carries the Portal's audience too.
#[tokio::test]
async fn only_the_collectors_own_client_appends_to_the_audit_trail() {
    let (app, state, signer, issuer) = portal_with_realm().await;

    // No credential at all: the CSRF lock answers first, because a POST with neither a session nor
    // an `Authorization` header is exactly the cross-site shape it exists to refuse
    // (`src/auth/csrf.rs`, `is_allowed`). Either way the handler is never reached.
    let (status, _) = ingest(&app, None, otlp(vec![record(PROJECT, "access.denied")])).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Another workload's token: the audience is the Portal's, and that is not enough.
    for other in [
        "context-gateway",
        "helsinki-agent-paper",
        "activity-ingest-2",
        "",
    ] {
        let token = workload_token(&signer, &issuer, other);
        let (status, body) = ingest(
            &app,
            Some(&token),
            otlp(vec![record(PROJECT, "access.denied")]),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{other:?}: {body}");
    }

    // A person's session, even the bootstrap role's, is not the collector.
    let jana = cookie(&state.config, "jana", vec!["portal-approver"]);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/activity")
                .header(header::COOKIE, jana)
                .header(CSRF_HEADER, CSRF)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    otlp(vec![record(PROJECT, "access.denied")]).to_string(),
                ))
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // The collector's own: accepted, and the record is in the trail.
    let token = workload_token(&signer, &issuer, COLLECTOR_CLIENT);
    let (status, body) = ingest(
        &app,
        Some(&token),
        otlp(vec![record(PROJECT, "access.denied")]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["partialSuccess"].is_null(),
        "a good batch was partly rejected: {body}"
    );
    let page = state
        .activity
        .list(PROJECT, &filter_with(50))
        .await
        .expect("a page");
    assert_eq!(page.items.len(), 1);
    // Nothing of the refused attempts above was written.
    let elsewhere = state
        .activity
        .list(ELSEWHERE, &filter_with(50))
        .await
        .expect("a page");
    assert!(elsewhere.items.is_empty());
}

/// OPS-48: one malformed record must not cost the other four hundred. A batch is accepted record by
/// record, the answer counts what was refused and quotes the first reason, and nothing outside the
/// vocabulary or outside this installation's projects is stored.
#[tokio::test]
async fn a_record_this_installation_cannot_place_is_rejected_on_its_own() {
    let (app, state, signer, issuer) = portal_with_realm().await;
    let token = workload_token(&signer, &issuer, COLLECTOR_CLIENT);

    let batch = otlp(vec![
        record(PROJECT, "access.denied"),
        record("a-project-nobody-has", "access.denied"),
        record(PROJECT, "kind.invented.by.a.client"),
        record("Not_A_Project", "access.denied"),
        json!({ "attributes": [] }),
        json!({}),
    ]);
    let (status, body) = ingest(&app, Some(&token), batch).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["partialSuccess"]["rejectedLogRecords"],
        json!(5),
        "the answer does not say what was refused: {body}",
    );
    assert!(
        body["partialSuccess"]["errorMessage"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "a refusal with no reason: {body}",
    );

    let page = state
        .activity
        .list(PROJECT, &filter_with(50))
        .await
        .expect("a page");
    assert_eq!(
        page.items.len(),
        1,
        "a record the vocabulary does not know was stored"
    );
    assert_eq!(page.items[0].kind, "access.denied");

    // A batch that is not a batch is accepted as nothing at all, and nothing panics on the way.
    for shapeless in [
        json!({}),
        json!({ "resourceLogs": [] }),
        json!({ "resourceLogs": "not an array" }),
        json!({ "resourceLogs": [{ "scopeLogs": {} }] }),
        json!({ "resourceLogs": [{ "scopeLogs": [{ "logRecords": "x" }] }] }),
        json!([]),
        json!("nothing"),
        Value::Null,
    ] {
        let (status, body) = ingest(&app, Some(&token), shapeless.clone()).await;
        assert_eq!(status, StatusCode::OK, "{shapeless}: {body}");
        assert!(body["partialSuccess"].is_null(), "{shapeless}: {body}");
    }
    let page = state
        .activity
        .list(PROJECT, &filter_with(50))
        .await
        .expect("a page");
    assert_eq!(page.items.len(), 1, "a shapeless batch wrote something");
}

/// T-4876, PF-59: a reader bound to one context space reads that space's events and the project's
/// own, never another space's, in the list and in the live tail alike.
#[tokio::test]
async fn a_reader_bound_to_one_space_follows_only_that_spaces_activity() {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    let config = Config::for_tests();
    let mirror = mirror();
    let envelope = |kind: &str, name: &str, namespace: &str, spec: Value| ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(namespace.into()),
            ..Default::default()
        },
        spec,
        status: None,
    };
    mirror.upsert(envelope("ContextSpace", "noise", PROJECT, json!({})));
    mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Project"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "noise-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": "sami" }], "role": "space-reader",
                "scope": { "contextSpace": "noise" } }),
    ));
    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let app = server::app(state.clone());
    let sami = cookie(&config, "sami", vec![]);
    let at = |space: Option<&str>, kind: &str, minutes: i64| ActivityEvent {
        space: space.map(str::to_owned),
        ..event_at(
            PROJECT,
            kind,
            "info",
            "endpoints/x",
            Utc::now() - Duration::minutes(minutes),
        )
    };

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{PROJECT}/activity/stream"))
                .header(header::COOKIE, &sami)
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    state
        .activity
        .append(&[
            at(Some("air-quality"), "pipeline.error", 3),
            at(Some("noise"), "endpoint.traffic", 2),
            at(None, "project.changed", 1),
        ])
        .await
        .expect("appended");

    let (status, list) = get(
        &app,
        Some(&sami),
        &format!("/api/v1/projects/{PROJECT}/activity"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut kinds: Vec<&str> = list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["kind"].as_str())
        .collect();
    kinds.sort_unstable();
    assert_eq!(kinds, ["endpoint.traffic", "project.changed"], "{list}");

    let mut tail = String::new();
    while let Ok(Some(Ok(frame))) =
        tokio::time::timeout(std::time::Duration::from_millis(300), body.frame()).await
    {
        if let Ok(data) = frame.into_data() {
            tail.push_str(&String::from_utf8_lossy(&data));
        }
    }
    assert!(tail.contains("event: endpoint.traffic"), "{tail}");
    assert!(tail.contains("event: project.changed"), "{tail}");
    assert!(
        !tail.contains("pipeline.error"),
        "another space's event: {tail}"
    );
}

/// F-POR1-009: once a person's last binding in the project is gone, the open tail delivers no
/// further event, the project's own included, and ends.
#[tokio::test]
async fn a_removed_binding_ends_the_live_tail() {
    use joinedcontext_portal::permissions::ORG_NAMESPACE;
    use joinedcontext_portal::resource::ResourceKey;
    let config = Config::for_tests();
    let mirror = mirror();
    let envelope = |kind: &str, name: &str, spec: Value| ResourceEnvelope {
        api_version: API_VERSION.into(),
        kind: kind.into(),
        metadata: ObjectMeta {
            name: name.into(),
            namespace: Some(ORG_NAMESPACE.into()),
            ..Default::default()
        },
        spec,
        status: None,
    };
    mirror.upsert(envelope(
        "Role",
        "project-reader",
        json!({ "rules": [{ "kinds": ["Project"], "verbs": ["read"] }] }),
    ));
    mirror.upsert(envelope(
        "RoleBinding",
        "lena-reader",
        json!({ "subjects": [{ "user": "lena" }], "role": "project-reader",
                "scope": { "project": PROJECT } }),
    ));
    let state = AppState::new(config.clone(), None).with_mirror(mirror);
    let response = server::app(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/projects/{PROJECT}/activity/stream"))
                .header(header::COOKIE, cookie(&config, "lena", vec![]))
                .body(Body::empty())
                .expect("a request"),
        )
        .await
        .expect("a response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    state.mirror.remove(&ResourceKey {
        namespace: ORG_NAMESPACE.into(),
        kind: "RoleBinding".into(),
        name: "lena-reader".into(),
    });
    state
        .activity
        .append(&[event_at(
            PROJECT,
            "project.changed",
            "info",
            "endpoints/x",
            Utc::now(),
        )])
        .await
        .expect("appended");
    let mut tail = String::new();
    while let Ok(Some(Ok(frame))) =
        tokio::time::timeout(std::time::Duration::from_millis(300), body.frame()).await
    {
        if let Ok(data) = frame.into_data() {
            tail.push_str(&String::from_utf8_lossy(&data));
        }
    }
    assert!(
        !tail.contains("project.changed"),
        "an event after the binding went: {tail}"
    );
}
