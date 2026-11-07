//! T-2889, T-2996, T-3983, API/01 §29: a space's size is the broker's entity count of its tenant
//! (`GET /q/tenants/{tenant}` with the admin token), read with the same rights as the space, kept
//! for five minutes, and a broker that cannot answer is a reason, never a zero.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum_extra::extract::cookie::PrivateCookieJar;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header as has_header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use joinedcontext_portal::auth::session::{self, Identity, Session};
use joinedcontext_portal::config::Config;
use joinedcontext_portal::permissions::ORG_NAMESPACE;
use joinedcontext_portal::resource::{ObjectMeta, ResourceEnvelope, API_VERSION};
use joinedcontext_portal::server;
use joinedcontext_portal::state::AppState;

const PROJECT: &str = "helsinki";
const READER: &str = "sami.air@hel.fi";
const STRANGER: &str = "nobody@hel.fi";

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
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(|raw| raw.split(';').next().unwrap_or_default().to_owned())
        .collect::<Vec<_>>()
        .join("; ")
}

const TOKEN: &str = "admin-token-of-the-broker-0123456789abcdef";

/// The broker's admin token, mounted as the deployment mounts it.
/// One file per call: tests run in parallel, and a shared file rewritten by one test reads empty
/// to another mid-write (T-4072).
fn token_file() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = std::env::temp_dir().join(format!("space-usage-token-{}-{n}", std::process::id()));
    std::fs::write(&file, format!("{TOKEN}\n")).expect("write the token file");
    file
}

/// Two spaces of helsinki, `air` pinned to its own name and `mobility` unpinned, and a reader
/// bound to `air` alone, against a broker at `broker`.
fn world(broker: Option<String>) -> AppState {
    world_with(broker, Some(token_file()))
}

fn world_with(broker: Option<String>, token: Option<std::path::PathBuf>) -> AppState {
    let state = AppState::new(
        Config {
            broker_url: broker,
            broker_admin_token_file: token,
            ..Config::for_tests()
        },
        None,
    );
    state.mirror.upsert(envelope(
        "ContextSpace",
        "air",
        PROJECT,
        json!({ "urnSegment": "air" }),
    ));
    state
        .mirror
        .upsert(envelope("ContextSpace", "mobility", PROJECT, json!({})));
    state.mirror.upsert(envelope(
        "Role",
        "space-reader",
        ORG_NAMESPACE,
        json!({ "rules": [{ "kinds": ["ContextSpace", "Project"], "verbs": ["read"] }] }),
    ));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "air-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": READER }], "role": "space-reader",
                "scope": { "contextSpace": "air" } }),
    ));
    state
}

async fn get(state: &AppState, email: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(uri)
        .header(header::COOKIE, cookie(&state.config, email))
        .body(Body::empty())
        .expect("a request");
    let response = server::app(state.clone())
        .oneshot(request)
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

const AIR: &str = "/api/v1/projects/helsinki/spaces/air/usage";

/// The broker's admin read of the tenant `air`, with its admin token.
fn count_query() -> wiremock::MockBuilder {
    tenant_read("air")
}

fn tenant_read(tenant: &str) -> wiremock::MockBuilder {
    Mock::given(method("GET"))
        .and(path(format!("/q/tenants/{tenant}")))
        .and(has_header(
            "authorization",
            format!("Bearer {TOKEN}").as_str(),
        ))
}

fn counts(entities: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "tenant": "air",
        "counts": { "entities": entities, "subscriptions": 0, "registrations": 0 }
    }))
}

fn nonexistent_tenant() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({
        "type": "https://uri.etsi.org/ngsi-ld/errors/ResourceNotFound",
        "title": "ResourceNotFound",
        "status": 404,
        "detail": "tenant air"
    }))
}

#[tokio::test]
async fn a_reader_gets_the_brokers_count_and_a_second_read_is_kept() {
    let broker = MockServer::start().await;
    count_query()
        .respond_with(counts(json!(14232)))
        .expect(1)
        .mount(&broker)
        .await;
    let state = world(Some(broker.uri()));

    let (status, body) = get(&state, READER, AIR).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entities"], json!(14232));
    assert!(body["observedAt"].is_string(), "{body}");
    // Within five minutes the same answer is shown and the broker is not asked again.
    let (status, again) = get(&state, READER, AIR).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again, body);
}

#[tokio::test]
async fn a_space_the_broker_has_no_tenant_for_holds_nothing() {
    let broker = MockServer::start().await;
    count_query()
        .respond_with(nonexistent_tenant())
        .mount(&broker)
        .await;
    let (status, body) = get(&world(Some(broker.uri())), READER, AIR).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entities"], json!(0));
}

#[tokio::test]
async fn a_404_that_is_not_the_brokers_missing_tenant_is_a_reason_and_never_a_zero() {
    // A proxy or a wrong address answers 404 too; only the broker's own problem is empty.
    let broker = MockServer::start().await;
    count_query()
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&broker)
        .await;
    let (status, body) = get(&world(Some(broker.uri())), READER, AIR).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.to_string().contains("404"), "{body}");
}

#[tokio::test]
async fn an_answer_without_a_readable_count_is_a_reason_and_never_a_zero() {
    for count in [Value::Null, json!("many"), json!(-1)] {
        let broker = MockServer::start().await;
        count_query()
            .respond_with(counts(count.clone()))
            .mount(&broker)
            .await;
        let (status, body) = get(&world(Some(broker.uri())), READER, AIR).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{count:?}: {body}");
        assert!(body.to_string().contains("count"), "{count:?}: {body}");
    }
}

#[tokio::test]
async fn a_space_the_caller_may_not_read_is_404_and_the_broker_is_not_asked() {
    let broker = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&broker)
        .await;
    let state = world(Some(broker.uri()));
    for (who, uri) in [
        (READER, "/api/v1/projects/helsinki/spaces/mobility/usage"),
        (STRANGER, AIR),
        (READER, "/api/v1/projects/helsinki/spaces/nowhere/usage"),
    ] {
        let (status, body) = get(&state, who, uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who} {uri}: {body}");
    }
}

#[tokio::test]
async fn a_broker_that_cannot_answer_is_a_reason_and_never_a_zero() {
    let broker = MockServer::start().await;
    count_query()
        .respond_with(ResponseTemplate::new(500))
        .mount(&broker)
        .await;
    let (status, body) = get(&world(Some(broker.uri())), READER, AIR).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.to_string().contains("500"), "{body}");

    let (status, body) = get(&world(None), READER, AIR).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.to_string().contains("JC_PORTAL_BROKER_URL"), "{body}");
}

#[tokio::test]
async fn the_count_is_read_where_the_policy_engine_does_not_refuse_it() {
    // T-3983: on dev the broker's `joinedcontext` policy engine answered the NGSI-LD count query
    // of a caller without subject headers 403 for every space, and the page showed 503. The
    // count is read from the admin route; the query route is never asked.
    let broker = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ngsi-ld/v1/entities"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "type": "https://uri.etsi.org/ngsi-ld/errors/NotAuthorized",
            "title": "NotAuthorized", "status": 403
        })))
        .expect(0)
        .mount(&broker)
        .await;
    count_query()
        .respond_with(counts(json!(812)))
        .expect(1)
        .mount(&broker)
        .await;
    let (status, body) = get(&world(Some(broker.uri())), READER, AIR).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entities"], json!(812));
}

#[tokio::test]
async fn an_unpinned_space_is_counted_in_its_rendered_tenant() {
    // PF-84: `mobility` of helsinki pins nothing, so its tenant is `helsinki-mobility`.
    let broker = MockServer::start().await;
    tenant_read("helsinki-mobility")
        .respond_with(counts(json!(3)))
        .expect(1)
        .mount(&broker)
        .await;
    let state = world(Some(broker.uri()));
    state.mirror.upsert(envelope(
        "RoleBinding",
        "mobility-readers",
        ORG_NAMESPACE,
        json!({ "subjects": [{ "user": READER }], "role": "space-reader",
                "scope": { "contextSpace": "mobility" } }),
    ));
    let (status, body) = get(
        &state,
        READER,
        "/api/v1/projects/helsinki/spaces/mobility/usage",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entities"], json!(3));
}

#[tokio::test]
async fn without_the_admin_token_or_with_a_refused_one_the_reason_is_answered() {
    let broker = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&broker)
        .await;
    let (status, body) = get(&world_with(Some(broker.uri()), None), READER, AIR).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(
        body.to_string()
            .contains("JC_PORTAL_BROKER_ADMIN_TOKEN_FILE"),
        "{body}"
    );
    assert!(!body.to_string().contains(TOKEN), "{body}");

    let (status, body) = get(&world(Some(broker.uri())), READER, AIR).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.to_string().contains("401"), "{body}");
    assert!(!body.to_string().contains(TOKEN), "{body}");
}

/// `POST .../usage/recalculate` with the session cookie and the CSRF pair the UI sends; no
/// cookie at all for `email` "".
async fn recalculate(state: &AppState, email: &str, uri: &str) -> (StatusCode, Value) {
    use joinedcontext_portal::auth::csrf::{CSRF_COOKIE, CSRF_HEADER};
    const CSRF: &str = "test-csrf-token-space-usage-recalculate";
    let mut request = Request::builder()
        .method("POST")
        .uri(format!("{uri}/recalculate"))
        .header(CSRF_HEADER, CSRF);
    request = if email.is_empty() {
        request.header(header::COOKIE, format!("{CSRF_COOKIE}={CSRF}"))
    } else {
        request.header(
            header::COOKIE,
            format!("{}; {CSRF_COOKIE}={CSRF}", cookie(&state.config, email)),
        )
    };
    let response = server::app(state.clone())
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

/// T-6208: a reader recalculates a space they may read; nobody else learns it is there, and a
/// caller without a session is asked to sign in.
#[tokio::test]
async fn a_reader_recalculates_and_nobody_else_may() {
    let broker = MockServer::start().await;
    count_query()
        .respond_with(counts(json!(7)))
        .expect(1)
        .mount(&broker)
        .await;
    let state = world(Some(broker.uri()));

    let (status, body) = recalculate(&state, READER, AIR).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entities"], json!(7));
    assert!(body["measuredAt"].is_string(), "{body}");
    let (stranger, _) = recalculate(&state, STRANGER, AIR).await;
    assert_eq!(stranger, StatusCode::NOT_FOUND);
    let (unread, _) = recalculate(
        &state,
        READER,
        "/api/v1/projects/helsinki/spaces/mobility/usage",
    )
    .await;
    assert_eq!(
        unread,
        StatusCode::NOT_FOUND,
        "the reader is bound to air alone"
    );
    let (missing, _) = recalculate(
        &state,
        READER,
        "/api/v1/projects/helsinki/spaces/nope/usage",
    )
    .await;
    assert_eq!(missing, StatusCode::NOT_FOUND);
    let (anonymous, _) = recalculate(&state, "", AIR).await;
    assert_eq!(anonymous, StatusCode::UNAUTHORIZED);
}

/// T-6208: concurrent recalculations of one space make one broker read.
#[tokio::test]
async fn concurrent_recalculations_ask_the_broker_once() {
    let broker = MockServer::start().await;
    count_query()
        .respond_with(counts(json!(3)).set_delay(std::time::Duration::from_millis(200)))
        .expect(1)
        .mount(&broker)
        .await;
    let state = world(Some(broker.uri()));
    let (a, b, c) = tokio::join!(
        recalculate(&state, READER, AIR),
        recalculate(&state, READER, AIR),
        recalculate(&state, READER, AIR),
    );
    for (status, body) in [a, b, c] {
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["entities"], json!(3));
    }
}
